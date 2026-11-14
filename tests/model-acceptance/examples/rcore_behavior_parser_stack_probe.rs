#[path = "../shared/rcore_behavior_parser_stack.rs"]
mod controls;
fn main() {
    assert_eq!(controls::verify(), 8);
}
#[unsafe(no_mangle)]
pub extern "C" fn verify_rcore_behavior_parser_stack() -> u32 {
    controls::verify() as u32
}
