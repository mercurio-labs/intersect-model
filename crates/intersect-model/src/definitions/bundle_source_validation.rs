//! Shared source identity rules with retained URI and SHA-256 progress.
//! Callers own cancellation, observation, source lifetime and error publication.
use super::DefinitionSource;
use sha2::{Digest, Sha256};

const INPUT_CHUNK_BYTES: usize = 256;

/// End of one UTF-8-aligned input chunk, bounded by INPUT_CHUNK_BYTES.
pub(crate) fn input_chunk_end(text: &str, start: usize) -> usize {
    let mut end = start.saturating_add(INPUT_CHUNK_BYTES).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// Shared nonempty/no-control identity rule for source URIs and namespaces.
pub(crate) fn validate_identity_chunk(
    text: &str,
    start: usize,
    non_whitespace: &mut bool,
) -> Result<usize, ()> {
    let end = input_chunk_end(text, start);
    for ch in text[start..end].chars() {
        if ch.is_control() {
            return Err(());
        }
        *non_whitespace |= !ch.is_whitespace();
    }
    if end == text.len() && !*non_whitespace {
        return Err(());
    }
    Ok(end)
}

enum Phase {
    Uri,
    Digest,
    Complete,
}

pub(crate) struct SourceValidation<'source> {
    source: &'source DefinitionSource,
    phase: Phase,
    offset: usize,
    non_whitespace: bool,
    digest: Sha256,
}

impl<'source> SourceValidation<'source> {
    pub(crate) fn new(source: &'source DefinitionSource) -> Self {
        Self {
            source,
            phase: Phase::Uri,
            offset: 0,
            non_whitespace: false,
            digest: Sha256::new(),
        }
    }

    /// Inspect at most one URI or content chunk. Digest finalization is bounded;
    /// invalid-URI errors precede digest errors, exactly as in source validation.
    pub(crate) fn advance(&mut self) -> Result<bool, String> {
        match self.phase {
            Phase::Uri => {
                let uri = &self.source.id.uri;
                let end = validate_identity_chunk(uri, self.offset, &mut self.non_whitespace)
                    .map_err(|()| {
                        "Definition source URI must be nonempty and contain no controls".to_owned()
                    })?;
                self.offset = end;
                if end == uri.len() {
                    self.offset = 0;
                    self.phase = Phase::Digest;
                }
            }
            Phase::Digest => {
                let bytes = self.source.text.as_bytes();
                let end = self
                    .offset
                    .saturating_add(INPUT_CHUNK_BYTES)
                    .min(bytes.len());
                self.digest.update(&bytes[self.offset..end]);
                self.offset = end;
                if end == bytes.len() {
                    let actual = format!("{:x}", self.digest.finalize_reset());
                    if self.source.id.sha256 != actual {
                        return Err(format!(
                            "Source content digest mismatch: {}",
                            self.source.id.uri
                        ));
                    }
                    self.phase = Phase::Complete;
                }
            }
            Phase::Complete => return Ok(true),
        }
        Ok(matches!(self.phase, Phase::Complete))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::bundle::SourceKind;

    #[test]
    fn retained_source_validation_matches_independent_sha256_vectors() {
        // Fixed digests independently checked with Python hashlib, including a
        // multi-block input and a string containing a supplementary character.
        for (text, digest, digest_chunks) in [
            (
                "".to_owned(),
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                1,
            ),
            (
                "abc".to_owned(),
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
                1,
            ),
            (
                "é𝄞".to_owned(),
                "35ff9281a42a4aa3ec7d0f1e84e40bfaba55a2e98101ac9d4b5fdeb3efaa4932",
                1,
            ),
            (
                "a".repeat(257),
                "e8d95cc2b4bc198c54b40bd214df958afb65f5e73d2c2eafe0593cf5c635c1f0",
                2,
            ),
        ] {
            let mut source = DefinitionSource::new("memory:vector", SourceKind::Ecore, text);
            source.id.sha256 = digest.into();
            let mut validation = SourceValidation::new(&source);
            assert!(!validation.advance().unwrap()); // URI, without content work.
            for index in 0..digest_chunks {
                assert_eq!(validation.advance().unwrap(), index + 1 == digest_chunks);
            }
            assert!(validation.advance().unwrap());
            source.validate().unwrap();
        }
    }

    #[test]
    fn retained_source_validation_chunks_uri_and_content_without_splitting_utf8() {
        let source = DefinitionSource::new(
            format!("memory:{}", "α".repeat(300)),
            SourceKind::Ecore,
            format!("{}𝄞{}", "a".repeat(255), "b".repeat(256)),
        );
        let mut validation = SourceValidation::new(&source);
        for offset in [255, 511, 0] {
            assert!(!validation.advance().unwrap());
            assert_eq!(validation.offset, offset);
        }
        for (offset, ready) in [(256, false), (512, false), (515, true)] {
            assert_eq!(validation.advance().unwrap(), ready);
            assert_eq!(validation.offset, offset);
        }
        assert_eq!(input_chunk_end(&source.text, 0), 255);
        assert_eq!(input_chunk_end(&source.text, 255), 511);
    }

    #[test]
    fn retained_source_validation_preserves_uri_before_digest_failure() {
        for uri in ["", " \u{2003} ", "memory:bad\0", "memory:\n"] {
            let mut source = DefinitionSource::new(uri, SourceKind::Ecore, "abc");
            source.id.sha256.clear();
            assert_eq!(
                source.validate().unwrap_err(),
                "Definition source URI must be nonempty and contain no controls"
            );
        }
        let mut source =
            DefinitionSource::new("memory:changed", SourceKind::Ecore, "a".repeat(257));
        source.id.sha256 = "0".repeat(64);
        let mut validation = SourceValidation::new(&source);
        assert!(!validation.advance().unwrap());
        assert!(!validation.advance().unwrap());
        assert_eq!(
            validation.advance().unwrap_err(),
            "Source content digest mismatch: memory:changed"
        );
    }
}
