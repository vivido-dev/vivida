//! Native geometry regression; run with `cargo test --test macos_pane_geometry -- --ignored`.

#[cfg(target_os = "macos")]
mod layout {
    pub use vivido::shell::PhysicalRect;
}

#[cfg(target_os = "macos")]
#[path = "../src/platform/macos/pane_geometry.rs"]
mod pane_geometry;

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSView, NSWindow, NSWindowOrderingMode};
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use winit::application::ApplicationHandler;
    use winit::dpi::{LogicalPosition, LogicalSize};
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::platform::macos::{EventLoopBuilderExtMacOS, WindowAttributesExtMacOS};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::{Window, WindowId};

    if std::env::args().any(|arg| arg == "--list") {
        println!("macos_pane_geometry: test");
        return;
    }
    if !std::env::args().any(|arg| arg == "--ignored") {
        println!("macos_pane_geometry: ignored (requires a macOS desktop; pass --ignored)");
        return;
    }

    fn native(window: &Window) -> objc2::rc::Retained<NSWindow> {
        assert!(MainThreadMarker::new().is_some());
        let RawWindowHandle::AppKit(handle) = window.window_handle().unwrap().as_raw() else {
            panic!("expected AppKit");
        };
        // SAFETY: the owning winit window is live, and this runs on the main event-loop thread.
        unsafe { handle.ns_view.cast::<NSView>().as_ref() }
            .window()
            .unwrap()
    }

    struct Regression;
    impl ApplicationHandler for Regression {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            let host = event_loop
                .create_window(
                    Window::default_attributes()
                        .with_visible(false)
                        .with_active(false)
                        .with_fullsize_content_view(true)
                        .with_inner_size(LogicalSize::new(800, 600)),
                )
                .unwrap();
            let chrome = native(&host);
            let panes = (0..3)
                .map(|_| {
                    event_loop
                        .create_window(
                            Window::default_attributes()
                                .with_visible(false)
                                .with_active(false)
                                .with_decorations(false)
                                .with_resizable(false),
                        )
                        .unwrap()
                })
                .collect::<Vec<_>>();
            let children = panes.iter().map(native).collect::<Vec<_>>();
            // Two split panes stay attached; an inactive tab remains detached and untouched.
            for pane in &children[..2] {
                // SAFETY: parent and children are live on the main thread until this test exits.
                unsafe { chrome.addChildWindow_ordered(pane, NSWindowOrderingMode::Above) };
            }
            let hidden_frame = children[2].frame();
            for (x, y) in [(120, 160), (240, 220), (80, 120)] {
                host.set_outer_position(LogicalPosition::new(x, y));
                let content = chrome.contentRectForFrameRect(chrome.frame());
                let scale = chrome.backingScaleFactor();
                for (index, pane) in children[..2].iter().enumerate() {
                    // Model AppKit independently relocating a child during a display change.
                    pane.setFrame_display(
                        NSRect::new(NSPoint::new(-600.0, -400.0), NSSize::new(50.0, 50.0)),
                        false,
                    );
                    let left = 80.0 + 350.0 * index as f64;
                    let rect = layout::PhysicalRect {
                        x: (left * scale).round() as i32,
                        y: (35.0 * scale).round() as i32,
                        width: (340.0 * scale).round() as u32,
                        height: (550.0 * scale).round() as u32,
                    };
                    for _ in 0..2 {
                        pane_geometry::place_pane(&chrome, pane, rect);
                        assert_eq!(
                            pane.frame(),
                            NSRect::new(
                                NSPoint::new(content.origin.x + left, content.origin.y + 15.0),
                                NSSize::new(340.0, 550.0),
                            ),
                        );
                        assert_eq!(
                            pane.parentWindow().unwrap().windowNumber(),
                            chrome.windowNumber()
                        );
                        assert!(!pane.isKeyWindow());
                    }
                }
                assert_eq!(children[2].frame(), hidden_frame);
                assert!(children[2].parentWindow().is_none());
            }
            event_loop.exit();
        }

        fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
    }

    let mut builder = EventLoop::builder();
    builder.with_activate_ignoring_other_apps(false);
    builder.build().unwrap().run_app(&mut Regression).unwrap();
}
