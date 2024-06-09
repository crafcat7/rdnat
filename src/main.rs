use tokio::io::{copy, split, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::time::{sleep, Duration};
use std::env;
use std::error::Error;
use hyper::{Client, Request, Body};
use hyper::body::HttpBody as _;  // for `data` method
use std::net::SocketAddr;
use std::str;

const RETRY_INTERVAL: u64 = 5;
const VERSION: &str = "v0.0.1";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    banner();
    let args: Vec<String> = env::args().collect();

    if args.len() == 2 {
        match args[1].as_str() {
            "-version" | "-v" | "-V" => {
                println!("{}", VERSION);
                return Ok(());
            }
            _ => {}
        }
    }

    if args.len() < 4 {
        help();
        return Ok(());
    }

    match args[1].as_str() {
        "-listen" | "-l" => {
            listener(&args[2], &args[3]).await?;
        }
        "-agent" | "-a" => {
            agent(&args[2], &args[3]).await?;
        }
        "-forward" | "-f" => {
            forward(&args[2], &args[3]).await?;
        }
        "-proxy" | "-p" => {
            proxy(&args[2], &args[3], &args).await?;
        }
        _ => {
            help();
        }
    }

    Ok(())
}

fn banner() {
    println!("
    ____  ____  _   _____  ______
   / __ \\/ __ \\/ | / /   |/_  __/
  / /_/ / / / /  |/ / /| | / /   
 / _, _/ /_/ / /|  / ___ |/ /    
/_/ |_/_____/_/ |_/_/  |_/_/     
    ");
}

fn help() {
    println!("Usage:");
    println!("  -version | -v | -V                      Display the version of the application.");
    println!("  -listen <listen_port0> <listen_port1>   Start listening on the specified ports for incoming connections.");
    println!("  -agent <target_address0> <target_address1>  Act as an agent forwarding data between two target addresses.");
    println!("  -forward <listen_port> <target_address> Forward incoming connections on a port to a target address.");
    println!("  -proxy <protocol> <listen_address> ...  Act as a proxy (HTTP, SOCKS5) based on specified protocol.");
    println!("\n");
    println!("  HTTP proxy usage:");
    println!("    -proxy http <listen_address>");
    println!("  SOCKS5 proxy usage:");
    println!("    -proxy socks5 <listen_address>");
    println!("\nExamples:");
    println!("  -listen 8000 8001");
    println!("  -agent 192.168.1.100:8000 192.168.1.101:8001");
    println!("  -forward 8000 192.168.1.102:8000");
    println!("  -proxy http 8888");
    println!("  -proxy socks5 1080");
}

async fn listener(listen_port0: &str, listen_port1: &str) -> Result<(), Box<dyn Error>> {
    let listener0 = TcpListener::bind(format!("0.0.0.0:{}", listen_port0)).await?;
    let listener1 = TcpListener::bind(format!("0.0.0.0:{}", listen_port1)).await?;
    println!("[*] listening on ports: {} and {}", listen_port0, listen_port1);

    loop {
        let (conn0, _) = listener0.accept().await?;
        let (conn1, _) = listener1.accept().await?;
        tokio::spawn(mutual_copy_io(conn0, conn1));
    }
}

async fn forward(listen_port: &str, target_address: &str) -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind(format!("0.0.0.0:{}", listen_port)).await?;
    println!("[*] listening on: {} and forwarding to: {}", listen_port, target_address);

    loop {
        let (conn, _) = listener.accept().await?;
        let target_address = target_address.to_string();
        tokio::spawn(async move {
            if let Err(e) = handle_forward(target_address, conn).await {
                eprintln!("[x] error: {}", e);
            }
        });
    }
}

async fn handle_forward(target_address: String, conn0: TcpStream) -> Result<(), Box<dyn Error>> {
    let conn1 = TcpStream::connect(target_address).await?;
    mutual_copy_io(conn0, conn1).await;
    Ok(())
}

async fn agent(target_address0: &str, target_address1: &str) -> Result<(), Box<dyn Error>> {
    println!("[*] agent with: {} {}", target_address0, target_address1);

    loop {
        match TcpStream::connect(target_address0).await {
            Ok(conn0) => {
                loop {
                    match TcpStream::connect(target_address1).await {
                        Ok(conn1) => {
                            mutual_copy_io(conn0, conn1).await;
                            break;
                        }
                        Err(e) => {
                            eprintln!("[x] error connecting to {}: {}", target_address1, e);
                            sleep(Duration::from_secs(RETRY_INTERVAL)).await;
                        }
                    }
                }
                break;
            }
            Err(e) => {
                eprintln!("[x] error connecting to {}: {}", target_address0, e);
                sleep(Duration::from_secs(RETRY_INTERVAL)).await;
            }
        }
    }

    Ok(())
}

async fn proxy(protocol: &str, listen_address: &str, _args: &[String]) -> Result<(), Box<dyn Error>> {
    match protocol {
        "http" => {
            let listener = TcpListener::bind(format!("0.0.0.0:{}", listen_address)).await?;
            println!("[*] HTTP proxy listening on: {}", listen_address);

            loop {
                let (stream, _) = listener.accept().await?;
                tokio::spawn(handle_http_proxy(stream));
            }
        }
        "socks5" => {
            let listener = TcpListener::bind(format!("0.0.0.0:{}", listen_address)).await?;
            println!("[*] SOCKS5 proxy listening on: {}", listen_address);

            loop {
                let (stream, _) = listener.accept().await?;
                tokio::spawn(handle_socks5_proxy(stream));
            }
        }
        _ => {
            eprintln!("protocol must be either http or socks5");
        }
    }

    Ok(())
}

