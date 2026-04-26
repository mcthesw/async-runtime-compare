# SOCKS5 Benchmarking

This benchmark compares the Tokio and Compio SOCKS5 CONNECT demos with the same
client workload and a local HTTP target server. It does not use the public
network, so the result is easier to reproduce on Windows and Linux.

The benchmark does this for each implementation:

1. Start a local HTTP server inside `socks5-bench`.
2. Start one SOCKS5 proxy process on `127.0.0.1:10800`.
3. Run warmup requests through the SOCKS5 proxy.
4. Run measured requests through the SOCKS5 proxy.
5. Record throughput and latency percentiles.
6. Stop the proxy and repeat for the next implementation.

Each measured request opens a new SOCKS5 CONNECT tunnel, sends one HTTP request,
reads the full HTTP response, and closes the connection. This measures the
combined cost of accept, SOCKS5 handshake, outbound connect, relay, and close.

## Quick Start

Windows PowerShell:

```powershell
.\scripts\bench-socks5.ps1
```

Linux/macOS:

```bash
chmod +x scripts/bench-socks5.sh
./scripts/bench-socks5.sh
```

The scripts build all benchmark binaries in release mode and then run
`socks5-bench`.

Default workload:

```text
requests:    2000 per implementation/concurrency
warmup:      200 per implementation/concurrency
concurrency: 1,16,64
body:        16384 bytes per HTTP response
```

Results are written to:

```text
target/socks5-bench/results.csv
target/socks5-bench/results.json
```

## Useful Runs

Small sanity check:

```bash
./scripts/bench-socks5.sh --requests 100 --warmup 20 --concurrency 1,8
```

Higher concurrency:

```bash
./scripts/bench-socks5.sh --requests 10000 --warmup 1000 --concurrency 1,16,64,256
```

Larger relay payload:

```bash
./scripts/bench-socks5.sh --requests 2000 --warmup 200 --concurrency 16,64 --body-bytes 1048576
```

One implementation only:

```bash
./scripts/bench-socks5.sh --only tokio
./scripts/bench-socks5.sh --only compio
```

Show proxy logs while debugging:

```bash
./scripts/bench-socks5.sh --requests 10 --warmup 0 --concurrency 1 --show-proxy-output
```

## Interpreting Output

The console table includes:

- `ok`: successful requests.
- `err`: failed requests.
- `req/s`: successful requests per second.
- `MiB/s`: total HTTP response bytes read per second.
- `avg ms`, `p95 ms`, `p99 ms`, `max ms`: end-to-end request latency.

Use `results.csv` or `results.json` for charts in the article.

## Linux Notes

For high-concurrency runs, raise the file descriptor limit before benchmarking:

```bash
ulimit -n 65535
```

For more stable numbers, run on an otherwise idle machine, use release builds,
and keep the same CPU governor across runs.

## Caveats

This benchmark opens a new SOCKS5 tunnel for every request. That is intentional:
it highlights the runtime differences around accept, connect, task scheduling,
buffer handling, and bidirectional relay. It is not a persistent-connection HTTP
benchmark.

The benchmark redirects proxy stdout/stderr by default, so the demo can keep
plain `println!` tracing without dominating the result.
