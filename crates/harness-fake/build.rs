//! Compiles `src/program.rs`, the fake program, with the same compiler, so
//! the tests of other crates get a ready program without a shell.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/program.rs");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let target = env::var("TARGET").expect("cargo sets TARGET");
    // The program runs where the tests run: on Windows it needs `.exe`.
    let suffix = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    let program = out.join(format!("harness-fake{suffix}"));
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let status = Command::new(rustc)
        .args(["--edition", "2021", "--crate-name", "harness_fake_program"])
        .args(["--target", &target])
        .arg("-o")
        .arg(&program)
        .arg("src/program.rs")
        .status()
        .expect("cannot start rustc");
    assert!(status.success(), "rustc could not compile src/program.rs");
    println!("cargo:rustc-env=HARNESS_FAKE_PROGRAM={}", program.display());
}
