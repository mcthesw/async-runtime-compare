use std::{
    fs::{self, File},
    io::Write as _,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, anyhow, bail};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    time::sleep,
};

const DEFAULT_PROXY_ADDR: &str = "127.0.0.1:10800";
const DEFAULT_TARGET_ADDR: &str = "127.0.0.1:18081";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_args()?;
    if config.help {
        print_help();
        return Ok(());
    }

    fs::create_dir_all(&config.output_dir).with_context(|| {
        format!(
            "failed to create output directory {}",
            config.output_dir.display()
        )
    })?;

    let target_addr = DEFAULT_TARGET_ADDR
        .parse::<SocketAddr>()
        .with_context(|| format!("invalid target address {DEFAULT_TARGET_ADDR}"))?;
    let target_listener = TcpListener::bind(target_addr).await?;
    let target_addr = target_listener.local_addr()?;
    let target_body = Arc::new(make_body(config.body_bytes));
    tokio::spawn(run_target_server(target_listener, target_body));

    println!("Target HTTP server: http://{target_addr}/");
    println!("SOCKS5 proxy address: {DEFAULT_PROXY_ADDR}");
    println!(
        "Requests: {}, warmup: {}, body: {} bytes, rounds: {}, concurrency: {:?}",
        config.requests, config.warmup, config.body_bytes, config.rounds, config.concurrency
    );
    println!();
    print_table_header();

    let request = Arc::new(http_request(target_addr));
    let mut results = Vec::new();

    for round in 1..=config.rounds {
        for proxy in config.proxies() {
            let mut child = ProxyProcess::start(proxy.name, &proxy.path, &config)?;
            wait_for_proxy(&mut child.child, DEFAULT_PROXY_ADDR, Duration::from_secs(5)).await?;

            for concurrency in &config.concurrency {
                if config.warmup > 0 {
                    let warmup = run_load(
                        DEFAULT_PROXY_ADDR,
                        target_addr,
                        request.clone(),
                        config.warmup,
                        *concurrency,
                    )
                    .await;

                    if warmup.errors > 0 {
                        bail!(
                            "{} warmup failed: {} of {} requests errored",
                            proxy.name,
                            warmup.errors,
                            config.warmup
                        );
                    }
                }

                let stats = run_load(
                    DEFAULT_PROXY_ADDR,
                    target_addr,
                    request.clone(),
                    config.requests,
                    *concurrency,
                )
                .await;

                let result = BenchResult::from_stats(
                    proxy.name,
                    round,
                    *concurrency,
                    config.requests,
                    config.body_bytes,
                    stats,
                );
                print_result_row(&result);
                results.push(result);
            }

            drop(child);
            sleep(Duration::from_millis(250)).await;
        }
    }

    write_csv(&config.output_dir.join("results.csv"), &results)?;
    write_json(&config.output_dir.join("results.json"), &results)?;
    println!();
    println!(
        "Wrote {} and {}",
        config.output_dir.join("results.csv").display(),
        config.output_dir.join("results.json").display()
    );

    Ok(())
}

#[derive(Debug, Clone)]
struct Config {
    requests: usize,
    warmup: usize,
    concurrency: Vec<usize>,
    body_bytes: usize,
    rounds: usize,
    output_dir: PathBuf,
    tokio_bin: PathBuf,
    compio_bin: PathBuf,
    only: Option<String>,
    show_proxy_output: bool,
    help: bool,
}

