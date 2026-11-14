//! Shared borrowed UTF-8 byte equality. Handwritten semantic implementation.
//!
//! Equality of valid UTF-8 strings is equality of their bytes. Chunk boundaries
//! need not be character boundaries because this engine never slices `str`.
//! The caller owns admission, current cancellation authority and logical fees.
//! No source, type, provider or completed-validation authority is created here.

// Keep long comparisons observable across the ordinary worker quantum256.
// This is a scheduling grain, not a logical work charge or input limit.
pub(crate) const CHUNK_BYTES: usize = 256;

pub(crate) struct Equality<'a> {
    left: &'a str,
    right: &'a str,
    offset: usize,
    result: Option<bool>,
}

impl<'a> Equality<'a> {
    pub(crate) fn new(left: &'a str, right: &'a str) -> Self {
        Self {
            left,
            right,
            offset: 0,
            result: None,
        }
    }

    /// One length check and at most one bounded pair of borrowed byte slices.
    /// Returning Pending does not itself create a semantic work debit.
    pub(crate) fn advance(&mut self) -> Option<bool> {
        if let Some(result) = self.result {
            return Some(result);
        }
        let left = self.left.as_bytes();
        let right = self.right.as_bytes();
        if left.len() != right.len() {
            self.result = Some(false);
            return self.result;
        }
        let end = self.offset.saturating_add(CHUNK_BYTES).min(left.len());
        if left[self.offset..end] != right[self.offset..end] {
            self.result = Some(false);
        } else {
            self.offset = end;
            if end == left.len() {
                self.result = Some(true);
            }
        }
        self.result
    }

    #[cfg(any(test, feature = "allocation-profile"))]
    pub(crate) fn progress(&self) -> (usize, usize) {
        (self.offset, self.left.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(left: &str, right: &str) -> (bool, usize) {
        let mut cursor = Equality::new(left, right);
        let mut last = 0;
        let mut steps = 0;
        loop {
            steps += 1;
            let result = cursor.advance();
            let (offset, total) = cursor.progress();
            assert_eq!(total, left.len());
            assert!(offset >= last && offset - last <= CHUNK_BYTES);
            assert!(offset <= total);
            last = offset;
            assert!(steps <= left.len().div_ceil(CHUNK_BYTES).max(1));
            if let Some(result) = result {
                assert_eq!(cursor.advance(), Some(result));
                assert_eq!(cursor.progress(), (last, total));
                return (result, steps);
            }
        }
    }

    #[test]
    fn full_cross_product_matches_independent_standard_string_equality() {
        let mut values = vec![
            String::new(),
            "x".into(),
            "y".into(),
            "a\"\\\n\0é€😀".into(),
        ];
        for length in [4095, 4096, 4097, 8191, 8192, 8193, 240000] {
            let original = "x".repeat(length);
            values.push(original.clone());
            for index in [0, length / 2, length - 1] {
                let mut changed = original.clone();
                changed.replace_range(index..index + 1, "y");
                values.push(changed);
            }
        }
        values.push("é€😀".repeat(4097));
        values.push("é€😀".repeat(4096) + "é€🙂");
        for left in &values {
            for right in &values {
                assert_eq!(drain(left, right).0, left == right);
            }
        }
    }

    #[test]
    fn byte_chunks_can_end_inside_multibyte_characters_without_slicing_str() {
        for padding in 0..4 {
            let same = "x".repeat(padding) + "😀é€".repeat(1200).as_str();
            let different = same.clone() + "x";
            assert!(drain(&same, &same).0);
            assert!(!drain(&same, &different).0);
        }
    }

    #[test]
    fn first_middle_last_mismatches_stop_in_the_first_mismatched_chunk() {
        let left = "x".repeat(240000);
        for index in [0, 4095, 4096, 120000, 239999] {
            let right = left[..index].to_owned() + "y" + &left[index + 1..];
            let (equal, steps) = drain(&left, &right);
            assert!(!equal);
            assert_eq!(steps, index / CHUNK_BYTES + 1);
        }
    }

    #[test]
    fn unequal_lengths_reject_before_scanning_the_long_equal_prefix() {
        let left = "x".repeat(240000);
        let mut cursor = Equality::new(&left, &left[..239999]);
        assert_eq!(cursor.advance(), Some(false));
        assert_eq!(cursor.progress(), (0, 240000));
    }

    #[test]
    fn dropping_a_partial_cursor_leaves_borrowed_inputs_usable() {
        let left = "AB".repeat(120000);
        let right = left.clone();
        {
            let mut cursor = Equality::new(&left, &right);
            assert_eq!(cursor.advance(), None);
            assert_eq!(cursor.progress(), (CHUNK_BYTES, left.len()));
        }
        assert_eq!(left, right);
        assert_eq!(
            drain(&left, &right),
            (true, left.len().div_ceil(CHUNK_BYTES))
        );
    }
}
