# rdnat
This is a dynamic network address translation (DNAT) tool based on Rust and Tokio asynchronous runtime library. The tool supports network monitoring, data forwarding, proxy server functions and other network operations.

## Installation

Clone the repository and build the project:

```shell
git clone https://github.com/crafcat7/rdnat.git
cd rdnat
cargo build --release
```

## Usage

```shell
rdnat [options] <username> <password>
```

Based on the HTTP, the function will start a proxy server. It is to monitor the port specified.

- Start the proxy with a specified username and password, using the default port (8000):

```shell
./rdnat user password
```

- Start the proxy, specifying port 8001 with a username and password:

```shell
./rdnat -p 8001 user password
```

- Start the proxy, specifying port 8001 with a username and default password (anonymous):

```shell
./rdnat -p 8001 user
```

- Start the proxy with no username and no password, using the default port (8000):

```shell
./rdnat
```

- Start the proxy and log output to a specified file:

```shell
./rdnat -d /path/to/logfile.log
```

- Start the proxy and log output to rdnat.log:

```shell
./rdnat -d rdnat.log
```
