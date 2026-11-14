//! Shared deterministic fingerprint used by resource lifecycle and opaque decode proofs.
use serde::Serialize;
use sha2::{Digest, Sha256};
pub(crate) fn digest(value: &impl Serialize) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
