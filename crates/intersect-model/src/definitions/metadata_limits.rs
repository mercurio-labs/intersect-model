//! Allocation-free, depth-bounded traversal of all serializable context metadata.
/// Shared logical metadata overheads for serialization and encoded JSON preflight.
pub const ATOM_METADATA_BYTES: usize = 32;
pub const STRING_METADATA_OVERHEAD: usize = 2;
pub const CONTAINER_METADATA_OVERHEAD: usize = 2;
pub const NULL_METADATA_BYTES: usize = 4;
use serde::{
    Serialize,
    ser::{
        self, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
        SerializeTupleStruct, SerializeTupleVariant,
    },
};
#[derive(Debug)]
struct Error(String);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
impl ser::Error for Error {
    fn custom<T: std::fmt::Display>(message: T) -> Self {
        Self(message.to_string())
    }
}
type Result<T> = std::result::Result<T, Error>;
struct Count {
    nodes: usize,
    bytes: usize,
    max_nodes: usize,
    max_bytes: usize,
    max_depth: usize,
}
struct Sink<'a> {
    count: &'a mut Count,
    depth: usize,
    work: Option<&'a dyn Fn(usize) -> std::result::Result<(), String>>,
}
impl Sink<'_> {
    fn charge(&mut self, bytes: usize) -> Result<()> {
        self.count.nodes = self
            .count
            .nodes
            .checked_add(1)
            .filter(|n| *n <= self.count.max_nodes)
            .ok_or_else(|| Error("Context metadata node limit exceeded".into()))?;
        self.count.bytes = self
            .count
            .bytes
            .checked_add(bytes)
            .filter(|n| *n <= self.count.max_bytes)
            .ok_or_else(|| Error("Context metadata byte limit exceeded".into()))?;
        if self.depth > self.count.max_depth {
            return Err(Error("Context metadata depth limit exceeded".into()));
        }
        if let Some(work) = self.work {
            work(1).map_err(Error)?;
        }
        Ok(())
    }
    fn child<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        value.serialize(Sink {
            count: self.count,
            depth: self.depth + 1,
            work: self.work,
        })
    }
    fn container(mut self, len: Option<usize>) -> Result<Self> {
        self.charge(CONTAINER_METADATA_OVERHEAD)?;
        if len.is_some_and(|n| n > self.count.max_nodes.saturating_sub(self.count.nodes)) {
            return Err(Error("Context metadata width limit exceeded".into()));
        }
        Ok(self)
    }
}
macro_rules! number{($($name:ident:$ty:ty),*)=>{$(fn $name(mut self,_:$ty)->Result<()>{self.charge(ATOM_METADATA_BYTES)})*};}
impl<'a> ser::Serializer for Sink<'a> {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;
    number!(serialize_bool:bool,serialize_i8:i8,serialize_i16:i16,serialize_i32:i32,serialize_i64:i64,serialize_u8:u8,serialize_u16:u16,serialize_u32:u32,serialize_u64:u64,serialize_f32:f32,serialize_f64:f64,serialize_char:char);
    fn serialize_str(mut self, v: &str) -> Result<()> {
        self.charge(v.len().saturating_add(STRING_METADATA_OVERHEAD))
    }
    fn collect_str<T: ?Sized + std::fmt::Display>(mut self, value: &T) -> Result<()> {
        use std::fmt::Write;
        // Precharge the string node before calling the formatter. Keep its
        // UTF-8 chunks borrowed instead of allocating the default to_string().
        self.charge(STRING_METADATA_OVERHEAD)?;
        struct Chunks<'a> {
            count: &'a mut Count,
            failure: Option<&'static str>,
        }
        impl std::fmt::Write for Chunks<'_> {
            fn write_str(&mut self, text: &str) -> std::fmt::Result {
                let next = self.count.bytes.checked_add(text.len()).ok_or_else(|| {
                    self.failure = Some("Context metadata size overflow");
                    std::fmt::Error
                })?;
                if next > self.count.max_bytes {
                    self.failure = Some("Context metadata byte limit exceeded");
                    return Err(std::fmt::Error);
                }
                self.count.bytes = next;
                Ok(())
            }
        }
        let mut chunks = Chunks {
            count: self.count,
            failure: None,
        };
        write!(&mut chunks, "{value}").map_err(|_| {
            Error(
                chunks
                    .failure
                    .unwrap_or("Context metadata display formatting failed")
                    .into(),
            )
        })
    }
    fn serialize_bytes(mut self, v: &[u8]) -> Result<()> {
        self.charge(v.len().saturating_add(STRING_METADATA_OVERHEAD))
    }
    fn serialize_none(mut self) -> Result<()> {
        self.charge(NULL_METADATA_BYTES)
    }
    fn serialize_some<T: ?Sized + Serialize>(mut self, v: &T) -> Result<()> {
        self.charge(1)?;
        self.child(v)
    }
    fn serialize_unit(mut self) -> Result<()> {
        self.charge(NULL_METADATA_BYTES)
    }
    fn serialize_unit_struct(mut self, _: &'static str) -> Result<()> {
        self.charge(NULL_METADATA_BYTES)
    }
    fn serialize_unit_variant(mut self, _: &'static str, _: u32, v: &'static str) -> Result<()> {
        self.charge(v.len())
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        mut self,
        _: &'static str,
        v: &T,
    ) -> Result<()> {
        self.charge(1)?;
        self.child(v)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        mut self,
        _: &'static str,
        _: u32,
        name: &'static str,
        v: &T,
    ) -> Result<()> {
        self.charge(name.len())?;
        self.child(v)
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self> {
        self.container(len)
    }
    fn serialize_tuple(self, len: usize) -> Result<Self> {
        self.container(Some(len))
    }
    fn serialize_tuple_struct(self, _: &'static str, len: usize) -> Result<Self> {
        self.container(Some(len))
    }
    fn serialize_tuple_variant(
        mut self,
        _: &'static str,
        _: u32,
        name: &'static str,
        len: usize,
    ) -> Result<Self> {
        self.charge(name.len())?;
        self.container(Some(len))
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self> {
        self.container(len)
    }
    fn serialize_struct(self, _: &'static str, len: usize) -> Result<Self> {
        self.container(Some(len))
    }
    fn serialize_struct_variant(
        mut self,
        _: &'static str,
        _: u32,
        name: &'static str,
        len: usize,
    ) -> Result<Self> {
        self.charge(name.len())?;
        self.container(Some(len))
    }
}
macro_rules! sequence {
    ($trait:ident,$method:ident) => {
        impl $trait for Sink<'_> {
            type Ok = ();
            type Error = Error;
            fn $method<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<()> {
                self.child(v)
            }
            fn end(self) -> Result<()> {
                Ok(())
            }
        }
    };
}
sequence!(SerializeSeq, serialize_element);
sequence!(SerializeTuple, serialize_element);
sequence!(SerializeTupleStruct, serialize_field);
sequence!(SerializeTupleVariant, serialize_field);
impl SerializeMap for Sink<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<()> {
        self.child(v)
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<()> {
        self.child(v)
    }
    fn end(self) -> Result<()> {
        Ok(())
    }
}
macro_rules! structure {
    ($trait:ident) => {
        impl $trait for Sink<'_> {
            type Ok = ();
            type Error = Error;
            fn serialize_field<T: ?Sized + Serialize>(
                &mut self,
                key: &'static str,
                v: &T,
            ) -> Result<()> {
                self.charge(key.len())?;
                self.child(v)
            }
            fn end(self) -> Result<()> {
                Ok(())
            }
        }
    };
}
structure!(SerializeStruct);
structure!(SerializeStructVariant);
/// Aggregate traversal allowance shared across independently borrowed metadata roots.
pub struct TraversalBudget {
    count: Count,
}
impl TraversalBudget {
    pub(crate) fn payload_bytes(&self) -> usize {
        self.count.bytes
    }
    pub(crate) fn inspect_at_depth<T: ?Sized + Serialize>(
        &mut self,
        value: &T,
        depth: usize,
        work: &dyn Fn(usize) -> std::result::Result<(), String>,
    ) -> std::result::Result<(), String> {
        value
            .serialize(Sink {
                count: &mut self.count,
                depth,
                work: Some(work),
            })
            .map_err(|e| e.to_string())
    }
    pub(crate) fn enter_container(
        &mut self,
        len: usize,
        depth: usize,
        work: &dyn Fn(usize) -> std::result::Result<(), String>,
    ) -> std::result::Result<(), String> {
        Sink {
            count: &mut self.count,
            depth,
            work: Some(work),
        }
        .container(Some(len))
        .map(|_| ())
        .map_err(|e| e.to_string())
    }
    pub(crate) fn field(
        &mut self,
        name: &str,
        depth: usize,
        work: &dyn Fn(usize) -> std::result::Result<(), String>,
    ) -> std::result::Result<(), String> {
        Sink {
            count: &mut self.count,
            depth,
            work: Some(work),
        }
        .charge(name.len())
        .map_err(|e| e.to_string())
    }
    pub fn visited_nodes(&self) -> usize {
        self.count.nodes
    }
    pub fn new(nodes: usize, depth: usize, bytes: usize) -> Self {
        Self {
            count: Count {
                nodes: 0,
                bytes: 0,
                max_nodes: nodes,
                max_depth: depth,
                max_bytes: bytes,
            },
        }
    }
    pub fn inspect<T: ?Sized + Serialize>(
        &mut self,
        value: &T,
    ) -> std::result::Result<usize, String> {
        self.inspect_inner(value, None)
    }
    /// Charge each metadata visit before traversing its children. This remains
    /// a synchronous serializer walk, with cooperative cancellation/budget hooks.
    pub fn inspect_with_work<T: ?Sized + Serialize>(
        &mut self,
        value: &T,
        work: &dyn Fn(usize) -> std::result::Result<(), String>,
    ) -> std::result::Result<usize, String> {
        self.inspect_inner(value, Some(work))
    }
    fn inspect_inner<T: ?Sized + Serialize>(
        &mut self,
        value: &T,
        work: Option<&dyn Fn(usize) -> std::result::Result<(), String>>,
    ) -> std::result::Result<usize, String> {
        value
            .serialize(Sink {
                count: &mut self.count,
                depth: 0,
                work,
            })
            .map_err(|e| e.to_string())?;
        Ok(self.count.bytes)
    }
}
pub fn measure<T: ?Sized + Serialize>(
    value: &T,
    nodes: usize,
    depth: usize,
    bytes: usize,
) -> std::result::Result<usize, String> {
    TraversalBudget::new(nodes, depth, bytes).inspect(value)
}
/// Synchronous measurement using the same depth/width/byte authority, with one
/// precharged work unit per visit and no serialized output allocation.
pub fn measure_with_work<T: ?Sized + Serialize>(
    value: &T,
    nodes: usize,
    depth: usize,
    bytes: usize,
    work: &dyn Fn(usize) -> std::result::Result<(), String>,
) -> std::result::Result<usize, String> {
    TraversalBudget::new(nodes, depth, bytes).inspect_with_work(value, work)
}
// Count actual encoded bytes without allocating the serialized result. Context
// preflight bounds traversal first; diagnostic metadata comes from verified IR.
pub fn serialized_size<T: ?Sized + Serialize>(
    value: &T,
    limit: usize,
) -> std::result::Result<usize, String> {
    serialized_size_inner(value, limit, None)
}
/// Count exact encoded bytes without retaining output, precharging one work
/// unit before each emitted chunk. Serialization remains synchronous.
pub fn serialized_size_with_work<T: ?Sized + Serialize>(
    value: &T,
    limit: usize,
    work: &dyn Fn(usize) -> std::result::Result<(), String>,
) -> std::result::Result<usize, String> {
    serialized_size_inner(value, limit, Some(work))
}
fn serialized_size_inner<T: ?Sized + Serialize>(
    value: &T,
    limit: usize,
    work: Option<&dyn Fn(usize) -> std::result::Result<(), String>>,
) -> std::result::Result<usize, String> {
    struct Writer<'a> {
        used: usize,
        limit: usize,
        work: Option<&'a dyn Fn(usize) -> std::result::Result<(), String>>,
    }
    impl std::io::Write for Writer<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let next = self
                .used
                .checked_add(bytes.len())
                .filter(|n| *n <= self.limit)
                .ok_or_else(|| std::io::Error::other("Diagnostic metadata byte limit exceeded"))?;
            if let Some(work) = self.work {
                work(1).map_err(std::io::Error::other)?;
            }
            self.used = next;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Writer {
        used: 0,
        limit,
        work,
    };
    serde_json::to_writer(&mut writer, value).map_err(|e| e.to_string())?;
    Ok(writer.used)
}

