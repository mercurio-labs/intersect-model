//! Handwritten allocation preflight for the shared serde JSON decoder.
//! Hosts retain I/O responsibility; bounded parsing and copying remain atomic.
use super::{InvocationContext, StorageResult};
use intersect_model::definitions::{
    metadata_limits::{
        ATOM_METADATA_BYTES, CONTAINER_METADATA_OVERHEAD, NULL_METADATA_BYTES,
        STRING_METADATA_OVERHEAD,
    },
    retained_storage::RETAINED_NODE_BYTES,
};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::fmt;

/// Preflight borrowed encoded text before constructing its owned JSON value.
/// Two logical parser scratch slots (64 each) reserve the encoded byte bound for
/// both passes. JSON nodes use the shared 128-per-node plus metadata-byte policy.
/// The first pass retains no value tree; serde owns final decoding semantics.
/// Unobserved compatibility decoding retains its existing shape profile.
pub fn decode_json_observed(
    text: &str,
    observation: &InvocationContext<'_>,
) -> StorageResult<Value> {
    JsonDecodeBudget::default().decode(text, observation)
}

/// One observed JSON root's shape budget, shared across separately stored items.
pub struct JsonDecodeBudget {
    remaining: usize,
    depth: usize,
}
impl Default for JsonDecodeBudget {
    fn default() -> Self {
        Self {
            remaining: 65_536,
            depth: 0,
        }
    }
}
impl JsonDecodeBudget {
    /// Reserve the outer array before decoding any of its separately stored items.
    pub fn array(observation: &InvocationContext<'_>) -> StorageResult<Self> {
        observation.checkpoint()?;
        if observation.has_observer() {
            observation.work(2)?;
            observation.retain_storage(RETAINED_NODE_BYTES + CONTAINER_METADATA_OVERHEAD)?;
        }
        Ok(Self {
            remaining: 65_535,
            depth: 1,
        })
    }
    pub fn decode(
        &mut self,
        text: &str,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Value> {
        observation.checkpoint()?;
        if !observation.has_observer() {
            return serde_json::from_str(text).map_err(|error| error.to_string());
        }
        let scratch = text
            .len()
            .checked_add(64)
            .and_then(|n| n.checked_mul(2))
            .ok_or("JSON parser scratch storage overflow")?;
        let work = text
            .len()
            .div_ceil(256)
            .checked_mul(2)
            .ok_or("JSON parser work overflow")?;
        observation.work(work)?;
        observation.retain_storage(scratch)?;
        let work = |units| observation.work(units);
        let mut state = ScanState {
            work: &work,
            nodes: 0,
            bytes: 0,
            failure: None,
            max_nodes: self.remaining,
        };
        let mut parser = serde_json::Deserializer::from_str(text);
        if let Err(error) = (Scan {
            state: &mut state,
            depth: self.depth,
        })
        .deserialize(&mut parser)
        {
            return Err(state.failure.take().unwrap_or_else(|| error.to_string()));
        }
        parser.end().map_err(|error| error.to_string())?;
        let retained = state
            .nodes
            .checked_mul(RETAINED_NODE_BYTES)
            .and_then(|n| n.checked_add(state.bytes))
            .ok_or("Retained JSON storage overflow")?;
        observation.retain_storage(retained)?;
        self.remaining -= state.nodes;
        serde_json::from_str(text).map_err(|error| error.to_string())
    }
}

struct ScanState<'a> {
    work: &'a dyn Fn(usize) -> StorageResult<()>,
    nodes: usize,
    bytes: usize,
    failure: Option<String>,
    max_nodes: usize,
}
impl ScanState<'_> {
    fn visit<E: de::Error>(&mut self, depth: usize) -> Result<(), E> {
        if let Err(error) = (self.work)(2) {
            let formatted = E::custom(&error);
            self.failure = Some(error);
            return Err(formatted);
        }
        if depth > 64 {
            return Err(E::custom("JSON metadata depth limit exceeded"));
        }
        if self.nodes >= self.max_nodes {
            return Err(E::custom("JSON metadata node limit exceeded"));
        }
        self.nodes += 1;
        Ok(())
    }
    fn add<E: de::Error>(&mut self, bytes: usize) -> Result<(), E> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| E::custom("JSON metadata size overflow"))?;
        Ok(())
    }
}
struct Scan<'a, 'b> {
    state: &'a mut ScanState<'b>,
    depth: usize,
}
impl<'de> DeserializeSeed<'de> for Scan<'_, '_> {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, parser: D) -> Result<(), D::Error> {
        self.state.visit::<D::Error>(self.depth)?;
        parser.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Scan<'_, '_> {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        self.state.add(NULL_METADATA_BYTES)
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        self.state.add(ATOM_METADATA_BYTES)
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        self.state.add(ATOM_METADATA_BYTES)
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
        self.state.add(ATOM_METADATA_BYTES)
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<(), E> {
        self.state.add(ATOM_METADATA_BYTES)
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<(), E> {
        self.state.add(
            value
                .len()
                .checked_add(STRING_METADATA_OVERHEAD)
                .ok_or_else(|| E::custom("JSON string storage overflow"))?,
        )
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<(), A::Error> {
        self.state.add(CONTAINER_METADATA_OVERHEAD)?;
        while sequence
            .next_element_seed(Scan {
                state: &mut *self.state,
                depth: self.depth + 1,
            })?
            .is_some()
        {}
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        self.state.add(CONTAINER_METADATA_OVERHEAD)?;
        while map
            .next_key_seed(Scan {
                state: &mut *self.state,
                depth: self.depth + 1,
            })?
            .is_some()
        {
            map.next_value_seed(Scan {
                state: &mut *self.state,
                depth: self.depth + 1,
            })?;
        }
        Ok(())
    }
}
