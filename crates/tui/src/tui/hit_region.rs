//! Tiny hit-region foundation (COH-06 / interaction layer START).
//!
//! Maps screen coordinates → logical targets for one list surface at a time.
//! Keyboard focus and mouse hover/click share the same selected row index —
//! dual-path, never mouse-only.
//!
//! Scope tonight: rect + id model + hit-test helpers. Consumers record regions
//! during render and resolve mouse events against them. No global mouse layer,
//! no Fleet rebuild.

use ratatui::layout::Rect;

/// Logical target inside a hit-tested list surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HitTarget {
    /// A selectable row at `index` within the list's current filter order.
    Row { index: usize },
}

/// One hit-testable region: a rectangle paired with a logical target id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitRegion {
    pub target: HitTarget,
    pub rect: Rect,
}

/// Ordered set of hit regions for the last paint of a list.
///
/// Regions are checked in reverse paint order (later = on top). Empty maps
/// never match — callers must rebuild after each render that lays out rows.
#[derive(Debug, Default, Clone)]
pub struct HitMap {
    regions: Vec<HitRegion>,
}

impl HitMap {
    #[must_use]
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
        }
    }

    /// Drop all regions (next paint rebuilds). Part of the foundation API for
    /// multi-frame consumers; single-paint lists often replace the whole map.
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    /// Record a full-width row hit region at terminal row `y`.
    pub fn push_row(&mut self, index: usize, x: u16, y: u16, width: u16) {
        if width == 0 {
            return;
        }
        self.regions.push(HitRegion {
            target: HitTarget::Row { index },
            rect: Rect {
                x,
                y,
                width,
                height: 1,
            },
        });
    }

    #[allow(dead_code)] // foundation API — mode picker uses push_row
    pub fn push(&mut self, target: HitTarget, rect: Rect) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        self.regions.push(HitRegion { target, rect });
    }

    /// Resolve terminal coordinates to a hit target, if any.
    #[must_use]
    pub fn hit_test(&self, column: u16, row: u16) -> Option<HitTarget> {
        // Reverse: later regions win (stacked/overlapping content).
        self.regions
            .iter()
            .rev()
            .find_map(|region| contains(region.rect, column, row).then_some(region.target))
    }

    /// Convenience: row index under the pointer, if the hit is a row.
    #[must_use]
    pub fn row_at(&self, column: u16, row: u16) -> Option<usize> {
        match self.hit_test(column, row)? {
            HitTarget::Row { index } => Some(index),
        }
    }

    #[must_use]
    #[allow(dead_code)] // foundation API — first consumer only hit-tests
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    #[must_use]
    #[allow(dead_code)] // foundation API — first consumer only hit-tests
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Test/debug access to recorded regions (paint-order).
    #[cfg(test)]
    #[must_use]
    pub fn regions_for_test(&self) -> &[HitRegion] {
        &self.regions
    }
}

#[must_use]
fn contains(rect: Rect, column: u16, row: u16) -> bool {
    column >= rect.x
        && column < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_test_finds_row_by_coordinates() {
        let mut map = HitMap::new();
        map.push_row(0, 10, 5, 40);
        map.push_row(1, 10, 6, 40);
        map.push_row(2, 10, 7, 40);

        assert_eq!(map.row_at(10, 5), Some(0));
        assert_eq!(map.row_at(49, 6), Some(1));
        assert_eq!(map.row_at(30, 7), Some(2));
        // Outside any row.
        assert_eq!(map.row_at(9, 5), None);
        assert_eq!(map.row_at(50, 5), None);
        assert_eq!(map.row_at(30, 8), None);
    }

    #[test]
    fn later_regions_win_on_overlap() {
        let mut map = HitMap::new();
        map.push(HitTarget::Row { index: 0 }, Rect::new(0, 0, 10, 3));
        map.push(HitTarget::Row { index: 9 }, Rect::new(2, 1, 4, 1));
        assert_eq!(map.row_at(3, 1), Some(9));
        assert_eq!(map.row_at(0, 0), Some(0));
    }

    #[test]
    fn empty_map_never_hits() {
        let map = HitMap::new();
        assert!(map.is_empty());
        assert_eq!(map.hit_test(0, 0), None);
    }
}
