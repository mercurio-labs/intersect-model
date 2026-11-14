//! Complete immutable schema input representation. This stores data, not proof.
//! Semantic admission must still recheck the original source and providers.
use super::{CheckedRuntimeFeatureDomainRepresentation, Schema, SchemaRepresentation};
#[path = "schema_runtime_domains.rs"]
mod domains;
use crate::definitions::conversion::InvocationContext;
pub(crate) use domains::RuntimeDomainInputs;
pub use domains::{RuntimeDomainSource, SharedRuntimeDomain};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::Arc};

/// Immutable alternative key. Serialization preserves the owned String wire form.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct SharedRuntimeTarget(Arc<str>);
impl AsRef<str> for SharedRuntimeTarget {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
impl std::borrow::Borrow<str> for SharedRuntimeTarget {
    fn borrow(&self) -> &str {
        &self.0
    }
}
impl Serialize for SharedRuntimeTarget {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_ref())
    }
}
impl<'de> Deserialize<'de> for SharedRuntimeTarget {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|value| Self(Arc::from(value)))
    }
}
/// Alternative keys carry immutable serializable data, never semantic outcomes.
mod sealed {
    pub trait Sealed {}
    impl Sealed for String {}
    impl Sealed for super::SharedRuntimeTarget {}
}
pub trait RuntimeAlternative:
    sealed::Sealed + AsRef<str> + Clone + Serialize + Ord + std::fmt::Debug
{
    type Source: RuntimeDomainSource;
    type Alternatives: Clone
        + std::fmt::Debug
        + PartialEq
        + Serialize
        + serde::de::DeserializeOwned
        + std::ops::Deref<Target = [Self]>
        + From<Vec<Self>>
        + IntoIterator<Item = Self>;
    const OBSERVED_SOURCE_TABLES: bool = false;
    const STREAM_ORDERED_ALTERNATIVES: bool = false;
    const COLLECTION_HEADER_BYTES: usize = 0;
    const ITERATION_HANDLE_BYTES: usize = 0;
}
impl RuntimeAlternative for String {
    type Source = crate::definitions::generic_resolution::RuntimeFeatureDomain;
    type Alternatives = Vec<Self>;
}
impl RuntimeAlternative for SharedRuntimeTarget {
    type Source = crate::definitions::generic_resolution::RuntimeFeatureDomain;
    type Alternatives = Vec<Self>;
}
/// Key ownership family for complete shared source-domain metadata.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SharedRuntimeDomainTarget(SharedRuntimeTarget);
impl AsRef<str> for SharedRuntimeDomainTarget {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}
impl std::borrow::Borrow<str> for SharedRuntimeDomainTarget {
    fn borrow(&self) -> &str {
        self.as_ref()
    }
}
impl From<SharedRuntimeTarget> for SharedRuntimeDomainTarget {
    fn from(value: SharedRuntimeTarget) -> Self {
        Self(value)
    }
}
impl sealed::Sealed for SharedRuntimeDomainTarget {}
impl RuntimeAlternative for SharedRuntimeDomainTarget {
    type Source = SharedRuntimeDomain;
    type Alternatives = Vec<Self>;
}

