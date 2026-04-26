use std::net::SocketAddr;

use anyhow::bail;
use socks5_core::{
    ATYP_DOMAIN, ATYP_IPV4, ATYP_IPV6, encode_method_selection_no_auth, encode_success_reply,
    parse_connect_request, select_no_auth,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const LISTEN_ADDR: &str = "127.0.0.1:10800";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let listener = TcpListener::bind(LISTEN_ADDR).await?;
    println!("Listening on {LISTEN_ADDR}");

    loop {
        match listener.accept().await {
            Err(e) => println!("Error: {e:#?}"),
            Ok((stream, src)) => {
                println!("Receive tcp stream from {src}");
                tokio::spawn(async move {
                    if let Err(e) = handle_socks5(stream, src).await {
                        println!("Handle {src} failed: {e:#}");
                    }
                });
            }
        }
    }
}

async fn handle_socks5(mut stream: TcpStream, src: SocketAddr) -> anyhow::Result<()> {
    let greeting = read_greeting(&mut stream).await?;
    select_no_auth(&greeting)?;
    stream.write_all(&encode_method_selection_no_auth()).await?;
    println!("Received client hello: {greeting:?}");

    let request_buf = read_connect_request(&mut stream).await?;
    let req = parse_connect_request(&request_buf)?;
    println!("Socks5 CONNECT command from {src}: {req:?}");

    let mut to_target = TcpStream::connect(req.target_string()).await?;
    stream.write_all(&encode_success_reply()).await?;

    let (client_to_target, target_to_client) =
        tokio::io::copy_bidirectional(&mut stream, &mut to_target).await?;
    println!(
        "Done: client -> target: {client_to_target} bytes, target -> client: {target_to_client} bytes"
    );

    Ok(())
}

/// Read client greeting and return the full greeting bytes
async fn read_greeting(stream: &mut TcpStream) -> anyhow::Result<Vec<u8>> {
    let mut greeting = vec![0u8; 2];
    stream.read_exact(&mut greeting).await?;
    if greeting[0] != 0x05 {
        bail!("Not socks5!")
    }

    let method_count = greeting[1] as usize;
    greeting.resize(2 + method_count, 0);

    stream.read_exact(&mut greeting[2..]).await?;

    Ok(greeting)
}

async fn read_connect_request(stream: &mut TcpStream) -> anyhow::Result<Vec<u8>> {
    let mut req = vec![0u8; 4];
    stream.read_exact(&mut req).await?;

    match req[3] {
        ATYP_IPV4 => {
            req.resize(4 + 4 + 2, 0);
            stream.read_exact(&mut req[4..]).await?;
        }
        ATYP_IPV6 => {
            req.resize(4 + 16 + 2, 0);
            stream.read_exact(&mut req[4..]).await?;
        }
        ATYP_DOMAIN => {
            let domain_len = stream.read_u8().await?;
            let new_len = 4 + 1 + domain_len as usize + 2; // head(4) len(1) domain port(2)
            req.resize(new_len, 0);
            req[4] = domain_len;
            stream.read_exact(&mut req[5..]).await?;
        }
        _ => {
            bail!("Unknown ATYP")
        }
    }

    Ok(req)
}