async fn handle_http_proxy(mut stream: TcpStream) {
    let mut buffer = [0u8; 4096];
    let n = match stream.read(&mut buffer).await {
        Ok(n) if n == 0 => return,
        Ok(n) => n,
        Err(_) => return,
    };

    let request_line = String::from_utf8_lossy(&buffer[..n]);
    if request_line.starts_with("CONNECT") {
        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 3 {
            return;
        }

        if let Err(_) = stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await {
            return;
        }

        let target_addr = parts[1];
        if let Ok(target_stream) = TcpStream::connect(target_addr).await {
            tokio::spawn(mutual_copy_io(stream, target_stream));
        }
    } else {
        let client = Client::new();
        let request = match Request::builder()
            .uri(String::from_utf8_lossy(&buffer[..n]).split_whitespace().nth(1).unwrap())
            .body(Body::from(buffer[..n].to_vec())) {
            Ok(req) => req,
            Err(_) => return,
        };

        match client.request(request).await {
            Ok(mut response) => {
                let status_line = format!("HTTP/1.1 {}\r\n", response.status());
                if let Err(_) = stream.write_all(status_line.as_bytes()).await {
                    return;
                }

                for (key, value) in response.headers() {
                    let header_line = format!("{}: {}\r\n", key, value.to_str().unwrap());
                    if let Err(_) = stream.write_all(header_line.as_bytes()).await {
                        return;
                    }
                }

                if let Err(_) = stream.write_all(b"\r\n").await {
                    return;
                }

                while let Some(chunk) = response.body_mut().data().await {
                    if let Ok(chunk) = chunk {
                        if let Err(_) = stream.write_all(&chunk).await {
                            return;
                        }
                    }
                }
            }
            Err(_) => {
                let _ = stream.write_all(b"HTTP/1.1 500 Internal Server Error\r\n\r\n").await;
            }
        }
    }
}

async fn handle_socks5_proxy(mut stream: TcpStream) {
    // Read the client's handshake request
    let mut buf = [0u8; 2];
    if stream.read_exact(&mut buf).await.is_err() {
        return;
    }

    // Ensure the handshake request is valid (SOCKS5)
    if buf[0] != 0x05 {
        return;
    }

    // Read the client's authentication methods
    let num_methods = buf[1] as usize;
    let mut methods = vec![0u8; num_methods];
    if stream.read_exact(&mut methods).await.is_err() {
        return;
    }

    // Respond with "no authentication required"
    if stream.write_all(&[0x05, 0x00]).await.is_err() {
        return;
    }

    // Read the client's connection request
    let mut buf = [0u8; 4];
    if stream.read_exact(&mut buf).await.is_err() {
        return;
    }

    if buf[1] != 0x01 {
        // Only support CONNECT command
        return;
    }

    // Parse the address the client wants to connect to
    let address = match buf[3] {
        0x01 => {
            // IPv4
            let mut ipv4 = [0u8; 4];
            if stream.read_exact(&mut ipv4).await.is_err() {
                return;
            }
            let mut port = [0u8; 2];
            if stream.read_exact(&mut port).await.is_err() {
                return;
            }
            let ip = std::net::Ipv4Addr::new(ipv4[0], ipv4[1], ipv4[2], ipv4[3]);
            let port = u16::from_be_bytes(port);
            SocketAddr::new(ip.into(), port)
        }
        0x03 => {
            // Domain name
            let mut domain_len = [0u8; 1];
            if stream.read_exact(&mut domain_len).await.is_err() {
                return;
            }
            let domain_len = domain_len[0] as usize;
            let mut domain = vec![0u8; domain_len];
            if stream.read_exact(&mut domain).await.is_err() {
                return;
            }
            let mut port = [0u8; 2];
            if stream.read_exact(&mut port).await.is_err() {
                return;
            }
            let domain = str::from_utf8(&domain).unwrap_or("");
            let port = u16::from_be_bytes(port);
            format!("{}:{}", domain, port).parse().unwrap()
        }
        0x04 => {
            // IPv6 (not implemented)
            return;
        }
        _ => return,
    };

    // Connect to the target address
    match TcpStream::connect(address).await {
        Ok(target_stream) => {
            // Send success response
            let response = [
                0x05, 0x00, 0x00, 0x01, // Version, Success, Reserved, Address Type (IPv4)
                0x00, 0x00, 0x00, 0x00, // BND.ADDR (IPv4: 0.0.0.0)
                0x00, 0x00, // BND.PORT (0)
            ];
            if stream.write_all(&response).await.is_err() {
                return;
            }

            // Start bi-directional copy
            tokio::spawn(mutual_copy_io(stream, target_stream));
        }
        Err(_) => {
            // Send failure response
            let response = [
                0x05, 0x01, 0x00, 0x01, // Version, General failure, Reserved, Address Type (IPv4)
                0x00, 0x00, 0x00, 0x00, // BND.ADDR (IPv4: 0.0.0.0)
                0x00, 0x00, // BND.PORT (0)
            ];
            let _ = stream.write_all(&response).await;
        }
    }
}

async fn mutual_copy_io(conn0: TcpStream, conn1: TcpStream) {
    let (mut conn0_read, mut conn0_write) = split(conn0);
    let (mut conn1_read, mut conn1_write) = split(conn1);

    let (client_to_server_done_tx, client_to_server_done_rx) = oneshot::channel();
    let (server_to_client_done_tx, server_to_client_done_rx) = oneshot::channel();

    tokio::spawn(async move {
        let _ = copy(&mut conn0_read, &mut conn1_write).await;
        let _ = client_to_server_done_tx.send(());
    });

    tokio::spawn(async move {
        let _ = copy(&mut conn1_read, &mut conn0_write).await;
        let _ = server_to_client_done_tx.send(());
    });

    let _ = client_to_server_done_rx.await;
    let _ = server_to_client_done_rx.await;
}
