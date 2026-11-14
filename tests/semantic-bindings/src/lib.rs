//! Executable fixture consumer; no semantic implementations belong to the test host.
pub use intersect_model_test_support::checked_contract;
pub use intersect_model_test_support::contract;
pub use intersect_model_test_support::native;
pub use intersect_model_test_support::typed as typed_native;
pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/twice.rs"));
    include!(concat!(env!("OUT_DIR"), "/doubled.rs"));
    include!(concat!(env!("OUT_DIR"), "/checked.rs"));
    include!(concat!(env!("OUT_DIR"), "/numbers.rs"));
    include!(concat!(env!("OUT_DIR"), "/optional.rs"));
}
mod qualification;
pub use qualification::qualify;
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn intersect_bindings_qualification() -> i32 {
    if qualify().is_ok() { 0 } else { 1 }
}
#[cfg(test)]
mod contracts_tests;
#[cfg(test)]
mod registry_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod values_tests;

#[cfg(feature = "rhai")]
pub mod transaction_qualification;
#[cfg(all(feature = "rhai", target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn intersect_transaction_qualification_report() -> u64 {
    let result =
        transaction_qualification::qualify_transactions().map_err(|error| error.to_string());
    let bytes = match serde_json::to_vec(&result) {
        Ok(bytes) => bytes,
        Err(_) => return 0,
    };
    let bytes = Box::leak(bytes.into_boxed_slice());
    ((bytes.len() as u64) << 32) | (bytes.as_ptr() as u32 as u64)
}
