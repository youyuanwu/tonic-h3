# h3-util

HTTP/3 server and client utilities used by [`tonic-h3`](https://github.com/youyuanwu/tonic-h3) and `axum-h3`.

Abstracts QUIC transports behind `H3Connector` (client) and `H3Acceptor` (server) traits, with built-in support for multiple backends:

| Feature | Backend |
|---------|---------|
| `quinn` | [Quinn](https://github.com/quinn-rs/quinn) |
| `msquic` | [MsQuic](https://github.com/youyuanwu/msquic-h3) |
| `s2n-quic` | [s2n-quic](https://github.com/aws/s2n-quic) |
| `quiche` | [quiche](https://github.com/cloudflare/quiche) |

## Usage

```toml
[dependencies]
h3-util = { version = "*", default-features = false, features = ["quinn"] }
```