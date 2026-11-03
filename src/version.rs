//! Version metadata from the application and its embedded terminal.

// Rust guideline compliant 2026-10-07
use std::sync::LazyLock;

use clap::CommandFactory;

pub const VIVIDA: &str = env!("CARGO_PKG_VERSION");

/// Return the embedded Vivido's own CLI version, including its build identifier.
pub fn vivido() -> &'static str {
    static VERSION: LazyLock<String> = LazyLock::new(|| {
        vivido::cli::Options::command()
            .get_version()
            .expect("Vivido's CLI declares its version")
            .to_owned()
    });
    &VERSION
}
