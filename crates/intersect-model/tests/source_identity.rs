//! The compiler identity is one SHA-256 digest in lowercase hex.
#[test]
fn source_identity_is_a_sha256_hex_digest() {
    let identity = intersect_model::SOURCE_IDENTITY;
    assert_eq!(identity.len(), 64, "{identity}");
    assert!(
        identity
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "{identity}"
    );
}
