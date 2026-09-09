//! Owner-scoped split gestures. Both shells use the same geometry and drag rules.

use winit::dpi::PhysicalPosition;
use winit::window::CursorIcon;

use crate::layout::{Axis, Node, PhysicalRect, SplitDivider, compute_rects, compute_split_layout};
use crate::model::PaneId;

/// Route an entire captured sequence in the shell, including release after cancellation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragEvent {
    End,
    Restore,
    Move(PhysicalPosition<f64>),
    Release,
    Start,
    Swallow,
    Pass,
}

pub fn drag_event(event: &winit::event::WindowEvent, chrome: bool, captured: bool) -> DragEvent {
    use winit::event::{ElementState, MouseButton, WindowEvent};
    match event {
        WindowEvent::Focused(false)
        | WindowEvent::Resized(_)
        | WindowEvent::ScaleFactorChanged { .. }
            if chrome =>
        {
            DragEvent::End
        }
        WindowEvent::KeyboardInput { event, .. } => {
            drag_key(event.physical_key, event.state, captured)
        }
        WindowEvent::CursorMoved { position, .. } if captured => DragEvent::Move(*position),
        WindowEvent::MouseInput {
            state: ElementState::Released,
            button: MouseButton::Left,
            ..
        } if captured => DragEvent::Release,
        WindowEvent::MouseInput {
            state: ElementState::Pressed,
            button: MouseButton::Left,
            ..
        } => DragEvent::Start,
        WindowEvent::MouseInput { .. }
        | WindowEvent::MouseWheel { .. }
        | WindowEvent::CursorLeft { .. }
            if captured =>
        {
            DragEvent::Swallow
        }
        _ => DragEvent::Pass,
    }
}

fn drag_key(
    key: winit::keyboard::PhysicalKey,
    state: winit::event::ElementState,
    captured: bool,
) -> DragEvent {
    use winit::event::ElementState;
    use winit::keyboard::{KeyCode, PhysicalKey};
    if captured && key == PhysicalKey::Code(KeyCode::Escape) {
        if state == ElementState::Pressed {
            DragEvent::Restore
        } else {
            DragEvent::Swallow
        }
    } else {
        DragEvent::Pass
    }
}

/// Include two logical points of the adjoining pane edges. Native child windows can own
/// the event at a divider boundary, so they must offer the same hit target as the chrome.
pub fn divider_contains(divider: &SplitDivider, point: PhysicalPosition<f64>, scale: f64) -> bool {
    let slop = 2.0 * scale;
    let rect = divider.rect;
    match divider.axis {
        Axis::Horizontal => {
            point.x >= f64::from(rect.x) - slop
                && point.x < f64::from(rect.right()) + slop
                && point.y >= f64::from(rect.y)
                && point.y < f64::from(rect.bottom())
        }
        Axis::Vertical => {
            point.y >= f64::from(rect.y) - slop
                && point.y < f64::from(rect.bottom()) + slop
                && point.x >= f64::from(rect.x)
                && point.x < f64::from(rect.right())
        }
    }
}

pub fn divider_cursor(axis: Axis) -> CursorIcon {
    match axis {
        Axis::Horizontal => CursorIcon::EwResize,
        Axis::Vertical => CursorIcon::NsResize,
    }
}

fn coordinate(axis: Axis, point: PhysicalPosition<f64>) -> f64 {
    match axis {
        Axis::Horizontal => point.x,
        Axis::Vertical => point.y,
    }
}

fn extent(axis: Axis, rect: PhysicalRect) -> u32 {
    match axis {
        Axis::Horizontal => rect.width,
        Axis::Vertical => rect.height,
    }
}

