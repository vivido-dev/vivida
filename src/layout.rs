use std::collections::HashMap;

use serde::{Deserialize, Serialize};
pub use vivido::shell::PhysicalRect;

use crate::model::PaneId;

pub const SPLIT_HANDLE_LOGICAL: f64 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Node {
    Leaf(PaneId),
    Split {
        axis: Axis,
        children: Vec<Node>,
        sizes: Vec<f32>,
    },
}

impl Node {
    pub fn first_pane(&self) -> PaneId {
        match self {
            Self::Leaf(pane_id) => *pane_id,
            Self::Split { children, .. } => children
                .first()
                .expect("a split must retain at least two children")
                .first_pane(),
        }
    }

    pub fn contains(&self, pane_id: PaneId) -> bool {
        match self {
            Self::Leaf(candidate) => *candidate == pane_id,
            Self::Split { children, .. } => children.iter().any(|child| child.contains(pane_id)),
        }
    }

    pub fn split(&mut self, pane_id: PaneId, new_pane_id: PaneId, axis: Axis) -> bool {
        match self {
            Self::Leaf(candidate) if *candidate == pane_id => {
                *self = Self::Split {
                    axis,
                    children: vec![Self::Leaf(pane_id), Self::Leaf(new_pane_id)],
                    sizes: vec![0.5, 0.5],
                };
                true
            }
            Self::Leaf(_) => false,
            Self::Split {
                axis: split_axis,
                children,
                sizes,
            } => {
                if *split_axis == axis {
                    let Some(index) = children.iter().position(|child| child.contains(pane_id))
                    else {
                        return false;
                    };
                    if matches!(children[index], Self::Leaf(candidate) if candidate == pane_id) {
                        let old_size = sizes[index];
                        sizes[index] = old_size / 2.0;
                        sizes.insert(index + 1, old_size / 2.0);
                        children.insert(index + 1, Self::Leaf(new_pane_id));
                        return true;
                    }
                }

                children
                    .iter_mut()
                    .any(|child| child.split(pane_id, new_pane_id, axis))
            }
        }
    }

    pub fn remove(&mut self, pane_id: PaneId) -> bool {
        let Self::Split {
            children, sizes, ..
        } = self
        else {
            return false;
        };

        if let Some(index) = children
            .iter()
            .position(|child| matches!(child, Self::Leaf(candidate) if *candidate == pane_id))
        {
            children.remove(index);
            sizes.remove(index);
            normalize_sizes(sizes);
        } else if !children.iter_mut().any(|child| child.remove(pane_id)) {
            return false;
        }

        collapse_single_child(self);
        true
    }
}

fn collapse_single_child(node: &mut Node) {
    let replacement = match node {
        Node::Split { children, .. } if children.len() == 1 => Some(children.remove(0)),
        _ => None,
    };
    if let Some(replacement) = replacement {
        *node = replacement;
    }
}

fn normalize_sizes(sizes: &mut [f32]) {
    let total: f32 = sizes.iter().copied().sum();
    if total <= f32::EPSILON {
        let equal = 1.0 / sizes.len().max(1) as f32;
        sizes.fill(equal);
        return;
    }
    for size in sizes {
        *size /= total;
    }
}

pub fn compute_rects(
    node: &Node,
    area: PhysicalRect,
    scale_factor: f64,
) -> HashMap<PaneId, PhysicalRect> {
    let mut rects = HashMap::new();
    compute_node_rects(node, area, scale_factor, &mut rects);
    rects
}

/// Adjust split weights so `pane_id` approaches the requested physical size without moving
/// sibling panes out of the tab's content area. Returns the resulting pane rectangle.
pub fn resize_pane(
    root: &mut Node,
    pane_id: PaneId,
    width: u32,
    height: u32,
    area: PhysicalRect,
    scale_factor: f64,
) -> Option<PhysicalRect> {
    resize_pane_axis(root, pane_id, Axis::Horizontal, width, area, scale_factor)?;
    resize_pane_axis(root, pane_id, Axis::Vertical, height, area, scale_factor)?;
    compute_rects(root, area, scale_factor).remove(&pane_id)
}

