#![cfg(target_os = "macos")]

use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

struct IdleShell(Child);

impl Drop for IdleShell {
    fn drop(&mut self) {
        // Do not let the temporary instance overwrite the user's saved session on exit.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn cpu_seconds(pid: u32) -> f64 {
    let output = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "time="])
        .output()
        .expect("read process CPU time");
    assert!(output.status.success(), "Vivida exited before measurement");
    let time = String::from_utf8(output.stdout).expect("CPU time is UTF-8");
    // macOS ps reports cumulative CPU time as minutes:seconds.hundredths.
    let (minutes, seconds) = time.trim().split_once(':').expect("CPU time format");
    minutes.parse::<f64>().expect("CPU minutes") * 60.0
        + seconds.parse::<f64>().expect("CPU seconds")
}

/// Hidden auxiliary windows receive startup redraws on macOS. Retrying a failed
/// presentation of those windows used to consume a full CPU core indefinitely.
#[test]
#[ignore = "requires a macOS desktop, GPU, and permission to inspect processes"]
fn closed_startup_popups_do_not_spin_the_event_loop() {
    let mut shell = IdleShell(
        Command::new(env!("CARGO_BIN_EXE_vivida"))
            .args(["-e", "/bin/sleep", "90"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start native Vivida"),
    );
    thread::sleep(Duration::from_secs(10));
    assert!(shell.0.try_wait().expect("check Vivida").is_none());
    let before = cpu_seconds(shell.0.id());
    thread::sleep(Duration::from_secs(3));
    let consumed = cpu_seconds(shell.0.id()) - before;
    // Allow background bookkeeping and cursor timers, but reject a busy redraw loop.
    assert!(
        consumed < 0.75,
        "idle Vivida consumed {consumed:.2}s of CPU in 3s"
    );
}
