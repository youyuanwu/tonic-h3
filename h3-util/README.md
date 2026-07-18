# h3-util

HTTP/3 server and client utilities used by [`tonic-h3`](https://github.com/youyuanwu/tonic-h3) and `axum-h3`.

This crate abstracts QUIC transports behind two traits so the same HTTP/3 server and client code can run over any supported backend:

* `client::H3Connector` — establishes client-side QUIC connections.
* `server::H3Acceptor` — accepts server-side QUIC connections.

On the client side, `client::H3Channel` wraps an `H3Connector` into a `tower::Service` / `tonic`-compatible channel that transparently (re)connects when the underlying HTTP/3 driver ends.

## Backends

Each backend is gated behind a cargo feature and provides its own `H3Connector` / `H3Acceptor` implementations:

| Feature | Backend | Support |
|---------|---------|---------|
| `quinn` | [Quinn](https://github.com/quinn-rs/quinn) ([h3-quinn](https://github.com/hyperium/h3/h3-quinn/)) | **production** |
| `msquic` | [MsQuic](https://github.com/microsoft/msquic) ([msquic-h3](https://github.com/youyuanwu/msquic-h3)) | experimental |
| `s2n-quic` | [s2n-quic](https://github.com/aws/s2n-quic) ([s2n-quic-h3](https://github.com/aws/s2n-quic/tree/main/quic/s2n-quic-h3)) | experimental |
| `quiche` | [quiche](https://github.com/cloudflare/quiche) ([quiche-h3](https://github.com/youyuanwu/quiche-h3)) | experimental |

No backend is enabled by default; enable the one you need.

> **Note:** Only the **quinn** backend is supported for production use. The `msquic`, `s2n-quic`, and `quiche` backends are **experimental** and provided for evaluation only. They have known limitations — for example, they do not release the listening UDP socket promptly on server shutdown, so a server cannot immediately rebind the same port.

## Usage

```toml
[dependencies]
h3-util = { version = "*", default-features = false, features = ["quinn"] }
```