//! Shared list-selection navigation (#4755).
//!
//! Modal lists and config screens should wrap at the ends so Down on the last
//! row returns to the top and Up on the first row returns to the bottom.
//! Centralizing the arithmetic keeps that behavior consistent without each
//! picker inventing its own clamp.

/// Move a 0-based selection by `delta`, wrapping at both ends.
///
/// Empty lists leave the selection at `0`. A zero `len` is treated as empty.
#[must_use]
pub fn wrap_index(selected: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    (selected as isize + delta).rem_euclid(len as isize) as usize
}

#[cfg(test)]
mod tests {
    use super::wrap_index;

    #[test]
    fn wraps_forward_and_backward() {
        assert_eq!(wrap_index(0, 3, -1), 2);
        assert_eq!(wrap_index(2, 3, 1), 0);
        assert_eq!(wrap_index(1, 3, 1), 2);
        assert_eq!(wrap_index(1, 3, -1), 0);
    }

    #[test]
    fn empty_list_stays_at_zero() {
        assert_eq!(wrap_index(5, 0, 1), 0);
        assert_eq!(wrap_index(0, 0, -1), 0);
    }
}
