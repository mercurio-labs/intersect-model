//! Ordered string membership without cloning immutable source strings.
//! A caller must retain the same values until insertion and cleanup finish.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum IndexError<E> {
    Observation(E),
    InvalidState(&'static str),
    Allocation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IndexEvent {
    Work(usize),
    Retain(usize),
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum IndexStep {
    Pending,
    Inserted,
    Duplicate,
}
#[derive(Clone, Copy)]
enum Stage {
    Ordered,
    Bootstrap { cursor: usize, length: usize },
    Search,
    Shift { destination: usize, cursor: usize },
}
struct Pending {
    input: usize,
    low: usize,
    high: usize,
    offset: usize,
    stage: Stage,
}
#[derive(Default)]
enum Mode {
    #[default]
    Indexed,
    Streaming {
        length: usize,
    },
    Fallback,
}
fn compare_chunk<E>(
    previous: &str,
    input: &str,
    offset: usize,
) -> Result<(std::cmp::Ordering, bool, usize), IndexError<E>> {
    let previous_end = offset.saturating_add(256).min(previous.len());
    let input_end = offset.saturating_add(256).min(input.len());
    let previous_bytes =
        previous
            .as_bytes()
            .get(offset..previous_end)
            .ok_or(IndexError::InvalidState(
                "Immutable string inventory changed",
            ))?;
    let input_bytes = input
        .as_bytes()
        .get(offset..input_end)
        .ok_or(IndexError::InvalidState(
            "Immutable string inventory changed",
        ))?;
    Ok((
        previous_bytes.cmp(input_bytes),
        previous_end == previous.len() && input_end == input.len(),
        previous_end,
    ))
}
#[derive(Default)]
pub(crate) struct BorrowedStringIndex {
    indices: Vec<usize>,
    pending: Option<Pending>,
    failed: bool,
    mode: Mode,
}
impl BorrowedStringIndex {
    pub(crate) fn begin<E>(&mut self, input: usize) -> Result<(), IndexError<E>> {
        if self.failed || self.pending.is_some() {
            return Err(IndexError::InvalidState(
                "String index preparation is not ready",
            ));
        }
        self.pending = Some(Pending {
            input,
            low: 0,
            high: self.indices.len(),
            offset: 0,
            stage: Stage::Search,
        });
        Ok(())
    }
    /// Recheck the order of one immutable input; cleanup discards the scan state.
    pub(crate) fn begin_streaming<E>(&mut self, input: usize) -> Result<(), IndexError<E>> {
        if self.failed || self.pending.is_some() {
            return Err(IndexError::InvalidState(
                "String index preparation is not ready",
            ));
        }
        if matches!(self.mode, Mode::Fallback) {
            return self.begin(input);
        }
        let length = match self.mode {
            Mode::Streaming { length } => length,
            _ => 0,
        };
        if input != length || !self.indices.is_empty() {
            return Err(IndexError::InvalidState(
                "Streaming string inputs must be sequential",
            ));
        }
        self.mode = Mode::Streaming { length };
        self.pending = Some(Pending {
            input,
            low: 0,
            high: 0,
            offset: 0,
            stage: Stage::Ordered,
        });
        Ok(())
    }
    pub(crate) fn advance<E>(
        &mut self,
        values: &[impl AsRef<str>],
        mut observe: impl FnMut(IndexEvent) -> Result<(), E>,
    ) -> Result<IndexStep, IndexError<E>> {
        if self.failed {
            return Err(IndexError::InvalidState(
                "String index preparation has failed",
            ));
        }
        let result = self.advance_inner(values, &mut observe);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner<E>(
        &mut self,
        values: &[impl AsRef<str>],
        observe: &mut impl FnMut(IndexEvent) -> Result<(), E>,
    ) -> Result<IndexStep, IndexError<E>> {
        use std::cmp::Ordering;
        observe(IndexEvent::Work(1)).map_err(IndexError::Observation)?;
        let mut pending = self
            .pending
            .take()
            .ok_or(IndexError::InvalidState("Missing string index input"))?;
        let input = values
            .get(pending.input)
            .map(AsRef::as_ref)
            .ok_or(IndexError::InvalidState("Missing immutable string input"))?;
        match pending.stage {
            Stage::Ordered => {
                if pending.input == 0 {
                    self.mode = Mode::Streaming { length: 1 };
                    return Ok(IndexStep::Inserted);
                }
                let previous = values.get(pending.input - 1).map(AsRef::as_ref).ok_or(
                    IndexError::InvalidState("Immutable string inventory changed"),
                )?;
                let (order, complete, next_offset) =
                    compare_chunk(previous, input, pending.offset)?;
                match order {
                    Ordering::Less => {
                        self.mode = Mode::Streaming {
                            length: pending.input + 1,
                        };
                        return Ok(IndexStep::Inserted);
                    }
                    Ordering::Equal if complete => {
                        self.failed = true;
                        return Ok(IndexStep::Duplicate);
                    }
                    Ordering::Equal => pending.offset = next_offset,
                    Ordering::Greater => {
                        self.mode = Mode::Fallback;
                        pending.offset = 0;
                        pending.stage = Stage::Bootstrap {
                            cursor: 0,
                            length: pending.input,
                        };
                    }
                }
            }
            Stage::Bootstrap { cursor, length } => {
                if cursor == length {
                    pending.low = 0;
                    pending.high = self.indices.len();
                    pending.stage = Stage::Search;
                } else {
                    observe(IndexEvent::Retain(32)).map_err(IndexError::Observation)?;
                    self.indices
                        .try_reserve(1)
                        .map_err(|_| IndexError::Allocation)?;
                    self.indices.push(cursor);
                    pending.stage = Stage::Bootstrap {
                        cursor: cursor + 1,
                        length,
                    };
                }
            }
            Stage::Search => {
                if pending.low == pending.high {
                    // Finish an empty search range in this bounded step. Every
                    // owning transition still observes before retaining it.
                    observe(IndexEvent::Work(1)).map_err(IndexError::Observation)?;
                    observe(IndexEvent::Retain(32)).map_err(IndexError::Observation)?;
                    self.indices
                        .try_reserve(1)
                        .map_err(|_| IndexError::Allocation)?;
                    let cursor = self.indices.len();
                    self.indices.push(pending.input);
                    if cursor == pending.low {
                        observe(IndexEvent::Work(1)).map_err(IndexError::Observation)?;
                        return Ok(IndexStep::Inserted);
                    }
                    pending.stage = Stage::Shift {
                        destination: pending.low,
                        cursor,
                    };
                } else {
                    // Ordered source alternatives append after one bounded
                    // comparison. Other orders retain the same binary search.
                    let middle = if pending.low == 0 && pending.high == self.indices.len() {
                        pending.high - 1
                    } else {
                        pending.low + (pending.high - pending.low) / 2
                    };
                    let index = self
                        .indices
                        .get(middle)
                        .ok_or(IndexError::InvalidState("Missing retained string index"))?;
                    let previous =
                        values
                            .get(*index)
                            .map(AsRef::as_ref)
                            .ok_or(IndexError::InvalidState(
                                "Immutable string inventory changed",
                            ))?;
                    let (order, complete, next_offset) =
                        compare_chunk(previous, input, pending.offset)?;
                    match order {
                        Ordering::Less => {
                            pending.low = middle + 1;
                            pending.offset = 0;
                        }
                        Ordering::Greater => {
                            pending.high = middle;
                            pending.offset = 0;
                        }
                        Ordering::Equal if complete => {
                            self.failed = true;
                            return Ok(IndexStep::Duplicate);
                        }
                        Ordering::Equal => {
                            pending.offset = next_offset;
                        }
                    }
                }
            }
            Stage::Shift {
                destination,
                cursor,
            } => {
                if cursor == destination {
                    return Ok(IndexStep::Inserted);
                }
                self.indices.swap(cursor, cursor - 1);
                pending.stage = Stage::Shift {
                    destination,
                    cursor: cursor - 1,
                };
            }
        }
        self.pending = Some(pending);
        Ok(IndexStep::Pending)
    }
    pub(crate) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
    pub(crate) fn clear_one(&mut self) -> bool {
        self.indices.pop();
        let empty = self.indices.is_empty();
        if empty {
            self.mode = Mode::Indexed;
        }
        empty
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn drive(
        values: &[impl AsRef<str>],
        storage: usize,
        stop: Option<usize>,
    ) -> (Result<(), IndexError<&'static str>>, usize, usize) {
        let mut index = BorrowedStringIndex::default();
        let mut bytes = 0;
        let mut calls = 0;
        let result = (|| {
            for input in 0..values.len() {
                assert!(!index.is_pending());
                assert_eq!(index.is_empty(), input == 0);
                index.begin(input)?;
                assert!(index.is_pending());
                loop {
                    let step = index.advance(values, |event| {
                        let call = calls;
                        calls += 1;
                        if stop == Some(call) {
                            return Err("cancelled");
                        }
                        if let IndexEvent::Retain(amount) = event {
                            if bytes + amount > storage {
                                return Err("storage denied");
                            }
                            bytes += amount;
                        }
                        Ok(())
                    })?;
                    match step {
                        IndexStep::Pending => {}
                        IndexStep::Inserted => break,
                        IndexStep::Duplicate => return Err(IndexError::InvalidState("duplicate")),
                    }
                }
            }
            while !index.clear_one() {}
            Ok(())
        })();
        if matches!(result, Err(IndexError::Observation(_))) {
            assert!(matches!(
                index.advance(values, |_| Ok::<_, &'static str>(())),
                Err(IndexError::InvalidState(_))
            ));
        }
        (result, bytes, calls)
    }
    #[test]
    fn borrowed_indices_preserve_exact_unicode_prefix_and_duplicate_membership() {
        for words in [
            vec!["a", "b", "c"],
            vec!["c", "b", "a"],
            vec!["", "é", "α", "😀", "éa"],
            vec!["a", "ab", "abc", "abcd"],
            vec!["a", "ab", "a"],
        ] {
            let values = words.into_iter().map(str::to_owned).collect::<Vec<_>>();
            let original = values.clone();
            let mut oracle = std::collections::BTreeSet::new();
            let unique = values.iter().all(|value| oracle.insert(value));
            assert_eq!(drive(&values, usize::MAX, None).0.is_ok(), unique);
            assert_eq!(values, original);
            let shared = values
                .iter()
                .map(|value| std::sync::Arc::<str>::from(value.as_str()))
                .collect::<Vec<_>>();
            assert_eq!(
                drive(&shared, usize::MAX, None),
                drive(&values, usize::MAX, None)
            );
        }
    }
    #[test]
    fn long_comparisons_and_shifts_cancel_at_every_observed_transition() {
        let values = ["c", "b", "a"].map(|suffix| format!("{}{suffix}", "α".repeat(2048)));
        let (result, bytes, calls) = drive(&values, 96, None);
        assert!(result.is_ok());
        assert_eq!(bytes, 96);
        assert!(calls > 32);
        let shared = values
            .iter()
            .map(|value| std::sync::Arc::<str>::from(value.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(drive(&shared, 96, None), drive(&values, 96, None));

        for stop in 0..calls {
            assert_eq!(
                drive(&values, 96, Some(stop)).0,
                Err(IndexError::Observation("cancelled"))
            );
            assert!(drive(&values, 96, None).0.is_ok());
            assert_eq!(
                drive(&shared, 96, Some(stop)),
                drive(&values, 96, Some(stop))
            );
            assert!(drive(&shared, 96, None).0.is_ok());
        }
    }
    #[test]
    fn index_slots_are_preflighted_at_exact_storage_boundaries() {
        let values = ["c", "b", "a"].map(str::to_owned);
        for limit in [0, 31, 32, 63, 64, 95] {
            let (result, admitted, _) = drive(&values, limit, None);
            assert_eq!(result, Err(IndexError::Observation("storage denied")));
            assert_eq!(admitted, limit / 32 * 32);
        }
        assert_eq!(drive(&values, 96, None).1, 96);
    }
    fn drive_streaming(
        values: &[String],
        storage: usize,
        stop: Option<usize>,
    ) -> (Result<(), IndexError<&'static str>>, usize, usize, usize) {
        let mut index = BorrowedStringIndex::default();
        let mut bytes = 0;
        let mut calls = 0;
        let result = (|| {
            for input in 0..values.len() {
                index.begin_streaming(input)?;
                loop {
                    let step = index.advance(values, |event| {
                        let call = calls;
                        calls += 1;
                        if stop == Some(call) {
                            return Err("cancelled");
                        }
                        if let IndexEvent::Retain(amount) = event {
                            if bytes + amount > storage {
                                return Err("storage denied");
                            }
                            bytes += amount;
                        }
                        Ok(())
                    })?;
                    match step {
                        IndexStep::Pending => {}
                        IndexStep::Inserted => break,
                        IndexStep::Duplicate => return Err(IndexError::InvalidState("duplicate")),
                    }
                }
            }
            Ok(())
        })();
        let capacity = index.indices.capacity();
        if result.is_err() {
            assert!(matches!(
                index.advance(values, |_| Ok::<_, &'static str>(())),
                Err(IndexError::InvalidState(_))
            ));
        }
        while !index.clear_one() {}
        if result.is_ok() {
            assert!(matches!(index.mode, Mode::Indexed));
        }
        (result, bytes, calls, capacity)
    }
    #[test]
    fn streaming_order_checks_avoid_index_storage_and_keep_unordered_duplicate_semantics() {
        for words in [
            vec!["a", "b", "c"],
            vec!["a", "b", "c", "d", "aa"],
            vec!["c", "b", "a"],
            vec!["", "\u{00e9}", "\u{03b1}", "\u{1f600}"],
            vec!["a", "b", "c", "b"],
            vec!["a", "a"],
            vec!["z", "a", "z"],
        ] {
            let values = words.into_iter().map(str::to_owned).collect::<Vec<_>>();
            let mut oracle = std::collections::BTreeSet::new();
            let unique = values.iter().all(|value| oracle.insert(value));
            assert_eq!(drive_streaming(&values, usize::MAX, None).0.is_ok(), unique);
            assert_eq!(drive(&values, usize::MAX, None).0.is_ok(), unique);
        }
        let sorted = ["a", "b", "c"].map(str::to_owned);
        let result = drive_streaming(&sorted, 0, None);
        assert!(result.0.is_ok());
        assert_eq!((result.1, result.3), (0, 0));
        assert_eq!(drive(&sorted, 96, None).1, 96);
        let duplicate = ["a", "a"].map(str::to_owned);
        assert!(drive_streaming(&duplicate, 0, None).0.is_err());
    }
    #[test]
    fn streaming_and_fallback_cancel_every_transition_and_preflight_actual_index_slots() {
        for suffixes in [["a", "b", "c"], ["c", "b", "a"]] {
            let values = suffixes.map(|suffix| format!("{}{suffix}", "\u{03b1}".repeat(2048)));
            let (result, bytes, calls, _) = drive_streaming(&values, usize::MAX, None);
            assert!(result.is_ok());
            assert!(calls > 16);
            for stop in 0..calls {
                assert_eq!(
                    drive_streaming(&values, usize::MAX, Some(stop)).0,
                    Err(IndexError::Observation("cancelled"))
                );
                assert!(drive_streaming(&values, usize::MAX, None).0.is_ok());
            }
            if suffixes[0] == "c" {
                assert_eq!(bytes, 96);
                for limit in [0, 31, 32, 63, 64, 95] {
                    let denied = drive_streaming(&values, limit, None);
                    assert_eq!(denied.0, Err(IndexError::Observation("storage denied")));
                    assert_eq!(denied.1, limit / 32 * 32);
                }
                assert!(drive_streaming(&values, 96, None).0.is_ok());
            } else {
                assert_eq!(bytes, 0);
            }
        }
    }
}
