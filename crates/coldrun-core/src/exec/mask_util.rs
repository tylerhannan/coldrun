//! Row selection helpers for sparse filters.

/// Row indices where `mask[i]` is true. Use when few rows pass the filter.
pub fn selected_indices(mask: &[bool]) -> Vec<usize> {
    mask.iter()
        .enumerate()
        .filter(|(_, m)| **m)
        .map(|(i, _)| i)
        .collect()
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
