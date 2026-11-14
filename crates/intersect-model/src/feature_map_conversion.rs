//! Detached typed feature entries; owner lifecycle and ExtendedMetaData remain E13.
use crate::collection_conversion::Member;
use intersect_model::definitions::{
    bundle::DefinitionBundle,
    conversion::{ConversionContract, ConversionProvider, InvocationContext, MemberType},
    features::{FeatureResolver, FeatureTarget},
    resources::DeclarationId,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureMapKind {
    Entry,
    Map,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub feature: DeclarationId,
    pub value: Value,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Map {
    entries: Vec<Entry>,
}

pub struct FeatureMapConverter {
    contract: ConversionContract,
    kind: FeatureMapKind,
    members: BTreeMap<DeclarationId, Member>,
}
impl FeatureMapConverter {
    pub fn new(
        contract: ConversionContract,
        kind: FeatureMapKind,
        bundle: &DefinitionBundle,
        members: Vec<Member>,
    ) -> Result<Self, String> {
        if contract.serializable {
            return Err("Feature-map adapters require a nonserializable contract".into());
        }
        if contract
            .declarations
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err("Feature scope must be sorted and unique".into());
        }
        if contract.declarations.len() != members.len()
            || contract.members
                != members
                    .iter()
                    .map(|member| member.contract().clone())
                    .collect::<Vec<_>>()
        {
            return Err("Feature scope and member policies do not match implementations".into());
        }
        let resolver = FeatureResolver::new(bundle)?;
        let mut selected = BTreeMap::new();
        for (declaration, member) in contract.declarations.iter().zip(members) {
            let feature = resolver.resolve(declaration)?;
            let agrees = match (&feature.target, &member.contract().value) {
                (FeatureTarget::Builtin(expected), MemberType::Builtin { codec }) => {
                    expected == codec
                }
                (FeatureTarget::Datatype(expected), MemberType::Datatype { declaration }) => {
                    expected == declaration
                }
                (FeatureTarget::Datatype(expected), MemberType::Converted { contract }) => {
                    expected == &contract.datatype
                }
                (FeatureTarget::Enumeration(expected), MemberType::Enumeration { declaration }) => {
                    expected == declaration
                }
                (FeatureTarget::Class(expected), MemberType::Reference { class, .. }) => {
                    expected == class
                }
                (FeatureTarget::Class(_), _) => {
                    return Err("Reference feature entries require a checked object context".into());
                }
                _ => false,
            };
            if !agrees {
                return Err("Feature member policy disagrees with declared feature type".into());
            }
            member.check_context()?;
            selected.insert(declaration.clone(), member);
        }
        Ok(Self {
            contract,
            kind,
            members: selected,
        })
    }
    fn contexts(&self) -> Result<(), String> {
        for member in self.members.values() {
            member.check_context()?;
        }
        Ok(())
    }
    fn check_entry(&self, entry: &Entry) -> Result<(), String> {
        self.check_entry_observed(entry, &InvocationContext::default())
    }
    fn check_entry_observed(
        &self,
        entry: &Entry,
        observation: &InvocationContext,
    ) -> Result<(), String> {
        self.members
            .get(&entry.feature)
            .ok_or("Unselected feature identity or revision")?
            .validate_observed(&entry.value, observation)
    }
    pub fn bind_reference(&self, feature: &DeclarationId, element: &str) -> Result<Value, String> {
        self.contexts()?;
        self.members
            .get(feature)
            .ok_or("Unselected feature identity or revision")?
            .bind_reference(element)
    }
    pub fn resolve_reference(
        &self,
        feature: &DeclarationId,
        value: &Value,
    ) -> Result<&intersect_model::core::model::Element, String> {
        self.contexts()?;
        self.members
            .get(feature)
            .ok_or("Unselected feature identity or revision")?
            .resolve_reference(value)
    }
    pub fn entry(&self, feature: DeclarationId, value: Value) -> Result<Entry, String> {
        self.contexts()?;
        let entry = Entry { feature, value };
        self.check_entry(&entry)?;
        self.contexts()?;
        Ok(entry)
    }
    pub fn read_entry(&self, value: &Value) -> Result<Entry, String> {
        if self.kind != FeatureMapKind::Entry {
            return Err("Entry access requires an entry provider".into());
        }
        self.validate(value)?;
        decode(value)
    }
    fn map(&self, value: &Value) -> Result<Map, String> {
        if self.kind != FeatureMapKind::Map {
            return Err("Operation requires a feature-map provider".into());
        }
        self.validate(value)?;
        decode(value)
    }
    fn publish(&self, value: &mut Value, map: Map) -> Result<(), String> {
        let next = serde_json::to_value(map).map_err(|e| e.to_string())?;
        self.validate(&next)?;
        *value = next;
        Ok(())
    }
    pub fn empty(&self) -> Result<Value, String> {
        if self.kind != FeatureMapKind::Map {
            return Err("A feature entry has no empty value".into());
        }
        let value = json!({"entries":[]});
        self.validate(&value)?;
        Ok(value)
    }
    pub fn values(&self, value: &Value, feature: &DeclarationId) -> Result<Vec<Value>, String> {
        let map = self.map(value)?;
        if !self.members.contains_key(feature) {
            return Err("Unselected feature identity or revision".into());
        }
        Ok(map
            .entries
            .into_iter()
            .filter(|entry| &entry.feature == feature)
            .map(|entry| entry.value)
            .collect())
    }
    pub fn insert(&self, value: &mut Value, index: usize, entry: Entry) -> Result<(), String> {
        let mut map = self.map(value)?;
        if index > map.entries.len() {
            return Err("Feature-map insertion index out of bounds".into());
        }
        self.check_entry(&entry)?;
        map.entries.insert(index, entry);
        self.publish(value, map)
    }
    pub fn set(&self, value: &mut Value, index: usize, entry: Entry) -> Result<Entry, String> {
        let mut map = self.map(value)?;
        self.check_entry(&entry)?;
        let slot = map
            .entries
            .get_mut(index)
            .ok_or("Feature-map index out of bounds")?;
        let old = std::mem::replace(slot, entry);
        self.publish(value, map)?;
        Ok(old)
    }
    pub fn remove(&self, value: &mut Value, index: usize) -> Result<Entry, String> {
        let mut map = self.map(value)?;
        if index >= map.entries.len() {
            return Err("Feature-map index out of bounds".into());
        }
        let old = map.entries.remove(index);
        self.publish(value, map)?;
        Ok(old)
    }
    pub fn move_entry(
        &self,
        value: &mut Value,
        new_position: usize,
        old_position: usize,
    ) -> Result<Entry, String> {
        let mut map = self.map(value)?;
        if new_position >= map.entries.len() || old_position >= map.entries.len() {
            return Err("Feature-map move index out of bounds".into());
        }
        let entry = map.entries.remove(old_position);
        map.entries.insert(new_position, entry.clone());
        self.publish(value, map)?;
        Ok(entry)
    }
    pub fn clear(&self, value: &mut Value) -> Result<(), String> {
        self.map(value)?;
        *value = self.empty()?;
        Ok(())
    }
}
fn decode<T: for<'de> Deserialize<'de> + Serialize>(value: &Value) -> Result<T, String> {
    let parsed: T = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    if serde_json::to_value(&parsed).map_err(|e| e.to_string())? != *value {
        return Err("Feature-map payload must contain exact explicit fields".into());
    }
    Ok(parsed)
}
impl ConversionProvider for FeatureMapConverter {
    fn contract(&self) -> &ConversionContract {
        &self.contract
    }
    fn parse(&self, _: &str) -> Result<Value, String> {
        Err("Feature maps and entries are not lexically serializable".into())
    }
    fn format(&self, _: &Value) -> Result<String, String> {
        Err("Feature maps and entries are not lexically serializable".into())
    }
    fn default_value(&self) -> Result<Option<Value>, String> {
        self.contexts()?;
        Ok(None)
    }
    fn validate(&self, value: &Value) -> Result<(), String> {
        self.validate_observed(value, &InvocationContext::default())
    }
    fn validate_observed(
        &self,
        value: &Value,
        observation: &InvocationContext,
    ) -> Result<(), String> {
        observation.work(1)?;
        self.contexts()?;
        match self.kind {
            FeatureMapKind::Entry => {
                self.check_entry_observed(&decode::<Entry>(value)?, observation)?
            }
            FeatureMapKind::Map => {
                for entry in decode::<Map>(value)?.entries {
                    observation.work(1)?;
                    self.check_entry_observed(&entry, observation)?;
                }
            }
        }
        self.contexts()
    }
}
