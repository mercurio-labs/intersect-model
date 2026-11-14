//! Language-owned portable class descriptors. Never loads or reflects host classes.
use intersect_model::definitions::conversion::{
    ConversionContract, ConversionProvider, ProviderId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const PRIMITIVES: [(&str, &str); 9] = [
    ("boolean", "Z"),
    ("byte", "B"),
    ("char", "C"),
    ("double", "D"),
    ("float", "F"),
    ("int", "I"),
    ("long", "J"),
    ("short", "S"),
    ("void", "V"),
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Component {
    Primitive { name: String },
    Named { name: String, identity: ProviderId },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    component: Component,
    dimensions: u8,
}

/// Checked name/version resolution supplied by a language package, not a class loader.
/// The table is immutable after construction; replacing it requires a new converter.
pub struct ClassConverter {
    contract: ConversionContract,
    names: BTreeMap<String, ProviderId>,
}
impl ClassConverter {
    /// Installed portable Java core names. These are descriptors, never host classes.
    pub fn portable_core(contract: ConversionContract) -> Result<Self, String> {
        let names = [
            "Object",
            "String",
            "Class",
            "Boolean",
            "Byte",
            "Character",
            "Double",
            "Float",
            "Integer",
            "Long",
            "Short",
            "Void",
        ]
        .into_iter()
        .map(|name| {
            (
                format!("java.lang.{name}"),
                ProviderId {
                    name: "intersect.portable-java-core".into(),
                    version: "1".into(),
                },
            )
        })
        .collect();
        Self::new(contract, names)
    }
    pub fn new(
        contract: ConversionContract,
        names: BTreeMap<String, ProviderId>,
    ) -> Result<Self, String> {
        for (name, identity) in &names {
            if !valid_name(name) || PRIMITIVES.iter().any(|(primitive, _)| name == primitive) {
                return Err(format!("Invalid registered class name {name:?}"));
            }
            if identity.name.trim().is_empty() || identity.version.trim().is_empty() {
                return Err(
                    "Named class descriptor requires a nonempty identity and version".into(),
                );
            }
        }
        Ok(Self { contract, names })
    }

    fn named(&self, name: &str) -> Result<Component, String> {
        let identity = self
            .names
            .get(name)
            .ok_or_else(|| format!("Unresolved portable class descriptor {name:?}"))?;
        Ok(Component::Named {
            name: name.into(),
            identity: identity.clone(),
        })
    }

    fn check(&self, descriptor: &Descriptor) -> Result<(), String> {
        match &descriptor.component {
            Component::Primitive { name } => {
                if !PRIMITIVES.iter().any(|(primitive, _)| name == primitive) {
                    return Err("Unknown primitive class descriptor".into());
                }
                if name == "void" && descriptor.dimensions != 0 {
                    return Err("Void cannot be an array component".into());
                }
            }
            Component::Named { name, identity } => {
                if self.names.get(name) != Some(identity) {
                    return Err("Unresolved or mismatched named class identity/version".into());
                }
            }
        }
        Ok(())
    }

    fn decode(&self, value: &Value) -> Result<Descriptor, String> {
        let descriptor: Descriptor = serde_json::from_value(value.clone())
            .map_err(|error| format!("Invalid class descriptor: {error}"))?;
        self.check(&descriptor)?;
        Ok(descriptor)
    }
}
fn valid_name(name: &str) -> bool {
    name.split('.').all(|part| {
        !part.is_empty()
            && part.chars().all(|ch| {
                !ch.is_whitespace() && !ch.is_control() && !matches!(ch, '[' | ']' | ';' | '/')
            })
    })
}
impl ConversionProvider for ClassConverter {
    fn contract(&self) -> &ConversionContract {
        &self.contract
    }
    fn parse(&self, text: &str) -> Result<Value, String> {
        let rank = text.bytes().take_while(|byte| *byte == b'[').count();
        let dimensions = u8::try_from(rank).map_err(|_| "Class array rank exceeds 255")?;
        let component = if rank == 0 {
            if PRIMITIVES.iter().any(|(name, _)| *name == text) {
                Component::Primitive { name: text.into() }
            } else {
                self.named(text)?
            }
        } else {
            let suffix = &text[rank..];
            if let Some((name, _)) = PRIMITIVES.iter().find(|(_, code)| *code == suffix) {
                Component::Primitive {
                    name: (*name).into(),
                }
            } else {
                let name = suffix
                    .strip_prefix('L')
                    .and_then(|s| s.strip_suffix(';'))
                    .ok_or("Invalid array class descriptor")?;
                self.named(name)?
            }
        };
        let descriptor = Descriptor {
            component,
            dimensions,
        };
        self.check(&descriptor)?;
        serde_json::to_value(descriptor).map_err(|error| error.to_string())
    }
    fn format(&self, value: &Value) -> Result<String, String> {
        let descriptor = self.decode(value)?;
        let mut result = "[".repeat(usize::from(descriptor.dimensions));
        match descriptor.component {
            Component::Primitive { name } => {
                if descriptor.dimensions == 0 {
                    result.push_str(&name);
                } else {
                    let (_, code) = PRIMITIVES
                        .iter()
                        .find(|(primitive, _)| *primitive == name)
                        .ok_or("Unknown primitive class descriptor")?;
                    result.push_str(code);
                }
            }
            Component::Named { name, .. } => {
                if descriptor.dimensions != 0 {
                    result.push('L');
                }
                result.push_str(&name);
                if descriptor.dimensions != 0 {
                    result.push(';');
                }
            }
        }
        Ok(result)
    }
    fn validate(&self, value: &Value) -> Result<(), String> {
        self.decode(value).map(|_| ())
    }
    fn default_value(&self) -> Result<Option<Value>, String> {
        Ok(None)
    }
}