/// Ownership family that shares complete immutable arrays, never admission results.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SharedRuntimeArrayTarget(SharedRuntimeTarget);
impl AsRef<str> for SharedRuntimeArrayTarget {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}
impl std::borrow::Borrow<str> for SharedRuntimeArrayTarget {
    fn borrow(&self) -> &str {
        self.as_ref()
    }
}
impl From<SharedRuntimeTarget> for SharedRuntimeArrayTarget {
    fn from(value: SharedRuntimeTarget) -> Self {
        Self(value)
    }
}
impl sealed::Sealed for SharedRuntimeArrayTarget {}
impl RuntimeAlternative for SharedRuntimeArrayTarget {
    type Source = SharedRuntimeDomain;
    type Alternatives = SharedRuntimeAlternatives;
    const COLLECTION_HEADER_BYTES: usize = 40;
    const ITERATION_HANDLE_BYTES: usize = 32;
}
/// Full wire-compatible alternative array. Deserialization establishes no authority.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SharedRuntimeAlternatives<T = SharedRuntimeArrayTarget>(Arc<Vec<T>>);
impl<T> std::ops::Deref for SharedRuntimeAlternatives<T> {
    type Target = [T];
    fn deref(&self) -> &Self::Target {
        self.0.as_slice()
    }
}
impl<T> From<Vec<T>> for SharedRuntimeAlternatives<T> {
    fn from(value: Vec<T>) -> Self {
        Self(Arc::new(value))
    }
}
impl<T: Serialize> Serialize for SharedRuntimeAlternatives<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for SharedRuntimeAlternatives<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::deserialize(deserializer).map(Self::from)
    }
}
pub struct SharedRuntimeAlternativeIterator<T = SharedRuntimeArrayTarget> {
    input: SharedRuntimeAlternatives<T>,
    index: usize,
}
impl<T: Clone> Iterator for SharedRuntimeAlternativeIterator<T> {
    type Item = T;
    fn next(&mut self) -> Option<Self::Item> {
        let value = self.input.get(self.index)?.clone();
        self.index += 1;
        Some(value)
    }
}
impl<'a, T> IntoIterator for &'a SharedRuntimeAlternatives<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
impl<T: Clone> IntoIterator for SharedRuntimeAlternatives<T> {
    type Item = T;
    type IntoIter = SharedRuntimeAlternativeIterator<T>;
    fn into_iter(self) -> Self::IntoIter {
        SharedRuntimeAlternativeIterator {
            input: self,
            index: 0,
        }
    }
}
pub type RuntimeArraySchemaInputs =
    crate::definitions::schema::SchemaRepresentation<SharedRuntimeArrayTarget>;
impl Schema {
    /// Compatibility transition from owned inputs. Cold construction is separate.
    pub fn into_shared_array_inputs_observed(
        self,
        observation: &InvocationContext<'_>,
    ) -> Result<RuntimeArraySchemaInputs, String> {
        let mut keys = RuntimeTargetInputs::default();
        self.map_runtime_alternatives_observed(
            observation,
            |key, context| {
                keys.intern_observed(&key, context)
                    .map(SharedRuntimeArrayTarget::from)
            },
            SharedRuntimeDomain::adopt_owned_observed,
        )
    }
}
impl RuntimeArraySchemaInputs {
    /// Full owned export precharges each cloned handle and copied target string.
    pub fn into_owned_observed(
        self,
        observation: &InvocationContext<'_>,
    ) -> Result<Schema, String> {
        self.map_runtime_alternatives_observed(
            observation,
            |key, context| {
                let bytes = 64usize
                    .checked_add(key.as_ref().len())
                    .ok_or("Owned target storage overflow")?;
                context.retain_storage(bytes)?;
                Ok(key.as_ref().to_owned())
            },
            RuntimeDomainSource::into_owned_observed,
        )
    }
}

/// Immutable input ownership with fresh ordered scanning; no order certificate is stored.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SharedRuntimeArrayViewTarget(SharedRuntimeTarget);
impl AsRef<str> for SharedRuntimeArrayViewTarget {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}
impl std::borrow::Borrow<str> for SharedRuntimeArrayViewTarget {
    fn borrow(&self) -> &str {
        self.as_ref()
    }
}
impl From<SharedRuntimeTarget> for SharedRuntimeArrayViewTarget {
    fn from(value: SharedRuntimeTarget) -> Self {
        Self(value)
    }
}
impl sealed::Sealed for SharedRuntimeArrayViewTarget {}
impl RuntimeAlternative for SharedRuntimeArrayViewTarget {
    type Source = SharedRuntimeDomain;
    type Alternatives = SharedRuntimeAlternatives<Self>;
    const STREAM_ORDERED_ALTERNATIVES: bool = true;
    const COLLECTION_HEADER_BYTES: usize = 40;
    const ITERATION_HANDLE_BYTES: usize = 32;
}
pub type RuntimeArrayViewSchemaInputs =
    crate::definitions::schema::SchemaRepresentation<SharedRuntimeArrayViewTarget>;
impl Schema {
    pub fn into_shared_array_view_inputs_observed(
        self,
        observation: &InvocationContext<'_>,
    ) -> Result<RuntimeArrayViewSchemaInputs, String> {
        let mut keys = RuntimeTargetInputs::default();
        self.map_runtime_alternatives_observed(
            observation,
            |key, context| {
                keys.intern_observed(&key, context)
                    .map(SharedRuntimeArrayViewTarget::from)
            },
            SharedRuntimeDomain::adopt_owned_observed,
        )
    }
}
impl RuntimeArrayViewSchemaInputs {
    pub fn into_owned_observed(
        self,
        observation: &InvocationContext<'_>,
    ) -> Result<Schema, String> {
        self.map_runtime_alternatives_observed(
            observation,
            |key, context| {
                let bytes = 64usize
                    .checked_add(key.as_ref().len())
                    .ok_or("Owned target storage overflow")?;
                context.retain_storage(bytes)?;
                Ok(key.as_ref().to_owned())
            },
            RuntimeDomainSource::into_owned_observed,
        )
    }
}