fn split_weights<'a>(root: &'a mut Node, divider: &SplitDivider) -> Option<&'a mut Vec<f32>> {
    let mut node = root;
    for &index in &divider.path {
        let Node::Split { children, .. } = node else {
            return None;
        };
        node = children.get_mut(index)?;
    }
    let Node::Split {
        axis,
        sizes,
        children,
    } = node
    else {
        return None;
    };
    (*axis == divider.axis
        && sizes.len() == children.len()
        && divider.before.checked_add(1)? < sizes.len())
    .then_some(sizes)
}

pub struct SplitDrag<Owner> {
    owner: Owner,
    divider: SplitDivider,
    baseline: Node,
    expected: Node,
    area: PhysicalRect,
    scale: f64,
    origin: f64,
    initial: f32,
    pair: f32,
    total: f32,
    low: f32,
    high: f32,
}

impl<Owner: PartialEq> SplitDrag<Owner> {
    pub fn begin(
        owner: Owner,
        root: &Node,
        area: PhysicalRect,
        scale: f64,
        position: PhysicalPosition<f64>,
    ) -> Option<Self> {
        if !scale.is_finite() || scale <= 0.0 || !position.x.is_finite() || !position.y.is_finite()
        {
            return None;
        }
        let layout = compute_split_layout(root, area, scale);
        let divider = layout
            .dividers
            .into_iter()
            .find(|divider| divider_contains(divider, position, scale))?;
        if divider.available == 0 {
            return None;
        }
        let mut baseline = root.clone();
        let weights = split_weights(&mut baseline, &divider)?;
        let initial = weights[divider.before];
        let pair = initial + weights[divider.before + 1];
        let total: f32 = weights.iter().sum();
        if !total.is_finite() || !pair.is_finite() || initial <= 0.0 || initial >= pair {
            return None;
        }
        // Preserve existing undersized leaves, and bound every leaf, including uneven nested splits.
        let minimum = (48.0 * scale).round() as u32;
        let floors: Vec<(PaneId, u32)> = layout
            .panes
            .into_iter()
            .map(|(id, rect)| (id, extent(divider.axis, rect).min(minimum)))
            .collect();
        let mut drag = Self {
            owner,
            origin: coordinate(divider.axis, position),
            divider,
            baseline: baseline.clone(),
            expected: baseline,
            area,
            scale,
            initial,
            pair,
            total,
            low: initial,
            high: initial,
        };
        for (endpoint, lower) in [(0.0, true), (pair, false)] {
            let mut good = initial;
            let mut bad = endpoint;
            for _ in 0..32 {
                let middle = (good + bad) * 0.5;
                let candidate = drag.candidate(middle)?;
                let panes = compute_rects(&candidate, area, scale);
                if floors.iter().all(|(id, floor)| {
                    panes
                        .get(id)
                        .is_some_and(|rect| extent(drag.divider.axis, *rect) >= *floor)
                }) {
                    good = middle;
                } else {
                    bad = middle;
                }
            }
            if lower {
                drag.low = good;
            } else {
                drag.high = good;
            }
        }
        // A subpixel weight interval is not a usable drag range.
        if compute_rects(&drag.candidate(drag.low)?, area, scale)
            == compute_rects(&drag.candidate(drag.high)?, area, scale)
        {
            return None;
        }
        Some(drag)
    }

    fn candidate(&self, weight: f32) -> Option<Node> {
        let mut root = self.baseline.clone();
        let weights = split_weights(&mut root, &self.divider)?;
        weights[self.divider.before] = weight;
        weights[self.divider.before + 1] = self.pair - weight;
        Some(root)
    }

    pub fn valid(&self, owner: &Owner, root: &Node, area: PhysicalRect, scale: f64) -> bool {
        self.owner == *owner && self.expected == *root && self.area == area && self.scale == scale
    }

