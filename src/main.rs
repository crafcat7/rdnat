/*************************************************
 * Use
 *************************************************/

use tokio::io::{copy, split, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use std::error::Error;
use hyper::{Body, Client, Request};
use hyper::body::HttpBody as _;
use std::str;
use std::env;
use std::fs::File;
use base64::encode;
use log::{info, error};

/*************************************************
 * Predefine
 *************************************************/

const DEFAULT_PORT: &str = "8000";
const DEFAULT_PASSWD: &str = "anonymous";
const DEFAULT_LOGPATH: &str = "rdnat.log";

/*************************************************
 * banner
 *************************************************/

fn banner() {
    println!("
            ____  ____  _   _____  ______
           / __ \\/ __ \\/ | / /   |/_  __/
          / /_/ / / / /  |/ / /| | / /
         / _, _/ /_/ / /|  / ___ |/ /
        /_/ |_/_____/_/ |_/_/  |_/_/
    ");
}

/*************************************************
 * help
 *************************************************/

fn help() {
    println!("Usage: rdnat [options] <username> <password>");
    println!();
    println!("Options:");
    println!("  -p <port>              Specify the port on which the proxy server will listen (default is 8000 if not provided)");
    println!("  -a <username> <password>  Specify the username and password for proxy authentication");
    println!("  -d, --debug            Enable debug logging to a log file (default log file is 'rdnat.log' in the current directory)");
    println!("  -h, --help             Display this help message and exit");
    println!();
    println!("Arguments:");
    println!("  <username>             The username for proxy authentication");
    println!("  <password>             The password for proxy authentication (ignored if no username is provided)");
    println!();
    println!("Examples:");
    println!("  ./rdnat                # Start the proxy with default settings: port 8000, no authentication");
    println!("  ./rdnat -p 8001        # Start the proxy on port 8001, no authentication");
    println!("  ./rdnat -a user passwd # Start the proxy with username 'user' and password 'passwd' on port 8000");
    println!("  ./rdnat -p 8001 -a user passwd # Start the proxy on port 8001 with username 'user' and password 'passwd'");
    println!("  ./rdnat -d             # Start the proxy with debug logging to 'rdnat.log'");
    println!("  ./rdnat -d -a user passwd # Enable debug logging and start the proxy with authentication");
}


/*************************************************
 * copy_io
 *************************************************/

async fn copy_io(conn0: TcpStream, conn1: TcpStream) {
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

/*************************************************
 * proxy_worker
 *************************************************/

async fn proxy_worker(mut stream: TcpStream, username: Option<String>, password: Option<String>) -> Result<(), Box<dyn Error>> {
    info!("HTTP connection from: {}", stream.peer_addr()?);
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
                if let Err(e) = stream.write_all(response.as_bytes()).await {
                    error!("Error writing authentication response: {}", e);
                }
                return Ok(());
            }
        }

        if let Err(_) = stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await {
            error!("Error sending 200 response to client");
            return Ok(());
        }

        let target_addr = parts[1];
        if let Ok(target_stream) = TcpStream::connect(target_addr).await {
            tokio::spawn(copy_io(stream, target_stream));
        } else {
            error!("Failed to connect to target address: {}", target_addr);
        }
    } else {
        let client = Client::new();
        let request = match Request::builder()
            .uri(String::from_utf8_lossy(&buffer[..n]).split_whitespace().nth(1).unwrap())
            .body(Body::from(buffer[..n].to_vec())) {
            Ok(req) => req,
            Err(_) => {
                error!("Failed to build request from buffer");
                return Ok(())
            },
        };

        match client.request(request).await {
            Ok(mut response) => {
                // Status
                let status_line = format!("HTTP/1.1 {}\r\n", response.status());
                if let Err(e) = stream.write_all(status_line.as_bytes()).await {
                    error!("Error writing status line to stream: {}", e);
                    return Ok(());
                }

                // Header
                for (key, value) in response.headers() {
                    let header_line = format!("{}: {}\r\n", key, value.to_str().unwrap());
                    if let Err(e) = stream.write_all(header_line.as_bytes()).await {
                        error!("Error writing header to stream: {}", e);
                        return Ok(());
                    }
                }

                if let Err(e) = stream.write_all(b"\r\n").await {
                    error!("Error writing body separator: {}", e);
                    return Ok(());
                }

                while let Some(chunk) = response.body_mut().data().await {
                    if let Ok(chunk) = chunk {
                        if let Err(e) = stream.write_all(&chunk).await {
                            error!("Error writing body chunk to stream: {}", e);
                            return Ok(());
                        }
                    }
                }
            }
            Err(e) => {
                error!("Request failed: {}", e);
                let _ = stream.write_all(b"HTTP/1.1 500 Internal Server Error\r\n\r\n").await;
            }
        }
    }

    Ok(())
}

/*************************************************
 * main
 *************************************************/

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut port = String::from(DEFAULT_PORT);
    let mut username = String::new();
    let mut password = String::new();
    let mut log_path: Option<String> = None;

    banner();

    let args: Vec<String> = env::args().collect();

    // Help flag
    if args.len() > 1 && (args[1] == "-h" || args[1] == "--help") {
        help();
        return Ok(());
    }

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            // Handle port flag
            "-p" | "--port" => {
                if i + 1 < args.len() {
                    port = args[i + 1].clone();
                    i += 2; // Skip the next argument as it was the port
                } else {
                    return Err("Error: Missing argument for -p or --port".into());
                }
            }
            // Handle authentication flag (-auth or -a)
            "-a" | "--auth" => {
                if i + 2 < args.len() {
                    username = args[i + 1].clone();
                    password = args[i + 2].clone();
                    i += 3; // Skip the username and password arguments
                } else {
                    return Err("Error: Missing username or password for -auth or -a".into());
                }
            }
            // Handle debug flag (-d or --debug)
            "-d" | "--debug" => {
                log_path = Some(DEFAULT_LOGPATH.to_string());
                i += 1; // Skip the debug flag
            }
            _ => {
                i += 1; // Skip unknown arguments
            }
        }
    }

    // Init logging
    let log_file_path = log_path.unwrap_or_else(|| String::from(DEFAULT_LOGPATH));
    let file = File::create(&log_file_path)?;
    env_logger::builder().target(env_logger::Target::Pipe(Box::new(file))).init();
    println!("Log file created at: {}", log_file_path);

    // Handle default password if not provided
    if username.is_empty() {
        password.clear(); // If no username, clear password
    } else if password.is_empty() {
        password = String::from(DEFAULT_PASSWD); // Use default password if not provided
    }

    // Display the proxy details
    println!("Starting proxy on port: {}", port);
    println!("Username: {}", username);
    println!("Password: {}", password);

    let username = if username.is_empty() { None } else { Some(username) };
    let password = if password.is_empty() { None } else { Some(password) };

    // Start listening for incoming connections
    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    println!("Listening on {}", port);

    loop {
        let (stream, _) = listener.accept().await?;
        let username = username.clone();
        let password = password.clone();

        tokio::spawn(async move {
            if let Err(e) = proxy_worker(stream, username, password).await {
                error!("[x] error: {}", e);
            }
        });
    }
}