/// Complete input ownership with observed source-table probes; never semantic outcomes.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SharedRuntimeSourceArrayTarget(SharedRuntimeTarget);
impl AsRef<str> for SharedRuntimeSourceArrayTarget {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}
impl std::borrow::Borrow<str> for SharedRuntimeSourceArrayTarget {
    fn borrow(&self) -> &str {
        self.as_ref()
    }
}
impl From<SharedRuntimeTarget> for SharedRuntimeSourceArrayTarget {
    fn from(value: SharedRuntimeTarget) -> Self {
        Self(value)
    }
}
impl sealed::Sealed for SharedRuntimeSourceArrayTarget {}
impl RuntimeAlternative for SharedRuntimeSourceArrayTarget {
    type Source = SharedRuntimeDomain;
    type Alternatives = SharedRuntimeAlternatives<Self>;
    const OBSERVED_SOURCE_TABLES: bool = true;
    const STREAM_ORDERED_ALTERNATIVES: bool = true;
    const COLLECTION_HEADER_BYTES: usize = 40;
    const ITERATION_HANDLE_BYTES: usize = 32;
}
pub type RuntimeSourceArraySchemaInputs =
    crate::definitions::schema::SchemaRepresentation<SharedRuntimeSourceArrayTarget>;
impl Schema {
    pub fn into_shared_source_array_inputs_observed(
        self,
        observation: &InvocationContext<'_>,
    ) -> Result<RuntimeSourceArraySchemaInputs, String> {
        let mut keys = RuntimeTargetInputs::default();
        self.map_runtime_alternatives_observed(
            observation,
            |key, context| {
                keys.intern_observed(&key, context)
                    .map(SharedRuntimeSourceArrayTarget::from)
            },
            SharedRuntimeDomain::adopt_owned_observed,
        )
    }
}
impl RuntimeSourceArraySchemaInputs {
    pub fn into_owned_observed(
        self,
        observation: &InvocationContext<'_>,
    ) -> Result<Schema, String> {
        self.map_runtime_alternatives_observed(
            observation,
            |key, context| {
                let bytes = 64usize
                    .checked_add(key.as_ref().len())
                    .ok_or("Owned target storage overflow")?;
                context.retain_storage(bytes)?;
                Ok(key.as_ref().to_owned())
            },
            RuntimeDomainSource::into_owned_observed,
        )
    }
}

