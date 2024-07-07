use tokio::io::{copy, split, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::time::{sleep, Duration};
use std::env;
use std::error::Error;
use hyper::{Body, Client, Request};
use hyper::body::HttpBody as _;  // for `data` method
use std::str;
use base64::encode;

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
                let no_auth = args.contains(&String::from("-no-auth"));
                let procotol = &args[2];
                let listen_address = &args[3];

                if no_auth {
                    proxy(procotol, &listen_address, None, None).await?
                } else {
                    if args.len() < 6 {
                        help();
                        return Ok(());
                    }

                    let username = &args[4];
                    let password = &args[5];
                    proxy(procotol, listen_address, Some(username.to_string()), Some(password.to_string())).await?
                }
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
    println!("  -proxy <protocol> <listen_address> [-no-auth | <username> <password>]  Act as a proxy (HTTP, SOCKS5) based on specified protocol.");
    println!("\n");
    println!("  HTTP proxy usage:");
    println!("    -proxy http <listen_address> -no-auth");
    println!("    -proxy http <listen_address> <username> <password>");
    println!("  SOCKS5 proxy usage:");
    println!("    -proxy socks5 <listen_address> -no-auth");
    println!("    -proxy socks5 <listen_address> <username> <password>");
    println!("\nExamples:");
    println!("  -listen 8000 8001");
    println!("  -agent 192.168.1.100:8000 192.168.1.101:8001");
    println!("  -forward 8000 192.168.1.102:8000");
    println!("  -proxy http 8888 -no-auth");
    println!("  -proxy http 8888 username password");
    println!("  -proxy socks5 1080 -no-auth");
    println!("  -proxy socks5 1080 username password");
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

async fn proxy(protocol: &str, listen_address: &str, username: Option<String>, password: Option<String>) -> Result<(), Box<dyn Error>> {
    match protocol {
        "http" => {
            let listener = TcpListener::bind(format!("0.0.0.0:{}", listen_address)).await?;
            println!("[*] HTTP proxy listening on: {}", listen_address);

            loop {
                let (stream, _) = listener.accept().await?;
                let username = username.clone();
                let password = password.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_http_proxy(stream, username, password).await {
                        eprintln!("[x] error: {}", e);
                    }
                });
            }
        }
        "socks5" => {
            let listener = TcpListener::bind(format!("0.0.0.0:{}", listen_address)).await?;
            println!("[*] SOCKS5 proxy listening on: {}", listen_address);

            loop {
                let (stream, _) = listener.accept().await?;
                let username = username.clone();
                let password = password.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_socks5_proxy(stream, username, password).await {
                        eprintln!("[x] error: {}", e);
                    }
                });
            }
        }
        _ => {
            eprintln!("protocol must be either http or socks5");
        }
    }

    Ok(())
}

async fn handle_http_proxy(mut stream: TcpStream, username: Option<String>, password: Option<String>) -> Result<(), Box<dyn Error>> {
    let mut buffer = [0u8; 4096];
    let n = match stream.read(&mut buffer).await {
        Ok(n) if n == 0 => return Ok(()),
        Ok(n) => n,
        Err(_) => return Ok(()),
    };

    let request_line = String::from_utf8_lossy(&buffer[..n]);
    if request_line.starts_with("CONNECT") {
        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 3 {
            return Ok(());
        }

        if let (Some(username), Some(password)) = (&username, &password) {
            let auth_header = format!("Proxy-Authorization: Basic {}", encode(format!("{}:{}", username, password)));
            if !request_line.contains(&auth_header) {
                let response = "HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"Proxy\"\r\n\r\n";
                stream.write_all(response.as_bytes()).await?;
                return Ok(());
            }
        }

        if let Err(_) = stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await {
            return Ok(());
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
            Err(_) => return Ok(()),
        };

        match client.request(request).await {
            Ok(mut response) => {
                let status_line = format!("HTTP/1.1 {}\r\n", response.status());
                if let Err(_) = stream.write_all(status_line.as_bytes()).await {
                    return Ok(());
                }

                for (key, value) in response.headers() {
                    let header_line = format!("{}: {}\r\n", key, value.to_str().unwrap());
                    if let Err(_) = stream.write_all(header_line.as_bytes()).await {
                        return Ok(());
                    }
                }

                if let Err(_) = stream.write_all(b"\r\n").await {
                    return Ok(());
                }

                while let Some(chunk) = response.body_mut().data().await {
                    if let Ok(chunk) = chunk {
                        if let Err(_) = stream.write_all(&chunk).await {
                            return Ok(());
                        }
                    }
                }
            }
            Err(_) => {
                let _ = stream.write_all(b"HTTP/1.1 500 Internal Server Error\r\n\r\n").await;
            }
        }
    }

    Ok(())
}