    pub fn update(
        &mut self,
        owner: &Owner,
        root: &mut Node,
        area: PhysicalRect,
        scale: f64,
        position: PhysicalPosition<f64>,
    ) -> bool {
        if !self.valid(owner, root, area, scale) {
            return false;
        }
        let delta = coordinate(self.divider.axis, position) - self.origin;
        if !delta.is_finite() {
            return true;
        }
        let weight = (f64::from(self.initial)
            + delta * f64::from(self.total) / f64::from(self.divider.available))
        .clamp(f64::from(self.low), f64::from(self.high)) as f32;
        if let Some(candidate) = self.candidate(weight) {
            *root = candidate;
            self.expected = root.clone();
        }
        true
    }

    pub fn restore(&self, owner: &Owner, root: &mut Node, area: PhysicalRect, scale: f64) -> bool {
        if !self.valid(owner, root, area, scale) {
            return false;
        }
        *root = self.baseline.clone();
        true
    }

    pub fn axis(&self) -> Axis {
        self.divider.axis
    }

    pub fn highlight(&self) -> Option<PhysicalRect> {
        compute_split_layout(&self.expected, self.area, self.scale)
            .dividers
            .into_iter()
            .find(|divider| {
                divider.path == self.divider.path && divider.before == self.divider.before
            })
            .map(|divider| divider.rect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> PhysicalRect {
        PhysicalRect {
            x: 100,
            y: 35,
            width: 1200,
            height: 700,
        }
    }
    fn center(rect: PhysicalRect) -> PhysicalPosition<f64> {
        PhysicalPosition::new(
            f64::from(rect.x) + f64::from(rect.width) / 2.0,
            f64::from(rect.y) + f64::from(rect.height) / 2.0,
        )
    }
    fn three(axis: Axis) -> Node {
        Node::Split {
            axis,
            children: vec![
                Node::Leaf(PaneId(1)),
                Node::Leaf(PaneId(2)),
                Node::Leaf(PaneId(3)),
            ],
            sizes: vec![0.25, 0.35, 0.4],
        }
    }

    #[test]
    fn adjoining_sections_resize_without_moving_the_third_at_multiple_scales() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for axis in [Axis::Horizontal, Axis::Vertical] {
                let mut root = three(axis);
                let layout = compute_split_layout(&root, area(), scale);
                let point = center(layout.dividers[0].rect);
                let mut drag = SplitDrag::begin((1, 1), &root, area(), scale, point).unwrap();
                let moved = match axis {
                    Axis::Horizontal => PhysicalPosition::new(point.x + 70.0, point.y),
                    Axis::Vertical => PhysicalPosition::new(point.x, point.y + 70.0),
                };
                assert!(drag.update(&(1, 1), &mut root, area(), scale, moved));
                let resized = compute_rects(&root, area(), scale);
                assert_eq!(resized[&PaneId(3)], layout.panes[&PaneId(3)]);
                assert_eq!(
                    extent(axis, resized[&PaneId(1)]),
                    extent(axis, layout.panes[&PaneId(1)]) + 70
                );
                assert_eq!(
                    extent(axis, resized[&PaneId(2)]),
                    extent(axis, layout.panes[&PaneId(2)]) - 70
                );
                let once = root.clone();
                for _ in 0..50 {
                    drag.update(&(1, 1), &mut root, area(), scale, moved);
                }
                assert_eq!(root, once);
                assert!(drag.restore(&(1, 1), &mut root, area(), scale));
                assert_eq!(root, three(axis));
            }
        }
    }

