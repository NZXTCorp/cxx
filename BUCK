rust_library(
    name = "cxx",
    srcs = glob(["src/**"]),
    visibility = ["PUBLIC"],
    deps = [
        ":core",
        ":macro",
        "//third-party:anyhow",
        "//third-party:cc",
        "//third-party:codespan-reporting",
        "//third-party:link-cplusplus",
        "//third-party:proc-macro2",
        "//third-party:quote",
        "//third-party:syn",
        "//third-party:thiserror",
    ],
)

rust_binary(
    name = "codegen",
    srcs = glob(["cmd/src/**"]),
    visibility = ["PUBLIC"],
    env = {
        "CARGO_PKG_AUTHORS": "David Tolnay <dtolnay@gmail.com>",
    },
    deps = [
        "//third-party:anyhow",
        "//third-party:codespan-reporting",
        "//third-party:proc-macro2",
        "//third-party:quote",
        "//third-party:structopt",
        "//third-party:syn",
        "//third-party:thiserror",
    ],
)

cxx_library(
    name = "core",
    srcs = ["src/cxx.cc"],
    visibility = ["PUBLIC"],
    header_namespace = "rust",
    exported_headers = {
        "cxx.h": "include/cxx.h",
    },
    exported_linker_flags = ["-lstdc++"],
)

rust_library(
    name = "macro",
    srcs = glob(["macro/src/**"]),
    proc_macro = True,
    crate = "cxxbridge_macro",
    deps = [
        "//third-party:proc-macro2",
        "//third-party:quote",
        "//third-party:syn",
    ],
)
