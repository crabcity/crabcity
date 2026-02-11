load("@rules_rust//rust:defs.bzl", "rust_binary")

rust_binary(
    name = "list_conversations",
    srcs = ["list_conversations.rs"],
    edition = "2024",
    deps = [
        "//packages/claude_convo",
        "@crate_index//:anyhow",
        "@crate_index//:chrono",
    ],
)
