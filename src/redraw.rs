//! Delayed redraw retries for shell-owned native windows. Mirrored in vvbox.

// Rust guideline compliant 2026-10-07

use std::time::{Duration, Instant};

use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::Window;

/// Limit failed presentation to ten attempts per second without delaying ordinary frames.
const RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// Retry and occlusion state belonging to one native window.
#[derive(Debug, Default)]
pub(crate) struct Redraw {
    retry_at: Option<Instant>,
    occluded: bool,
}

impl Redraw {
    /// Coalesce damage without keeping an unavailable window's event loop awake.
    pub(crate) fn request(&self, window: &Window) {
        if self.can_draw(Instant::now(), presentable(window)) {
            window.request_redraw();
        }
    }

    /// Track visibility events and reject redraws that cannot run yet.
    pub(crate) fn handle_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::Occluded(occluded) => {
                self.set_occluded(*occluded);
                if !occluded {
                    window.request_redraw();
                }
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.retry_at = None;
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                return self.ready(Instant::now(), presentable(window));
            }
            _ => (),
        }
        true
    }

    /// Schedule a retry only when the latest frame could not be presented.
    pub(crate) fn complete(&mut self, presented: bool, now: Instant) {
        self.retry_at = if presented {
            None
        } else {
            Some(now + RETRY_INTERVAL)
        };
    }

    fn set_occluded(&mut self, occluded: bool) {
        self.occluded = occluded;
        self.retry_at = None;
    }

    fn ready(&mut self, now: Instant, presentable: bool) -> bool {
        if self.occluded || !presentable {
            self.retry_at = None;
            return false;
        }
        if !self.can_draw(now, presentable) {
            return false;
        }
        self.retry_at = None;
        true
    }

    fn can_draw(&self, now: Instant, presentable: bool) -> bool {
        !self.occluded && presentable && self.retry_at.is_none_or(|deadline| now >= deadline)
    }

    fn take_retry(&mut self, now: Instant, presentable: bool) -> bool {
        self.retry_at.is_some() && self.ready(now, presentable)
    }
}

fn presentable(window: &Window) -> bool {
    let size = window.inner_size();
    window.is_visible() != Some(false)
        && window.is_minimized() != Some(true)
        && size.width > 0
        && size.height > 0
}

/// Request due frames and preserve the processor's earlier wake deadline.
pub(crate) fn pump<const N: usize>(
    event_loop: &ActiveEventLoop,
    windows: [(Option<&Window>, &mut Redraw); N],
) {
    let now = Instant::now();
    let mut retry_at = None;
    for (window, state) in windows {
        let Some(window) = window else {
            state.retry_at = None;
            continue;
        };
        if state.take_retry(now, presentable(window)) {
            window.request_redraw();
        }
        if let Some(deadline) = state.retry_at {
            retry_at = Some(retry_at.map_or(deadline, |current: Instant| current.min(deadline)));
        }
    }
    event_loop.set_control_flow(earlier_wake(event_loop.control_flow(), retry_at));
}

fn earlier_wake(control_flow: ControlFlow, retry_at: Option<Instant>) -> ControlFlow {
    match (control_flow, retry_at) {
        (ControlFlow::Wait, Some(deadline)) => ControlFlow::WaitUntil(deadline),
        (ControlFlow::WaitUntil(current), Some(deadline)) => {
            ControlFlow::WaitUntil(current.min(deadline))
        }
        (current, _) => current,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_redraw_requests_cannot_bypass_failed_presentation_delay() {
        let now = Instant::now();
        let mut redraw = Redraw::default();
        assert!(redraw.ready(now, true));
        redraw.complete(false, now);

        for _ in 0..1024 {
            assert!(!redraw.can_draw(now + RETRY_INTERVAL / 2, true));
            assert!(!redraw.ready(now + RETRY_INTERVAL / 2, true));
            assert!(!redraw.take_retry(now + RETRY_INTERVAL / 2, true));
        }
        assert_eq!(redraw.retry_at, Some(now + RETRY_INTERVAL));
        assert!(redraw.take_retry(now + RETRY_INTERVAL, true));
        assert!(!redraw.take_retry(now + RETRY_INTERVAL, true));
    }

    #[test]
    fn continuous_damage_cannot_drive_failed_presentation_above_ten_hertz() {
        let start = Instant::now();
        let mut redraw = Redraw::default();
        let mut attempts = 0;
        for elapsed in 0..1000 {
            let now = start + Duration::from_millis(elapsed);
            if redraw.ready(now, true) {
                attempts += 1;
                redraw.complete(false, now);
            }
        }
        assert_eq!(attempts, 10);
    }

    #[test]
    fn successful_presentation_cancels_retry_without_delaying_new_frames() {
        let now = Instant::now();
        let mut redraw = Redraw::default();
        redraw.complete(false, now);
        redraw.complete(true, now);
        assert!(redraw.ready(now, true));
        assert!(!redraw.take_retry(now + RETRY_INTERVAL, true));
    }

    #[test]
    fn occlusion_cancels_retries_and_restoration_allows_immediate_drawing() {
        let now = Instant::now();
        let mut redraw = Redraw::default();
        redraw.complete(false, now);
        redraw.set_occluded(true);
        assert_eq!(redraw.retry_at, None);
        assert!(!redraw.can_draw(now + RETRY_INTERVAL, true));
        assert!(!redraw.ready(now + RETRY_INTERVAL, true));
        assert!(!redraw.take_retry(now + RETRY_INTERVAL, true));
        redraw.set_occluded(false);
        assert!(redraw.ready(now, true));
    }

    #[test]
    fn hidden_or_zero_sized_windows_cancel_pending_retries() {
        let now = Instant::now();
        let mut redraw = Redraw::default();
        redraw.complete(false, now);
        assert!(!redraw.take_retry(now, false));
        assert_eq!(redraw.retry_at, None);
        assert!(!redraw.can_draw(now + RETRY_INTERVAL, false));
        assert!(!redraw.ready(now + RETRY_INTERVAL, false));
        assert!(redraw.ready(now + RETRY_INTERVAL, true));
    }

    #[test]
    fn each_window_keeps_its_own_retry_and_occlusion_state() {
        let now = Instant::now();
        let mut chrome = Redraw::default();
        let mut popup = Redraw::default();
        chrome.complete(false, now);
        popup.set_occluded(true);
        assert!(!popup.ready(now + RETRY_INTERVAL, true));
        assert!(chrome.take_retry(now + RETRY_INTERVAL, true));
    }

    #[test]
    fn retry_deadlines_preserve_earlier_processor_timers_and_polling() {
        let retry = Instant::now() + RETRY_INTERVAL;
        let earlier = retry - RETRY_INTERVAL / 2;
        let later = retry + RETRY_INTERVAL;
        assert_eq!(
            earlier_wake(ControlFlow::Wait, Some(retry)),
            ControlFlow::WaitUntil(retry)
        );
        assert_eq!(
            earlier_wake(ControlFlow::Poll, Some(retry)),
            ControlFlow::Poll
        );
        assert_eq!(
            earlier_wake(ControlFlow::WaitUntil(earlier), Some(retry)),
            ControlFlow::WaitUntil(earlier)
        );
        assert_eq!(
            earlier_wake(ControlFlow::WaitUntil(later), Some(retry)),
            ControlFlow::WaitUntil(retry)
        );
        assert_eq!(earlier_wake(ControlFlow::Wait, None), ControlFlow::Wait);
    }
}
