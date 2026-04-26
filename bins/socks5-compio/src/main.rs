use std::{net::SocketAddr, num::NonZeroUsize};

use anyhow::bail;
use compio::{
    BufResult,
    buf::{IntoInner, IoBuf},
    dispatcher::Dispatcher,
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use socks5_core::{
    ATYP_DOMAIN, ATYP_IPV4, ATYP_IPV6, encode_method_selection_no_auth, encode_success_reply,
    parse_connect_request, select_no_auth,
};

const LISTEN_ADDR: &str = "127.0.0.1:10800";

#[compio::main]
async fn main() -> anyhow::Result<()> {
    let listener = TcpListener::bind(LISTEN_ADDR).await?;
    let worker_threads =
        std::thread::available_parallelism().unwrap_or_else(|_| NonZeroUsize::new(1).unwrap());
    let dispatcher = Dispatcher::builder()
        .worker_threads(worker_threads)
        .build()?;

    println!("Listening on {LISTEN_ADDR} with {worker_threads} compio worker threads");

    loop {
        match listener.accept().await {
            Err(e) => println!("Error: {e:#?}"),
            Ok((stream, src)) => {
                println!("Receive tcp stream from {src}");
                if dispatcher
                    .dispatch(move || async move {
                        if let Err(e) = handle_socks5(stream, src).await {
                            println!("Handle {src} failed: {e:#}");
                        }
                    })
                    .is_err()
                {
                    println!("Dispatch {src} failed");
                }
            }
        }
    }
}

async fn handle_socks5(mut stream: TcpStream, src: SocketAddr) -> anyhow::Result<()> {
    let greeting = read_greeting(&mut stream).await?;
    select_no_auth(&greeting)?;
    stream
        .write_all(encode_method_selection_no_auth())
        .await
        .0?;
    println!("Received client hello: {greeting:?}");

    let request_buf = read_connect_request(&mut stream).await?;
    let req = parse_connect_request(&request_buf)?;
    println!("Socks5 CONNECT command from {src}: {req:?}");

    let to_target = TcpStream::connect(req.target_string()).await?;
    stream.write_all(encode_success_reply()).await.0?;

    if let (Ok(client_to_target), Ok(target_to_client)) =
        compio::io::util::copy_bidirectional(stream, to_target).await
    {
        println!(
            "Done: client -> target: {client_to_target} bytes, target -> client: {target_to_client} bytes"
        );
    };

    Ok(())
}

/// Read client greeting and return the full greeting bytes
async fn read_greeting(stream: &mut TcpStream) -> anyhow::Result<Vec<u8>> {
    // Compio owns the buffer while the operation is in flight and returns it in BufResult.
    let BufResult(res, mut greeting) = stream.read_exact(Vec::with_capacity(2)).await;
    res?;

    if greeting[0] != 0x05 {
        bail!("Not socks5!");
    }

    let method_count = greeting[1] as usize;
    let total = 2 + method_count;
    greeting.resize(total, 0);

    let BufResult(res, greeting_tail) = stream.read_exact(greeting.slice(2..total)).await;
    res?;

    Ok(greeting_tail.into_inner())
}

async fn read_connect_request(stream: &mut TcpStream) -> anyhow::Result<Vec<u8>> {
    let BufResult(res, mut req) = stream.read_exact(vec![0u8; 4]).await;
    res?;

    Ok(match req[3] {
        ATYP_IPV4 => {
            let new_len = 4 + 4 + 2;
            req.resize(new_len, 0);
            let BufResult(res, req) = stream.read_exact(req.slice(4..new_len)).await;
            res?;
            req.into_inner()
        }
        ATYP_IPV6 => {
            let new_len = 4 + 16 + 2;
            req.resize(new_len, 0);
            let BufResult(res, req) = stream.read_exact(req.slice(4..new_len)).await;
            res?;
            req.into_inner()
        }
        ATYP_DOMAIN => {
            let domain_len = stream.read_u8().await?;
            let new_len = 4 + 1 + domain_len as usize + 2;
            req.resize(new_len, 0);
            req[4] = domain_len;
            let BufResult(res, req) = stream.read_exact(req.slice(5..new_len)).await;
            res?;
            req.into_inner()
        }
        _ => {
            bail!("Unknown ATYP")
        }
    })
}
