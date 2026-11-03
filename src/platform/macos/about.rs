//! Version details for the native About Vivida panel.

// Rust guideline compliant 2026-10-07
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAboutPanelOptionApplicationName, NSAboutPanelOptionApplicationVersion,
    NSAboutPanelOptionCredits, NSAboutPanelOptionVersion, NSApplication,
};
use objc2_foundation::{NSAttributedString, NSDictionary, NSString, ns_string};

define_class!(
    // SAFETY: NSObject has no subclassing requirements. This target has no ivars and AppKit
    // invokes it exclusively on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "VividaAboutTarget"]
    struct AboutTarget;

    impl AboutTarget {
        #[unsafe(method(showAboutVivida:))]
        fn show_about(&self, _sender: &AnyObject) {
            let version = NSString::from_str(crate::version::VIVIDA);
            let terminal = NSString::from_str(&format!("Vivido {}", crate::version::vivido()));
            let credits = NSAttributedString::from_nsstring(&terminal);
            // SAFETY: these AppKit keys are immutable strings. Each option has its documented
            // value type: NSString for names and versions, NSAttributedString for credits.
            unsafe {
                let options = NSDictionary::<NSString, AnyObject>::from_slices(
                    &[
                        NSAboutPanelOptionApplicationName,
                        NSAboutPanelOptionApplicationVersion,
                        NSAboutPanelOptionVersion,
                        NSAboutPanelOptionCredits,
                    ],
                    &[
                        ns_string!("Vivida").as_ref(),
                        version.as_ref(),
                        ns_string!("").as_ref(),
                        credits.as_ref(),
                    ],
                );
                NSApplication::sharedApplication(self.mtm())
                    .orderFrontStandardAboutPanelWithOptions(&options);
            }
        }
    }
);

/// Replace the standard About action after winit installs the application menu.
pub fn configure_about_menu() {
    let mtm = MainThreadMarker::new().expect("application menus run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    let Some(menu) = app
        .mainMenu()
        .and_then(|menu| menu.itemAtIndex(0))
        .and_then(|item| item.submenu())
    else {
        return;
    };
    let Some(item) = menu
        .itemArray()
        .iter()
        .find(|item| item.action() == Some(sel!(orderFrontStandardAboutPanel:)))
    else {
        return;
    };
    let target = mtm.alloc::<AboutTarget>().set_ivars(());
    // SAFETY: this fresh NSObject subclass uses the superclass initializer. The selector
    // matches its single-object action method, and representedObject retains the weak target
    // for the lifetime of the menu item.
    unsafe {
        let target: Retained<AboutTarget> = msg_send![super(target), init];
        item.setTitle(ns_string!("About Vivida"));
        item.setAction(Some(sel!(showAboutVivida:)));
        item.setTarget(Some(&target));
        item.setRepresentedObject(Some(&target));
    }
}