#[cfg(test)]
mod work_tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;

    #[test]
    fn collected_text_is_precharged_and_measured_in_borrowed_chunks() {
        struct Text<'a>(&'a Cell<usize>);
        impl std::fmt::Display for Text<'_> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.set(self.0.get() + 1);
                f.write_str("?")?;
                f.write_str("é")
            }
        }
        impl Serialize for Text<'_> {
            fn serialize<S: ser::Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }
        let calls = Cell::new(0);
        let value = Text(&calls);
        let mut budget = TraversalBudget::new(1, 0, 5);
        assert_eq!(budget.inspect(&value).unwrap(), 5);
        assert_eq!(budget.visited_nodes(), 1);
        assert_eq!(calls.get(), 1);
        let mut budget = TraversalBudget::new(1, 0, 5);
        assert_eq!(
            budget
                .inspect_with_work(&value, &|_| Err("cancelled before formatting".into()))
                .unwrap_err(),
            "cancelled before formatting"
        );
        assert_eq!(calls.get(), 1);
        let mut budget = TraversalBudget::new(1, 0, 4);
        assert_eq!(
            budget.inspect(&value).unwrap_err(),
            "Context metadata byte limit exceeded"
        );
        struct Broken;
        impl std::fmt::Display for Broken {
            fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                Err(std::fmt::Error)
            }
        }
        impl Serialize for Broken {
            fn serialize<S: ser::Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }
        assert_eq!(
            TraversalBudget::new(1, 0, 5).inspect(&Broken).unwrap_err(),
            "Context metadata display formatting failed"
        );
    }

    #[test]
    fn encoded_size_charges_each_chunk_and_preserves_exact_escaping() {
        let value = json!("\0\né");
        assert_eq!(serialized_size(&value, 12).unwrap(), 12);
        assert!(serialized_size(&value, 11).is_err());
        let total = Cell::new(0usize);
        assert_eq!(
            serialized_size_with_work(&value, 12, &|units| {
                assert_eq!(units, 1);
                total.set(total.get() + units);
                Ok(())
            })
            .unwrap(),
            12
        );
        assert!(total.get() > 3);
        for allowance in 0..total.get() {
            let used = Cell::new(0usize);
            let error = serialized_size_with_work(&value, 12, &|units| {
                if used.get() == allowance {
                    return Err("encoding cancelled".into());
                }
                used.set(used.get() + units);
                Ok(())
            })
            .unwrap_err();
            assert_eq!(error, "encoding cancelled");
            assert_eq!(used.get(), allowance);
        }
        assert_eq!(
            serialized_size_with_work(&value, 11, &|_| Ok(())),
            serialized_size(&value, 11)
        );
        assert_eq!(value, json!("\0\né"));
    }

    #[test]
    fn work_is_precharged_at_every_visit_and_stops_before_remaining_children() {
        let value = json!([1, 2, 3]);
        let expected = measure(&value, 4, 2, 98).unwrap();
        assert_eq!(expected, 98);
        for allowance in 0..=4 {
            let used = Cell::new(0usize);
            let work = |units| {
                assert_eq!(units, 1);
                if used.get() == allowance {
                    return Err("metadata cancelled".into());
                }
                used.set(used.get() + units);
                Ok(())
            };
            let result = measure_with_work(&value, 4, 2, 98, &work);
            assert_eq!(used.get(), allowance);
            if allowance < 4 {
                assert_eq!(result.unwrap_err(), "metadata cancelled");
            } else {
                assert_eq!(result.unwrap(), expected);
            }
        }
        assert_eq!(value, json!([1, 2, 3]));
    }

    #[test]
    fn observed_and_unobserved_measurement_share_shape_errors_and_aggregate_limits() {
        let value = json!({"nested": [["payload"]]});
        for (nodes, depth, bytes) in [(1, 8, 1024), (32, 1, 1024), (32, 8, 4)] {
            assert_eq!(
                measure_with_work(&value, nodes, depth, bytes, &|_| Ok(())),
                measure(&value, nodes, depth, bytes)
            );
        }
        let mut budget = TraversalBudget::new(2, 1, 64);
        let used = Cell::new(0usize);
        let work = |units| {
            used.set(used.get() + units);
            Ok(())
        };
        assert_eq!(budget.inspect_with_work(&1, &work).unwrap(), 32);
        assert_eq!(budget.inspect_with_work(&2, &work).unwrap(), 64);
        assert!(budget.inspect_with_work(&3, &work).is_err());
        assert_eq!(used.get(), 2);
    }
}
