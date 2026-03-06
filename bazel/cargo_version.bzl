"""Extract the package version from a Cargo.toml at repo-setup time."""

def _cargo_version_repo_impl(rctx):
    content = rctx.read(rctx.path(rctx.attr.cargo_toml))
    for line in content.split("\n"):
        line = line.strip()
        if line.startswith("version = \""):
            version = line.split('"')[1]
            rctx.file("BUILD.bazel", "")
            rctx.file("defs.bzl", 'VERSION = "{}"\n'.format(version))
            return
    fail("Could not parse version from " + str(rctx.attr.cargo_toml))

_cargo_version_repo = repository_rule(
    implementation = _cargo_version_repo_impl,
    attrs = {"cargo_toml": attr.label(mandatory = True)},
)

def _ext_impl(module_ctx):
    for mod in module_ctx.modules:
        for tag in mod.tags.parse:
            _cargo_version_repo(
                name = tag.name,
                cargo_toml = tag.cargo_toml,
            )

_parse_tag = tag_class(attrs = {
    "name": attr.string(mandatory = True),
    "cargo_toml": attr.label(mandatory = True),
})

cargo_version = module_extension(
    implementation = _ext_impl,
    tag_classes = {"parse": _parse_tag},
)
