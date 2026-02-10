load("@rules_rust//rust:defs.bzl", "rust_binary", "rust_library", "rust_test")

rust_library(
    name = "claude_convo",
    srcs = glob(["src/**/*.rs"]),
    edition = "2024",
    visibility = ["//visibility:public"],
    deps = [
        "@crate_index//:anyhow",
        "@crate_index//:chrono",
        "@crate_index//:serde",
        "@crate_index//:serde_json",
        "@crate_index//:thiserror",
    ],
)

rust_test(
    name = "claude_convo_test",
    crate = ":claude_convo",
    edition = "2024",
    deps = [
        "@crate_index//:tempfile",
    ],
)

rust_binary(
    name = "claude-cli",
    srcs = ["src/bin/claude-cli.rs"],
    edition = "2024",
    deps = [
        ":claude_convo",
        "@crate_index//:anyhow",
        "@crate_index//:chrono",
        "@crate_index//:clap",
        "@crate_index//:serde",
        "@crate_index//:serde_json",
    ],
)
