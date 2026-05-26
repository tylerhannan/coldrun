//! Row selection helpers for sparse filters.

/// Row indices where `mask[i]` is true. Use when few rows pass the filter.
pub fn selected_indices(mask: &[bool]) -> Vec<usize> {
    mask.iter()
        .enumerate()
        .filter(|(_, m)| **m)
        .map(|(i, _)| i)
        .collect()
}

/// Invoke `f(row)` for each row where `mask[row]` is true (no allocation on dense masks).
pub fn for_each_selected<F>(mask: &[bool], row_count: usize, mut f: F)
where
    F: FnMut(usize),
{
    if mask_is_sparse(mask) {
        for i in selected_indices(mask) {
            f(i);
        }
    } else {
        for i in 0..row_count {
            if mask.get(i).copied().unwrap_or(false) {
                f(i);
            }
        }
    }
}

/// True when iterating only selected rows is likely cheaper than a full scan.
pub fn mask_is_sparse(mask: &[bool]) -> bool {
    let n = mask.len();
    if n == 0 {
        return false;
    }
    let selected = mask.iter().filter(|&&b| b).count();
    selected * 4 < n
}
