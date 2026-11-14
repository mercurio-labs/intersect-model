//! Handwritten collection adapters over exact, explicitly selected member domains.
use intersect_model::definitions::{
    conversion::{
        ConversionContract, ConversionProvider, ConversionRegistry, ConvertedValue,
        InvocationContext, MemberContract, MemberType,
    },
    datatypes::{Datatype, DatatypeValue},
    enums::{EnumValue, Enumeration},
    scalars::Builtin,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub struct Member {
    contract: MemberContract,
    implementation: Implementation,
}
enum Implementation {
    Applied(Box<crate::conversion::ApplicationProvider>),
    Reference(crate::model_reference::ReferenceDomain),
    Builtin(Builtin),
    Datatype(Datatype),
    Enumeration(Enumeration),
    Converted {
        datatype: Datatype,
        provider: Box<dyn ConversionProvider>,
    },
}
impl Member {
    pub fn applied(
        nullable: bool,
        provider: crate::conversion::ApplicationProvider,
    ) -> Result<Self, String> {
        use intersect_model::definitions::conversion::AppliedConversionProvider;
        provider.with_registry(|_| Ok(()))?;
        Ok(Self {
            contract: MemberContract {
                nullable,
                value: MemberType::Applied {
                    contract: Box::new(provider.contract().clone()),
                },
            },
            implementation: Implementation::Applied(Box::new(provider)),
        })
    }
    pub fn reference(nullable: bool, domain: crate::model_reference::ReferenceDomain) -> Self {
        Self {
            contract: MemberContract {
                nullable,
                value: match domain.runtime_target() {
                    Some(target) => MemberType::RuntimeReference {
                        target: target.clone(),
                        context: Box::new(domain.context().clone()),
                    },
                    None => MemberType::Reference {
                        class: domain.class().clone(),
                        context: Box::new(domain.context().clone()),
                    },
                },
            },
            implementation: Implementation::Reference(domain),
        }
    }

    pub fn builtin(nullable: bool, codec: Builtin) -> Self {
        Self {
            contract: MemberContract {
                nullable,
                value: MemberType::Builtin { codec },
            },
            implementation: Implementation::Builtin(codec),
        }
    }
    pub fn datatype(nullable: bool, datatype: Datatype) -> Result<Self, String> {
        datatype.codec()?;
        Ok(Self {
            contract: MemberContract {
                nullable,
                value: MemberType::Datatype {
                    declaration: datatype.declaration().clone(),
                },
            },
            implementation: Implementation::Datatype(datatype),
        })
    }
    pub fn enumeration(nullable: bool, enumeration: Enumeration) -> Self {
        Self {
            contract: MemberContract {
                nullable,
                value: MemberType::Enumeration {
                    declaration: enumeration.declaration().clone(),
                },
            },
            implementation: Implementation::Enumeration(enumeration),
        }
    }
    pub fn converted(
        nullable: bool,
        datatype: Datatype,
        provider: Box<dyn ConversionProvider>,
    ) -> Result<Self, String> {
        if datatype.codec().is_ok() {
            return Err("Converted member cannot override a builtin codec".into());
        }
        let mut registry = ConversionRegistry::default();
        registry.register(&datatype, provider.as_ref())?;
        drop(registry);
        let contract = MemberContract {
            nullable,
            value: MemberType::Converted {
                contract: Box::new(provider.contract().clone()),
            },
        };
        Ok(Self {
            contract,
            implementation: Implementation::Converted { datatype, provider },
        })
    }
    /// Lexical support is checked before installing a serializable composite codec.
    pub(crate) fn require_lexical(&self) -> Result<(), String> {
        use intersect_model::definitions::conversion::AppliedConversionProvider;
        self.check_context()?;
        let supported = match &self.implementation {
            Implementation::Builtin(_) => true,
            Implementation::Datatype(value) => value.serializable(),
            Implementation::Enumeration(value) => value.serializable(),
            Implementation::Converted { provider, .. } => provider.contract().serializable,
            Implementation::Applied(provider) => provider.contract().conversion.serializable,
            Implementation::Reference(_) => false,
        };
        if supported {
            Ok(())
        } else {
            Err("Sequence member has no lexical codec".into())
        }
    }
    pub(crate) fn parse_lexical_observed(
        &self,
        text: &str,
        observation: &InvocationContext,
    ) -> Result<Value, String> {
        use intersect_model::definitions::conversion::AppliedConversionProvider;
        self.require_lexical()?;
        let result = match &self.implementation {
            Implementation::Builtin(codec) => codec.parse(text)?,
            Implementation::Datatype(datatype) => {
                serde_json::to_value(datatype.parse(text)?).map_err(|e| e.to_string())?
            }
            Implementation::Enumeration(enumeration) => {
                serde_json::to_value(enumeration.parse(text)?).map_err(|e| e.to_string())?
            }
            Implementation::Converted { datatype, provider } => {
                let mut registry =
                    ConversionRegistry::default().with_observation(observation.clone());
                registry.register(datatype, provider.as_ref())?;
                serde_json::to_value(registry.parse(datatype.declaration(), text)?)
                    .map_err(|e| e.to_string())?
            }
            Implementation::Applied(provider) => {
                provider.with_registry_observed(observation, |registry| {
                    serde_json::to_value(registry.parse(&provider.contract().application, text)?)
                        .map_err(|e| e.to_string())
                })?
            }
            Implementation::Reference(_) => {
                return Err("Reference members have no lexical codec".into());
            }
        };
        self.validate_observed(&result, observation)?;
        Ok(result)
    }
    pub(crate) fn format_lexical_observed(
        &self,
        value: &Value,
        observation: &InvocationContext,
    ) -> Result<String, String> {
        self.require_lexical()?;
        self.validate_observed(value, observation)?;
        match &self.implementation {
            Implementation::Builtin(codec) => codec
                .format(value)?
                .ok_or_else(|| "Absent scalar has no lexical string".into()),
            Implementation::Datatype(datatype) => datatype.format(&decode(value)?),
            Implementation::Enumeration(enumeration) => {
                enumeration.format(&decode(value)?).map(str::to_owned)
            }
            Implementation::Converted { datatype, provider } => {
                let mut registry =
                    ConversionRegistry::default().with_observation(observation.clone());
                registry.register(datatype, provider.as_ref())?;
                registry.format(&decode(value)?)
            }
            Implementation::Applied(provider) => provider
                .with_registry_observed(observation, |registry| registry.format(&decode(value)?)),
            Implementation::Reference(_) => Err("Reference members have no lexical codec".into()),
        }
    }
    pub fn bind_reference(&self, element: &str) -> Result<Value, String> {
        match &self.implementation {
            Implementation::Reference(domain) => domain.bind(element),
            _ => Err("Member does not select a reference domain".into()),
        }
    }
    pub fn resolve_reference(
        &self,
        value: &Value,
    ) -> Result<&intersect_model::core::model::Element, String> {
        match &self.implementation {
            Implementation::Reference(domain) => domain.resolve(value),
            _ => Err("Member does not select a reference domain".into()),
        }
    }
    pub fn contract(&self) -> &MemberContract {
        &self.contract
    }
    pub(crate) fn check_context(&self) -> Result<(), String> {
        if let Implementation::Applied(provider) = &self.implementation {
            use intersect_model::definitions::conversion::AppliedConversionProvider;
            let MemberType::Applied { contract } = &self.contract.value else {
                return Err("Invalid applied member context".into());
            };
            if provider.contract() != contract.as_ref() {
                return Err("Applied member contract changed".into());
            }
            provider.with_registry(|_| Ok(()))?;
        }
        if let Implementation::Converted { datatype, provider } = &self.implementation {
            let MemberType::Converted { contract } = &self.contract.value else {
                return Err("Invalid converted member context".into());
            };
            if provider.contract() != contract.as_ref() {
                return Err("Collection member provider changed its frozen contract".into());
            }
            let mut registry = ConversionRegistry::default();
            registry.register(datatype, provider.as_ref())?;
        }
        Ok(())
    }
    pub fn validate(&self, value: &Value) -> Result<(), String> {
        self.validate_observed(value, &InvocationContext::default())
    }
    pub fn validate_observed(
        &self,
        value: &Value,
        observation: &InvocationContext,
    ) -> Result<(), String> {
        observation.work(1)?;
        self.check_context()?;
        if value.is_null() {
            return if self.contract.nullable {
                Ok(())
            } else {
                Err("Collection member does not allow null".into())
            };
        }
        match &self.implementation {
            Implementation::Applied(provider) => {
                use intersect_model::definitions::conversion::{
                    AppliedConversionProvider, AppliedConvertedValue,
                };
                let converted = decode::<AppliedConvertedValue>(value)?;
                provider.with_registry_observed(observation, |registry| {
                    registry
                        .validate_for(&provider.contract().application, &converted)
                        .map(|_| ())
                })?;
            }
            Implementation::Reference(domain) => {
                domain.resolve(value)?;
            }
            Implementation::Builtin(codec) => codec.validate(value)?,
            Implementation::Datatype(datatype) => {
                datatype.validate_value(&decode::<DatatypeValue>(value)?)?;
            }
            Implementation::Enumeration(enumeration) => {
                enumeration.validate_value(&decode::<EnumValue>(value)?)?;
            }
            Implementation::Converted { datatype, provider } => {
                let mut registry =
                    ConversionRegistry::default().with_observation(observation.clone());
                registry.register(datatype, provider.as_ref())?;
                registry.validate_for(datatype, &decode::<ConvertedValue>(value)?)?;
            }
        }
        self.check_context()?;
        observation.checkpoint()
    }
}

fn decode<T: for<'de> Deserialize<'de> + Serialize>(value: &Value) -> Result<T, String> {
    let parsed: T = serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
    if serde_json::to_value(&parsed).map_err(|error| error.to_string())? != *value {
        return Err("Collection value is not canonical".into());
    }
    Ok(parsed)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollectionKind {
    List,
    Map,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    items: Vec<Value>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Map {
    entries: Vec<Entry>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    key: Value,
    value: Value,
}

pub struct CollectionConverter {
    contract: ConversionContract,
    kind: CollectionKind,
    members: Vec<Member>,
}
impl CollectionConverter {
    pub fn new(
        contract: ConversionContract,
        kind: CollectionKind,
        members: Vec<Member>,
    ) -> Result<Self, String> {
        let count = match kind {
            CollectionKind::List => 1,
            CollectionKind::Map => 2,
        };
        if contract.serializable || !contract.declarations.is_empty() {
            return Err(
                "Collection adapter requires a nonserializable contract without declaration scope"
                    .into(),
            );
        }
        if members.len() != count
            || contract.members
                != members
                    .iter()
                    .map(|member| member.contract.clone())
                    .collect::<Vec<_>>()
        {
            return Err("Collection member contracts do not match their implementations".into());
        }
        for member in &members {
            member.check_context()?;
        }
        Ok(Self {
            contract,
            kind,
            members,
        })
    }
    fn list(&self, value: &Value) -> Result<List, String> {
        if self.kind != CollectionKind::List {
            return Err("Operation requires a list provider".into());
        }
        self.validate(value)?;
        decode(value)
    }
    fn map(&self, value: &Value) -> Result<Map, String> {
        if self.kind != CollectionKind::Map {
            return Err("Operation requires a map provider".into());
        }
        self.validate(value)?;
        decode(value)
    }
    fn publish<T: Serialize>(&self, destination: &mut Value, updated: T) -> Result<(), String> {
        let value = serde_json::to_value(updated).map_err(|error| error.to_string())?;
        self.validate(&value)?;
        *destination = value;
        Ok(())
    }
    /// Bind against one exact selected member domain; never infer a context digest.
    pub fn bind_reference(&self, member: usize, element: &str) -> Result<Value, String> {
        self.members
            .get(member)
            .ok_or("Collection member index out of bounds")?
            .bind_reference(element)
    }
    pub fn resolve_reference(
        &self,
        member: usize,
        value: &Value,
    ) -> Result<&intersect_model::core::model::Element, String> {
        self.members
            .get(member)
            .ok_or("Collection member index out of bounds")?
            .resolve_reference(value)
    }
    pub fn empty(&self) -> Result<Value, String> {
        let value = match self.kind {
            CollectionKind::List => serde_json::json!({"items":[]}),
            CollectionKind::Map => serde_json::json!({"entries":[]}),
        };
        self.validate(&value)?;
        Ok(value)
    }
    pub fn clear(&self, value: &mut Value) -> Result<(), String> {
        self.validate(value)?;
        *value = self.empty()?;
        Ok(())
    }
    pub fn insert(&self, value: &mut Value, index: usize, item: Value) -> Result<(), String> {
        let mut list = self.list(value)?;
        if index > list.items.len() {
            return Err("List insertion index out of bounds".into());
        }
        self.members[0].validate(&item)?;
        list.items.insert(index, item);
        self.publish(value, list)
    }
    pub fn set(&self, value: &mut Value, index: usize, item: Value) -> Result<Value, String> {
        let mut list = self.list(value)?;
        self.members[0].validate(&item)?;
        let slot = list
            .items
            .get_mut(index)
            .ok_or("List index out of bounds")?;
        let old = std::mem::replace(slot, item);
        self.publish(value, list)?;
        Ok(old)
    }
    pub fn remove_at(&self, value: &mut Value, index: usize) -> Result<Value, String> {
        let mut list = self.list(value)?;
        if index >= list.items.len() {
            return Err("List index out of bounds".into());
        }
        let old = list.items.remove(index);
        self.publish(value, list)?;
        Ok(old)
    }
    /// Both positions refer to the original list; the result occupies new_position.
    pub fn move_item(
        &self,
        value: &mut Value,
        new_position: usize,
        old_position: usize,
    ) -> Result<Value, String> {
        let mut list = self.list(value)?;
        if new_position >= list.items.len() || old_position >= list.items.len() {
            return Err("List move index out of bounds".into());
        }
        let item = list.items.remove(old_position);
        list.items.insert(new_position, item.clone());
        self.publish(value, list)?;
        Ok(item)
    }
    pub fn get(&self, value: &Value, key: &Value) -> Result<Option<Value>, String> {
        let map = self.map(value)?;
        self.members[0].validate(key)?;
        Ok(map
            .entries
            .into_iter()
            .find(|entry| &entry.key == key)
            .map(|entry| entry.value))
    }
    /// Replacing a key keeps its insertion position; new keys append.
    pub fn put(&self, value: &mut Value, key: Value, item: Value) -> Result<Option<Value>, String> {
        let mut map = self.map(value)?;
        self.members[0].validate(&key)?;
        self.members[1].validate(&item)?;
        let old = if let Some(entry) = map.entries.iter_mut().find(|entry| entry.key == key) {
            Some(std::mem::replace(&mut entry.value, item))
        } else {
            map.entries.push(Entry { key, value: item });
            None
        };
        self.publish(value, map)?;
        Ok(old)
    }
    pub fn remove_key(&self, value: &mut Value, key: &Value) -> Result<Option<Value>, String> {
        let mut map = self.map(value)?;
        self.members[0].validate(key)?;
        let old = map
            .entries
            .iter()
            .position(|entry| &entry.key == key)
            .map(|index| map.entries.remove(index).value);
        self.publish(value, map)?;
        Ok(old)
    }
}
impl ConversionProvider for CollectionConverter {
    fn contract(&self) -> &ConversionContract {
        &self.contract
    }
    fn parse(&self, _text: &str) -> Result<Value, String> {
        Err("Collection is not lexically serializable".into())
    }
    fn format(&self, _value: &Value) -> Result<String, String> {
        Err("Collection is not lexically serializable".into())
    }
    fn default_value(&self) -> Result<Option<Value>, String> {
        for member in &self.members {
            member.check_context()?;
        }
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
        for member in &self.members {
            member.check_context()?;
        }
        match self.kind {
            CollectionKind::List => {
                for item in decode::<List>(value)?.items {
                    self.members[0].validate_observed(&item, observation)?;
                }
            }
            CollectionKind::Map => {
                let map = decode::<Map>(value)?;
                for (index, entry) in map.entries.iter().enumerate() {
                    self.members[0].validate_observed(&entry.key, observation)?;
                    self.members[1].validate_observed(&entry.value, observation)?;
                    for previous in &map.entries[..index] {
                        observation.work(1)?;
                        if previous.key == entry.key {
                            return Err("Duplicate map key".into());
                        }
                    }
                }
            }
        }
        for member in &self.members {
            member.check_context()?;
        }
        Ok(())
    }
}
