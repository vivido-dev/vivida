use objc2_app_kit::NSWindow;
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::layout::PhysicalRect;

pub(super) fn place_pane(chrome: &NSWindow, pane: &NSWindow, rect: PhysicalRect) {
    let content = chrome.contentRectForFrameRect(chrome.frame());
    let frame = pane_frame(content, chrome.backingScaleFactor(), rect);
    // Setting an unchanged frame can generate more native move/resize notifications.
    if pane.frame() != frame {
        pane.setFrame_display(frame, true);
    }
}

/// Convert the host's top-left physical layout to AppKit's bottom-left screen points.
fn pane_frame(content: NSRect, scale: f64, rect: PhysicalRect) -> NSRect {
    // Parent and child can have different backing scales while crossing displays (or while a
    // hidden tab still belongs to its old display). Passing physical screen coordinates through
    // the child's winit setters would divide them by the child's scale, displacing the pane.
    // Use only the host's scale and set position and size together in native screen points.
    NSRect::new(
        NSPoint::new(
            content.origin.x + f64::from(rect.x) / scale,
            content.origin.y + content.size.height
                - (f64::from(rect.y) + f64::from(rect.height)) / scale,
        ),
        NSSize::new(
            f64::from(rect.width) / scale,
            f64::from(rect.height) / scale,
        ),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn pane_offsets_survive_host_scale_and_screen_origin_changes() {
        use super::*;

        // The same logical split on Retina and non-Retina displays, including displays above
        // and to the left of the primary screen. No child scale participates in the conversion.
        for scale in [1.0, 2.0, 1.0] {
            for (x, y) in [(100.0, 200.0), (-1440.0, -900.0), (1920.0, 1080.0)] {
                let content = NSRect::new(NSPoint::new(x, y), NSSize::new(800.0, 600.0));
                let rect = PhysicalRect {
                    x: (80.0 * scale) as i32,
                    y: (35.0 * scale) as i32,
                    width: (400.0 * scale) as u32,
                    height: (550.0 * scale) as u32,
                };
                assert_eq!(
                    pane_frame(content, scale, rect),
                    NSRect::new(NSPoint::new(x + 80.0, y + 15.0), NSSize::new(400.0, 550.0)),
                );
            }
        }
    }
}
