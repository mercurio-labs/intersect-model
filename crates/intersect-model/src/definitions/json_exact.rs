//! Exact comparison of canonical typed wire serialization with immutable JSON.
//! Canonical wire serializers emit unique object fields. Callers bound their
//! serialization depth. The walk remains synchronous; work hooks can interrupt
//! each visit. No output JSON graph is constructed. Map keys are temporary atoms.
use serde::{
    Serialize,
    ser::{
        self, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
        SerializeTupleStruct, SerializeTupleVariant,
    },
};
use serde_json::Value;

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
type Work<'w> = Option<&'w dyn Fn(usize) -> std::result::Result<(), String>>;
struct Compare<'a, 'w> {
    expected: Option<&'a Value>,
    work: Work<'w>,
}
impl<'a, 'w> Compare<'a, 'w> {
    fn touch(&self) -> Result<()> {
        if let Some(work) = self.work {
            work(1).map_err(Error)?;
        }
        Ok(())
    }
    fn atomic(&self, value: impl Serialize) -> Result<bool> {
        self.touch()?;
        let value = serde_json::to_value(value).map_err(|e| Error(e.to_string()))?;
        Ok(self.expected == Some(&value))
    }
    fn child<T: ?Sized + Serialize>(&self, value: &T, expected: Option<&'a Value>) -> Result<bool> {
        value.serialize(Compare {
            expected,
            work: self.work,
        })
    }
    fn compound(self, object: bool) -> Result<Compound<'a, 'w>> {
        self.touch()?;
        let matched = self.expected.is_some_and(|value| {
            if object {
                value.is_object()
            } else {
                value.is_array()
            }
        });
        Ok(Compound {
            compare: self,
            object,
            count: 0,
            key: None,
            matched,
        })
    }
    fn variant(self, name: &str) -> (Self, bool) {
        let object = self.expected.and_then(Value::as_object);
        let matched = object.is_some_and(|object| object.len() == 1 && object.contains_key(name));
        (
            Self {
                expected: object.and_then(|object| object.get(name)),
                work: self.work,
            },
            matched,
        )
    }
}
struct Compound<'a, 'w> {
    compare: Compare<'a, 'w>,
    object: bool,
    count: usize,
    key: Option<String>,
    matched: bool,
}
impl Compound<'_, '_> {
    fn element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        let expected = self
            .compare
            .expected
            .and_then(Value::as_array)
            .and_then(|values| values.get(self.count));
        self.matched &= self.compare.child(value, expected)?;
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| Error("Canonical comparison count overflow".into()))?;
        Ok(())
    }
    fn field<T: ?Sized + Serialize>(&mut self, name: &str, value: &T) -> Result<()> {
        let expected = self
            .compare
            .expected
            .and_then(Value::as_object)
            .and_then(|values| values.get(name));
        self.matched &= self.compare.child(value, expected)?;
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| Error("Canonical comparison count overflow".into()))?;
        Ok(())
    }
    fn finish(self) -> Result<bool> {
        if self.key.is_some() {
            return Err(Error("Canonical map value is missing".into()));
        }
        let length = if self.object {
            self.compare
                .expected
                .and_then(Value::as_object)
                .map(|value| value.len())
        } else {
            self.compare
                .expected
                .and_then(Value::as_array)
                .map(Vec::len)
        };
        Ok(self.matched && length == Some(self.count))
    }
}
macro_rules! numbers {($($name:ident:$ty:ty),*) => {$(fn $name(self, value:$ty)->Result<bool> {self.atomic(value)})*};}
impl<'a, 'w> ser::Serializer for Compare<'a, 'w> {
    type Ok = bool;
    type Error = Error;
    type SerializeSeq = Compound<'a, 'w>;
    type SerializeTuple = Compound<'a, 'w>;
    type SerializeTupleStruct = Compound<'a, 'w>;
    type SerializeTupleVariant = Compound<'a, 'w>;
    type SerializeMap = Compound<'a, 'w>;
    type SerializeStruct = Compound<'a, 'w>;
    type SerializeStructVariant = Compound<'a, 'w>;
    numbers!(serialize_bool:bool,serialize_i8:i8,serialize_i16:i16,serialize_i32:i32,serialize_i64:i64,
        serialize_i128:i128,serialize_u8:u8,serialize_u16:u16,serialize_u32:u32,serialize_u64:u64,
        serialize_u128:u128,serialize_f32:f32,serialize_f64:f64);
    fn serialize_char(self, value: char) -> Result<bool> {
        self.touch()?;
        Ok(self.expected.and_then(Value::as_str).is_some_and(|text| {
            let mut chars = text.chars();
            chars.next() == Some(value) && chars.next().is_none()
        }))
    }
    fn serialize_str(self, value: &str) -> Result<bool> {
        self.touch()?;
        Ok(self.expected.and_then(Value::as_str) == Some(value))
    }
    fn serialize_bytes(self, value: &[u8]) -> Result<bool> {
        let mut sequence = self.compound(false)?;
        for value in value {
            sequence.element(value)?;
        }
        sequence.finish()
    }
    fn serialize_none(self) -> Result<bool> {
        self.serialize_unit()
    }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<bool> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<bool> {
        self.touch()?;
        Ok(self.expected.is_some_and(Value::is_null))
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<bool> {
        self.serialize_unit()
    }
    fn serialize_unit_variant(self, _: &'static str, _: u32, name: &'static str) -> Result<bool> {
        self.serialize_str(name)
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<bool> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        _: u32,
        name: &'static str,
        value: &T,
    ) -> Result<bool> {
        self.touch()?;
        let (compare, matched) = self.variant(name);
        Ok(value.serialize(compare)? && matched)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq> {
        self.compound(false)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple> {
        self.compound(false)
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct> {
        self.compound(false)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        name: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant> {
        self.touch()?;
        let (compare, matched) = self.variant(name);
        let mut sequence = compare.compound(false)?;
        sequence.matched &= matched;
        Ok(sequence)
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap> {
        self.compound(true)
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self::SerializeStruct> {
        self.compound(true)
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        name: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant> {
        self.touch()?;
        let (compare, matched) = self.variant(name);
        let mut object = compare.compound(true)?;
        object.matched &= matched;
        Ok(object)
    }
    fn collect_str<T: ?Sized + std::fmt::Display>(self, value: &T) -> Result<bool> {
        self.touch()?;
        Ok(self.expected.and_then(Value::as_str) == Some(value.to_string().as_str()))
    }
}
macro_rules! sequences {($($trait:ident:$method:ident),*)=>{$(impl $trait for Compound<'_, '_> {
    type Ok=bool;type Error=Error;
    fn $method<T:?Sized+Serialize>(&mut self,value:&T)->Result<()> {self.element(value)}
    fn end(self)->Result<bool> {self.finish()}
})*};}
sequences!(SerializeSeq:serialize_element,SerializeTuple:serialize_element,
    SerializeTupleStruct:serialize_field,SerializeTupleVariant:serialize_field);
macro_rules! structs {($($trait:ident),*)=>{$(impl $trait for Compound<'_, '_> {
    type Ok=bool;type Error=Error;
    fn serialize_field<T:?Sized+Serialize>(&mut self,key:&'static str,value:&T)->Result<()> {self.field(key,value)}
    fn end(self)->Result<bool> {self.finish()}
})*};}
structs!(SerializeStruct, SerializeStructVariant);
// Let serde_json own map-key interpretation, including its rejection of invalid
// key types. This allocates only one key envelope, never the associated value.
struct Key<'a, T: ?Sized>(&'a T);
impl<T: ?Sized + Serialize> Serialize for Key<'_, T> {
    fn serialize<S: ser::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry(self.0, &())?;
        map.end()
    }
}
impl SerializeMap for Compound<'_, '_> {
    type Ok = bool;
    type Error = Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<()> {
        if self.key.is_some() {
            return Err(Error("Canonical map key has no value".into()));
        }
        self.compare.touch()?;
        let Value::Object(key) =
            serde_json::to_value(Key(key)).map_err(|e| Error(e.to_string()))?
        else {
            return Err(Error("Invalid canonical map key".into()));
        };
        self.key = Some(
            key.into_iter()
                .next()
                .ok_or_else(|| Error("Missing canonical map key".into()))?
                .0,
        );
        Ok(())
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        let key = self
            .key
            .take()
            .ok_or_else(|| Error("Canonical map value has no key".into()))?;
        self.field(&key, value)
    }
    fn end(self) -> Result<bool> {
        self.finish()
    }
}
/// Compare a canonical typed wire value without constructing its output JSON.
/// Object field order is irrelevant; field presence, array order and atom values
/// are exact. Canonical serializers must emit unique object fields.
#[cfg(test)]
pub(crate) fn matches<T: ?Sized + Serialize>(
    value: &T,
    expected: &Value,
) -> std::result::Result<bool, String> {
    compare(value, expected, None)
}
/// Same comparison, precharging one work unit at every JSON visit (including
/// map keys). A terminal callback error is never hidden by a shape mismatch.
pub fn matches_with_work<T: ?Sized + Serialize>(
    value: &T,
    expected: &Value,
    work: &dyn Fn(usize) -> std::result::Result<(), String>,
) -> std::result::Result<bool, String> {
    compare(value, expected, Some(work))
}
fn compare<T: ?Sized + Serialize>(
    value: &T,
    expected: &Value,
    work: Work<'_>,
) -> std::result::Result<bool, String> {
    value
        .serialize(Compare {
            expected: Some(expected),
            work,
        })
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{cell::Cell, collections::BTreeMap};

    #[derive(Serialize)]
    enum Variant {
        Unit,
        Newtype(i64),
        Tuple(bool, String),
        Struct { value: Vec<Option<u8>> },
    }
    #[derive(Serialize)]
    struct Wire {
        name: String,
        bytes: Vec<u8>,
        choice: Variant,
        #[serde(skip_serializing_if = "Option::is_none")]
        absent: Option<bool>,
    }
    #[test]
    fn canonical_fields_arrays_and_atoms_match_literal_json_without_output_graphs() {
        let wire = Wire {
            name: "e\u{301}\"\n".into(),
            bytes: vec![0, 255],
            choice: Variant::Struct {
                value: vec![None, Some(7)],
            },
            absent: None,
        };
        let literal =
            json!({"name":"e\u{301}\"\n","bytes":[0,255],"choice":{"Struct":{"value":[null,7]}}});
        assert!(matches(&wire, &literal).unwrap());
        assert_eq!(serde_json::to_value(&wire).unwrap(), literal);
        for mutation in 0..6 {
            let mut wrong = literal.clone();
            match mutation {
                0 => wrong["extra"] = json!(true),
                1 => {
                    wrong.as_object_mut().unwrap().remove("name");
                }
                2 => wrong["absent"] = Value::Null,
                3 => wrong["bytes"] = json!([255, 0]),
                4 => wrong["choice"]["Struct"]["value"] = json!([7, null]),
                _ => wrong["name"] = json!("\u{e9}\"\n"),
            }
            assert!(!matches(&wire, &wrong).unwrap());
        }
        for value in [
            Variant::Unit,
            Variant::Newtype(i64::MIN),
            Variant::Tuple(true, "kept".into()),
            Variant::Struct { value: vec![] },
        ] {
            let expected = serde_json::to_value(&value).unwrap();
            assert!(matches(&value, &expected).unwrap());
            assert!(!matches(&value, &json!({"unexpected":expected})).unwrap());
        }
        assert!(matches(&(), &Value::Null).unwrap());
        assert!(matches(&'\u{1f642}', &json!("\u{1f642}")).unwrap());
        assert!(!matches(&'x', &json!("xx")).unwrap());
        assert!(matches(&u128::from(u64::MAX), &json!(u64::MAX)).unwrap());
        assert!(matches(&f64::NAN, &Value::Null).unwrap());
        assert!(matches(&-0.0f64, &json!(-0.0f64)).unwrap());
        assert!(!matches(&1.0f64, &json!(1)).unwrap());
        assert!(matches(&u128::MAX, &Value::Null).is_err());
    }

    #[test]
    fn map_keys_use_the_json_authority_and_visits_cancel_even_after_mismatch() {
        let map = BTreeMap::from([(1u32, json!([1, 2])), (2, json!({"name":"kept"}))]);
        let expected = json!({"1":[1,2],"2":{"name":"kept"}});
        assert_eq!(serde_json::to_value(&map).unwrap(), expected);
        assert!(matches(&map, &expected).unwrap());
        let used = Cell::new(0usize);
        let work = |units| {
            assert_eq!(units, 1);
            used.set(used.get() + units);
            Ok(())
        };
        assert!(matches_with_work(&map, &expected, &work).unwrap());
        let visits = used.get();
        assert!(visits > map.len());
        for allowance in 0..=visits {
            used.set(0);
            let limited = |units| {
                if used.get() == allowance {
                    return Err("comparison cancelled".into());
                }
                used.set(used.get() + units);
                Ok(())
            };
            let result = matches_with_work(&map, &expected, &limited);
            assert_eq!(used.get(), allowance);
            if allowance < visits {
                assert_eq!(result.unwrap_err(), "comparison cancelled");
            } else {
                assert!(result.unwrap());
            }
            if allowance < visits {
                used.set(0);
                assert_eq!(
                    matches_with_work(&map, &Value::Null, &limited).unwrap_err(),
                    "comparison cancelled"
                );
            }
        }
        assert_eq!(expected, json!({"1":[1,2],"2":{"name":"kept"}}));
        let invalid = BTreeMap::from([(vec![1u8], true)]);
        assert_eq!(
            matches(&invalid, &json!({})).unwrap_err(),
            serde_json::to_value(&invalid).unwrap_err().to_string()
        );
    }

    #[test]
    fn byte_serialization_and_display_atoms_preserve_json_and_precharge_work() {
        struct Bytes;
        impl Serialize for Bytes {
            fn serialize<S: ser::Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.serialize_bytes(&[0, 1, 255])
            }
        }
        assert!(matches(&Bytes, &json!([0, 1, 255])).unwrap());
        assert!(!matches(&Bytes, &json!([0, 255])).unwrap());
        struct DisplayAtom(Cell<usize>);
        impl std::fmt::Display for DisplayAtom {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.set(self.0.get() + 1);
                formatter.write_str("retained")
            }
        }
        impl Serialize for DisplayAtom {
            fn serialize<S: ser::Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }
        let value = DisplayAtom(Cell::new(0));
        assert_eq!(
            matches_with_work(&value, &json!("retained"), &|_| Err(
                "cancel before formatting".into()
            ))
            .unwrap_err(),
            "cancel before formatting"
        );
        assert_eq!(value.0.get(), 0);
        assert!(matches(&value, &json!("retained")).unwrap());
        assert_eq!(value.0.get(), 1);
    }
}