    #[test]
    fn every_pair_keeps_distant_boundaries_fixed_in_wide_splits() {
        for width in 700..760 {
            for scale in [1.0, 1.25, 2.0] {
                let bounds = PhysicalRect { width, ..area() };
                let baseline = Node::Split {
                    axis: Axis::Horizontal,
                    children: (1..=5).map(|id| Node::Leaf(PaneId(id))).collect(),
                    sizes: vec![0.1, 0.2, 0.3, 0.15, 0.25],
                };
                let layout = compute_split_layout(&baseline, bounds, scale);
                for (index, divider) in layout.dividers.iter().enumerate() {
                    for delta in [-53.0, -11.0, 17.0, 49.0] {
                        let mut root = baseline.clone();
                        let point = center(divider.rect);
                        let Some(mut drag) = SplitDrag::begin(1, &root, bounds, scale, point)
                        else {
                            continue;
                        };
                        drag.update(
                            &1,
                            &mut root,
                            bounds,
                            scale,
                            PhysicalPosition::new(point.x + delta, point.y),
                        );
                        let panes = compute_rects(&root, bounds, scale);
                        for id in 1..=5 {
                            if id != index as u64 + 1 && id != index as u64 + 2 {
                                assert_eq!(
                                    panes[&PaneId(id)],
                                    layout.panes[&PaneId(id)],
                                    "width {width}, pair {index}, delta {delta}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn uneven_nested_leaves_are_clamped_and_other_subtrees_are_preserved() {
        let mut root = three(Axis::Horizontal);
        if let Node::Split { children, .. } = &mut root {
            children[0] = Node::Split {
                axis: Axis::Horizontal,
                children: vec![Node::Leaf(PaneId(1)), Node::Leaf(PaneId(4))],
                sizes: vec![0.2, 0.8],
            };
        }
        let layout = compute_split_layout(&root, area(), 1.0);
        let divider = layout.dividers.iter().find(|d| d.path.is_empty()).unwrap();
        let point = center(divider.rect);
        let mut drag = SplitDrag::begin(1, &root, area(), 1.0, point).unwrap();
        drag.update(
            &1,
            &mut root,
            area(),
            1.0,
            PhysicalPosition::new(-10000.0, point.y),
        );
        let after = compute_rects(&root, area(), 1.0);
        assert!(after[&PaneId(1)].width >= 48);
        assert!(after[&PaneId(4)].width >= 48);
        assert_eq!(after[&PaneId(3)], layout.panes[&PaneId(3)]);
    }

    #[test]
    fn undersized_panes_cannot_shrink_further() {
        let root = three(Axis::Vertical);
        let small = PhysicalRect {
            height: 90,
            ..area()
        };
        let layout = compute_split_layout(&root, small, 1.0);
        let point = center(layout.dividers[0].rect);
        assert!(SplitDrag::begin(1, &root, small, 1.0, point).is_none());
        assert_eq!(compute_rects(&root, small, 1.0), layout.panes);
    }

    #[test]
    fn native_pane_edges_can_start_a_drag_without_capturing_the_pane_interior() {
        for axis in [Axis::Horizontal, Axis::Vertical] {
            let root = three(axis);
            let divider = compute_split_layout(&root, area(), 2.0).dividers.remove(0);
            let point = match axis {
                Axis::Horizontal => {
                    PhysicalPosition::new(f64::from(divider.rect.right()) + 1.0, 100.0)
                }
                Axis::Vertical => {
                    PhysicalPosition::new(200.0, f64::from(divider.rect.bottom()) + 1.0)
                }
            };
            assert!(divider_contains(&divider, point, 2.0));
            assert!(SplitDrag::begin(1, &root, area(), 2.0, point).is_some());
            let interior = match axis {
                Axis::Horizontal => PhysicalPosition::new(point.x + 5.0, point.y),
                Axis::Vertical => PhysicalPosition::new(point.x, point.y + 5.0),
            };
            assert!(!divider_contains(&divider, interior, 2.0));
            assert!(SplitDrag::begin(1, &root, area(), 2.0, interior).is_none());
        }
    }

    #[test]
    fn stale_gestures_cannot_change_another_owner_or_a_modified_layout() {
        let baseline = three(Axis::Horizontal);
        let layout = compute_split_layout(&baseline, area(), 1.0);
        let point = center(layout.dividers[0].rect);
        let mut drag = SplitDrag::begin((1, 1), &baseline, area(), 1.0, point).unwrap();
        let mut unrelated = baseline.clone(); // Same local pane IDs in another workspace/tab.
        for owner in [(2, 1), (1, 2)] {
            assert!(!drag.update(
                &owner,
                &mut unrelated,
                area(),
                1.0,
                PhysicalPosition::new(800.0, 100.0)
            ));
            assert!(!drag.restore(&owner, &mut unrelated, area(), 1.0));
            assert_eq!(unrelated, baseline);
        }
        let mut changed = baseline.clone();
        changed.split(PaneId(2), PaneId(4), Axis::Vertical);
        let snapshot = changed.clone();
        assert!(!drag.update(&(1, 1), &mut changed, area(), 1.0, point));
        assert!(!drag.restore(&(1, 1), &mut changed, area(), 1.0));
        assert_eq!(changed, snapshot);
        assert!(!drag.valid(
            &(1, 1),
            &baseline,
            PhysicalRect {
                width: 999,
                ..area()
            },
            1.0
        ));
        assert!(!drag.valid(&(1, 1), &baseline, area(), 2.0));
    }

    #[test]
    fn hit_testing_matches_dividers_and_excludes_pane_interiors() {
        let mut root = three(Axis::Horizontal);
        root.split(PaneId(2), PaneId(4), Axis::Vertical);
        let layout = compute_split_layout(&root, area(), 1.5);
        for rect in layout.panes.values() {
            assert!(SplitDrag::begin(1, &root, area(), 1.5, center(*rect)).is_none());
        }
        for divider in &layout.dividers {
            let drag = SplitDrag::begin(1, &root, area(), 1.5, center(divider.rect)).unwrap();
            assert_eq!(drag.axis(), divider.axis);
            assert_eq!(drag.highlight(), Some(divider.rect));
        }
    }
}

#[cfg(test)]
mod event_tests {
    use super::*;
    use winit::event::{DeviceId, ElementState, MouseButton, WindowEvent};
    use winit::keyboard::{KeyCode, PhysicalKey};

    #[test]
    fn captured_release_and_escape_do_not_reach_a_terminal_after_cancellation() {
        let release = WindowEvent::MouseInput {
            device_id: DeviceId::dummy(),
            state: ElementState::Released,
            button: MouseButton::Left,
        };
        assert_eq!(drag_event(&release, false, true), DragEvent::Release);
        assert_eq!(drag_event(&release, false, false), DragEvent::Pass);
        assert_eq!(
            drag_event(&WindowEvent::Focused(false), true, true),
            DragEvent::End
        );
        // The starting pane loses focus when chrome takes over; that must not end the gesture.
        assert_eq!(
            drag_event(&WindowEvent::Focused(false), false, true),
            DragEvent::Pass
        );
        assert_eq!(
            drag_key(
                PhysicalKey::Code(KeyCode::Escape),
                ElementState::Pressed,
                true
            ),
            DragEvent::Restore
        );
        assert_eq!(
            drag_key(
                PhysicalKey::Code(KeyCode::Escape),
                ElementState::Released,
                true
            ),
            DragEvent::Swallow
        );
        assert_eq!(
            drag_key(
                PhysicalKey::Code(KeyCode::Escape),
                ElementState::Pressed,
                false
            ),
            DragEvent::Pass
        );
    }

    #[test]
    fn drag_motion_stays_captured_outside_the_gap_but_pane_resizes_do_not_cancel_it() {
        let point = PhysicalPosition::new(-100.0, 4000.0);
        let motion = WindowEvent::CursorMoved {
            device_id: DeviceId::dummy(),
            position: point,
        };
        assert_eq!(drag_event(&motion, true, true), DragEvent::Move(point));
        assert_eq!(drag_event(&motion, false, true), DragEvent::Move(point));
        assert_eq!(drag_event(&motion, true, false), DragEvent::Pass);
        let resize = WindowEvent::Resized(winit::dpi::PhysicalSize::new(400, 300));
        assert_eq!(drag_event(&resize, true, true), DragEvent::End);
        assert_eq!(drag_event(&resize, false, true), DragEvent::Pass);
    }
}
