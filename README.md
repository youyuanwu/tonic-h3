# tonic-h3
[![build](https://github.com/youyuanwu/msquic-h3/actions/workflows/build.yaml/badge.svg)](https://github.com/youyuanwu/msquic-h3/actions/workflows/build.yaml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://raw.githubusercontent.com/youyuanwu/tonic-h3/main/LICENSE)

Experimental implementation of running [tonic](https://github.com/hyperium/tonic) grpc on [h3](https://github.com/hyperium/h3), following the proposal: [G2-http3-protocol](https://github.com/grpc/proposal/blob/master/G2-http3-protocol.md)

tonic-h3 is targeted to support all quic transport implementations that integrates with h3. Currently the following are incoporated and tested:
* [Quinn](https://github.com/quinn-rs/quinn) ([h3-quinn](https://github.com/hyperium/h3/h3-quinn/))
* [s2n-quic](https://github.com/aws/s2n-quic) ([s2n-quic-h3](https://github.com/aws/s2n-quic/tree/main/quic/s2n-quic-h3))

Not yet working:
* [msquic](https://github.com/microsoft/msquic) ([msquic-h3](https://github.com/youyuanwu/msquic-h3))  Need to rewrite msquic-h3.

See [examples](./tonic-h3-tests/examples/) and [tests](./tonic-h3-tests/src/lib.rs) for getting started.

Compatibility with grpc-dotnet with http3 is tested [here](./dotnet/).

## License

MIT license. See [LICENSE](LICENSE).