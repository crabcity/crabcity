load("@rules_rust//rust:defs.bzl", "rust_binary", "rust_library", "rust_test")

rust_library(
    name = "tty_wrapper",
    srcs = glob(["src/**/*.rs"]),
    edition = "2021",
    visibility = ["//visibility:public"],
    deps = [
        "@crate_index//:anyhow",
        "@crate_index//:axum",
        "@crate_index//:axum-extra",
        "@crate_index//:chrono",
        "@crate_index//:clap",
        "@crate_index//:futures",
        "@crate_index//:futures-util",
        "@crate_index//:portable-pty",
        "@crate_index//:serde",
        "@crate_index//:serde_json",
        "@crate_index//:tokio",
        "@crate_index//:tower",
        "@crate_index//:tower-http",
        "@crate_index//:tracing",
        "@crate_index//:tracing-subscriber",
        "@crate_index//:uuid",
    ] + select({
        "@platforms//os:linux": ["@crate_index//:nix"],
        "@platforms//os:macos": ["@crate_index//:nix"],
        "//conditions:default": [],
    }),
)

rust_binary(
    name = "wrapper",
    srcs = glob(["src/**/*.rs"]),
    edition = "2021",
    visibility = ["//visibility:public"],
    deps = [
        "@crate_index//:anyhow",
        "@crate_index//:axum",
        "@crate_index//:chrono",
        "@crate_index//:clap",
        "@crate_index//:futures",
        "@crate_index//:futures-util",
        "@crate_index//:maud",
        "@crate_index//:portable-pty",
        "@crate_index//:serde",
        "@crate_index//:serde_json",
        "@crate_index//:tokio",
        "@crate_index//:tower",
        "@crate_index//:tower-http",
        "@crate_index//:tracing",
        "@crate_index//:tracing-subscriber",
        "@crate_index//:uuid",
    ] + select({
        "@platforms//os:linux": ["@crate_index//:nix"],
        "@platforms//os:macos": ["@crate_index//:nix"],
        "//conditions:default": [],
    }),
)
