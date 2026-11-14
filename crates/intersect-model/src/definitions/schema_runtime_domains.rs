//! Complete immutable runtime domain metadata. Sharing inputs grants no authority.
use crate::definitions::{
    conversion::InvocationContext,
    generic_resolution::{
        MemberSourceRef, RuntimeDomainRef, RuntimeFeatureDomain, RuntimeProjectionRef,
        SpecializedMember,
    },
    generic_types::{RuntimeType, Type},
    metadata_limits::TraversalBudget,
    resources::DeclarationId,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// Complete source metadata in either transport or shared runtime ownership.
pub trait RuntimeDomainSource:
    Clone + std::fmt::Debug + PartialEq + Serialize + DeserializeOwned
{
    fn view(&self) -> RuntimeDomainRef<'_>;
    fn into_owned_observed(
        self,
        observation: &InvocationContext<'_>,
    ) -> Result<RuntimeFeatureDomain, String>;
}
impl RuntimeDomainSource for RuntimeFeatureDomain {
    fn view(&self) -> RuntimeDomainRef<'_> {
        self.into()
    }
    fn into_owned_observed(self, _: &InvocationContext<'_>) -> Result<Self, String> {
        Ok(self)
    }
}

/// Every native domain field is retained; only immutable input ownership differs.
#[derive(Clone, Debug, PartialEq)]
pub struct SharedRuntimeDomain {
    receiver: RuntimeType,
    source: Arc<SpecializedMember>,
    storage_owner: RuntimeType,
    bindings: Vec<crate::definitions::generic_types::ParameterBinding>,
    reference: bool,
    expression: Arc<Type>,
    erasure: RuntimeType,
}
impl Serialize for SharedRuntimeDomain {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.view().serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for SharedRuntimeDomain {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        RuntimeFeatureDomain::deserialize(deserializer).map(Self::from_owned)
    }
}
impl SharedRuntimeDomain {
    pub(crate) fn from_parts(
        receiver: RuntimeType,
        source: Arc<SpecializedMember>,
        storage_owner: RuntimeType,
        bindings: Vec<crate::definitions::generic_types::ParameterBinding>,
        reference: bool,
        expression: Arc<Type>,
        erasure: RuntimeType,
    ) -> Self {
        Self {
            receiver,
            source,
            storage_owner,
            bindings,
            reference,
            expression,
            erasure,
        }
    }
    pub(crate) fn from_owned(domain: RuntimeFeatureDomain) -> Self {
        Self {
            receiver: domain.projection.receiver,
            source: Arc::new(domain.projection.source),
            storage_owner: domain.projection.storage_owner,
            bindings: domain.projection.bindings,
            reference: domain.reference,
            expression: Arc::new(domain.expression),
            erasure: domain.erasure,
        }
    }
    pub(crate) fn adopt_owned_observed(
        domain: RuntimeFeatureDomain,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, String> {
        observation.retain_storage(128 + 32 + 128 + 32)?;
        Ok(Self::from_owned(domain))
    }
}
impl RuntimeDomainSource for SharedRuntimeDomain {
    fn view(&self) -> RuntimeDomainRef<'_> {
        RuntimeDomainRef {
            projection: RuntimeProjectionRef {
                receiver: &self.receiver,
                source: self.source.as_ref().into(),
                storage_owner: &self.storage_owner,
                bindings: &self.bindings,
            },
            reference: self.reference,
            expression: &self.expression,
            erasure: &self.erasure,
        }
    }
    fn into_owned_observed(
        self,
        observation: &InvocationContext<'_>,
    ) -> Result<RuntimeFeatureDomain, String> {
        // Preflight complete source/type copies before compatibility export. Moves remain moves.
        crate::definitions::retained_storage::reserve_serialized(
            self.source.as_ref(),
            observation,
        )?;
        crate::definitions::retained_storage::reserve_serialized(
            self.expression.as_ref(),
            observation,
        )?;
        Ok(RuntimeFeatureDomain {
            projection: crate::definitions::generic_resolution::RuntimeMemberProjection {
                receiver: self.receiver,
                source: self.source.as_ref().clone(),
                storage_owner: self.storage_owner,
                bindings: self.bindings,
            },
            reference: self.reference,
            expression: self.expression.as_ref().clone(),
            erasure: self.erasure,
        })
    }
}

