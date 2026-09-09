//! Native launch menus stay above the terminal child windows.

use std::cell::Cell;
use std::error::Error;

use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSMenu, NSMenuItem};
use objc2_foundation::{NSPoint, NSRect, NSString, ns_string};
use raw_window_handle::HasWindowHandle;
use vivido::shell::LaunchEntry;
use winit::dpi::PhysicalPosition;
use winit::window::Window;

define_class!(
    // SAFETY: NSObject has no subclassing requirements. The target owns only a Rust Cell and
    // is used exclusively by AppKit on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "VividaLaunchMenuTarget"]
    #[ivars = Cell<Option<usize>>]
    struct LaunchMenuTarget;

    impl LaunchMenuTarget {
        #[unsafe(method(selectLaunchEntry:))]
        fn select_entry(&self, sender: &NSMenuItem) {
            self.ivars().set(usize::try_from(sender.tag()).ok());
        }
    }
);

impl LaunchMenuTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let target = mtm.alloc::<Self>().set_ivars(Cell::new(None));
        // SAFETY: this freshly allocated NSObject subclass uses its superclass initializer.
        unsafe { msg_send![super(target), init] }
    }
}

pub fn show_launch_menu(
    chrome: &Window,
    entries: &[LaunchEntry],
    anchor: PhysicalPosition<i32>,
) -> Result<Option<usize>, Box<dyn Error>> {
    let mtm = MainThreadMarker::new().ok_or("launch menu requires the main thread")?;
    let owner =
        super::ns_window(chrome.window_handle()?.as_raw()).ok_or("chrome has no macOS window")?;
    let view = owner.contentView().ok_or("chrome has no content view")?;
    let target = LaunchMenuTarget::new(mtm);
    let menu = NSMenu::initWithTitle(mtm.alloc(), ns_string!(""));
    menu.setAutoenablesItems(false);
    for (index, entry) in entries.iter().enumerate() {
        // SAFETY: the selector is implemented above with AppKit's single-menu-item signature.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &NSString::from_str(&entry.label),
                Some(sel!(selectLaunchEntry:)),
                ns_string!(""),
            )
        };
        item.setTag(isize::try_from(index)?);
        // SAFETY: the target stays retained until synchronous menu tracking has finished.
        unsafe { item.setTarget(Some(&target)) };
        menu.addItem(&item);
    }
    menu.popUpMenuPositioningItem_atLocation_inView(
        None,
        menu_location(
            anchor,
            chrome.scale_factor(),
            view.bounds(),
            view.isFlipped(),
        ),
        Some(&view),
    );
    Ok(target.ivars().get())
}

fn menu_location(
    anchor: PhysicalPosition<i32>,
    scale: f64,
    bounds: NSRect,
    flipped: bool,
) -> NSPoint {
    let anchor = anchor.to_logical::<f64>(scale);
    let y = if flipped {
        bounds.origin.y + anchor.y
    } else {
        bounds.origin.y + bounds.size.height - anchor.y
    };
    NSPoint::new(bounds.origin.x + anchor.x, y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::NSSize;

    #[test]
    fn popup_stays_at_the_plus_button_on_retina_and_unflipped_views() {
        let bounds = NSRect::new(NSPoint::new(10.0, 20.0), NSSize::new(800.0, 600.0));
        for scale in [1.0, 2.0] {
            let anchor = PhysicalPosition::new(300.0 * scale, 35.0 * scale).cast();
            assert_eq!(
                menu_location(anchor, scale, bounds, true),
                NSPoint::new(310.0, 55.0),
            );
            assert_eq!(
                menu_location(anchor, scale, bounds, false),
                NSPoint::new(310.0, 585.0),
            );
        }
    }
}
