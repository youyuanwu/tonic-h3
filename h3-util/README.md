# h3-util

[h3-util](./h3-util/) contains HTTP/3 server and client utilities used by `axum-h3` and `tonic-h3`.
See repo [tonic-h3](https://github.com/youyuanwu/tonic-h3) for details.

## Get started
Add deps to your cargo.toml
```toml
h3-util = { version="*" , default-features = false, features = ["quinn"] }
```