impl Config {
    fn from_args() -> anyhow::Result<Self> {
        let mut config = Self {
            requests: 2_000,
            warmup: 200,
            concurrency: vec![1, 16, 64],
            body_bytes: 16 * 1024,
            rounds: 1,
            output_dir: PathBuf::from("target/socks5-bench"),
            tokio_bin: sibling_binary("socks5-tokio")?,
            compio_bin: sibling_binary("socks5-compio")?,
            only: None,
            show_proxy_output: false,
            help: false,
        };

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-h" | "--help" => config.help = true,
                "--requests" => config.requests = parse_arg(&mut args, "--requests")?,
                "--warmup" => config.warmup = parse_arg(&mut args, "--warmup")?,
                "--concurrency" => {
                    let value = take_arg(&mut args, "--concurrency")?;
                    config.concurrency = parse_concurrency(&value)?;
                }
                "--body-bytes" => config.body_bytes = parse_arg(&mut args, "--body-bytes")?,
                "--rounds" => config.rounds = parse_arg(&mut args, "--rounds")?,
                "--output-dir" => {
                    config.output_dir = PathBuf::from(take_arg(&mut args, "--output-dir")?)
                }
                "--tokio-bin" => {
                    config.tokio_bin = PathBuf::from(take_arg(&mut args, "--tokio-bin")?)
                }
                "--compio-bin" => {
                    config.compio_bin = PathBuf::from(take_arg(&mut args, "--compio-bin")?)
                }
                "--only" => {
                    let only = take_arg(&mut args, "--only")?;
                    if only != "tokio" && only != "compio" {
                        bail!("--only must be tokio or compio");
                    }
                    config.only = Some(only);
                }
                "--show-proxy-output" => config.show_proxy_output = true,
                _ => bail!("unknown argument: {arg}"),
            }
        }

        if config.requests == 0 {
            bail!("--requests must be greater than zero");
        }
        if config.concurrency.is_empty() || config.concurrency.contains(&0) {
            bail!("--concurrency values must be greater than zero");
        }
        if config.body_bytes == 0 {
            bail!("--body-bytes must be greater than zero");
        }
        if config.rounds == 0 {
            bail!("--rounds must be greater than zero");
        }

        Ok(config)
    }

    fn proxies(&self) -> Vec<ProxyConfig> {
        let mut proxies = Vec::new();
        if self.only.as_deref() != Some("compio") {
            proxies.push(ProxyConfig {
                name: "tokio",
                path: self.tokio_bin.clone(),
            });
        }
        if self.only.as_deref() != Some("tokio") {
            proxies.push(ProxyConfig {
                name: "compio",
                path: self.compio_bin.clone(),
            });
        }
        proxies
    }
}

#[derive(Debug, Clone)]
struct ProxyConfig {
    name: &'static str,
    path: PathBuf,
}

struct ProxyProcess {
    child: Child,
}

impl ProxyProcess {
    fn start(name: &str, path: &PathBuf, config: &Config) -> anyhow::Result<Self> {
        if !path.exists() {
            bail!(
                "{name} binary not found at {}. Build release binaries first.",
                path.display()
            );
        }

        let mut command = Command::new(path);

        if config.show_proxy_output {
            command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        } else {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }

        let child = command
            .spawn()
            .with_context(|| format!("failed to start {name} proxy at {}", path.display()))?;

        Ok(Self { child })
    }
}

impl Drop for ProxyProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug)]
struct LoadStats {
    elapsed: Duration,
    latencies: Vec<Duration>,
    bytes: usize,
    errors: usize,
}

#[derive(Debug)]
struct RequestOutcome {
    latency: Duration,
    bytes: usize,
    error: Option<String>,
}

#[derive(Debug)]
struct BenchResult {
    implementation: &'static str,
    round: usize,
    concurrency: usize,
    requests: usize,
    successes: usize,
    errors: usize,
    body_bytes: usize,
    elapsed_ms: f64,
    requests_per_sec: f64,
    mib_per_sec: f64,
    avg_ms: f64,
    min_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
}