// Operation-local immutable keys only. Each caller still performs fresh source,
// membership and provider checks. No accepted/rejected outcome lives here.
#[derive(Default)]
pub(crate) struct RuntimeTargetInputs {
    keys: BTreeSet<SharedRuntimeTarget>,
}
impl RuntimeTargetInputs {
    pub(crate) fn intern_observed(
        &mut self,
        key: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<SharedRuntimeTarget, String> {
        observation.checkpoint()?;
        if let Some(shared) = self.keys.get(key) {
            observation.retain_storage(32)?;
            return Ok(shared.clone());
        }
        // Tree record, immutable string header, retained key and output handle.
        let bytes = 256usize
            .checked_add(key.len())
            .ok_or("Shared target storage overflow")?;
        observation.retain_storage(bytes)?;
        let shared = SharedRuntimeTarget(Arc::from(key));
        self.keys.insert(shared.clone());
        Ok(shared)
    }
}

/// Every Schema field remains present. Only immutable alternative keys share storage.
/// Transport deserialization and conversion do not establish semantic authority.
pub type RuntimeSchemaInputs =
    crate::definitions::schema::SchemaRepresentation<SharedRuntimeTarget>;
/// Complete source-domain and alternative inputs, distinct from the legacy adapter policy.
pub type RuntimeDomainSchemaInputs =
    crate::definitions::schema::SchemaRepresentation<SharedRuntimeDomainTarget>;

impl Schema {
    // Construction-only transition before domains are installed. Never discard
    // a populated table or publish a schema missing its compiled domains.
    pub(crate) fn into_empty_runtime_representation<A: RuntimeAlternative>(
        self,
    ) -> Result<SchemaRepresentation<A>, String> {
        if !self.runtime_feature_domains.is_empty() {
            return Err("Runtime domains are already installed".into());
        }
        self.map_runtime_alternatives_observed(
            &InvocationContext::default(),
            |_, _| Err("Unexpected runtime domain during construction".into()),
            |_, _| Err("Unexpected source domain during construction".into()),
        )
    }
    /// Adopt all existing fields and share exact alternative strings within this input.
    /// This compatibility adapter starts from owned data; cold compiler construction
    /// and runtime consumers must be integrated separately to avoid initial copies.
    pub fn into_shared_runtime_inputs_observed(
        self,
        observation: &InvocationContext<'_>,
    ) -> Result<RuntimeSchemaInputs, String> {
        let mut keys = RuntimeTargetInputs::default();
        self.map_runtime_alternatives_observed(
            observation,
            |key, observation| keys.intern_observed(&key, observation),
            |source, _| Ok(source),
        )
    }
}
impl Schema {
    pub fn into_shared_domain_inputs_observed(
        self,
        observation: &InvocationContext<'_>,
    ) -> Result<RuntimeDomainSchemaInputs, String> {
        let mut keys = RuntimeTargetInputs::default();
        self.map_runtime_alternatives_observed(
            observation,
            |key, observation| {
                keys.intern_observed(&key, observation)
                    .map(SharedRuntimeDomainTarget::from)
            },
            SharedRuntimeDomain::adopt_owned_observed,
        )
    }
}
impl RuntimeDomainSchemaInputs {
    pub fn into_owned_observed(
        self,
        observation: &InvocationContext<'_>,
    ) -> Result<Schema, String> {
        self.map_runtime_alternatives_observed(
            observation,
            |key, observation| {
                let bytes = 64usize
                    .checked_add(key.as_ref().len())
                    .ok_or("Owned target storage overflow")?;
                observation.retain_storage(bytes)?;
                Ok(key.as_ref().to_owned())
            },
            RuntimeDomainSource::into_owned_observed,
        )
    }
}
impl RuntimeSchemaInputs {
    /// Export the complete legacy owned DTO, precharging each copied target string.
    /// All non-alternative fields move unchanged, including source payloads/revisions.
    pub fn into_owned_observed(
        self,
        observation: &InvocationContext<'_>,
    ) -> Result<Schema, String> {
        self.map_runtime_alternatives_observed(
            observation,
            |key, observation| {
                let bytes = 64usize
                    .checked_add(key.as_ref().len())
                    .ok_or("Owned target storage overflow")?;
                observation.retain_storage(bytes)?;
                Ok(key.as_ref().to_owned())
            },
            RuntimeDomainSource::into_owned_observed,
        )
    }
}
impl<A: RuntimeAlternative> crate::definitions::schema::SchemaRepresentation<A> {
    fn map_runtime_alternatives_observed<B: RuntimeAlternative>(
        self,
        observation: &InvocationContext<'_>,
        mut convert: impl FnMut(A, &InvocationContext<'_>) -> Result<B, String>,
        mut convert_source: impl FnMut(A::Source, &InvocationContext<'_>) -> Result<B::Source, String>,
    ) -> Result<crate::definitions::schema::SchemaRepresentation<B>, String> {
        let mut runtime_feature_domains = std::collections::BTreeMap::new();
        for (receiver, domains) in self.runtime_feature_domains {
            observation.work(1)?;
            let bytes = domains
                .len()
                .checked_mul(32)
                .and_then(|n| n.checked_add(256))
                .ok_or("Runtime domain storage overflow")?;
            observation.retain_storage(bytes)?;
            let mut converted = Vec::new();
            converted
                .try_reserve_exact(domains.len())
                .map_err(|_| "Runtime domain allocation failed")?;
            for domain in domains {
                observation.work(1)?;
                let bytes = domain
                    .alternatives
                    .len()
                    .checked_mul(32)
                    .and_then(|n| n.checked_add(256))
                    .ok_or("Runtime alternative storage overflow")?;
                observation.retain_storage(bytes)?;
                let mut alternatives = Vec::new();
                alternatives
                    .try_reserve_exact(domain.alternatives.len())
                    .map_err(|_| "Runtime alternative allocation failed")?;
                let count = domain.alternatives.len();
                let mut targets = domain.alternatives.into_iter();
                for _ in 0..count {
                    observation.work(1)?;
                    if A::ITERATION_HANDLE_BYTES != 0 {
                        observation.retain_storage(A::ITERATION_HANDLE_BYTES)?;
                    }
                    let target = targets.next().ok_or("Missing runtime alternative input")?;
                    alternatives.push(convert(target, observation)?);
                }
                if B::COLLECTION_HEADER_BYTES != 0 {
                    observation.retain_storage(B::COLLECTION_HEADER_BYTES)?;
                }
                converted.push(CheckedRuntimeFeatureDomainRepresentation {
                    source: convert_source(domain.source, observation)?,
                    alternatives: alternatives.into(),
                    unresolved: domain.unresolved,
                    lexical_target: domain.lexical_target,
                    default: domain.default,
                });
            }
            runtime_feature_domains.insert(receiver, converted);
        }
        Ok(SchemaRepresentation {
            runtime_feature_domains,
            reference_top: self.reference_top,
            raw_reference_bounds: self.raw_reference_bounds,
            runtime_warnings: self.runtime_warnings,
            declared_operation_selections: self.declared_operation_selections,
            instantiations: self.instantiations,
            generic_signatures: self.generic_signatures,
            conversion_contracts: self.conversion_contracts,
            applied_conversion_contracts: self.applied_conversion_contracts,
            constructor_contracts: self.constructor_contracts,
            revision: self.revision,
            uri: self.uri,
            name: self.name,
            classes: self.classes,
            enumerations: self.enumerations,
            datatypes: self.datatypes,
            declarations: self.declarations,
        })
    }
}
/// Borrow every domain field through one interface for owned and shared inputs.
/// This interface describes retained data and conveys no source-check certificate.
pub trait RuntimeFeatureDomainRead {
    fn source(&self) -> crate::definitions::generic_resolution::RuntimeDomainRef<'_>;
    fn alternative_count(&self) -> usize;
    fn alternative(&self, index: usize) -> Option<&str>;
    fn unresolved(&self) -> &[super::UnresolvedRuntimeFeatureCandidate];
    fn lexical_target(&self) -> Option<&str>;
    fn default_value(&self) -> &serde_json::Value;
}
impl<A: RuntimeAlternative> RuntimeFeatureDomainRead
    for CheckedRuntimeFeatureDomainRepresentation<A>
{
    fn source(&self) -> crate::definitions::generic_resolution::RuntimeDomainRef<'_> {
        self.source.view()
    }
    fn alternative_count(&self) -> usize {
        self.alternatives.len()
    }
    fn alternative(&self, index: usize) -> Option<&str> {
        self.alternatives.get(index).map(AsRef::as_ref)
    }
    fn unresolved(&self) -> &[super::UnresolvedRuntimeFeatureCandidate] {
        &self.unresolved
    }
    fn lexical_target(&self) -> Option<&str> {
        self.lexical_target.as_deref()
    }
    fn default_value(&self) -> &serde_json::Value {
        &self.default
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        conversion::{InvocationObserver, ProviderId},
        generic_types::{RuntimeType, Type},
    };
    use std::{cell::Cell, rc::Rc};
    struct Meter {
        bytes: Cell<usize>,
        work: Cell<usize>,
        max_bytes: usize,
        max_work: usize,
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            panic!("schema input adapters cannot invoke providers")
        }
        fn work(&self, n: usize) -> Result<(), String> {
            let next = self.work.get() + n;
            if next > self.max_work {
                return Err("input work denied".into());
            }
            self.work.set(next);
            Ok(())
        }
        fn retain_storage(&self, n: usize) -> Result<(), String> {
            let next = self.bytes.get() + n;
            if next > self.max_bytes {
                return Err("input storage denied".into());
            }
            self.bytes.set(next);
            Ok(())
        }
    }
    fn meter(max_bytes: usize, max_work: usize) -> Rc<Meter> {
        Rc::new(Meter {
            bytes: Cell::new(0),
            work: Cell::new(0),
            max_bytes,
            max_work,
        })
    }
    fn fixture() -> Schema {
        let features=(0..2).map(|i|format!("<eStructuralFeatures x:type=\"e:EReference\" name=\"r{i}\"><eGenericType eTypeParameter=\"#//Root/T\"/></eStructuralFeatures>")).collect::<String>();
        let children = (0..3)
            .map(|i| {
                format!("<eClassifiers x:type=\"e:EClass\" name=\"C{i}\" eSuperTypes=\"#//Root\"/>")
            })
            .collect::<String>();
        let source = DefinitionSource::new(
            "memory:shared-schema.ecore",
            SourceKind::Ecore,
            format!(
                "<e:EPackage xmlns:e=\"http://www.eclipse.org/emf/2002/Ecore\" xmlns:x=\"http://www.w3.org/2001/XMLSchema-instance\" name=\"m\" nsURI=\"urn:shared-schema\"><eClassifiers x:type=\"e:EClass\" name=\"Root\"><eTypeParameters name=\"T\"><eBounds eClassifier=\"#//Root\"/></eTypeParameters>{features}</eClassifiers>{children}</e:EPackage>"
            ),
        );
        let (source, ids) = crate::definitions::migration_test_fixture::source_with_ids(source);
        let types = (0..4)
            .map(|i| {
                RuntimeType::new(Type::Classifier {
                    declaration: ids[&vec![i]].clone(),
                    arguments: vec![],
                    raw: i == 0,
                })
                .unwrap()
            })
            .collect::<Vec<_>>();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        crate::definitions::compiler::materialize_runtime_bundle(
            &bundle,
            &source.id,
            "urn:shared-schema",
            &types,
            None,
        )
        .unwrap()
    }
    #[test]
    fn complete_shared_schema_preserves_every_serialized_field_and_owned_export() {
        let owned = fixture();
        let encoded = serde_json::to_vec(&owned).unwrap();
        let shared = owned
            .clone()
            .into_shared_runtime_inputs_observed(&InvocationContext::default())
            .unwrap();
        assert_eq!(serde_json::to_vec(&shared).unwrap(), encoded);
        for (receiver, domains) in &owned.runtime_feature_domains {
            for (a, b) in domains
                .iter()
                .zip(&shared.runtime_feature_domains[receiver])
            {
                let a: &dyn RuntimeFeatureDomainRead = a;
                let b: &dyn RuntimeFeatureDomainRead = b;
                assert_eq!(a.source(), b.source());
                assert_eq!(a.unresolved(), b.unresolved());
                assert_eq!(a.lexical_target(), b.lexical_target());
                assert_eq!(a.default_value(), b.default_value());
                assert_eq!(a.alternative_count(), b.alternative_count());
                for i in 0..=a.alternative_count() {
                    assert_eq!(a.alternative(i), b.alternative(i));
                }
            }
        }

        let keys = shared
            .runtime_feature_domains
            .values()
            .flatten()
            .flat_map(|d| &d.alternatives)
            .collect::<Vec<_>>();
        assert_eq!(keys.len(), 32);
        assert!(keys.iter().all(|key| key.as_ref().len() == 91));
        for a in &keys {
            for b in &keys {
                if a == b {
                    assert!(Arc::ptr_eq(&a.0, &b.0));
                }
            }
        }
        let restored = shared
            .into_owned_observed(&InvocationContext::default())
            .unwrap();
        assert_eq!(restored, owned);
        assert_eq!(serde_json::to_vec(&restored).unwrap(), encoded);
        let transport: RuntimeSchemaInputs = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(
            transport
                .into_owned_observed(&InvocationContext::default())
                .unwrap(),
            owned
        );
    }

