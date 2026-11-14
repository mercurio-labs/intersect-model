//! Portable logical sizes for storage and work accounting.
//!
//! Accounting counters (retained model storage, logical bytes, work derived from
//! bytes) are part of observable results and must agree across native and WASM
//! hosts. `std::mem::size_of` depends on target pointer width, so accounting
//! never charges it directly. Each charge names a fixed logical size instead,
//! pinned to the 64-bit layout, with a compile-time check that the type's real
//! layout fits on every target.

/// One logical machine word, independent of target pointer width.
pub const LOGICAL_WORD: usize = 8;

/// Fixed logical size for accounting `$ty`, independent of target pointer width.
///
/// `$bytes` is the type's 64-bit layout size. Compilation fails if the real
/// layout of `$ty` exceeds `$bytes` on the current target.
#[macro_export]
macro_rules! logical_size {
    ($ty:ty, $bytes:expr) => {{
        const { assert!(::core::mem::size_of::<$ty>() <= $bytes) };
        let bytes: usize = $bytes;
        bytes
    }};
}

#[cfg(test)]
mod tests {
    #[test]
    fn logical_size_is_the_declared_portable_value() {
        assert_eq!(crate::logical_size!(&str, 16), 16);
        assert_eq!(crate::logical_size!(usize, super::LOGICAL_WORD), 8);
    }
}