impl BenchResult {
    fn from_stats(
        implementation: &'static str,
        round: usize,
        concurrency: usize,
        requests: usize,
        body_bytes: usize,
        stats: LoadStats,
    ) -> Self {
        let successes = stats.latencies.len();
        let elapsed_secs = stats.elapsed.as_secs_f64();
        let elapsed_ms = elapsed_secs * 1_000.0;
        let requests_per_sec = successes as f64 / elapsed_secs;
        let mib_per_sec = stats.bytes as f64 / 1024.0 / 1024.0 / elapsed_secs;
        let mut latencies = stats.latencies;
        latencies.sort_unstable();

        Self {
            implementation,
            round,
            concurrency,
            requests,
            successes,
            errors: stats.errors,
            body_bytes,
            elapsed_ms,
            requests_per_sec,
            mib_per_sec,
            avg_ms: avg_ms(&latencies),
            min_ms: percentile_ms(&latencies, 0.0),
            p50_ms: percentile_ms(&latencies, 50.0),
            p95_ms: percentile_ms(&latencies, 95.0),
            p99_ms: percentile_ms(&latencies, 99.0),
            max_ms: percentile_ms(&latencies, 100.0),
        }
    }
}

async fn run_target_server(listener: TcpListener, body: Arc<Vec<u8>>) -> anyhow::Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let body = body.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_target_connection(stream, body).await {
                eprintln!("target connection failed: {error:#}");
            }
        });
    }
}

async fn handle_target_connection(mut stream: TcpStream, body: Arc<Vec<u8>>) -> anyhow::Result<()> {
    let mut request = Vec::with_capacity(1024);
    let mut buf = [0u8; 1024];

    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        request.extend_from_slice(&buf[..n]);
        if request.len() > 16 * 1024 {
            bail!("HTTP request header too large");
        }
    }

    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(&body).await?;
    stream.shutdown().await?;

    Ok(())
}

async fn run_load(
    proxy_addr: &str,
    target_addr: SocketAddr,
    request: Arc<Vec<u8>>,
    requests: usize,
    concurrency: usize,
) -> LoadStats {
    let start = Instant::now();
    let counter = Arc::new(AtomicUsize::new(0));
    let proxy_addr = Arc::new(proxy_addr.to_owned());
    let (tx, mut rx) = mpsc::channel::<RequestOutcome>(concurrency * 2);

    for _ in 0..concurrency {
        let counter = counter.clone();
        let proxy_addr = proxy_addr.clone();
        let request = request.clone();
        let tx = tx.clone();

        tokio::spawn(async move {
            loop {
                let index = counter.fetch_add(1, Ordering::Relaxed);
                if index >= requests {
                    break;
                }

                let started = Instant::now();
                let outcome = match run_one_request(&proxy_addr, target_addr, &request).await {
                    Ok(bytes) => RequestOutcome {
                        latency: started.elapsed(),
                        bytes,
                        error: None,
                    },
                    Err(error) => RequestOutcome {
                        latency: started.elapsed(),
                        bytes: 0,
                        error: Some(format!("{error:#}")),
                    },
                };

                if tx.send(outcome).await.is_err() {
                    break;
                }
            }
        });
    }
    drop(tx);

    let mut latencies = Vec::with_capacity(requests);
    let mut bytes = 0usize;
    let mut errors = 0usize;
    let mut first_errors = Vec::new();

    while let Some(outcome) = rx.recv().await {
        if let Some(error) = outcome.error {
            errors += 1;
            if first_errors.len() < 3 {
                first_errors.push(error);
            }
        } else {
            latencies.push(outcome.latency);
            bytes += outcome.bytes;
        }
    }

    for error in first_errors {
        eprintln!("request failed: {error}");
    }

    LoadStats {
        elapsed: start.elapsed(),
        latencies,
        bytes,
        errors,
    }
}

async fn run_one_request(
    proxy_addr: &str,
    target_addr: SocketAddr,
    request: &[u8],
) -> anyhow::Result<usize> {
    let mut stream = connect_via_socks(proxy_addr, target_addr).await?;
    stream.write_all(request).await?;

    let mut total = 0usize;
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        total += n;
    }

    Ok(total)
}