    #[test]
    fn shared_schema_semantics_preserve_checks_and_reject_mutated_domain_inputs() {
        let owned = fixture();
        let shared = owned
            .clone()
            .into_shared_runtime_inputs_observed(&InvocationContext::default())
            .unwrap();
        assert_eq!(shared.validate_instantiations(), Ok(()));
        assert_eq!(shared.validate(), owned.validate());
        for (receiver, ty) in &owned.instantiations {
            assert_eq!(shared.class_key_for_type(ty), owned.class_key_for_type(ty));
            assert_eq!(shared.features(receiver), owned.features(receiver));
            assert_eq!(
                shared.effective_operations(receiver),
                owned.effective_operations(receiver)
            );
            for actual in owned.classes.keys() {
                let run = |schema: &dyn Fn(
                    &mut super::super::ReferenceConformancePreparation,
                ) -> Result<bool, String>| {
                    let mut pending =
                        super::super::ReferenceConformancePreparation::new(receiver, "r0", actual);
                    while !schema(&mut pending)? {}
                    pending.finish()
                };
                let a = run(&|p| p.advance(&owned));
                let b = run(&|p| p.advance(&shared));
                assert_eq!(a, b);
                assert_eq!(a, Ok(true));
            }
        }
        let proof = super::super::ValidatedRuntimeTypes::prepare_observed(
            &shared,
            &InvocationContext::default(),
        )
        .unwrap();
        assert!(proof.belongs_to(&shared));
        assert!(!proof.belongs_to(&shared.clone()));
        for mode in [0, 1] {
            let mut a = owned.clone();
            let mut b = shared.clone();
            let da = a
                .runtime_feature_domains
                .values_mut()
                .next()
                .unwrap()
                .first_mut()
                .unwrap();
            let db = b
                .runtime_feature_domains
                .values_mut()
                .next()
                .unwrap()
                .first_mut()
                .unwrap();
            if mode == 0 {
                da.alternatives.push(da.alternatives[0].clone());
                db.alternatives.push(db.alternatives[0].clone());
            } else {
                da.source.projection.source.declaration.path.push(999);
                db.source.projection.source.declaration.path.push(999);
            }
            let error = a.validate_instantiations().unwrap_err();
            assert_eq!(b.validate_instantiations().unwrap_err(), error);
        }
        assert_eq!(shared.validate_instantiations(), Ok(()));
    }

