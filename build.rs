use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const GHOSTTY_REVISION: &str = "4c725242b7dbe8c77c6e227ef1f9540c5ef17921";

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let ghostty = root.join("vendor/ghostty");
    assert!(
        ghostty.join("build.zig").is_file(),
        "missing Ghostty submodule; run git submodule update --init"
    );

    println!("cargo:rerun-if-env-changed=ZIG");
    println!("cargo:rerun-if-changed=native/ghostty_bridge.c");
    println!("cargo:rerun-if-changed=native/ghostty_bridge.h");
    println!("cargo:rerun-if-changed=vendor/ghostty/include");

    build_ghostty(&ghostty);
    build_bridge(&root, &ghostty);
    println!("cargo:rustc-env=GHOSTTY_SOURCE_REVISION={GHOSTTY_REVISION}");
}

fn build_ghostty(ghostty: &Path) {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let install = out.join("ghostty-install");
    let cache = out.join("ghostty-zig-cache");
    let target = env::var("TARGET").expect("TARGET");
    let zig = env::var_os("ZIG").unwrap_or_else(|| OsString::from("zig"));

    let status = Command::new(zig)
        .arg("build")
        .arg("-Demit-lib-vt=true")
        .arg("-Demit-xcframework=false")
        .arg("-Dapp-runtime=none")
        .arg(format!("-Doptimize={}", optimize_mode()))
        .arg("--prefix")
        .arg(&install)
        .arg("--cache-dir")
        .arg(&cache)
        .arg(format!("-Dtarget={}", zig_target(&target)))
        .current_dir(ghostty)
        .status()
        .expect("launch Zig Ghostty build");
    assert!(status.success(), "Ghostty Zig build failed: {status}");

    let lib_dir = install.join("lib");
    if target.contains("windows") {
        let source = lib_dir.join("ghostty-vt-static.lib");
        assert!(source.is_file(), "missing {}", source.display());
        let link_dir = out.join("ghostty-rust-link");
        fs::create_dir_all(&link_dir).expect("create Windows link directory");
        fs::copy(&source, link_dir.join("ghostty-vt.lib")).expect("copy Ghostty static library");
        println!("cargo:rustc-link-search=native={}", link_dir.display());
        println!("cargo:rustc-link-lib=ntdll");
        println!("cargo:rustc-link-lib=kernel32");
    } else {
        assert!(
            lib_dir.join("libghostty-vt.a").is_file(),
            "missing libghostty-vt.a"
        );
        println!("cargo:rustc-link-search=native={}", lib_dir.display());
    }
    println!("cargo:rustc-link-lib=static=ghostty-vt");
}

fn build_bridge(root: &Path, ghostty: &Path) {
    cc::Build::new()
        .file(root.join("native/ghostty_bridge.c"))
        .include(root.join("native"))
        .include(ghostty.join("include"))
        .define("GHOSTTY_STATIC", None)
        .warnings(true)
        .compile("ghostty_spike_bridge");
}

fn optimize_mode() -> &'static str {
    if env::var("DEBUG").as_deref() == Ok("true") {
        "Debug"
    } else {
        "ReleaseFast"
    }
}

fn zig_target(target: &str) -> &'static str {
    match target {
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "aarch64-apple-darwin" => "aarch64-macos-none",
        "x86_64-apple-darwin" => "x86_64-macos-none",
        "x86_64-pc-windows-msvc" => "x86_64-windows-msvc",
        other => panic!("unsupported target: {other}"),
    }
}
