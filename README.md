# rdnat
This is a dynamic network address translation (DNAT) tool based on Rust and Tokio asynchronous runtime library. The tool supports network monitoring, data forwarding, proxy server functions and other network operations.

## Installation

Clone the repository and build the project:

```shell
git clone https://github.com/crafcat7/rdnat.git
cd rdnat
cargo build --release
```

### Listening on Ports

This function starts two TCP listener port, waiting for the connection from the two ports. Once the two ports to receive connection at the same time, will start between them.

``` shell
./rdnat -listen 8000 8001
```

This command will be in the machine start listening on ports 8000 and 8001.

### Agent

In proxy mode, the tool will connect to two specified destination addresses and forward data between the two connections. This is similar to the listening function, but in this case the destination address is remote, rather than a local listening port.

```shell
./rdnat -agent 192.168.1.100:8000 192.168.1.101:8001
```

This will connect to port 8000 of 192.168.1.100 and port 8001 of 192.168.1.101 respectively and forward data between the two.

### Forward

The forwarding function enables the application to listen to the local port and forward all received connections and their data to a specified remote address.

```shell
./rdnat -forward 8000 192.168.1.102:8000
```

This will be the local port 8000 listeners, all inbound connections will be forwarded to the remote address 192.168.1.102 8000.

### Proxy

Based on the specified protocol (HTTP or SOCKS5), the function will start a proxy server. It is to monitor the address specified.

- HTTP proxy without authentication

```shell
./rdnat -proxy http 8888 -no-auth
```

- HTTP proxy with authentication

```shell
./rdnat -proxy http 8888 <username> <password>
```

- SOCKET5 proxy without authentication

```shell
./rdnat -proxy socks5 1080 -no-auth
```

- SOCKS5 proxy with authentication

```shell
./rdnat -proxy socks5 1080 <username> <password>
```