fn resize_pane_axis(
    root: &mut Node,
    pane_id: PaneId,
    axis: Axis,
    requested: u32,
    area: PhysicalRect,
    scale_factor: f64,
) -> Option<()> {
    let mut targets = Vec::new();
    collect_split_targets(root, pane_id, axis, &mut Vec::new(), &mut targets);

    // The closest split gives the most direct adjustment. If it cannot reach the requested
    // extent, progressively adjust its ancestors while preserving all other weight ratios.
    for (path, child_index) in targets.into_iter().rev() {
        let current = compute_rects(root, area, scale_factor).remove(&pane_id)?;
        if pane_extent(current, axis) == requested {
            break;
        }

        let baseline = root.clone();
        let mut best = baseline.clone();
        let mut best_error = pane_extent(current, axis).abs_diff(requested);
        let mut low = 0.0f32;
        let mut high = 1.0f32;

        for _ in 0..32 {
            let fraction = (low + high) / 2.0;
            let mut candidate = baseline.clone();
            set_child_fraction(&mut candidate, &path, child_index, fraction);
            let candidate_rect = compute_rects(&candidate, area, scale_factor)
                .remove(&pane_id)
                .expect("the target pane remains in an unchanged split tree");
            let extent = pane_extent(candidate_rect, axis);
            let error = extent.abs_diff(requested);
            if error < best_error {
                best = candidate;
                best_error = error;
            }
            if extent < requested {
                low = fraction;
            } else {
                high = fraction;
            }
        }

        *root = best;
    }
    Some(())
}

fn pane_extent(rect: PhysicalRect, axis: Axis) -> u32 {
    match axis {
        Axis::Horizontal => rect.width,
        Axis::Vertical => rect.height,
    }
}

fn collect_split_targets(
    node: &Node,
    pane_id: PaneId,
    axis: Axis,
    path: &mut Vec<usize>,
    targets: &mut Vec<(Vec<usize>, usize)>,
) {
    let Node::Split {
        axis: split_axis,
        children,
        ..
    } = node
    else {
        return;
    };
    let Some(child_index) = children.iter().position(|child| child.contains(pane_id)) else {
        return;
    };
    if *split_axis == axis {
        targets.push((path.clone(), child_index));
    }
    path.push(child_index);
    collect_split_targets(&children[child_index], pane_id, axis, path, targets);
    path.pop();
}

fn set_child_fraction(root: &mut Node, path: &[usize], child_index: usize, fraction: f32) {
    let mut node = root;
    for &index in path {
        let Node::Split { children, .. } = node else {
            return;
        };
        node = &mut children[index];
    }
    let Node::Split { sizes, .. } = node else {
        return;
    };

    let fraction = fraction.clamp(0.000_1, 0.999_9);
    let other_total = sizes
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != child_index)
        .map(|(_, size)| *size)
        .sum::<f32>();
    let remaining = 1.0 - fraction;
    let other_count = sizes.len().saturating_sub(1).max(1) as f32;
    for (index, size) in sizes.iter_mut().enumerate() {
        if index == child_index {
            *size = fraction;
        } else if other_total > f32::EPSILON {
            *size = *size / other_total * remaining;
        } else {
            *size = remaining / other_count;
        }
    }
}

