/*************************************************
 * Use
 *************************************************/

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use std::error::Error;
use hyper::{Body, Client, Request};
use hyper::body::HttpBody as _;
use std::str;
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

async fn copy_io(mut stream1: TcpStream, mut stream2: TcpStream) {
    let (mut r1, mut w1) = stream1.split();
    let (mut r2, mut w2) = stream2.split();

    let (res1, res2) = tokio::join!(
        tokio::io::copy(&mut r1, &mut w2),
        tokio::io::copy(&mut r2, &mut w1)
    );

    if let Err(e) = res1 {
        log::error!("Error copying from stream1 to stream2: {}", e);
    }

    if let Err(e) = res2 {
        log::error!("Error copying from stream2 to stream1: {}", e);
    }
}

/*************************************************
 * handle_tunneling
 *************************************************/

async fn handle_tunneling(
    mut stream: TcpStream,
    target_addr: &str,
) -> Result<(), Box<dyn Error>> {
    let target_stream = TcpStream::connect(target_addr).await?;
    stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
    tokio::spawn(copy_io(stream, target_stream));
    Ok(())
}

/*************************************************
 * handle_http_request
 *************************************************/

async fn handle_http_request(
    mut stream: TcpStream,
    buffer: &[u8],
    n: usize,
) -> Result<(), Box<dyn Error>> {
    let client = Client::new();

    let uri = {
        let raw_uri = String::from_utf8_lossy(&buffer[..n]);
        raw_uri.split_whitespace().nth(1).unwrap_or_default().to_string()
    };

    let request = Request::builder()
        .uri(uri)
        .body(Body::from(buffer[..n].to_vec()))?;

    let mut response = client.request(request).await?;
    stream.write_all(format!("HTTP/1.1 {}\r\n", response.status()).as_bytes()).await?;
    for (key, value) in response.headers() {
        let header_line = format!("{}: {}\r\n", key, value.to_str().unwrap());
        stream.write_all(header_line.as_bytes()).await?;
    }
    stream.write_all(b"\r\n").await?;
    while let Some(chunk) = response.body_mut().data().await {
        if let Ok(chunk) = chunk {
            stream.write_all(&chunk).await?;
        }
    }
    Ok(())
}

/*************************************************
 * proxy_worker
 *************************************************/

async fn proxy_worker(
    mut stream: TcpStream,
    username: Option<String>,
    password: Option<String>,
) -> Result<(), Box<dyn Error>> {
    info!("HTTP connection from: {}", stream.peer_addr()?);
    let mut buffer = [0u8; 4096];
    let n = stream.read(&mut buffer).await?;

    if n == 0 {
        return Ok(());
    }

    let request_line = String::from_utf8_lossy(&buffer[..n]);
    if request_line.starts_with("CONNECT") {
        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 3 {
            return Ok(());
        }

        if let (Some(username), Some(password)) = (&username, &password) {
            let auth_header = format!(
                "Proxy-Authorization: Basic {}",
                encode(format!("{}:{}", username, password))
            );
            if !request_line.contains(&auth_header) {
                let response = "HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"Proxy\"\r\n\r\n";
                stream.write_all(response.as_bytes()).await?;
                return Ok(());
            }
        }

        handle_tunneling(stream, parts[1]).await?;
    } else {
        handle_http_request(stream, &buffer, n).await?;
    }

    Ok(())
}

/*************************************************
 * init_logging
 *************************************************/

fn init_logging(log_path: Option<String>) -> Result<(), Box<dyn Error>> {
    let log_file_path = log_path.unwrap_or_else(|| String::from(DEFAULT_LOGPATH));
    let file = std::fs::File::create(&log_file_path)?;
    env_logger::builder().target(env_logger::Target::Pipe(Box::new(file))).init();
    println!("Log file created at: {}", log_file_path);
    Ok(())
}

/*************************************************
 * parse_arguments
 *************************************************/

fn parse_arguments(
    args: &[String],
    port: &mut String,
    username: &mut String,
    password: &mut String,
    log_path: &mut Option<String>,
) -> Result<(), Box<dyn Error>> {
    if args.len() > 1 && (args[1] == "-h" || args[1] == "--help") {
        help();
        return Ok(());
    }

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-p" | "--port" => {
                if i + 1 < args.len() {
                    *port = args[i + 1].clone();
                    i += 2;
                } else {
                    return Err("Error: Missing argument for -p or --port".into());
                }
            }
            "-a" | "--auth" => {
                if i + 2 < args.len() {
                    *username = args[i + 1].clone();
                    *password = args[i + 2].clone();
                    i += 3;
                } else {
                    return Err("Error: Missing username or password for -auth or -a".into());
                }
            }
            "-d" | "--debug" => {
                *log_path = Some(DEFAULT_LOGPATH.to_string());
                i += 1;
            }
            _ => {
                eprintln!("Warning: Unknown argument: {}", args[i]);
                i += 1;
            }
        }
    }

    if username.is_empty() {
        password.clear();
    } else if password.is_empty() {
        *password = String::from(DEFAULT_PASSWD);
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
    let args: Vec<String> = std::env::args().collect();
    parse_arguments(&args, &mut port, &mut username, &mut password, &mut log_path)?;

    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    println!("Proxy listening on port: {}", port);
    if !username.is_empty() {
        println!("Username: {}", username);
        println!("Password: {}", password);
    }

    init_logging(log_path)?;

    let username = if username.is_empty() { None } else { Some(username) };
    let password = if password.is_empty() { None } else { Some(password) };

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
