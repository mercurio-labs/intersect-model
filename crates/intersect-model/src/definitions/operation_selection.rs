//! Shared most-specific candidate selection for concrete and generic operations.
pub(crate) fn unique_dominating_candidate<T, E>(
    candidates: &[T],
    mut dominates: impl FnMut(&T, &T) -> Result<bool, E>,
) -> Result<Option<usize>, E> {
    let mut winner = None;
    for (index, candidate) in candidates.iter().enumerate() {
        let mut all = true;
        for other in candidates {
            if !dominates(candidate, other)? {
                all = false;
                break;
            }
        }
        if all {
            if winner.is_some() {
                return Ok(None);
            }
            winner = Some(index);
        }
    }
    Ok(winner)
}
