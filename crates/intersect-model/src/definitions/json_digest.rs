//! Handwritten streaming hash adapter over Serde's authoritative JSON encoder.
//! Serialization remains synchronous; the writer retains no encoded payload.
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::{self, Write};

pub(crate) fn digest_with_work<T: Serialize + ?Sized>(
    value: &T,
    work: &dyn Fn(usize) -> Result<(), String>,
) -> Result<String, String> {
    digest_inner(value, work, false)
}
/// Caller precharges the fixed 256-byte buffer and owned digest string.
pub(crate) fn digest_with_work_buffered<T: Serialize + ?Sized>(
    value: &T,
    work: &dyn Fn(usize) -> Result<(), String>,
) -> Result<String, String> {
    digest_inner(value, work, true)
}
fn digest_inner<T: Serialize + ?Sized>(
    value: &T,
    work: &dyn Fn(usize) -> Result<(), String>,
    buffered: bool,
) -> Result<String, String> {
    let mut writer = HashWriter {
        hash: Sha256::new(),
        work,
        failure: None,
    };
    let encoded = if buffered {
        let mut blocks = BufferedWrite {
            writer: &mut writer,
            bytes: [0; 256],
            used: 0,
        };
        serde_json::to_writer(&mut blocks, value)
            .and_then(|()| blocks.flush().map_err(serde_json::Error::io))
    } else {
        serde_json::to_writer(&mut writer, value)
    };
    if let Err(error) = encoded {
        return Err(writer.failure.unwrap_or_else(|| error.to_string()));
    }
    Ok(format!("{:x}", writer.hash.finalize()))
}

pub(crate) struct BufferedWrite<'writer, W> {
    pub(crate) writer: &'writer mut W,
    pub(crate) bytes: [u8; 256],
    pub(crate) used: usize,
}
impl<W: std::io::Write> std::io::Write for BufferedWrite<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut remaining = bytes;
        while !remaining.is_empty() {
            let count = remaining.len().min(self.bytes.len() - self.used);
            self.bytes[self.used..self.used + count].copy_from_slice(&remaining[..count]);
            self.used += count;
            remaining = &remaining[count..];
            if self.used == self.bytes.len() {
                self.flush()?;
            }
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if self.used != 0 {
            self.writer.write_all(&self.bytes[..self.used])?;
            self.used = 0;
        }
        Ok(())
    }
}
struct HashWriter<'a> {
    hash: Sha256,
    work: &'a dyn Fn(usize) -> Result<(), String>,
    failure: Option<String>,
}
impl Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        // Charge before hashing, including escape and punctuation bytes emitted
        // by Serde. An observer error never publishes a partial digest.
        if let Err(error) = (self.work)(bytes.len()) {
            self.failure = Some(error.clone());
            return Err(io::Error::other(error));
        }
        self.hash.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[derive(Serialize)]
    struct Wire<'a> {
        title: &'a str,
        absent: Option<usize>,
        values: Vec<serde_json::Value>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        omitted: Vec<usize>,
    }

    #[test]
    fn streaming_identity_and_work_match_exact_authoritative_bytes() {
        let wire = Wire {
            title: "é\n\"\\",
            absent: None,
            values: vec![
                serde_json::json!({"z": [1, true], "a": null}),
                serde_json::json!(-0.0),
            ],
            omitted: Vec::new(),
        };
        let encoded = serde_json::to_vec(&wire).unwrap();
        assert_eq!(
            std::str::from_utf8(&encoded).unwrap(),
            "{\"title\":\"é\\n\\\"\\\\\",\"absent\":null,\"values\":[{\"a\":null,\"z\":[1,true]},-0.0]}"
        );
        let bytes = Cell::new(0);
        let digest = digest_with_work(&wire, &|units| {
            bytes.set(bytes.get() + units);
            Ok(())
        })
        .unwrap();
        assert_eq!(digest, format!("{:x}", Sha256::digest(&encoded)));
        assert_eq!(bytes.get(), encoded.len());
        assert_eq!(
            digest_with_work(&serde_json::json!({}), &|_| Ok(())).unwrap(),
            "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        );

        for limit in 0..encoded.len() {
            let charged = Cell::new(0);
            assert_eq!(
                digest_with_work(&wire, &|units| {
                    if charged.get() + units > limit {
                        return Err("exact digest work limit".into());
                    }
                    charged.set(charged.get() + units);
                    Ok(())
                }),
                Err("exact digest work limit".into())
            );
            assert!(charged.get() <= limit);
        }
    }

    #[test]
    fn buffered_digest_preserves_authoritative_bytes_and_bounds_each_actual_hash_update() {
        for value in [
            serde_json::json!({}),
            serde_json::json!({"unicode":"é\n\"\\", "values":[true,null,-17,"x".repeat(4097)]}),
        ] {
            let encoded = serde_json::to_vec(&value).unwrap();
            let bytes = Cell::new(0);
            let largest = Cell::new(0);
            let calls = Cell::new(0);
            let result = digest_with_work_buffered(&value, &|n| {
                bytes.set(bytes.get() + n);
                largest.set(largest.get().max(n));
                calls.set(calls.get() + 1);
                Ok(())
            })
            .unwrap();
            assert_eq!(result, format!("{:x}", Sha256::digest(&encoded)));
            assert_eq!(bytes.get(), encoded.len());
            assert!(largest.get() <= 256);
            assert_eq!(calls.get(), encoded.len().div_ceil(256));
            for stop in 0..calls.get() {
                let seen = Cell::new(0);
                assert_eq!(
                    digest_with_work_buffered(&value, &|_| {
                        let call = seen.get();
                        seen.set(call + 1);
                        if call == stop {
                            Err("buffered digest cancelled".into())
                        } else {
                            Ok(())
                        }
                    }),
                    Err("buffered digest cancelled".into())
                );
                assert_eq!(
                    digest_with_work_buffered(&value, &|_| Ok(())).unwrap(),
                    result
                );
            }
        }
        let invalid = std::collections::BTreeMap::from([(vec![1, 2], true)]);
        assert_eq!(
            digest_with_work_buffered(&invalid, &|_| Ok(())),
            Err(serde_json::to_vec(&invalid).unwrap_err().to_string())
        );
    }
    #[test]
    fn cancellation_precedes_hashing_and_serialization_failures_remain_errors() {
        let calls = Cell::new(0);
        assert_eq!(
            digest_with_work(&serde_json::json!(["large", 42]), &|_| {
                calls.set(calls.get() + 1);
                Err("cancel exact message".into())
            }),
            Err("cancel exact message".into())
        );
        assert_eq!(calls.get(), 1);
        let invalid = std::collections::BTreeMap::from([(vec![1, 2], true)]);
        let error = serde_json::to_vec(&invalid).unwrap_err().to_string();
        assert_eq!(digest_with_work(&invalid, &|_| Ok(())), Err(error));
    }
}
