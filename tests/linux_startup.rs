//! Backend selection regressions that require no display server or GPU.

#![cfg(target_os = "linux")]

// Rust guideline compliant 2026-10-07
use std::process::Command;

fn startup_error(display: Option<&str>, wayland_display: Option<&str>) -> String {
    // Only the child environment changes; parallel tests keep their own display settings.
    let mut command = Command::new(env!("CARGO_BIN_EXE_vivida"));
    command
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .env_remove("DISPLAY");
    if let Some(display) = display {
        command.env("DISPLAY", display);
    }
    if let Some(display) = wayland_display {
        command.env("WAYLAND_DISPLAY", display);
    }
    let output = command.output().expect("run Vivida startup");
    assert!(!output.status.success());
    String::from_utf8(output.stderr).expect("startup error is UTF-8")
}

#[test]
fn x11_session_does_not_require_a_wayland_compositor() {
    // An invalid display fails before window/GPU creation but identifies the chosen backend.
    let error = startup_error(Some(":not-a-display"), None);
    assert!(error.contains("XNotSupported"), "{error}");
}

#[test]
fn missing_display_reports_the_session_environment() {
    let error = startup_error(None, None);
    assert!(
        error.contains("neither WAYLAND_DISPLAY nor WAYLAND_SOCKET nor DISPLAY is set"),
        "{error}"
    );
}

#[test]
fn unreachable_wayland_compositor_falls_back_to_x11() {
    // /dev/null cannot be a compositor socket, regardless of XDG_RUNTIME_DIR.
    let error = startup_error(Some(":not-a-display"), Some("/dev/null"));
    assert!(error.contains("XNotSupported"), "{error}");
}

#[test]
fn wayland_only_session_preserves_the_connection_error() {
    let error = startup_error(None, Some("/dev/null"));
    assert!(error.contains("NoCompositor"), "{error}");
}
