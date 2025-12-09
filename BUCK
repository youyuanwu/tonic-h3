load("@prelude//rust:cargo_package.bzl", "cargo")

# h3-util library
cargo.rust_library(
    name = "h3-util",
    srcs = glob([
        "h3-util/src/**/*.rs",
    ]),
    crate_root = "h3-util/src/lib.rs",
    visibility = ["PUBLIC"],
    deps = [
        "//third-party:bytes",
        "//third-party:futures",
        "//third-party:h3",
        "//third-party:hyper",
        "//third-party:hyper-util",
        "//third-party:tokio",
        "//third-party:tower",
        "//third-party:tracing",
    ],
    features = [],
)

# axum-h3 library
cargo.rust_library(
    name = "axum-h3",
    srcs = glob([
        "axum-h3/src/**/*.rs",
    ]),
    crate_root = "axum-h3/src/lib.rs",
    visibility = ["PUBLIC"],
    deps = [
        ":h3-util",
        "//third-party:axum",
        "//third-party:futures",
        "//third-party:h3",
        "//third-party:hyper",
        "//third-party:hyper-util",
        "//third-party:tokio",
        "//third-party:tower",
        "//third-party:tracing",
    ],
)

# tonic-h3 library
cargo.rust_library(
    name = "tonic-h3",
    srcs = glob([
        "tonic-h3/src/**/*.rs",
    ]),
    crate_root = "tonic-h3/src/lib.rs",
    visibility = ["PUBLIC"],
    deps = [
        ":axum-h3",
        ":h3-util",
        "//third-party:futures",
        "//third-party:http",
        "//third-party:hyper",
        "//third-party:tonic",
        "//third-party:tower",
    ],
)

# tonic-h3-test library
cargo.rust_library(
    name = "tonic-h3-test",
    srcs = glob([
        "tonic-h3-tests/src/**/*.rs",
    ]),
    crate_root = "tonic-h3-tests/src/lib.rs",
    visibility = ["PUBLIC"],
    deps = [
        ":axum-h3",
        ":h3-util",
        ":tonic-h3",
        "//third-party:axum",
        "//third-party:futures",
        "//third-party:futures-util",
        "//third-party:http",
        "//third-party:http-body",
        "//third-party:http-body-util",
        "//third-party:hyper",
        "//third-party:msquic",
        "//third-party:prost",
        "//third-party:quinn",
        "//third-party:rcgen",
        "//third-party:rustls",
        "//third-party:serial_test",
        "//third-party:tokio",
        "//third-party:tokio-quiche",
        "//third-party:tokio-util",
        "//third-party:tonic",
        "//third-party:tonic-prost",
        "//third-party:tracing",
        "//third-party:tracing-subscriber",
    ],
)
