//! How a session's panes share its area: each split halves one pane.

use crate::chrome::Rect;

pub type PaneId = u64;

/// Where a split puts the new pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    Right,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tree {
    Pane(PaneId),
    Split {
        split: Split,
        first: Box<Tree>,
        second: Box<Tree>,
    },
}

/// A way to move focus between panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Tree {
    /// Every pane, left to right and top to bottom.
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<PaneId>) {
        match self {
            Tree::Pane(id) => out.push(*id),
            Tree::Split { first, second, .. } => {
                first.collect(out);
                second.collect(out);
            }
        }
    }

    pub fn contains(&self, id: PaneId) -> bool {
        match self {
            Tree::Pane(p) => *p == id,
            Tree::Split { first, second, .. } => first.contains(id) || second.contains(id),
        }
    }

    /// Halves pane `at`, putting `new` to its right or below it.
    pub fn split(&mut self, at: PaneId, new: PaneId, split: Split) -> bool {
        match self {
            Tree::Pane(p) if *p == at => {
                *self = Tree::Split {
                    split,
                    first: Box::new(Tree::Pane(at)),
                    second: Box::new(Tree::Pane(new)),
                };
                true
            }
            Tree::Pane(_) => false,
            Tree::Split { first, second, .. } => {
                first.split(at, new, split) || second.split(at, new, split)
            }
        }
    }

    /// Takes pane `id` out; its sibling gets the space. The last pane can't
    /// be removed: the session closes instead.
    pub fn remove(&mut self, id: PaneId) -> bool {
        let rest = match self {
            Tree::Pane(_) => return false,
            Tree::Split { first, second, .. } => {
                if **first == Tree::Pane(id) {
                    std::mem::replace(&mut **second, Tree::Pane(id))
                } else if **second == Tree::Pane(id) {
                    std::mem::replace(&mut **first, Tree::Pane(id))
                } else {
                    return first.remove(id) || second.remove(id);
                }
            }
        };
        *self = rest;
        true
    }

    /// Each pane's rectangle in `area`, and the `gap`-wide dividers between
    /// them.
    pub fn layout(
        &self,
        area: Rect,
        gap: u32,
        panes: &mut Vec<(PaneId, Rect)>,
        dividers: &mut Vec<Rect>,
    ) {
        match self {
            Tree::Pane(id) => panes.push((*id, area)),
            Tree::Split {
                split,
                first,
                second,
            } => {
                let (a, divider, b) = match split {
                    Split::Right => {
                        let w = area.w.saturating_sub(gap) / 2;
                        let x = area.x + w + gap;
                        (
                            Rect { w, ..area },
                            Rect {
                                x: area.x + w,
                                w: gap,
                                ..area
                            },
                            Rect {
                                x,
                                w: (area.x + area.w).saturating_sub(x),
                                ..area
                            },
                        )
                    }
                    Split::Down => {
                        let h = area.h.saturating_sub(gap) / 2;
                        let y = area.y + h + gap;
                        (
                            Rect { h, ..area },
                            Rect {
                                y: area.y + h,
                                h: gap,
                                ..area
                            },
                            Rect {
                                y,
                                h: (area.y + area.h).saturating_sub(y),
                                ..area
                            },
                        )
                    }
                };
                dividers.push(divider);
                first.layout(a, gap, panes, dividers);
                second.layout(b, gap, panes, dividers);
            }
        }
    }

    /// The tree's shape in preorder: `r` and `d` for splits, `.` for panes.
    pub fn shape(&self) -> String {
        let mut out = String::new();
        self.write_shape(&mut out);
        out
    }

    fn write_shape(&self, out: &mut String) {
        match self {
            Tree::Pane(_) => out.push('.'),
            Tree::Split {
                split,
                first,
                second,
            } => {
                out.push(match split {
                    Split::Right => 'r',
                    Split::Down => 'd',
                });
                first.write_shape(out);
                second.write_shape(out);
            }
        }
    }

    /// Rebuilds a tree from [`Tree::shape`], numbering panes from `ids`.
    pub fn from_shape(shape: &str, ids: &mut impl Iterator<Item = PaneId>) -> Option<Tree> {
        let mut chars = shape.chars();
        let tree = Self::read_shape(&mut chars, ids)?;
        chars.next().is_none().then_some(tree)
    }

    fn read_shape(
        chars: &mut std::str::Chars<'_>,
        ids: &mut impl Iterator<Item = PaneId>,
    ) -> Option<Tree> {
        let split = match chars.next()? {
            '.' => return ids.next().map(Tree::Pane),
            'r' => Split::Right,
            'd' => Split::Down,
            _ => return None,
        };
        Some(Tree::Split {
            split,
            first: Box::new(Self::read_shape(chars, ids)?),
            second: Box::new(Self::read_shape(chars, ids)?),
        })
    }
}