    #[test]
    fn shared_schema_attribute_codecs_and_operation_metadata_use_the_same_semantics() {
        let source = DefinitionSource::new(
            "memory:shared-attribute.ecore",
            SourceKind::Ecore,
            r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:shared-attribute"><eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"><eBounds eClassifier="http://www.eclipse.org/emf/2002/Ecore#//EString"/></eTypeParameters><eStructuralFeatures x:type="e:EAttribute" name="value"><eGenericType eTypeParameter="#//Box/T"/></eStructuralFeatures><eOperations name="echo" eType="http://www.eclipse.org/emf/2002/Ecore#//EString"><eParameters name="input" eType="http://www.eclipse.org/emf/2002/Ecore#//EString"/></eOperations></eClassifiers></e:EPackage>"##,
        );
        let (source, ids) = crate::definitions::migration_test_fixture::source_with_ids(source);
        let target = RuntimeType::new(Type::Classifier {
            declaration: ids[&vec![0]].clone(),
            arguments: vec![],
            raw: true,
        })
        .unwrap();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let owned = crate::definitions::compiler::materialize_runtime_bundle(
            &bundle,
            &source.id,
            "urn:shared-attribute",
            &[target],
            None,
        )
        .unwrap();
        let shared = owned
            .clone()
            .into_shared_runtime_inputs_observed(&InvocationContext::default())
            .unwrap();
        let receiver = owned.classes.keys().next().unwrap();
        let a = owned.prepare_runtime_features(receiver).unwrap();
        let b = shared.prepare_runtime_features(receiver).unwrap();
        assert_eq!(a.fields(), b.fields());
        assert_eq!(
            b.require_attribute_conversion("value", None),
            a.require_attribute_conversion("value", None)
        );
        assert_eq!(b.require_attribute_conversion("value", None), Ok(()));
        for value in [
            serde_json::json!("text"),
            serde_json::json!(true),
            serde_json::json!(7),
            serde_json::Value::Null,
            serde_json::json!(["text"]),
        ] {
            assert_eq!(
                b.validate_attribute("value", &value, None),
                a.validate_attribute("value", &value, None)
            );
        }
        assert_eq!(
            b.validate_attribute("value", &serde_json::json!("text"), None),
            Ok(())
        );
        assert!(
            b.validate_attribute("value", &serde_json::json!(true), None)
                .is_err()
        );
        assert_eq!(owned.effective_operations(receiver).unwrap().len(), 1);
        assert_eq!(
            shared.effective_operations(receiver),
            owned.effective_operations(receiver)
        );
        assert_eq!(
            shared.declared_operation_selections,
            owned.declared_operation_selections
        );
        assert_eq!(shared.validate_instantiations(), Ok(()));
    }
    #[test]
    fn shared_schema_validation_preserves_each_observation_and_recovers_freshly() {
        let owned = fixture();
        let shared = owned
            .clone()
            .into_shared_runtime_inputs_observed(&InvocationContext::default())
            .unwrap();
        fn run<A: RuntimeAlternative>(
            schema: &crate::definitions::schema::SchemaRepresentation<A>,
            limit: usize,
        ) -> (Result<(), String>, usize, usize) {
            let budget = meter(usize::MAX, limit);
            let observation = InvocationContext::new(budget.clone());
            let mut validation = super::super::InstantiationValidation::new();
            let result = (|| {
                while !validation
                    .advance_observed(schema, &observation)
                    .map_err(|e| e.to_string())?
                {}
                Ok(())
            })();
            (result, budget.bytes.get(), budget.work.get())
        }
        let baseline = run(&owned, usize::MAX);
        assert_eq!(baseline.0, Ok(()));
        assert_eq!(run(&shared, usize::MAX), baseline);
        for limit in 0..baseline.2 {
            let a = run(&owned, limit);
            let b = run(&shared, limit);
            assert_eq!(a, b);
            assert_eq!(a.0, Err("input work denied".into()));
        }
        assert_eq!(run(&shared, usize::MAX), baseline);
    }
    #[test]
    fn complete_schema_input_adapters_keep_literal_storage_limits_and_interruptions() {
        let owned = fixture();
        // Four receiver rows: 4*(256+2*32); eight domain rows: 8*(256+4*32).
        // Four unique 91-byte class keys (__type_ plus identity): 4*(256+91); 28 shared handles: 28*32.
        // Owned export retains all 32 strings: 32*(64+91), plus the same rows.
        for (shared_cost, owned_cost) in [(6636, 9312)] {
            for limit in [0, shared_cost - 1, shared_cost] {
                let budget = meter(limit, usize::MAX);
                let result = owned
                    .clone()
                    .into_shared_runtime_inputs_observed(&InvocationContext::new(budget.clone()));
                if limit < shared_cost {
                    assert_eq!(result.unwrap_err(), "input storage denied");
                } else {
                    assert!(result.is_ok());
                    assert_eq!(budget.bytes.get(), shared_cost);
                }
            }
            let shared = owned
                .clone()
                .into_shared_runtime_inputs_observed(&InvocationContext::default())
                .unwrap();
            for limit in [0, owned_cost - 1, owned_cost] {
                let budget = meter(limit, usize::MAX);
                let result = shared
                    .clone()
                    .into_owned_observed(&InvocationContext::new(budget.clone()));
                if limit < owned_cost {
                    assert_eq!(result.unwrap_err(), "input storage denied");
                } else {
                    assert_eq!(result.unwrap(), owned);
                    assert_eq!(budget.bytes.get(), owned_cost);
                }
            }
            // Four receivers + eight domains + 32 alternatives = 44 visits.
            for limit in 0..44 {
                let budget = meter(usize::MAX, limit);
                assert_eq!(
                    owned
                        .clone()
                        .into_shared_runtime_inputs_observed(&InvocationContext::new(
                            budget.clone()
                        ))
                        .unwrap_err(),
                    "input work denied"
                );
                assert_eq!(budget.work.get(), limit);
                let budget = meter(usize::MAX, limit);
                assert_eq!(
                    shared
                        .clone()
                        .into_owned_observed(&InvocationContext::new(budget.clone()))
                        .unwrap_err(),
                    "input work denied"
                );
                assert_eq!(budget.work.get(), limit);
            }
            let budget = meter(owned_cost, 44);
            assert_eq!(
                shared
                    .into_owned_observed(&InvocationContext::new(budget.clone()))
                    .unwrap(),
                owned
            );
            assert_eq!(budget.work.get(), 44);
        }
    }
    #[test]
    fn array_compatibility_export_precharges_complete_copies_and_recovers_at_exact_limits() {
        let owned = fixture();
        let encoded = serde_json::to_vec(&owned).unwrap();
        let arrays = owned
            .clone()
            .into_shared_array_inputs_observed(&InvocationContext::default())
            .unwrap();
        let probe = meter(usize::MAX, usize::MAX);
        assert_eq!(
            arrays
                .clone()
                .into_owned_observed(&InvocationContext::new(probe.clone()))
                .unwrap(),
            owned
        );
        let needed = probe.bytes.get();
        assert!(needed > 0);
        for limit in [0, needed - 1, needed] {
            let observation = meter(limit, usize::MAX);
            let result = arrays
                .clone()
                .into_owned_observed(&InvocationContext::new(observation.clone()));
            if limit < needed {
                assert_eq!(result.unwrap_err(), "input storage denied");
            } else {
                assert_eq!(observation.bytes.get(), needed);
                assert_eq!(serde_json::to_vec(&result.unwrap()).unwrap(), encoded);
            }
        }
        let denied = meter(usize::MAX, 0);
        assert_eq!(
            arrays
                .clone()
                .into_owned_observed(&InvocationContext::new(denied))
                .unwrap_err(),
            "input work denied"
        );
        assert_eq!(
            serde_json::to_vec(
                &arrays
                    .into_owned_observed(&InvocationContext::default())
                    .unwrap()
            )
            .unwrap(),
            encoded
        );
    }
}