async fn connect_via_socks(proxy_addr: &str, target_addr: SocketAddr) -> anyhow::Result<TcpStream> {
    let mut stream = TcpStream::connect(proxy_addr).await?;

    stream.write_all(&[0x05, 0x01, 0x00]).await?;
    let mut method = [0u8; 2];
    stream.read_exact(&mut method).await?;
    if method != [0x05, 0x00] {
        bail!("unexpected SOCKS5 method selection: {method:?}");
    }

    let request = encode_socks_connect_request(target_addr);
    stream.write_all(&request).await?;

    let mut response = [0u8; 10];
    stream.read_exact(&mut response).await?;
    if response[0] != 0x05 || response[1] != 0x00 {
        bail!("SOCKS5 CONNECT failed: {response:?}");
    }

    Ok(stream)
}

fn encode_socks_connect_request(target_addr: SocketAddr) -> Vec<u8> {
    let mut request = Vec::with_capacity(22);
    request.extend_from_slice(&[0x05, 0x01, 0x00]);
    match target_addr.ip() {
        IpAddr::V4(ip) => {
            request.push(0x01);
            request.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            request.push(0x04);
            request.extend_from_slice(&ip.octets());
        }
    }
    request.extend_from_slice(&target_addr.port().to_be_bytes());
    request
}

async fn wait_for_proxy(
    child: &mut Child,
    proxy_addr: &str,
    timeout: Duration,
) -> anyhow::Result<()> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            bail!("proxy exited before becoming ready: {status}");
        }

        if TcpStream::connect(proxy_addr).await.is_ok() {
            return Ok(());
        }

        if started.elapsed() >= timeout {
            bail!("proxy at {proxy_addr} did not become ready within {timeout:?}");
        }

        sleep(Duration::from_millis(50)).await;
    }
}

fn make_body(len: usize) -> Vec<u8> {
    const PATTERN: &[u8] = b"async-runtime-socks5-benchmark\n";
    let mut body = Vec::with_capacity(len);
    while body.len() < len {
        let remaining = len - body.len();
        let take = remaining.min(PATTERN.len());
        body.extend_from_slice(&PATTERN[..take]);
    }
    body
}

fn http_request(target_addr: SocketAddr) -> Vec<u8> {
    format!("GET /benchmark HTTP/1.1\r\nHost: {target_addr}\r\nConnection: close\r\n\r\n")
        .into_bytes()
}

fn sibling_binary(name: &str) -> anyhow::Result<PathBuf> {
    let current_exe = std::env::current_exe()?;
    let directory = current_exe
        .parent()
        .ok_or_else(|| anyhow!("current executable has no parent directory"))?;
    let mut path = directory.join(name);
    if cfg!(windows) {
        path.set_extension("exe");
    }
    Ok(path)
}

fn take_arg(args: &mut impl Iterator<Item = String>, name: &str) -> anyhow::Result<String> {
    args.next()
        .ok_or_else(|| anyhow!("{name} requires a value"))
}

fn parse_arg<T>(args: &mut impl Iterator<Item = String>, name: &str) -> anyhow::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let value = take_arg(args, name)?;
    value
        .parse()
        .with_context(|| format!("invalid value for {name}: {value}"))
}

fn parse_concurrency(value: &str) -> anyhow::Result<Vec<usize>> {
    value
        .split(',')
        .map(|part| {
            part.trim()
                .parse::<usize>()
                .with_context(|| format!("invalid concurrency value: {part}"))
        })
        .collect()
}

fn avg_ms(latencies: &[Duration]) -> f64 {
    if latencies.is_empty() {
        return 0.0;
    }

    let total_secs: f64 = latencies.iter().map(Duration::as_secs_f64).sum();
    total_secs * 1_000.0 / latencies.len() as f64
}

fn percentile_ms(latencies: &[Duration], percentile: f64) -> f64 {
    if latencies.is_empty() {
        return 0.0;
    }

    let index = if percentile >= 100.0 {
        latencies.len() - 1
    } else {
        ((percentile / 100.0) * (latencies.len() - 1) as f64).round() as usize
    };
    latencies[index].as_secs_f64() * 1_000.0
}