fn compute_node_rects(
    node: &Node,
    area: PhysicalRect,
    scale_factor: f64,
    rects: &mut HashMap<PaneId, PhysicalRect>,
) {
    match node {
        Node::Leaf(pane_id) => {
            rects.insert(*pane_id, area);
        }
        Node::Split {
            axis,
            children,
            sizes,
        } => {
            let handle = (SPLIT_HANDLE_LOGICAL * scale_factor).round() as u32;
            let handle_total = handle.saturating_mul(children.len().saturating_sub(1) as u32);
            let extent = match axis {
                Axis::Horizontal => area.width,
                Axis::Vertical => area.height,
            };
            let available = extent.saturating_sub(handle_total);
            let mut cursor = 0u32;
            let mut remaining = available;
            let total_weight = sizes.iter().copied().sum::<f32>().max(f32::EPSILON);

            for (index, child) in children.iter().enumerate() {
                let child_extent = if index + 1 == children.len() {
                    remaining
                } else {
                    let weighted =
                        ((available as f32) * sizes[index] / total_weight).round() as u32;
                    weighted.min(remaining)
                };
                let child_area = match axis {
                    Axis::Horizontal => PhysicalRect {
                        x: area.x.saturating_add(cursor as i32),
                        y: area.y,
                        width: child_extent,
                        height: area.height,
                    },
                    Axis::Vertical => PhysicalRect {
                        x: area.x,
                        y: area.y.saturating_add(cursor as i32),
                        width: area.width,
                        height: child_extent,
                    },
                };
                compute_node_rects(child, child_area, scale_factor, rects);
                remaining = remaining.saturating_sub(child_extent);
                cursor = cursor.saturating_add(child_extent);
                if index + 1 < children.len() {
                    cursor = cursor.saturating_add(handle);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_axis_split_merges_and_cross_axis_wraps() {
        let mut root = Node::Leaf(PaneId(1));
        assert!(root.split(PaneId(1), PaneId(2), Axis::Horizontal));
        assert!(root.split(PaneId(2), PaneId(3), Axis::Horizontal));
        let Node::Split { children, .. } = &root else {
            panic!("expected split")
        };
        assert_eq!(children.len(), 3);

        assert!(root.split(PaneId(2), PaneId(4), Axis::Vertical));
        let Node::Split { children, .. } = &root else {
            panic!("expected split")
        };
        assert!(matches!(
            children[1],
            Node::Split {
                axis: Axis::Vertical,
                ..
            }
        ));
    }

    #[test]
    fn removal_collapses_a_single_child_split() {
        let mut root = Node::Leaf(PaneId(1));
        root.split(PaneId(1), PaneId(2), Axis::Horizontal);
        assert!(root.remove(PaneId(2)));
        assert_eq!(root, Node::Leaf(PaneId(1)));
    }

    #[test]
    fn computed_rects_leave_one_physical_handle_gap() {
        let mut root = Node::Leaf(PaneId(1));
        root.split(PaneId(1), PaneId(2), Axis::Horizontal);
        let rects = compute_rects(
            &root,
            PhysicalRect {
                x: 10,
                y: 20,
                width: 204,
                height: 80,
            },
            1.0,
        );
        assert_eq!(
            rects[&PaneId(1)],
            PhysicalRect {
                x: 10,
                y: 20,
                width: 100,
                height: 80
            }
        );
        assert_eq!(
            rects[&PaneId(2)],
            PhysicalRect {
                x: 114,
                y: 20,
                width: 100,
                height: 80
            }
        );
    }

    #[test]
    fn resize_updates_nested_split_weights_and_preserves_siblings() {
        let mut root = Node::Leaf(PaneId(1));
        root.split(PaneId(1), PaneId(2), Axis::Horizontal);
        root.split(PaneId(1), PaneId(3), Axis::Vertical);
        let area = PhysicalRect {
            x: 10,
            y: 20,
            width: 604,
            height: 404,
        };

        let resized = resize_pane(&mut root, PaneId(1), 400, 300, area, 1.0).unwrap();
        assert_eq!((resized.width, resized.height), (400, 300));

        let rects = compute_rects(&root, area, 1.0);
        assert_eq!(rects.len(), 3);
        assert!(rects[&PaneId(2)].width > 0);
        assert!(rects[&PaneId(3)].height > 0);
        assert_eq!(rects[&PaneId(1)].x, area.x);
        assert_eq!(rects[&PaneId(1)].y, area.y);
    }
}
