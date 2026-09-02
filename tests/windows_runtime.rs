#![cfg(windows)]

use std::fs;
use std::process::Command;

#[test]
fn cargo_installed_binary_starts_without_staged_ffmpeg_dlls() {
    let install_root = tempfile::tempdir().expect("could not create temporary install directory");
    let installed_binary = install_root.path().join("vivida.exe");
    fs::copy(env!("CARGO_BIN_EXE_vivida"), &installed_binary)
        .expect("could not copy Vivida into temporary install directory");

    let output = Command::new(&installed_binary)
        .args(["list", "--all", "--json"])
        .env("PATH", "")
        .output()
        .expect("could not launch installed Vivida");

    assert!(
        output.status.success(),
        "installed Vivida failed without adjacent FFmpeg DLLs: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