fn print_table_header() {
    println!(
        "{:<8} {:>5} {:>5} {:>8} {:>8} {:>10} {:>10} {:>9} {:>9} {:>9} {:>9}",
        "impl",
        "round",
        "conc",
        "ok",
        "err",
        "req/s",
        "MiB/s",
        "avg ms",
        "p95 ms",
        "p99 ms",
        "max ms"
    );
}

fn print_result_row(result: &BenchResult) {
    println!(
        "{:<8} {:>5} {:>5} {:>8} {:>8} {:>10.1} {:>10.1} {:>9.3} {:>9.3} {:>9.3} {:>9.3}",
        result.implementation,
        result.round,
        result.concurrency,
        result.successes,
        result.errors,
        result.requests_per_sec,
        result.mib_per_sec,
        result.avg_ms,
        result.p95_ms,
        result.p99_ms,
        result.max_ms
    );
}

fn write_csv(path: &PathBuf, results: &[BenchResult]) -> anyhow::Result<()> {
    let mut file = File::create(path)?;
    writeln!(
        file,
        "implementation,round,concurrency,requests,successes,errors,body_bytes,elapsed_ms,requests_per_sec,mib_per_sec,avg_ms,min_ms,p50_ms,p95_ms,p99_ms,max_ms"
    )?;

    for result in results {
        writeln!(
            file,
            "{},{},{},{},{},{},{},{:.3},{:.3},{:.3},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}",
            result.implementation,
            result.round,
            result.concurrency,
            result.requests,
            result.successes,
            result.errors,
            result.body_bytes,
            result.elapsed_ms,
            result.requests_per_sec,
            result.mib_per_sec,
            result.avg_ms,
            result.min_ms,
            result.p50_ms,
            result.p95_ms,
            result.p99_ms,
            result.max_ms
        )?;
    }

    Ok(())
}

fn write_json(path: &PathBuf, results: &[BenchResult]) -> anyhow::Result<()> {
    let mut file = File::create(path)?;
    writeln!(file, "[")?;

    for (index, result) in results.iter().enumerate() {
        let comma = if index + 1 == results.len() { "" } else { "," };
        writeln!(
            file,
            "  {{\"implementation\":\"{}\",\"round\":{},\"concurrency\":{},\"requests\":{},\"successes\":{},\"errors\":{},\"body_bytes\":{},\"elapsed_ms\":{:.3},\"requests_per_sec\":{:.3},\"mib_per_sec\":{:.3},\"avg_ms\":{:.6},\"min_ms\":{:.6},\"p50_ms\":{:.6},\"p95_ms\":{:.6},\"p99_ms\":{:.6},\"max_ms\":{:.6}}}{}",
            result.implementation,
            result.round,
            result.concurrency,
            result.requests,
            result.successes,
            result.errors,
            result.body_bytes,
            result.elapsed_ms,
            result.requests_per_sec,
            result.mib_per_sec,
            result.avg_ms,
            result.min_ms,
            result.p50_ms,
            result.p95_ms,
            result.p99_ms,
            result.max_ms,
            comma
        )?;
    }

    writeln!(file, "]")?;
    Ok(())
}

fn print_help() {
    println!(
        "Usage: socks5-bench [options]\n\
\n\
Options:\n\
  --requests N             Measured requests per implementation/concurrency (default: 2000)\n\
  --warmup N               Warmup requests before each measured run (default: 200)\n\
  --concurrency LIST       Comma-separated concurrency levels (default: 1,16,64)\n\
  --body-bytes N           HTTP response body size from local target server (default: 16384)\n\
  --rounds N               Repeat all runs N times (default: 1)\n\
  --tokio-bin PATH         Path to socks5-tokio binary\n\
  --compio-bin PATH        Path to socks5-compio binary\n\
  --only tokio|compio      Run one implementation only\n\
  --output-dir PATH        Directory for results.csv/results.json (default: target/socks5-bench)\n\
  --show-proxy-output      Inherit proxy stdout/stderr instead of silencing it\n\
  -h, --help               Print this help\n"
    );
}
