//! Private publication stages over the existing checked schema services.
use super::*;
use std::ops::Bound::{Excluded, Unbounded};
type Schema = crate::definitions::schema::RuntimeSourceArraySchemaInputs;
struct TypeProof<'a> {
    pending: Option<
        crate::definitions::schema::RuntimeTypesPreparation<
            'a,
            crate::definitions::schema::SharedRuntimeSourceArrayTarget,
        >,
    >,
    ready: Option<
        crate::definitions::schema::ValidatedRuntimeTypes<
            'a,
            crate::definitions::schema::SharedRuntimeSourceArrayTarget,
        >,
    >,
}
self_cell::self_cell! {
    struct FrozenSchema {
        owner: Schema,
        #[covariant]
        dependent: TypeProof,
    }
}
// Native layout: schema 560 bytes plus the existing 3712-byte validator.
// Keep a portable conservative allowance including the self-cell record.
const FROZEN_BYTES: usize = 8192;
const _: () = assert!(
    std::mem::size_of::<Schema>() + std::mem::size_of::<TypeProof<'static>>() + 128 <= FROZEN_BYTES
);
impl FrozenSchema {
    #[cfg(test)]
    fn prepare(
        schema: Schema,
        root: &Xml,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Box<Self>, ImportError> {
        Self::prepare_with_batches(schema, root, false, observation)
    }
    fn prepare_with_batches(
        schema: Schema,
        root: &Xml,
        batch_duplicates: bool,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Box<Self>, ImportError> {
        observation.reserve(FROZEN_BYTES, &root.opening.source)?;
        Ok(Box::new(Self::new(schema, |schema| TypeProof {
            pending: Some({
                if batch_duplicates {
                    crate::definitions::schema::ValidatedRuntimeTypes::begin_reused_reference_arrays(
                        schema,
                    )
                } else {
                    crate::definitions::schema::ValidatedRuntimeTypes::begin(schema)
                }
            }),
            ready: None,
        })))
    }
    fn advance(
        &mut self,
        root: &Xml,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        self.with_dependent_mut(|_, proof| {
            observation.preparation(&root.opening.source, |operation| {
                if proof.ready.is_some() {
                    return Ok(true);
                }
                let pending = proof.pending.as_mut().ok_or_else(|| {
                    crate::definitions::schema::PreparationError::Schema(
                        "Missing publication type preparation".into(),
                    )
                })?;
                if pending.advance_observed(operation)? {
                    proof.ready = Some(
                        proof
                            .pending
                            .take()
                            .ok_or_else(|| {
                                crate::definitions::schema::PreparationError::Schema(
                                    "Missing complete type preparation".into(),
                                )
                            })?
                            .finish()?,
                    );
                    Ok(true)
                } else {
                    Ok(false)
                }
            })
        })
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Types,
    Defaults,
    Operations,
    Install,
    FinalTypes,
    FinalOperations,
    Complete,
}
pub(super) struct PublicationTail<'source> {
    schema: Option<Box<FrozenSchema>>,
    selections: ReportSelections<'source>,
    root: Xml,
    contracts: BTreeMap<String, crate::definitions::operations::DeclaredOperationSet>,
    batch_duplicates: bool,
    cursor: Option<String>,
    stage: Stage,
    failed: bool,
}
const TAIL_BYTES: usize = 2048;
const _: () = assert!(std::mem::size_of::<PublicationTail<'static>>() <= TAIL_BYTES);
fn next_key<V>(
    map: &BTreeMap<String, V>,
    cursor: Option<&str>,
    root: &Xml,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<Option<String>, ImportError> {
    observation.work(1, &root.opening.source)?;
    let next = match cursor {
        Some(cursor) => map.range::<str, _>((Excluded(cursor), Unbounded)).next(),
        None => map.iter().next(),
    };
    if let Some((key, _)) = next {
        observation.string(key.len(), &root.opening.source)?;
        Ok(Some(key.clone()))
    } else {
        Ok(None)
    }
}
impl<'source> PublicationTail<'source> {
    #[cfg(test)]
    pub(super) fn diagnostic_phase(&self) -> &'static str {
        match self.stage {
            Stage::Types => "types",
            Stage::Defaults => "defaults",
            Stage::Operations => "operations",
            Stage::Install => "install",
            Stage::FinalTypes => "final_types",
            Stage::FinalOperations => "final_operations",
            Stage::Complete => "complete",
        }
    }
    pub(super) fn new(
        publication: ClassedPublication<
            'source,
            crate::definitions::schema::SharedRuntimeSourceArrayTarget,
        >,
        batch_duplicates: bool,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Box<Self>, ImportError> {
        let ClassedPublication {
            mut schema,
            inventory,
            selections,
            root,
        } = publication;
        observation.reserve(TAIL_BYTES, &root.opening.source)?;
        schema.generic_signatures = Some(inventory);
        let schema =
            FrozenSchema::prepare_with_batches(schema, &root, batch_duplicates, observation)?;
        Ok(Box::new(Self {
            schema: Some(schema),
            selections,
            root,
            contracts: BTreeMap::new(),
            batch_duplicates,
            cursor: None,
            stage: Stage::Types,
            failed: false,
        }))
    }
    pub(super) fn advance(
        &mut self,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        if self.failed {
            return Err(self.root.error("Publication tail has failed"));
        }
        let result = self.advance_inner(conversions, observation);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner(
        &mut self,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        observation.work(0, &self.root.opening.source)?;
        match self.stage {
            Stage::Types | Stage::FinalTypes => {
                if self
                    .schema
                    .as_mut()
                    .ok_or_else(|| self.root.error("Missing frozen publication schema"))?
                    .advance(&self.root, observation)?
                {
                    self.stage = if self.stage == Stage::Types {
                        Stage::Defaults
                    } else {
                        Stage::FinalOperations
                    };
                    self.cursor = None;
                }
            }
            Stage::Defaults => {
                let schema = self
                    .schema
                    .as_ref()
                    .ok_or_else(|| self.root.error("Missing publication schema"))?
                    .borrow_owner();
                if let Some(key) = next_key(
                    &schema.runtime_feature_domains,
                    self.cursor.as_deref(),
                    &self.root,
                    observation,
                )? {
                    validate_publication_defaults(schema, &key, &self.root, conversions)?;
                    self.cursor = Some(key);
                } else {
                    self.stage = Stage::Operations;
                    self.cursor = None;
                }
            }
            Stage::Operations => {
                if let Some(key) = next_key(
                    &self.selections,
                    self.cursor.as_deref(),
                    &self.root,
                    observation,
                )? {
                    let (declared, span) = self
                        .selections
                        .get(&key)
                        .ok_or_else(|| self.root.error("Missing declared publication input"))?;
                    let (key, operations) = self
                        .schema
                        .as_ref()
                        .ok_or_else(|| self.root.error("Missing publication schema"))?
                        .with_dependent(|schema, proof| {
                            let proof = proof.ready.as_ref().ok_or_else(|| {
                                self.root.error("Publication types are not validated")
                            })?;
                            prepare_publication_operations(
                                schema,
                                &key,
                                declared,
                                span,
                                proof,
                                &self.root,
                                observation,
                            )
                        })?;
                    // Keep the additional cursor's actual ownership precharged.
                    observation.string(key.len(), &self.root.opening.source)?;
                    self.cursor = Some(key.clone());
                    self.contracts.insert(key, operations);
                } else {
                    self.stage = Stage::Install;
                    self.cursor = None;
                }
            }
            Stage::Install => {
                // Release the immutable proof before modifying the schema.
                let mut schema = (*self
                    .schema
                    .take()
                    .ok_or_else(|| self.root.error("Missing publication schema"))?)
                .into_owner();
                schema.declared_operation_selections = Some(std::mem::take(&mut self.contracts));
                schema.revision = resource_compiler::json_digest(
                    &(&schema.revision, &schema.declared_operation_selections),
                    &self.root.opening.source,
                    observation,
                )
                .map_err(|e| self.root.error(e.message))?;
                self.schema = Some(FrozenSchema::prepare_with_batches(
                    schema,
                    &self.root,
                    self.batch_duplicates,
                    observation,
                )?);
                self.stage = Stage::FinalTypes;
            }
            Stage::FinalOperations => {
                if let Some(key) = next_key(
                    &self.selections,
                    self.cursor.as_deref(),
                    &self.root,
                    observation,
                )? {
                    let (_, span) = self
                        .selections
                        .get(&key)
                        .ok_or_else(|| self.root.error("Missing final publication input"))?;
                    self.schema
                        .as_ref()
                        .ok_or_else(|| self.root.error("Missing final publication schema"))?
                        .with_dependent(|schema, proof| {
                            let proof = proof.ready.as_ref().ok_or_else(|| {
                                self.root.error("Final publication types are not validated")
                            })?;
                            observation.preparation(&span.source, |operation| {
                                schema.validate_declared_operation_inventory_prepared(
                                    &key, proof, operation,
                                )
                            })
                        })
                        .map_err(|e| {
                            ImportError::new(ErrorKind::InvalidDefinition, span, e.message)
                        })?;
                    self.cursor = Some(key);
                } else {
                    self.stage = Stage::Complete;
                }
            }
            Stage::Complete => return Ok(true),
        }
        Ok(self.stage == Stage::Complete)
    }
    pub(super) fn finish(mut self) -> Result<Schema, ImportError> {
        if self.failed || self.stage != Stage::Complete {
            return Err(self.root.error("Publication tail is incomplete or failed"));
        }
        Ok((*self
            .schema
            .take()
            .ok_or_else(|| self.root.error("Missing complete publication schema"))?)
        .into_owner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::{
        bundle::{DefinitionSource, SourceKind},
        conversion::{InvocationContext, InvocationObserver, ProviderId},
        generic_types::{RuntimeType, Type},
    };
    use std::{cell::Cell, rc::Rc};
    struct Meter {
        work: Cell<usize>,
        bytes: Cell<usize>,
        limit: usize,
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("Unexpected provider".into())
        }
        fn work(&self, n: usize) -> Result<(), String> {
            self.work.set(self.work.get() + n);
            Ok(())
        }
        fn retain_storage(&self, n: usize) -> Result<(), String> {
            let next = self.bytes.get() + n;
            if next > self.limit {
                return Err("Frozen schema storage denied".into());
            }
            self.bytes.set(next);
            Ok(())
        }
    }
    #[test]
    fn publication_type_proofs_remain_sealed_reject_foreign_owners_and_restart_after_mutation() {
        let source = DefinitionSource::new(
            "memory:publication-proof.ecore",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:publication-proof'><eClassifiers x:type='e:EClass' name='Root'><eTypeParameters name='T'/></eClassifiers></e:EPackage>",
        );
        let source = crate::definitions::migration_test_fixture::source(source);
        let bundle = crate::definitions::bundle::DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let ty = RuntimeType::new(Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![0],
            },
            arguments: vec![],
            raw: true,
        })
        .unwrap();
        let operation = InvocationContext::default();
        let observation = crate::definitions::ecore::ParserObservation::new(&operation);
        let schema =
            crate::definitions::compiler::materialize_runtime_source_array_inputs_observed(
                &bundle,
                &source.id,
                "urn:publication-proof",
                &[ty],
                None,
                &operation,
            )
            .unwrap();
        let root = xml_root(&source);
        let foreign = schema.clone();
        let mut frozen = FrozenSchema::prepare(schema, &root, &observation).unwrap();
        let mut advances = 0;
        while !frozen.advance(&root, &observation).unwrap() {
            advances += 1;
        }
        assert!(advances > 1);
        frozen.with_dependent(|owner, proof| {
            let ready = proof.ready.as_ref().unwrap();
            assert!(ready.belongs_to(owner));
            assert!(!ready.belongs_to(&foreign));
            assert!(proof.pending.is_none());
        });
        let mut changed = (*frozen).into_owner();
        let duplicate = changed.instantiations.values().next().unwrap().clone();
        changed
            .instantiations
            .insert("duplicate-identity".into(), duplicate);
        let meter = Rc::new(Meter {
            work: Cell::new(0),
            bytes: Cell::new(0),
            limit: usize::MAX,
        });
        let operation = InvocationContext::new(meter.clone());
        let observation = crate::definitions::ecore::ParserObservation::new(&operation);
        let mut fresh = FrozenSchema::prepare(changed, &root, &observation).unwrap();
        fresh.with_dependent(|_, proof| assert!(proof.ready.is_none()));
        loop {
            match fresh.advance(&root, &observation) {
                Ok(false) => {}
                Ok(true) => panic!("Duplicate instantiated identity cannot be validated"),
                Err(_) => break,
            }
        }
        assert!(meter.work.get() > 0);
        assert!(fresh.advance(&root, &observation).is_err());
        fresh.with_dependent(|_, proof| assert!(proof.ready.is_none()));
        let denied = InvocationContext::new(Rc::new(Meter {
            work: Cell::new(0),
            bytes: Cell::new(0),
            limit: FROZEN_BYTES - 1,
        }));
        assert!(
            FrozenSchema::prepare(
                foreign,
                &root,
                &crate::definitions::ecore::ParserObservation::new(&denied)
            )
            .is_err()
        );
    }
    #[test]
    fn staged_operation_contract_checks_match_installed_checks_and_reject_foreign_proof() {
        let source = DefinitionSource::new(
            "memory:staged-operations.ecore",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:staged-operations'><eClassifiers x:type='e:EClass' name='Root'><eOperations name='read' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString'><eParameters name='value' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString'/></eOperations></eClassifiers><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root'/></e:EPackage>",
        );
        let source = crate::definitions::migration_test_fixture::source(source);
        let requested = (0..2)
            .map(|i| {
                RuntimeType::new(Type::Classifier {
                    declaration: DeclarationId {
                        source: source.id.clone(),
                        path: vec![i],
                    },
                    arguments: vec![],
                    raw: false,
                })
                .unwrap()
            })
            .collect::<Vec<_>>();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let operation = InvocationContext::default();
        let schema =
            crate::definitions::compiler::materialize_runtime_source_array_inputs_observed(
                &bundle,
                &source.id,
                "urn:staged-operations",
                &requested,
                None,
                &operation,
            )
            .unwrap();
        let selections = schema.declared_operation_selections.as_ref().unwrap();
        let proof = crate::definitions::schema::ValidatedRuntimeTypes::prepare_observed(
            &schema, &operation,
        )
        .unwrap();
        for variant in 0..6 {
            let mut staged = selections.clone();
            let key = staged.keys().next().unwrap().clone();
            match variant {
                0 => {}
                1 => {
                    staged.remove(&key);
                }
                2 => {
                    let other = requested
                        .iter()
                        .find(|ty| **ty != staged[&key].receiver)
                        .unwrap();
                    staged.get_mut(&key).unwrap().receiver = other.clone();
                }
                3 => {
                    staged.get_mut(&key).unwrap().groups[0]
                        .signature
                        .name
                        .push_str("_forged");
                }
                4 => {
                    staged.get_mut(&key).unwrap().groups[0].operations[0]
                        .operation
                        .result
                        .lower = 99;
                }
                _ => {
                    let group = staged.get(&key).unwrap().groups[0].clone();
                    staged.get_mut(&key).unwrap().groups.push(group);
                }
            }
            let mut installed = schema.clone();
            installed.declared_operation_selections = Some(staged.clone());
            let installed_proof =
                crate::definitions::schema::ValidatedRuntimeTypes::prepare_observed(
                    &installed, &operation,
                )
                .unwrap();
            let mut rejected = false;
            for receiver in schema.classes.keys() {
                let staged_result = schema
                    .validate_selected_operation_inventory_prepared(
                        receiver, &staged, &proof, &operation,
                    )
                    .map_err(|e| e.to_string());
                let installed_result = installed
                    .validate_declared_operation_inventory_prepared(
                        receiver,
                        &installed_proof,
                        &operation,
                    )
                    .map_err(|e| e.to_string());
                assert_eq!(staged_result, installed_result);
                if variant == 0 {
                    assert!(staged_result.is_ok());
                } else {
                    rejected |= staged_result.is_err();
                }
            }
            assert!(variant == 0 || rejected);
            assert!(
                installed
                    .validate_selected_operation_inventory_prepared(
                        &key, &staged, &proof, &operation
                    )
                    .is_err()
            );
        }
    }
    fn batch_fixture(children: usize) -> (Schema, Xml) {
        // Root plus C0..C{children}; migration preserves declaration order.
        // (Counting "class `" in the migrated text broke when eeb500c4 made
        // the Rcore writer spell plain names without backticks.)
        let count = children + 1;
        let children = (0..children)
            .map(|i| format!("<eClassifiers x:type='e:EClass' name='C{i}' eSuperTypes='#//Root'/>"))
            .collect::<String>();
        let source = DefinitionSource::new(
            "memory:batch-proof.ecore",
            SourceKind::Ecore,
            format!(
                "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:batch-proof'><eClassifiers x:type='e:EClass' name='Root'><eTypeParameters name='T'><eBounds eClassifier='#//Root'/></eTypeParameters><eStructuralFeatures x:type='e:EReference' name='r'><eGenericType eTypeParameter='#//Root/T'/></eStructuralFeatures></eClassifiers>{children}</e:EPackage>"
            ),
        );
        let source = crate::definitions::migration_test_fixture::source(source);
        let requested = (0..count)
            .map(|i| {
                RuntimeType::new(Type::Classifier {
                    declaration: DeclarationId {
                        source: source.id.clone(),
                        path: vec![i],
                    },
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
        let operation = InvocationContext::default();
        let schema =
            crate::definitions::compiler::materialize_runtime_source_array_inputs_observed(
                &bundle,
                &source.id,
                "urn:batch-proof",
                &requested,
                None,
                &operation,
            )
            .unwrap();
        (schema, xml_root(&source))
    }
    fn check_batch(schema: Schema, root: &Xml, batch: bool) -> (Result<(), String>, usize) {
        let operation = InvocationContext::default();
        let obs = crate::definitions::ecore::ParserObservation::new(&operation);
        let mut frozen = FrozenSchema::prepare_with_batches(schema, root, batch, &obs).unwrap();
        let mut advances = 0;
        loop {
            advances += 1;
            match frozen.advance(root, &obs) {
                Ok(true) => return (Ok(()), advances),
                Ok(false) => {}
                Err(e) => return (Err(e.message), advances),
            }
        }
    }
    #[test]
    fn batched_index_transitions_keep_full_validation_and_duplicate_error_precedence() {
        let (schema, root) = batch_fixture(32);
        let original = check_batch(schema.clone(), &root, false);
        let batched = check_batch(schema.clone(), &root, true);
        assert_eq!(original.0, batched.0);
        assert!(batched.1 < original.1);
        for variant in 0..4 {
            let mut changed = schema.clone();
            let domain = changed
                .runtime_feature_domains
                .values_mut()
                .next()
                .unwrap()
                .first_mut()
                .unwrap();
            let mut values = domain
                .alternatives
                .iter()
                .map(|v| v.as_ref().to_owned())
                .collect::<Vec<_>>();
            match variant {
                0 => values.reverse(),
                1 => {
                    values.push(values[0].clone());
                }
                2 => {
                    values[0] = "missing-class".into();
                    values.push(values[1].clone());
                }
                _ => {
                    values[0] = "missing-class".into();
                }
            }
            domain.alternatives = serde_json::from_value(serde_json::json!(values)).unwrap();
            let old = check_batch(changed.clone(), &root, false);
            let new = check_batch(changed, &root, true);
            assert_eq!(old.0, new.0);
            if variant == 2 {
                assert!(
                    new.0
                        .unwrap_err()
                        .contains("Duplicate runtime domain alternative")
                );
            }
        }
    }
    #[test]
    fn shared_array_addresses_cannot_grant_validation_to_another_schema_or_wire_replay() {
        let (schema, root) = batch_fixture(4);
        let pointers = |schema: &Schema| {
            schema
                .runtime_feature_domains
                .values()
                .flatten()
                .map(|domain| {
                    (
                        domain.alternatives.as_ptr() as usize,
                        domain.alternatives.len(),
                    )
                })
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert_eq!(pointers(&schema).len(), 1);
        assert!(check_batch(schema.clone(), &root, false).0.is_ok());
        assert!(check_batch(schema.clone(), &root, true).0.is_ok());
        let decoded: Schema =
            serde_json::from_slice(&serde_json::to_vec(&schema).unwrap()).unwrap();
        assert_eq!(
            serde_json::to_value(&decoded).unwrap(),
            serde_json::to_value(&schema).unwrap()
        );
        assert!(pointers(&decoded).is_disjoint(&pointers(&schema)));
        assert!(check_batch(decoded.clone(), &root, false).0.is_ok());
        assert!(check_batch(decoded, &root, true).0.is_ok());
        // Schema cloning shares immutable arrays. Their address and bytes alone
        // cannot certify class membership in a different immutable class world.
        let mut changed = schema.clone();
        let removed = changed.classes.keys().next_back().unwrap().clone();
        changed.classes.remove(&removed);
        assert_eq!(pointers(&changed), pointers(&schema));
        let plain = check_batch(changed.clone(), &root, false).0;
        let batched = check_batch(changed, &root, true).0;
        assert!(plain.is_err());
        assert_eq!(plain, batched);
    }
    #[test]
    fn owned_array_reuse_matches_full_validation_and_error_precedence() {
        let (schema, _) = batch_fixture(16);
        let run = |schema: &Schema, reuse: bool| {
            let observation = InvocationContext::default();
            let mut pending = if reuse {
                crate::definitions::schema::ValidatedRuntimeTypes::begin_reused_reference_arrays(
                    schema,
                )
            } else {
                crate::definitions::schema::ValidatedRuntimeTypes::begin_batched_duplicates(schema)
            };
            let mut advances = 0;
            loop {
                advances += 1;
                match pending.advance_observed(&observation) {
                    Ok(true) => {
                        return (
                            pending
                                .finish()
                                .map(|proof| proof.belongs_to(schema))
                                .map_err(|e| e.to_string()),
                            advances,
                        );
                    }
                    Ok(false) => {}
                    Err(error) => {
                        assert!(pending.advance_observed(&observation).is_err());
                        assert!(pending.finish().is_err());
                        return (Err(error.to_string()), advances);
                    }
                }
            }
        };
        let original = run(&schema, false);
        let reused = run(&schema, true);
        assert_eq!(original.0, reused.0);
        assert!(reused.1 < original.1);
        for variant in 0..5 {
            let mut changed = schema.clone();
            let domain = changed
                .runtime_feature_domains
                .values_mut()
                .last()
                .unwrap()
                .last_mut()
                .unwrap();
            let mut values = domain
                .alternatives
                .iter()
                .map(|value| value.as_ref().to_owned())
                .collect::<Vec<_>>();
            match variant {
                0 => values.reverse(),
                1 => values.push(values[0].clone()),
                2 => {
                    values[0] = "missing-class".into();
                    values.push(values[1].clone());
                }
                3 => values[0] = "missing-class".into(),
                _ => {
                    domain.source = serde_json::from_value({
                        let mut value = serde_json::to_value(&domain.source).unwrap();
                        value["reference"] = serde_json::json!(false);
                        value
                    })
                    .unwrap();
                }
            }
            domain.alternatives = serde_json::from_value(serde_json::json!(values)).unwrap();
            let original = run(&changed, false);
            let reused = run(&changed, true);
            assert_eq!(original.0, reused.0);
            if variant == 2 {
                assert!(
                    reused
                        .0
                        .unwrap_err()
                        .contains("Duplicate runtime domain alternative")
                );
            }
        }
        let decoded: Schema =
            serde_json::from_slice(&serde_json::to_vec(&schema).unwrap()).unwrap();
        assert_eq!(run(&decoded, false).0, run(&decoded, true).0);
    }
    #[test]
    fn owned_array_reuse_cancellation_never_publishes_partial_authority() {
        let (schema, _) = batch_fixture(2);
        let run = |stop| {
            let control = Rc::new(BatchAbort {
                calls: Cell::new(0),
                stop,
            });
            let operation = InvocationContext::new(control.clone());
            let mut pending =
                crate::definitions::schema::ValidatedRuntimeTypes::begin_reused_reference_arrays(
                    &schema,
                );
            loop {
                match pending.advance_observed(&operation) {
                    Ok(false) => {}
                    Ok(true) => {
                        assert!(pending.finish().unwrap().belongs_to(&schema));
                        return (true, control.calls.get());
                    }
                    Err(_) => {
                        assert!(
                            pending
                                .advance_observed(&InvocationContext::default())
                                .is_err()
                        );
                        assert!(pending.finish().is_err());
                        return (false, control.calls.get());
                    }
                }
            }
        };
        let (complete, calls) = run(usize::MAX);
        assert!(complete);
        for stop in 0..calls {
            assert!(!run(stop).0, "cancel callback {stop}");
        }
        assert!(run(usize::MAX).0);
    }
    // Test-only ownership audit. Production validation is unchanged.
    // Full completion of the original validator is the only constructor authority.
    struct OwnedWorldArrayProof<'schema> {
        types: crate::definitions::schema::ValidatedRuntimeTypes<
            'schema,
            crate::definitions::schema::SharedRuntimeSourceArrayTarget,
        >,
        schema: &'schema Schema,
        values: &'schema [crate::definitions::schema::SharedRuntimeSourceArrayTarget],
    }
    impl<'schema> OwnedWorldArrayProof<'schema> {
        fn from_complete(
            types: crate::definitions::schema::ValidatedRuntimeTypes<
                'schema,
                crate::definitions::schema::SharedRuntimeSourceArrayTarget,
            >,
            schema: &'schema Schema,
            values: &'schema [crate::definitions::schema::SharedRuntimeSourceArrayTarget],
        ) -> Result<Self, String> {
            if !types.belongs_to(schema) {
                return Err("Foreign type authority".into());
            }
            if !schema
                .runtime_feature_domains
                .values()
                .flatten()
                .any(|domain| {
                    domain.source.view().reference
                        && std::ptr::eq(domain.alternatives.as_ref(), values)
                })
            {
                return Err("Array is not retained by this schema".into());
            }
            Ok(Self {
                types,
                schema,
                values,
            })
        }
        fn belongs_to(
            &self,
            schema: &Schema,
            values: &[crate::definitions::schema::SharedRuntimeSourceArrayTarget],
        ) -> bool {
            self.types.belongs_to(schema)
                && std::ptr::eq(self.schema, schema)
                && std::ptr::eq(self.values, values)
        }
    }
    #[test]
    fn completed_array_authority_requires_exact_schema_and_retained_array_lifetimes() {
        let (schema, _) = batch_fixture(4);
        let operation = InvocationContext::default();
        let values = schema.runtime_feature_domains.values().next().unwrap()[0]
            .alternatives
            .as_ref();
        let types = crate::definitions::schema::ValidatedRuntimeTypes::prepare_observed(
            &schema, &operation,
        )
        .unwrap();
        let proof = OwnedWorldArrayProof::from_complete(types, &schema, values).unwrap();
        assert!(proof.belongs_to(&schema, values));
        for domain in schema.runtime_feature_domains.values().flatten() {
            assert!(proof.belongs_to(&schema, domain.alternatives.as_ref()));
        }
        let clone = schema.clone();
        let cloned_values = clone.runtime_feature_domains.values().next().unwrap()[0]
            .alternatives
            .as_ref();
        assert!(std::ptr::eq(values, cloned_values));
        assert!(!proof.belongs_to(&clone, cloned_values));
        let decoded: Schema =
            serde_json::from_slice(&serde_json::to_vec(&schema).unwrap()).unwrap();
        let decoded_values = decoded.runtime_feature_domains.values().next().unwrap()[0]
            .alternatives
            .as_ref();
        assert!(!proof.belongs_to(&decoded, decoded_values));
        assert!(!proof.belongs_to(&schema, decoded_values));
        let foreign_types =
            crate::definitions::schema::ValidatedRuntimeTypes::prepare_observed(&clone, &operation)
                .unwrap();
        assert!(OwnedWorldArrayProof::from_complete(foreign_types, &schema, values).is_err());
        let mut malformed = clone;
        let removed = malformed.classes.keys().next_back().unwrap().clone();
        malformed.classes.remove(&removed);
        assert!(
            crate::definitions::schema::ValidatedRuntimeTypes::prepare_observed(
                &malformed, &operation
            )
            .is_err()
        );
        // The exact borrowed schema and array remain alive until proof destruction.
        // Rust forbids mutating or moving either owner while this proof is usable.
        assert!(proof.belongs_to(&schema, values));
    }
    struct BatchAbort {
        calls: Cell<usize>,
        stop: usize,
    }
    impl BatchAbort {
        fn touch(&self) -> Result<(), String> {
            let n = self.calls.get();
            self.calls.set(n + 1);
            if n == self.stop {
                Err("batch cancelled".into())
            } else {
                Ok(())
            }
        }
    }
    impl InvocationObserver for BatchAbort {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("Unexpected provider".into())
        }
        fn work(&self, _: usize) -> Result<(), String> {
            self.touch()
        }
        fn retain_storage(&self, _: usize) -> Result<(), String> {
            self.touch()
        }
    }
    #[test]
    fn batched_validation_cancels_every_callback_without_publishing_partial_proof() {
        let (schema, root) = batch_fixture(2);
        let run = |stop| {
            let meter = Rc::new(BatchAbort {
                calls: Cell::new(0),
                stop,
            });
            let operation = InvocationContext::new(meter.clone());
            let obs = crate::definitions::ecore::ParserObservation::new(&operation);
            let mut frozen =
                match FrozenSchema::prepare_with_batches(schema.clone(), &root, true, &obs) {
                    Ok(value) => value,
                    Err(_) => return (false, meter.calls.get()),
                };
            loop {
                match frozen.advance(&root, &obs) {
                    Ok(true) => return (true, meter.calls.get()),
                    Ok(false) => {}
                    Err(_) => {
                        assert!(
                            frozen
                                .advance(
                                    &root,
                                    &crate::definitions::ecore::ParserObservation::new(
                                        &InvocationContext::default()
                                    )
                                )
                                .is_err()
                        );
                        frozen.with_dependent(|_, proof| assert!(proof.ready.is_none()));
                        return (false, meter.calls.get());
                    }
                }
            }
        };
        let (ok, calls) = run(usize::MAX);
        assert!(ok);
        for stop in 0..calls {
            assert!(!run(stop).0);
        }
        assert!(run(usize::MAX).0);
    }
    fn xml_root(source: &DefinitionSource) -> Xml {
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let resources = ResourceSet::new(&bundle).unwrap();
        let document = resources.document(&source.id).unwrap();
        Xml::from_node(&document.root, &document.source.text).unwrap()
    }
}
