use std::collections::HashMap;

use serde::{Deserialize, Serialize};

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl PhysicalRect {
    pub fn contains(self, x: f64, y: f64) -> bool {
        x >= f64::from(self.x)
            && y >= f64::from(self.y)
            && x < f64::from(self.x) + f64::from(self.width)
            && y < f64::from(self.y) + f64::from(self.height)
    }

    pub fn right(self) -> i32 {
        self.x
            .saturating_add(i32::try_from(self.width).unwrap_or(i32::MAX))
    }

    pub fn bottom(self) -> i32 {
        self.y
            .saturating_add(i32::try_from(self.height).unwrap_or(i32::MAX))
    }
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
}