async fn handle_socks5_proxy(mut stream: TcpStream, username: Option<String>, password: Option<String>) -> Result<(), Box<dyn Error>> {
    let mut buf = [0u8; 2];
    if stream.read_exact(&mut buf).await.is_err() {
        return Ok(());
    }

    if buf[0] != 0x05 {
        return Ok(());
    }

    let num_methods = buf[1] as usize;
    let mut methods = vec![0u8; num_methods];
    if stream.read_exact(&mut methods).await.is_err() {
        return Ok(());
    }

    if username.is_some() && password.is_some() {
        if !methods.contains(&0x02) {
            let response = [0x05, 0xFF];
            stream.write_all(&response).await?;
            return Ok(());
        }

        let response = [0x05, 0x02];
        stream.write_all(&response).await?;

        let mut buf = [0u8; 512];
        stream.read_exact(&mut buf[..2]).await?;
        let uname_len = buf[1] as usize;
        let (head, tail) = buf.split_at_mut(2 + uname_len);
        let provided_username = String::from_utf8_lossy(&head[2..2 + uname_len]).to_string();

        stream.read_exact(&mut tail[..1]).await?;
        let passwd_len = tail[0] as usize;
        let (head, _tail) = tail.split_at_mut(1 + passwd_len);
        stream.read_exact(&mut head[..passwd_len]).await?;
        let provided_password = String::from_utf8_lossy(&head[..passwd_len]).to_string();

        if provided_username != username.unwrap_or_default() || provided_password != password.unwrap_or_default() {
            let response = [0x01, 0x01];
            stream.write_all(&response).await?;
            return Ok(());
        }

        let response = [0x01, 0x00];
        stream.write_all(&response).await?;
    } else {
        if !methods.contains(&0x00) {
            let response = [0x05, 0xFF];
            stream.write_all(&response).await?;
            return Ok(());
        }

        let response = [0x05, 0x00];
        stream.write_all(&response).await?;
    }

    let mut buf = [0u8; 4];
    if stream.read_exact(&mut buf).await.is_err() {
        return Ok(());
    }

    if buf[1] != 0x01 {
        return Ok(());
    }

    let address = match buf[3] {
        0x01 => {
            let mut ipv4 = [0u8; 4];
            if stream.read_exact(&mut ipv4).await.is_err() {
                return Ok(());
            }
            format!("{}.{}.{}.{}", ipv4[0], ipv4[1], ipv4[2], ipv4[3])
        }
        0x03 => {
            let mut len = [0u8; 1];
            if stream.read_exact(&mut len).await.is_err() {
                return Ok(());
            }
            let mut domain = vec![0u8; len[0] as usize];
            if stream.read_exact(&mut domain).await.is_err() {
                return Ok(());
            }
            String::from_utf8_lossy(&domain).to_string()
        }
        0x04 => {
            let mut ipv6 = [0u8; 16];
            if stream.read_exact(&mut ipv6).await.is_err() {
                return Ok(());
            }
            format!("{:X}:{:X}:{:X}:{:X}:{:X}:{:X}:{:X}:{:X}", 
                ((ipv6[0] as u16) << 8) | (ipv6[1] as u16),
                ((ipv6[2] as u16) << 8) | (ipv6[3] as u16),
                ((ipv6[4] as u16) << 8) | (ipv6[5] as u16),
                ((ipv6[6] as u16) << 8) | (ipv6[7] as u16),
                ((ipv6[8] as u16) << 8) | (ipv6[9] as u16),
                ((ipv6[10] as u16) << 8) | (ipv6[11] as u16),
                ((ipv6[12] as u16) << 8) | (ipv6[13] as u16),
                ((ipv6[14] as u16) << 8) | (ipv6[15] as u16))
        }
        _ => return Ok(()),
    };

    let mut port = [0u8; 2];
    if stream.read_exact(&mut port).await.is_err() {
        return Ok(());
    }
    let port = ((port[0] as u16) << 8) | port[1] as u16;

    match TcpStream::connect((address.as_str(), port)).await {
        Ok(mut target_stream) => {
            let mut response = vec![0x05, 0x00, 0x00, 0x01];
            response.extend_from_slice(&[0, 0, 0, 0]);
            response.extend_from_slice(&[0, 0]);
            stream.write_all(&response).await?;

            let (mut ri, mut wi) = stream.split();
            let (mut ro, mut wo) = target_stream.split();

            let client_to_server = tokio::io::copy(&mut ri, &mut wo);
            let server_to_client = tokio::io::copy(&mut ro, &mut wi);

            tokio::try_join!(client_to_server, server_to_client).ok();
        }
        Err(_) => {
            let response = [0x05, 0x01];
            stream.write_all(&response).await?;
        }
    }

    Ok(())
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