/// The pane next to `from` in `direction`: the nearest one that lines up
/// with it, preferring the one whose edge is closest to `from`'s.
pub fn neighbor(panes: &[(PaneId, Rect)], from: PaneId, direction: Direction) -> Option<PaneId> {
    let (_, at) = panes.iter().find(|(id, _)| *id == from)?;
    let overlaps = |a: u32, aw: u32, b: u32, bw: u32| a < b + bw && b < a + aw;
    panes
        .iter()
        .filter(|(id, _)| *id != from)
        .filter_map(|(id, r)| {
            let (ahead, lined_up, cross) = match direction {
                Direction::Right => (
                    r.x >= at.x + at.w,
                    overlaps(r.y, r.h, at.y, at.h),
                    r.y.abs_diff(at.y),
                ),
                Direction::Left => (
                    r.x + r.w <= at.x,
                    overlaps(r.y, r.h, at.y, at.h),
                    r.y.abs_diff(at.y),
                ),
                Direction::Down => (
                    r.y >= at.y + at.h,
                    overlaps(r.x, r.w, at.x, at.w),
                    r.x.abs_diff(at.x),
                ),
                Direction::Up => (
                    r.y + r.h <= at.y,
                    overlaps(r.x, r.w, at.x, at.w),
                    r.x.abs_diff(at.x),
                ),
            };
            let distance = match direction {
                Direction::Right => r.x.saturating_sub(at.x + at.w),
                Direction::Left => at.x.saturating_sub(r.x + r.w),
                Direction::Down => r.y.saturating_sub(at.y + at.h),
                Direction::Up => at.y.saturating_sub(r.y + r.h),
            };
            (ahead && lined_up).then_some((distance, cross, *id))
        })
        .min()
        .map(|(_, _, id)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        w: 201,
        h: 101,
    };

    fn layout(tree: &Tree) -> (Vec<(PaneId, Rect)>, Vec<Rect>) {
        let (mut panes, mut dividers) = (Vec::new(), Vec::new());
        tree.layout(AREA, 1, &mut panes, &mut dividers);
        (panes, dividers)
    }

    #[test]
    fn splitting_halves_the_pane_and_removing_gives_the_space_back() {
        let mut tree = Tree::Pane(1);
        assert!(tree.split(1, 2, Split::Right));
        assert!(tree.split(2, 3, Split::Down));
        assert!(!tree.split(9, 4, Split::Down));
        assert_eq!(tree.panes(), [1, 2, 3]);
        let (panes, dividers) = layout(&tree);
        assert_eq!(
            panes[0].1,
            Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 101
            }
        );
        assert_eq!(
            panes[1].1,
            Rect {
                x: 101,
                y: 0,
                w: 100,
                h: 50
            }
        );
        assert_eq!(
            panes[2].1,
            Rect {
                x: 101,
                y: 51,
                w: 100,
                h: 50
            }
        );
        assert_eq!(dividers.len(), 2);

        assert!(tree.remove(2));
        assert_eq!(tree.panes(), [1, 3]);
        assert_eq!(
            layout(&tree).0[1].1,
            Rect {
                x: 101,
                y: 0,
                w: 100,
                h: 101
            }
        );
        assert!(tree.remove(1));
        assert_eq!(tree, Tree::Pane(3));
        assert!(!tree.remove(3));
    }

    #[test]
    fn shape_round_trips() {
        let mut tree = Tree::Pane(1);
        tree.split(1, 2, Split::Right);
        tree.split(1, 3, Split::Down);
        assert_eq!(tree.shape(), "rd...");
        let back = Tree::from_shape("rd...", &mut (10..)).unwrap();
        assert_eq!(back.panes(), [10, 11, 12]);
        assert_eq!(back.shape(), "rd...");
        assert_eq!(Tree::from_shape(".", &mut (5..)), Some(Tree::Pane(5)));
        assert_eq!(Tree::from_shape("r.", &mut (0..)), None);
        assert_eq!(Tree::from_shape("..", &mut (0..)), None);
        assert_eq!(Tree::from_shape("x", &mut (0..)), None);
    }

    #[test]
    fn neighbors_follow_the_layout() {
        let mut tree = Tree::Pane(1);
        tree.split(1, 2, Split::Right);
        tree.split(2, 3, Split::Down);
        let (panes, _) = layout(&tree);
        assert_eq!(neighbor(&panes, 1, Direction::Right), Some(2));
        assert_eq!(neighbor(&panes, 3, Direction::Left), Some(1));
        assert_eq!(neighbor(&panes, 2, Direction::Down), Some(3));
        assert_eq!(neighbor(&panes, 3, Direction::Up), Some(2));
        assert_eq!(neighbor(&panes, 1, Direction::Left), None);
        assert_eq!(neighbor(&panes, 1, Direction::Up), None);
    }
}
