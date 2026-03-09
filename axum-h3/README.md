# axum-h3

HTTP/3 server adapter for [axum](https://github.com/tokio-rs/axum) using the [h3](https://github.com/hyperium/h3) crate. Developed as part of [tonic-h3](https://github.com/youyuanwu/tonic-h3).

Accepts QUIC connections via any [`h3-util::H3Acceptor`](../h3-util) backend (Quinn, MsQuic, s2n-quic) and serves them through axum's `Router`.

## Usage

```toml
[dependencies]
axum-h3 = "*"
h3-util = { version = "*", default-features = false, features = ["quinn"] }
```

See [tests](../tonic-h3-tests/src/axum.rs) for examples.