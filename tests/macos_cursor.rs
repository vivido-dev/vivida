//! Chrome cursor regression; run with `cargo test --test macos_cursor -- --ignored`.

#[cfg(target_os = "macos")]
#[path = "../src/platform/macos/cursor.rs"]
#[expect(dead_code, reason = "the regression drives only the cursor lookup")]
mod cursor;

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    use objc2::MainThreadMarker;
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::{NSApplication, NSCursor};
    use winit::window::CursorIcon;

    if std::env::args().any(|arg| arg == "--list") {
        println!("macos_cursor: test");
        return;
    }
    if !std::env::args().any(|arg| arg == "--ignored") {
        println!("macos_cursor: ignored (requires a macOS desktop; pass --ignored)");
        return;
    }

    // AppKit hands out cursors only to a process connected to the window server.
    let mtm = MainThreadMarker::new().expect("a harness-free test runs on the main thread");
    let _app = NSApplication::sharedApplication(mtm);

    // Hovering a window corner looks its cursor up on every pointer move. AppKit keeps the
    // diagonal cursors cached and hands them out unowned, so a lookup that releases one it does
    // not own frees the shared instance, and a later pool drain or lookup touches freed memory.
    let arrow = NSCursor::arrowCursor();
    for icon in [CursorIcon::NwseResize, CursorIcon::NeswResize] {
        for _ in 0..64 {
            autoreleasepool(|_| {
                let shape = cursor::hover_cursor(icon);
                assert!(
                    !std::ptr::eq(&*shape, &*arrow),
                    "{icon:?} fell back to the arrow"
                );
            });
        }
    }
    println!("macos_cursor: ok");
}
