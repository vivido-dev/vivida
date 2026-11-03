//! Expanded sidebar geometry and mouse drag bounds.

// Rust guideline compliant 2026-10-07
use winit::dpi::{PhysicalPosition, PhysicalSize};

use crate::chrome::{
    ChromeLayout, MAX_SIDEBAR_LOGICAL, SidebarMode, bounded_sidebar_width, compute_chrome_layout,
};
use crate::layout::PhysicalRect;

/// Keep the grab area inside the chrome so native terminal panes cannot cover it.
pub fn resize_handle(layout: ChromeLayout, mode: SidebarMode, scale: f64) -> PhysicalRect {
    if mode != SidebarMode::Expanded {
        return PhysicalRect::default();
    }
    // Six logical pixels are easy to grab without obscuring the workspace row controls.
    let width = ((6.0 * scale).round() as u32).min(layout.sidebar.width);
    PhysicalRect {
        x: layout.sidebar.right() - width as i32,
        y: layout.content.y,
        width,
        height: layout.content.height,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SidebarDrag {
    pub original_width: f64,
    origin_x: f64,
    initial_width: f64,
    scale: f64,
}

impl SidebarDrag {
    pub fn begin(width: f64, layout: ChromeLayout, scale: f64, x: f64) -> Self {
        Self {
            original_width: width,
            origin_x: x,
            initial_width: f64::from(layout.sidebar.width) / scale,
            scale,
        }
    }

    pub fn width(self, position: PhysicalPosition<f64>, size: PhysicalSize<u32>) -> f64 {
        if !position.x.is_finite() {
            return self.original_width;
        }
        let requested = self.initial_width + (position.x - self.origin_x) / self.scale;
        let limit =
            compute_chrome_layout(size, self.scale, SidebarMode::Expanded, MAX_SIDEBAR_LOGICAL);
        bounded_sidebar_width(requested.min(f64::from(limit.sidebar.width) / self.scale))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_drag_scales_and_stops_at_both_bounds_and_the_pane() {
        for scale in [1.0, 1.25, 2.0] {
            let size = PhysicalSize::new((1000.0 * scale) as u32, (600.0 * scale) as u32);
            let layout = compute_chrome_layout(size, scale, SidebarMode::Expanded, 200.0);
            let origin = f64::from(layout.sidebar.right()) - 2.0;
            let drag = SidebarDrag::begin(200.0, layout, scale, origin);
            let point = |delta| PhysicalPosition::new(origin + delta * scale, 100.0);
            assert_eq!(drag.width(point(50.0), size), 250.0);
            assert_eq!(drag.width(point(-1000.0), size), 132.0);
            assert_eq!(drag.width(point(1000.0), size), 400.0);
            let narrow = PhysicalSize::new((460.0 * scale) as u32, size.height);
            assert_eq!(drag.width(point(1000.0), narrow), 300.0);
        }
    }

    #[test]
    fn only_the_expanded_sidebar_edge_below_the_tab_bar_is_draggable() {
        for scale in [1.0, 1.25, 2.0] {
            for mode in [
                SidebarMode::Expanded,
                SidebarMode::Compact,
                SidebarMode::Hidden,
            ] {
                let layout =
                    compute_chrome_layout(PhysicalSize::new(1200, 800), scale, mode, 280.0);
                let handle = resize_handle(layout, mode, scale);
                let edge = f64::from(layout.sidebar.right()) - 1.0;
                assert_eq!(handle.contains(edge, 100.0), mode == SidebarMode::Expanded);
                assert!(!handle.contains(edge, 0.0));
                assert!(!handle.contains(f64::from(layout.content.x), 100.0));
            }
        }
    }
}
