use std::path::PathBuf;
use std::process::Command;

fn main() {
    build_local_ai_sidecar();
    tauri_build::build()
}

/// Builds the local model runtime as a separate executable before Tauri
/// validates and bundles its external binaries. Whisper and llama.cpp both
/// embed GGML, and linking both into the app itself creates duplicate native
/// symbols. A private target directory avoids deadlocking Cargo's outer
/// workspace build lock while this build script compiles the helper.
fn build_local_ai_sidecar() {
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by Cargo"),
    );
    let repo_root = manifest_dir.join("../../..");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let target = std::env::var("TARGET").expect("TARGET is set by Cargo");
    let profile = std::env::var("PROFILE").expect("PROFILE is set by Cargo");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let sidecar_target = out_dir.join("local-ai-target");

    let mut command = Command::new(cargo);
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(repo_root.join("Cargo.toml"))
        .args(["-p", "simstim-local-ai", "--target", &target])
        .env("CARGO_TARGET_DIR", &sidecar_target);
    if profile == "release" {
        command.arg("--release");
    }
    let status = command.status().expect("start local AI sidecar build");
    assert!(status.success(), "local AI sidecar build failed");

    let source = sidecar_target
        .join(&target)
        .join(&profile)
        .join("simstim-local-ai");
    let binaries = manifest_dir.join("binaries");
    std::fs::create_dir_all(&binaries).expect("create sidecar bundle directory");
    std::fs::copy(
        &source,
        binaries.join(format!("simstim-local-ai-{target}")),
    )
    .expect("copy local AI sidecar for Tauri");

    println!("cargo:rerun-if-changed={}", repo_root.join("crates/local-ai").display());
    println!("cargo:rerun-if-changed={}", repo_root.join("Cargo.lock").display());
}
