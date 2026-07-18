# axum-h3

HTTP/3 server adapter for [axum](https://github.com/tokio-rs/axum) using the [h3](https://github.com/hyperium/h3) crate. Developed as part of [tonic-h3](https://github.com/youyuanwu/tonic-h3).

Serves an `axum::Router` over HTTP/3 by accepting QUIC connections from any [`h3-util`](../h3-util) backend (via the `H3Acceptor` trait) and dispatching each request to the router. Each connection is served on its own task, and each request on that connection is spawned separately using a `SharedExec` executor (Tokio by default).

The concrete QUIC backend is chosen through `h3-util`'s cargo features:

| Feature | Backend | Support |
|---------|---------|---------|
| `quinn` | [Quinn](https://github.com/quinn-rs/quinn) ([h3-quinn](https://github.com/hyperium/h3/h3-quinn/)) | **production** |
| `msquic` | [MsQuic](https://github.com/microsoft/msquic) ([msquic-h3](https://github.com/youyuanwu/msquic-h3)) | experimental |
| `s2n-quic` | [s2n-quic](https://github.com/aws/s2n-quic) ([s2n-quic-h3](https://github.com/aws/s2n-quic/tree/main/quic/s2n-quic-h3)) | experimental |
| `quiche` | [quiche](https://github.com/cloudflare/quiche) ([quiche-h3](https://github.com/youyuanwu/quiche-h3)) | experimental |

> **Note:** Only the **quinn** backend is supported for production use. The `msquic`, `s2n-quic`, and `quiche` backends are **experimental** and provided for evaluation only.

## Usage

```toml
[dependencies]
axum-h3 = "*"
h3-util = { version = "*", default-features = false, features = ["quinn"] }
```

```rust,ignore
use axum::{routing::get, Router};
use axum_h3::H3Router;
use h3_util::server::H3Acceptor;

// `acceptor` is any `H3Acceptor` from an h3-util backend (e.g. quinn).
async fn serve<A: H3Acceptor>(acceptor: A) -> Result<(), h3_util::Error> {
    let app = Router::new().route("/", get(|| async { "hello over h3" }));
    H3Router::from(app).serve(acceptor).await
}
```

See [tests](../tonic-h3-tests/src/axum.rs) for runnable examples.