struct IndexedInput<T> {
    value: Arc<T>,
    comparison_units: usize,
}
fn indexed_input_key(
    value: &impl Serialize,
    observation: &InvocationContext<'_>,
) -> Result<(String, usize), String> {
    // Fresh bounded traversal checks only input shape and measures comparison
    // cost. No source membership, bounds, provider or validity result is stored.
    let mut traversal = TraversalBudget::new(65_536, 64, usize::MAX);
    let bytes = traversal.inspect_with_work(value, &|units| observation.work(units))?;
    let units = traversal
        .visited_nodes()
        .checked_add(bytes.div_ceil(256).max(1))
        .ok_or("Source input comparison work overflow")?;
    observation.retain_storage(256 + 128)?;
    let key = crate::definitions::json_digest::digest_with_work_buffered(value, &|bytes| {
        observation.work(bytes.div_ceil(256).max(1))
    })?;
    Ok((key, units))
}
// Operation-local complete inputs, never membership/conformance/provider results.
#[derive(Default)]
pub(crate) struct RuntimeDomainInputs {
    indexed_inputs: bool,
    indexed_members: BTreeMap<String, Vec<IndexedInput<SpecializedMember>>>,
    indexed_expressions: BTreeMap<String, Vec<IndexedInput<Type>>>,
    indexed_expression_count: usize,
    members: BTreeMap<DeclarationId, Vec<Arc<SpecializedMember>>>,
    expressions: BTreeSet<Arc<Type>>,
    member_count: usize,
    observed_tables: bool,
    ordered_members: Vec<Vec<Arc<SpecializedMember>>>,
    ordered_expressions: Vec<Arc<Type>>,
}
fn comparison_work(
    value: &impl Serialize,
    comparisons: usize,
    observation: &InvocationContext<'_>,
) -> Result<(), String> {
    let mut traversal = TraversalBudget::new(65_536, 64, usize::MAX);
    let bytes = traversal.inspect_with_work(value, &|units| observation.work(units))?;
    let units = traversal
        .visited_nodes()
        .checked_add(bytes.div_ceil(256).max(1))
        .and_then(|units| units.checked_mul(comparisons))
        .ok_or("Source input comparison work overflow")?;
    observation.work(units)
}
impl RuntimeDomainInputs {
    pub(crate) fn with_observed_tables(enabled: bool) -> Self {
        Self {
            observed_tables: enabled,
            ..Self::default()
        }
    }
    pub(crate) fn enable_indexed_inputs(&mut self) -> Result<(), String> {
        if self.member_count != 0
            || self.indexed_expression_count != 0
            || !self.indexed_members.is_empty()
            || !self.indexed_expressions.is_empty()
            || !self.expressions.is_empty()
            || !self.ordered_expressions.is_empty()
        {
            return Err("Source input indexing must be selected before retention".into());
        }
        self.indexed_inputs = true;
        Ok(())
    }
    fn indexed_member(
        &mut self,
        input: MemberSourceRef<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<Arc<SpecializedMember>, String> {
        let (key, units) = indexed_input_key(&input, observation)?;
        self.indexed_member_at_key(input, key, units, observation)
    }
    fn indexed_member_at_key(
        &mut self,
        input: MemberSourceRef<'_>,
        key: String,
        units: usize,
        observation: &InvocationContext<'_>,
    ) -> Result<Arc<SpecializedMember>, String> {
        observation.work(1)?;
        if let Some(rows) = self.indexed_members.get(&key) {
            for row in rows {
                // The complete immutable input carries its comparison footprint.
                // Equality still checks every typed field; a digest is not proof.
                observation.work(
                    units
                        .checked_add(row.comparison_units)
                        .ok_or("Source input comparison work overflow")?,
                )?;
                if input == row.value.as_ref().into() {
                    observation.retain_storage(32)?;
                    return Ok(row.value.clone());
                }
            }
        }
        crate::definitions::retained_storage::reserve_serialized(&input, observation)?;
        observation.retain_storage(128 + 32)?;
        let value = Arc::new(input.owned_clone());
        if self.member_count < 4096 {
            if !self.indexed_members.contains_key(&key) {
                observation.retain_storage(128)?;
            }
            observation.retain_storage(48)?;
            let rows = self.indexed_members.entry(key).or_default();
            rows.try_reserve_exact(1)
                .map_err(|_| "Indexed source input allocation failed")?;
            rows.push(IndexedInput {
                value: value.clone(),
                comparison_units: units,
            });
            self.member_count += 1;
        }
        Ok(value)
    }
    fn indexed_expression(
        &mut self,
        input: &Type,
        observation: &InvocationContext<'_>,
    ) -> Result<Arc<Type>, String> {
        let (key, units) = indexed_input_key(input, observation)?;
        observation.work(1)?;
        if let Some(rows) = self.indexed_expressions.get(&key) {
            for row in rows {
                observation.work(
                    units
                        .checked_add(row.comparison_units)
                        .ok_or("Source input comparison work overflow")?,
                )?;
                if input == row.value.as_ref() {
                    observation.retain_storage(32)?;
                    return Ok(row.value.clone());
                }
            }
        }
        crate::definitions::retained_storage::reserve_serialized(input, observation)?;
        observation.retain_storage(128 + 32)?;
        let value = Arc::new(input.clone());
        if self.indexed_expression_count < 4096 {
            if !self.indexed_expressions.contains_key(&key) {
                observation.retain_storage(128)?;
            }
            observation.retain_storage(48)?;
            let rows = self.indexed_expressions.entry(key).or_default();
            rows.try_reserve_exact(1)
                .map_err(|_| "Indexed expression input allocation failed")?;
            rows.push(IndexedInput {
                value: value.clone(),
                comparison_units: units,
            });
            self.indexed_expression_count += 1;
        }
        Ok(value)
    }
    pub(crate) fn prepare_source_observed(
        &mut self,
        input: RuntimeDomainRef<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<(Arc<SpecializedMember>, Arc<Type>), String> {
        Ok((
            self.member(input.projection.source, observation)?,
            self.expression(input.expression, observation)?,
        ))
    }
    fn member(
        &mut self,
        input: MemberSourceRef<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<Arc<SpecializedMember>, String> {
        if self.indexed_inputs {
            return self.indexed_member(input, observation);
        }
        if self.observed_tables {
            return self.ordered_member(input, observation);
        }
        comparison_work(&input, 1, observation)?;
        comparison_work(input.declaration, 14, observation)?;
        if let Some(candidates) = self.members.get(input.declaration) {
            for candidate in candidates {
                comparison_work(&MemberSourceRef::from(candidate.as_ref()), 1, observation)?;
                if input == candidate.as_ref().into() {
                    observation.retain_storage(32)?;
                    return Ok(candidate.clone());
                }
            }
        }
        crate::definitions::retained_storage::reserve_serialized(&input, observation)?;
        observation.retain_storage(128 + 32)?;
        let member = Arc::new(input.owned_clone());
        if self.member_count < 4096 {
            if !self.members.contains_key(input.declaration) {
                crate::definitions::retained_storage::reserve_serialized(
                    input.declaration,
                    observation,
                )?;
                observation.retain_storage(128)?;
            }
            observation.retain_storage(32)?;
            if let Some(candidates) = self.members.get_mut(input.declaration) {
                candidates.push(member.clone());
            } else {
                self.members
                    .insert(input.declaration.clone(), vec![member.clone()]);
            }
            self.member_count += 1;
        }
        Ok(member)
    }
    fn expression(
        &mut self,
        input: &Type,
        observation: &InvocationContext<'_>,
    ) -> Result<Arc<Type>, String> {
        if self.indexed_inputs {
            return self.indexed_expression(input, observation);
        }
        if self.observed_tables {
            return self.ordered_expression(input, observation);
        }
        comparison_work(input, 14, observation)?;
        if let Some(value) = self.expressions.get(input) {
            observation.retain_storage(32)?;
            return Ok(value.clone());
        }
        crate::definitions::retained_storage::reserve_serialized(input, observation)?;
        observation.retain_storage(128 + 32)?;
        let value = Arc::new(input.clone());
        if self.expressions.len() < 4096 {
            observation.retain_storage(128 + 32)?;
            self.expressions.insert(value.clone());
        }
        Ok(value)
    }
    fn ordered_member(
        &mut self,
        input: MemberSourceRef<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<Arc<SpecializedMember>, String> {
        observation.work(1)?;
        let mut low = 0;
        let mut high = self.ordered_members.len();
        let mut group = None;
        while low < high {
            let middle = low + (high - low) / 2;
            let first = self.ordered_members[middle]
                .first()
                .ok_or("Empty source input group")?;
            let previous = MemberSourceRef::from(first.as_ref()).declaration;
            comparison_work(input.declaration, 1, observation)?;
            comparison_work(previous, 1, observation)?;
            match input.declaration.cmp(previous) {
                std::cmp::Ordering::Less => high = middle,
                std::cmp::Ordering::Greater => low = middle + 1,
                std::cmp::Ordering::Equal => {
                    group = Some(middle);
                    break;
                }
            }
        }
        if let Some(index) = group {
            for candidate in &self.ordered_members[index] {
                comparison_work(&input, 1, observation)?;
                comparison_work(&MemberSourceRef::from(candidate.as_ref()), 1, observation)?;
                if input == candidate.as_ref().into() {
                    observation.retain_storage(32)?;
                    return Ok(candidate.clone());
                }
            }
        }
        crate::definitions::retained_storage::reserve_serialized(&input, observation)?;
        observation.retain_storage(128 + 32)?;
        let member = Arc::new(input.owned_clone());
        if self.member_count < 4096 {
            if let Some(index) = group {
                observation.retain_storage(32)?;
                self.ordered_members[index]
                    .try_reserve_exact(1)
                    .map_err(|_| "Source input group allocation failed")?;
                self.ordered_members[index].push(member.clone());
            } else {
                observation.retain_storage(40 + 32)?;
                observation.work(self.ordered_members.len() - low)?;
                self.ordered_members
                    .try_reserve_exact(1)
                    .map_err(|_| "Source input table allocation failed")?;
                let mut row = Vec::new();
                row.try_reserve_exact(1)
                    .map_err(|_| "Source input group allocation failed")?;
                row.push(member.clone());
                self.ordered_members.insert(low, row);
            }
            self.member_count += 1;
        }
        Ok(member)
    }
    fn ordered_expression(
        &mut self,
        input: &Type,
        observation: &InvocationContext<'_>,
    ) -> Result<Arc<Type>, String> {
        observation.work(1)?;
        let mut low = 0;
        let mut high = self.ordered_expressions.len();
        while low < high {
            let middle = low + (high - low) / 2;
            let previous = self.ordered_expressions[middle].as_ref();
            comparison_work(input, 1, observation)?;
            comparison_work(previous, 1, observation)?;
            match input.cmp(previous) {
                std::cmp::Ordering::Less => high = middle,
                std::cmp::Ordering::Greater => low = middle + 1,
                std::cmp::Ordering::Equal => {
                    observation.retain_storage(32)?;
                    return Ok(self.ordered_expressions[middle].clone());
                }
            }
        }
        crate::definitions::retained_storage::reserve_serialized(input, observation)?;
        observation.retain_storage(128 + 32)?;
        let value = Arc::new(input.clone());
        if self.ordered_expressions.len() < 4096 {
            observation.retain_storage(32)?;
            observation.work(self.ordered_expressions.len() - low)?;
            self.ordered_expressions
                .try_reserve_exact(1)
                .map_err(|_| "Source expression table allocation failed")?;
            self.ordered_expressions.insert(low, value.clone());
        }
        Ok(value)
    }
    pub(crate) fn retain_observed(
        &mut self,
        input: RuntimeDomainRef<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<SharedRuntimeDomain, String> {
        let source = self.member(input.projection.source, observation)?;
        let expression = self.expression(input.expression, observation)?;
        observation.retain_storage(128)?;
        crate::definitions::retained_storage::reserve_serialized(
            input.projection.bindings,
            observation,
        )?;
        observation.retain_storage(32 * 3)?;
        Ok(SharedRuntimeDomain {
            receiver: input.projection.receiver.clone(),
            source,
            storage_owner: input.projection.storage_owner.clone(),
            bindings: input.projection.bindings.to_vec(),
            reference: input.reference,
            expression,
            erasure: input.erasure.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        compiler,
        conversion::{InvocationObserver, ProviderId},
    };
    use std::{cell::Cell, rc::Rc};
    fn fixture() -> RuntimeFeatureDomain {
        let source = DefinitionSource::new(
            "memory:source-inputs.ecore",
            SourceKind::Ecore,
            r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:source-inputs"><eClassifiers x:type="e:EClass" name="Root"><eTypeParameters name="T"><eBounds eClassifier="#//Root"/></eTypeParameters><eStructuralFeatures x:type="e:EReference" name="r"><eGenericType eTypeParameter="#//Root/T"/><!-- retained é😀 --></eStructuralFeatures></eClassifiers></e:EPackage>"##,
        );
        let (source, ids) = crate::definitions::migration_test_fixture::source_with_ids(source);
        let ty = RuntimeType::new(Type::Classifier {
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
        compiler::materialize_runtime_bundle(&bundle, &source.id, "urn:source-inputs", &[ty], None)
            .unwrap()
            .runtime_feature_domains
            .into_values()
            .next()
            .unwrap()
            .remove(0)
            .source
    }
    struct Meter {
        bytes: Cell<usize>,
        work: Cell<usize>,
        max_bytes: usize,
        max_work: usize,
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            panic!("source ownership cannot invoke providers")
        }
        fn work(&self, n: usize) -> Result<(), String> {
            let next = self.work.get() + n;
            if next > self.max_work {
                return Err("source input work denied".into());
            }
            self.work.set(next);
            Ok(())
        }
        fn retain_storage(&self, n: usize) -> Result<(), String> {
            let next = self.bytes.get() + n;
            if next > self.max_bytes {
                return Err("source input storage denied".into());
            }
            self.bytes.set(next);
            Ok(())
        }
    }
    fn run(
        input: &RuntimeFeatureDomain,
        max_bytes: usize,
        max_work: usize,
    ) -> (Result<SharedRuntimeDomain, String>, usize, usize) {
        let meter = Rc::new(Meter {
            bytes: Cell::new(0),
            work: Cell::new(0),
            max_bytes,
            max_work,
        });
        let context = InvocationContext::new(meter.clone());
        let result = RuntimeDomainInputs::default().retain_observed(input.into(), &context);
        (result, meter.bytes.get(), meter.work.get())
    }
    #[test]
    fn indexed_inputs_preserve_all_fields_handle_digest_collisions_and_measure_warm_work() {
        let original = fixture();
        let operation = InvocationContext::default();
        let mut indexed = RuntimeDomainInputs::with_observed_tables(true);
        indexed.enable_indexed_inputs().unwrap();
        let first = indexed
            .retain_observed((&original).into(), &operation)
            .unwrap();
        let repeated = indexed
            .retain_observed((&original).into(), &operation)
            .unwrap();
        assert!(Arc::ptr_eq(&first.source, &repeated.source));
        assert!(Arc::ptr_eq(&first.expression, &repeated.expression));
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&original).unwrap()
        );
        for field in 0..7 {
            let mut changed = original.clone();
            match field {
                0 => changed.projection.source.source_payload.push_str(" é😀"),
                1 => changed.projection.source.span.end += 1,
                2 => changed.projection.source.declaration.source.uri.push('x'),
                3 => changed.projection.source.owner = changed.expression.clone(),
                4 => changed.projection.source.name.push('x'),
                5 => changed.projection.source.signatures.clear(),
                _ => changed.projection.source.formals.push(
                    crate::definitions::generic_resolution::SpecializedFormal {
                        declaration: crate::definitions::generics::TypeParameter {
                            declaration: DeclarationId {
                                source: original.projection.source.declaration.source.clone(),
                                path: vec![123],
                            },
                            span: original.projection.source.span.clone(),
                            source_payload: "complete formal".into(),
                            name: "T".into(),
                            bounds: vec![],
                        },
                        bounds: vec![],
                    },
                ),
            }
            assert_ne!(
                changed.projection.source, original.projection.source,
                "Changed field {field} must be exercised"
            );
            let distinct = indexed
                .retain_observed((&changed).into(), &operation)
                .unwrap();
            assert!(!Arc::ptr_eq(&first.source, &distinct.source));
            assert_eq!(distinct.into_owned_observed(&operation).unwrap(), changed);
        }
        let mut collision = RuntimeDomainInputs::with_observed_tables(true);
        collision.enable_indexed_inputs().unwrap();
        let input = MemberSourceRef::from(&original.projection.source);
        let (_, units) = indexed_input_key(&input, &operation).unwrap();
        let first = collision
            .indexed_member_at_key(input, "collision".into(), units, &operation)
            .unwrap();
        let mut changed = original.projection.source.clone();
        changed.source_payload.push_str(" distinct");
        let (_, other_units) =
            indexed_input_key(&MemberSourceRef::from(&changed), &operation).unwrap();
        let second = collision
            .indexed_member_at_key(
                (&changed).into(),
                "collision".into(),
                other_units,
                &operation,
            )
            .unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(first.as_ref(), &original.projection.source);
        assert_eq!(second.as_ref(), &changed);
        assert!(Arc::ptr_eq(
            &first,
            &collision
                .indexed_member_at_key(input, "collision".into(), units, &operation)
                .unwrap()
        ));
        let measure = |indexed_mode| {
            let mut inputs = RuntimeDomainInputs::with_observed_tables(true);
            if indexed_mode {
                inputs.enable_indexed_inputs().unwrap();
            }
            inputs
                .retain_observed((&original).into(), &operation)
                .unwrap();
            let meter = Rc::new(Meter {
                bytes: Cell::new(0),
                work: Cell::new(0),
                max_bytes: usize::MAX,
                max_work: usize::MAX,
            });
            let context = InvocationContext::new(meter.clone());
            assert_eq!(
                inputs
                    .retain_observed((&original).into(), &context)
                    .unwrap()
                    .into_owned_observed(&operation)
                    .unwrap(),
                original
            );
            meter.work.get()
        };
        let indexed_work = measure(true);
        let original_work = measure(false);
        assert!(
            indexed_work < original_work,
            "indexed={indexed_work}, original={original_work}"
        );
        assert!(indexed.enable_indexed_inputs().is_err());
    }
    #[test]
    fn indexed_input_boundaries_cancel_without_authority_and_capacity_falls_back_completely() {
        let original = fixture();
        let operation = InvocationContext::default();
        let run = |limit, work_limit| {
            let mut inputs = RuntimeDomainInputs::with_observed_tables(true);
            inputs.enable_indexed_inputs().unwrap();
            inputs
                .retain_observed((&original).into(), &operation)
                .unwrap();
            let meter = Rc::new(Meter {
                bytes: Cell::new(0),
                work: Cell::new(0),
                max_bytes: limit,
                max_work: work_limit,
            });
            let context = InvocationContext::new(meter.clone());
            let result = inputs.retain_observed((&original).into(), &context);
            (result, meter.bytes.get(), meter.work.get())
        };
        let (full, bytes, work) = run(usize::MAX, usize::MAX);
        assert_eq!(
            full.unwrap().into_owned_observed(&operation).unwrap(),
            original
        );
        assert!(run(bytes - 1, usize::MAX).0.is_err());
        assert!(run(bytes, usize::MAX).0.is_ok());
        for limit in 0..work {
            assert!(run(usize::MAX, limit).0.is_err());
        }
        assert!(run(usize::MAX, work).0.is_ok());
        let mut inputs = RuntimeDomainInputs::with_observed_tables(true);
        inputs.enable_indexed_inputs().unwrap();
        inputs.member_count = 4096;
        inputs.indexed_expression_count = 4096;
        let first = inputs
            .retain_observed((&original).into(), &operation)
            .unwrap();
        let second = inputs
            .retain_observed((&original).into(), &operation)
            .unwrap();
        assert!(inputs.indexed_members.is_empty() && inputs.indexed_expressions.is_empty());
        assert!(!Arc::ptr_eq(&first.source, &second.source));
        assert_eq!(first.into_owned_observed(&operation).unwrap(), original);
        assert_eq!(second.into_owned_observed(&operation).unwrap(), original);
    }
    #[test]
    fn complete_domain_inputs_share_only_exact_metadata_and_preserve_transport() {
        let input = fixture();
        let context = InvocationContext::default();
        let mut inputs = RuntimeDomainInputs::default();
        let first = inputs.retain_observed((&input).into(), &context).unwrap();
        let repeated = inputs.retain_observed((&input).into(), &context).unwrap();
        assert!(Arc::ptr_eq(&first.source, &repeated.source));
        assert!(Arc::ptr_eq(&first.expression, &repeated.expression));
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&input).unwrap()
        );
        let restored: SharedRuntimeDomain =
            serde_json::from_slice(&serde_json::to_vec(&first).unwrap()).unwrap();
        assert_eq!(restored.into_owned_observed(&context).unwrap(), input);
        for mode in 0..4 {
            let mut changed = input.clone();
            match mode {
                0 => changed.projection.source.source_payload.push(' '),
                1 => changed.projection.source.span.end += 1,
                2 => {
                    changed.projection.source.owner = Type::Classifier {
                        declaration: changed.projection.source.declaration.clone(),
                        arguments: vec![],
                        raw: false,
                    }
                }
                _ => changed.projection.source.name.push('x'),
            }
            let distinct = inputs.retain_observed((&changed).into(), &context).unwrap();
            assert!(!Arc::ptr_eq(&first.source, &distinct.source));
            assert_eq!(distinct.into_owned_observed(&context).unwrap(), changed);
        }
        assert_eq!(first.into_owned_observed(&context).unwrap(), input);
    }
    #[test]
    fn domain_input_allocation_boundaries_and_every_work_interruption_recover() {
        let input = fixture();
        let (baseline, bytes, work) = run(&input, usize::MAX, usize::MAX);
        assert!(baseline.is_ok());
        for limit in [0, bytes - 1] {
            assert_eq!(
                run(&input, limit, usize::MAX).0.unwrap_err(),
                "source input storage denied"
            );
        }
        assert!(run(&input, bytes, usize::MAX).0.is_ok());
        for limit in 0..work {
            assert_eq!(
                run(&input, usize::MAX, limit).0.unwrap_err(),
                "source input work denied"
            );
        }
        assert_eq!(
            run(&input, usize::MAX, usize::MAX)
                .0
                .unwrap()
                .into_owned_observed(&InvocationContext::default())
                .unwrap(),
            input
        );
    }
    #[test]
    fn observed_tables_preserve_complete_variants_and_insertion_order() {
        let original = fixture();
        let context = InvocationContext::default();
        let mut variants = Vec::new();
        for index in 0..5 {
            let mut value = original.clone();
            value
                .projection
                .source
                .source_payload
                .push_str(&format!(" é😀{index}"));
            value.projection.source.declaration.path.push(index);
            variants.push(value);
        }
        // A second complete value for the same declaration is not interchangeable.
        let mut variant = variants[2].clone();
        variant.projection.source.name.push_str("different");
        variants.push(variant);
        for order in [
            vec![0, 1, 2, 3, 4, 5],
            vec![5, 4, 3, 2, 1, 0],
            vec![2, 0, 4, 1, 5, 3],
        ] {
            let mut inputs = RuntimeDomainInputs::with_observed_tables(true);
            let mut first = Vec::new();
            for index in order {
                let value = inputs
                    .retain_observed((&variants[index]).into(), &context)
                    .unwrap();
                assert_eq!(
                    serde_json::to_value(&value).unwrap(),
                    serde_json::to_value(&variants[index]).unwrap()
                );
                first.push((index, value));
            }
            for (index, value) in first {
                let repeated = inputs
                    .retain_observed((&variants[index]).into(), &context)
                    .unwrap();
                assert!(Arc::ptr_eq(&value.source, &repeated.source));
                assert!(Arc::ptr_eq(&value.expression, &repeated.expression));
                assert_eq!(
                    repeated.into_owned_observed(&context).unwrap(),
                    variants[index]
                );
            }
            assert_eq!(inputs.member_count, 6);
            assert_eq!(inputs.ordered_members.len(), 5);
            assert_eq!(inputs.ordered_expressions.len(), 1);
            assert!(inputs.members.is_empty() && inputs.expressions.is_empty());
        }
    }
    #[test]
    fn observed_tables_precharge_cold_hit_variant_and_insertion_boundaries() {
        let original = fixture();
        let context = InvocationContext::default();
        let mut predecessor = original.clone();
        predecessor
            .projection
            .source
            .declaration
            .source
            .uri
            .insert(0, 'a');
        let mut different = original.clone();
        different
            .projection
            .source
            .source_payload
            .push_str(" changed");
        for tested in [&original, &predecessor, &different] {
            let run = |bytes, work| {
                let mut inputs = RuntimeDomainInputs::with_observed_tables(true);
                inputs
                    .retain_observed((&original).into(), &context)
                    .unwrap();
                let meter = Rc::new(Meter {
                    bytes: Cell::new(0),
                    work: Cell::new(0),
                    max_bytes: bytes,
                    max_work: work,
                });
                let observed = InvocationContext::new(meter.clone());
                let result = inputs.retain_observed(tested.into(), &observed);
                (result, meter.bytes.get(), meter.work.get())
            };
            let (result, bytes, work) = run(usize::MAX, usize::MAX);
            assert_eq!(
                result.unwrap().into_owned_observed(&context).unwrap(),
                *tested
            );
            assert_eq!(
                run(bytes - 1, usize::MAX).0.unwrap_err(),
                "source input storage denied"
            );
            assert!(run(bytes, usize::MAX).0.is_ok());
            for limit in 0..work {
                assert_eq!(
                    run(usize::MAX, limit).0.unwrap_err(),
                    "source input work denied"
                );
            }
            assert!(run(usize::MAX, work).0.is_ok());
        }
    }
}
