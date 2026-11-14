//! Host-only identity of this package's inputs; no sibling source inspection.
fn main() -> Result<(), String> {
    intersect_build_support::emit("INTERSECT_MODEL_SOURCE_IDENTITY")
}
