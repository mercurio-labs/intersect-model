//! Receiver-independent feature signature code in one immutable native source world.
//! No receiver membership, application-bound or provider outcome is retained.
use super::*;
#[path = "generic_signature_preparation.rs"]
mod signature_preparation;
pub(super) use signature_preparation::SignaturePreparation;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
#[derive(Clone, Debug, Default)]
pub(crate) struct CompiledFeatureSignatureInputs {
    signatures: BTreeMap<DeclarationId, Rc<Vec<SpecializedSignature>>>,
    nominal_inputs: Vec<NominalSourceInput>,
    nominal_input_count: usize,
}
#[derive(Clone, Debug)]
struct NominalSourceInput {
    source: crate::definitions::bundle::SourceId,
    declarations: Vec<(Vec<usize>, CompiledNominalInputs)>,
}
#[derive(Clone, Debug, Default)]
struct CompiledNominalInputs {
    closed: Option<Rc<Type>>,
    raw: Option<Rc<Type>>,
}
impl Resolver {
    pub(crate) fn compiled_feature_inputs_selected(
        &self,
        span: &SourceSpan,
    ) -> Result<bool, ImportError> {
        self.prepared_feature_signatures
            .try_borrow()
            .map(|inputs| inputs.is_some())
            .map_err(|_| error(span, "Feature signature inputs are already borrowed"))
    }
    // Exact argument-free descriptor data only. A hit grants no source,
    // classifier, bound, provider or application validity; consumers check those.
    pub(crate) fn compiled_nominal_type_input(
        &self,
        declaration: &DeclarationId,
        raw: bool,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Option<Rc<Type>>, ImportError> {
        let prepared = self
            .prepared_feature_signatures
            .try_borrow()
            .map_err(|_| error(span, "Feature signature inputs are already borrowed"))?
            .clone();
        let Some(prepared) = prepared else {
            return Ok(None);
        };
        // Compare the exact source identity once, then only declaration paths.
        // This shares descriptor data, never a classifier or conformance result.
        let (source_position, source_missing, position, missing, generation) = {
            let table = prepared
                .try_borrow()
                .map_err(|_| error(span, "Nominal descriptor inputs are already borrowed"))?;
            let mut low = 0;
            let mut high = table.nominal_inputs.len();
            let mut source_found = None;
            while low < high {
                let middle = low + (high - low) / 2;
                let id = &table.nominal_inputs[middle].source;
                observation.work(
                    1 + declaration.source.uri.len().div_ceil(256)
                        + id.uri.len().div_ceil(256)
                        + declaration.source.sha256.len().div_ceil(256)
                        + id.sha256.len().div_ceil(256),
                    &span.source,
                )?;
                match declaration.source.cmp(id) {
                    std::cmp::Ordering::Less => high = middle,
                    std::cmp::Ordering::Greater => low = middle + 1,
                    std::cmp::Ordering::Equal => {
                        source_found = Some(middle);
                        break;
                    }
                }
            }
            let source_position = source_found.unwrap_or(low);
            let mut low = 0;
            let mut high =
                source_found.map_or(0, |source| table.nominal_inputs[source].declarations.len());
            let mut found = None;
            while low < high {
                let middle = low + (high - low) / 2;
                let path = &table.nominal_inputs[source_position].declarations[middle].0;
                observation.work(1 + declaration.path.len() + path.len(), &span.source)?;
                match declaration.path.cmp(path) {
                    std::cmp::Ordering::Less => high = middle,
                    std::cmp::Ordering::Greater => low = middle + 1,
                    std::cmp::Ordering::Equal => {
                        found = Some(middle);
                        break;
                    }
                }
            }
            if let Some(position) = found {
                let inputs = &table.nominal_inputs[source_position].declarations[position].1;
                let input = if raw { &inputs.raw } else { &inputs.closed };
                if let Some(input) = input {
                    observation.reserve(32, &span.source)?;
                    return Ok(Some(Rc::clone(input)));
                }
            }
            if table.nominal_input_count >= 4096
                || (source_found.is_none() && table.nominal_inputs.len() >= 4096)
            {
                return Ok(None);
            }
            (
                source_position,
                source_found.is_none(),
                found.unwrap_or(low),
                found.is_none(),
                table.nominal_input_count,
            )
        };
        if source_missing {
            observation.serialized(&declaration.source, &span.source)?;
            observation.reserve(128, &span.source)?;
        }
        if missing {
            observation.serialized(&declaration.path, &span.source)?;
            observation.reserve(128, &span.source)?;
        }
        observation.serialized(declaration, &span.source)?;
        observation.reserve(128 + 32, &span.source)?;
        let expression = Rc::new(Type::Classifier {
            declaration: declaration.clone(),
            arguments: vec![],
            raw,
        });
        let mut table = prepared
            .try_borrow_mut()
            .map_err(|_| error(span, "Nominal descriptor inputs are already borrowed"))?;
        if table.nominal_input_count != generation {
            return Err(error(
                span,
                "Nominal descriptor inputs changed during preparation",
            ));
        }
        // Preflight both ordered movements before retaining a new source record.
        // Cancellation cannot accumulate empty source/path records in this cache.
        if source_missing {
            observation.work(table.nominal_inputs.len() - source_position, &span.source)?;
        }
        if missing {
            let length = if source_missing {
                0
            } else {
                table.nominal_inputs[source_position].declarations.len()
            };
            observation.work(length - position, &span.source)?;
        }
        if source_missing {
            let mut declarations = Vec::new();
            declarations
                .try_reserve_exact(1)
                .map_err(|_| error(span, "Nominal descriptor input allocation failed"))?;
            declarations.push((declaration.path.clone(), CompiledNominalInputs::default()));
            table
                .nominal_inputs
                .try_reserve_exact(1)
                .map_err(|_| error(span, "Nominal source input allocation failed"))?;
            table.nominal_inputs.insert(
                source_position,
                NominalSourceInput {
                    source: declaration.source.clone(),
                    declarations,
                },
            );
        } else if missing {
            let declarations = &mut table.nominal_inputs[source_position].declarations;
            declarations
                .try_reserve_exact(1)
                .map_err(|_| error(span, "Nominal descriptor input allocation failed"))?;
            declarations.insert(
                position,
                (declaration.path.clone(), CompiledNominalInputs::default()),
            );
        }
        let declarations = &mut table
            .nominal_inputs
            .get_mut(source_position)
            .ok_or_else(|| error(span, "Missing nominal source input record"))?
            .declarations;
        let inputs = &mut declarations
            .get_mut(position)
            .ok_or_else(|| error(span, "Missing nominal descriptor input record"))?
            .1;
        let input = if raw {
            &mut inputs.raw
        } else {
            &mut inputs.closed
        };
        *input = Some(Rc::clone(&expression));
        table.nominal_input_count += 1;
        Ok(Some(expression))
    }
    pub(crate) fn enable_compiled_feature_signatures_observed(
        &self,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        observation.work(1, &self.standard_id)?;
        if self
            .prepared_feature_signatures
            .try_borrow()
            .map_err(|_| {
                error(
                    &fallback(&DeclarationId {
                        source: self.standard_id.clone(),
                        path: vec![],
                    }),
                    "Feature signature inputs are already borrowed",
                )
            })?
            .is_some()
        {
            return Ok(());
        }
        observation.reserve(128 + 32, &self.standard_id)?;
        *self
            .prepared_feature_signatures
            .try_borrow_mut()
            .map_err(|_| {
                error(
                    &fallback(&DeclarationId {
                        source: self.standard_id.clone(),
                        path: vec![],
                    }),
                    "Feature signature inputs are already borrowed",
                )
            })? = Some(Rc::new(RefCell::new(
            CompiledFeatureSignatureInputs::default(),
        )));
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn compiled_feature_signature_inputs(
        &self,
        owner: &DeclarationId,
        environment: &Substitution,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<ErasedSignatures, ImportError> {
        let mut preparation = SignaturePreparation::begin(self, true);
        while !preparation.advance(owner, environment, observation)? {}
        preparation.finish(owner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        conversion::{InvocationContext, InvocationObserver, ProviderId},
        ecore::ParserObservation,
    };
    use std::cell::Cell;
    pub(super) fn fixture() -> (DefinitionBundle, Vec<RuntimeType>) {
        let source = DefinitionSource::new(
            "memory:compiled-fields.ecore",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:compiled-fields'><eClassifiers x:type='e:EClass' name='Root'><eTypeParameters name='T'><eBounds eClassifier='#//A'/></eTypeParameters><eStructuralFeatures x:type='e:EReference' name='r'><eAnnotations source='urn:note'><details key='note' value='Ω'/></eAnnotations><eGenericType eTypeParameter='#//Root/T'/></eStructuralFeatures><eStructuralFeatures x:type='e:EReference' name='q'><eGenericType eTypeParameter='#//Root/T'/></eStructuralFeatures></eClassifiers><eClassifiers x:type='e:EClass' name='A'/><eClassifiers x:type='e:EClass' name='B'/><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root'/></e:EPackage>",
        );
        let types = (0..4)
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
            .collect();
        (
            DefinitionBundle {
                version: 1,
                sources: vec![source],
                namespaces: vec![],
            },
            types,
        )
    }
    pub(super) fn prepared(bundle: &DefinitionBundle, enabled: bool) -> Resolver {
        let operation = InvocationContext::default();
        let obs = ParserObservation::new(&operation);
        let mut resolver = Resolver::new(bundle).unwrap().with_borrowed_owner_inputs();
        resolver
            .resolve_materialization_inventory_observed(&obs)
            .unwrap();
        resolver.enable_parameter_bound_inputs(&obs).unwrap();
        if enabled {
            resolver
                .enable_compiled_feature_signatures_observed(&obs)
                .unwrap();
        }
        resolver
    }
    pub(super) fn cache(resolver: &Resolver) -> Rc<RefCell<CompiledFeatureSignatureInputs>> {
        resolver
            .prepared_feature_signatures
            .borrow()
            .as_ref()
            .unwrap()
            .clone()
    }
    #[test]
    fn borrowed_superclass_checks_keep_exact_identity_and_duplicate_diagnostics() {
        for duplicate in [false, true] {
            let other = if duplicate { "p/T" } else { "q/T" };
            let text = format!(
                "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:parent-inputs'><eSubpackages name='p' nsURI='urn:p'><eClassifiers x:type='e:EClass' name='T'/></eSubpackages><eSubpackages name='q' nsURI='urn:q'><eClassifiers x:type='e:EClass' name='T'/></eSubpackages><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//p/T #//{other}'/></e:EPackage>"
            );
            let source =
                DefinitionSource::new("memory:parent-inputs.ecore", SourceKind::Ecore, text);
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source.clone()],
                namespaces: vec![],
            };
            let ty = Type::Classifier {
                declaration: DeclarationId {
                    source: source.id.clone(),
                    path: vec![2],
                },
                arguments: vec![],
                raw: false,
            };
            let span = SourceSpan {
                source: source.id.clone(),
                start: 0,
                end: source.text.len(),
            };
            let operation = InvocationContext::default();
            let obs = ParserObservation::new(&operation);
            let old = Resolver::new(&bundle).unwrap();
            let current = Resolver::new(&bundle).unwrap();
            current
                .enable_compiled_feature_signatures_observed(&obs)
                .unwrap();
            let ordinary = old.direct_parent_types_observed(&ty, &span, &obs);
            let actual = current.direct_parent_types_observed(&ty, &span, &obs);
            assert_eq!(format!("{ordinary:?}"), format!("{actual:?}"));
            if duplicate {
                assert!(
                    actual
                        .unwrap_err()
                        .message
                        .contains("Duplicate direct generic superclass")
                );
            } else {
                let parents = actual.unwrap();
                assert_eq!(parents.len(), 2);
                assert_ne!(parents[0].0, parents[1].0);
            }
        }
    }
    #[test]
    fn compiled_source_code_shares_across_receivers_without_sharing_domains_or_public_copies() {
        let (bundle, types) = fixture();
        let old = prepared(&bundle, false);
        let current = prepared(&bundle, true);
        let operation = InvocationContext::default();
        let obs = ParserObservation::new(&operation);
        let first = current
            .prepare_runtime_members_observed(&types[0], &obs)
            .unwrap();
        let second = current
            .prepare_runtime_members_observed(&types[3], &obs)
            .unwrap();
        assert_eq!(cache(&current).borrow().signatures.len(), 2);
        for i in 0..2 {
            let a = first.checked_feature_domain(i).unwrap();
            let b = second.checked_feature_domain(i).unwrap();
            assert!(std::ptr::eq(
                a.source().projection.source.signatures,
                b.source().projection.source.signatures
            ));
            assert_ne!(
                a.source().projection.receiver,
                b.source().projection.receiver
            );
            assert!(
                a.source()
                    .projection
                    .source
                    .source_payload
                    .contains("eStructuralFeatures")
            );
        }
        for ty in &types {
            // Compare the same private materialization path; public reports also
            // retain source_members, while this private path uses source-backed views.
            let ordinary = old
                .runtime_members_for_materialization_observed(ty, &obs)
                .unwrap()
                .into_owned_observed(&obs)
                .unwrap();
            let actual = current
                .runtime_members_for_materialization_observed(ty, &obs)
                .unwrap()
                .into_owned_observed(&obs)
                .unwrap();
            assert_eq!(
                serde_json::to_value(&ordinary).unwrap(),
                serde_json::to_value(&actual).unwrap()
            );
            assert_eq!(
                serde_json::to_value(old.runtime_members(ty).unwrap()).unwrap(),
                serde_json::to_value(current.runtime_members(ty).unwrap()).unwrap()
            );
        }
        let fresh = prepared(&bundle, true);
        assert!(cache(&fresh).borrow().signatures.is_empty());
    }
    #[test]
    fn applied_bindings_provider_policy_and_full_code_tables_use_original_projection() {
        let (bundle, types) = fixture();
        let resolver = prepared(&bundle, true);
        let operation = InvocationContext::default();
        let obs = ParserObservation::new(&operation);
        let owner = DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![0, 1],
        };
        let formal = DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![0, 0],
        };
        let env = Substitution::new(vec![ParameterBinding {
            parameter: formal,
            argument: types[1].expression().clone(),
        }])
        .unwrap();
        let expected = resolver
            .specialize_signatures_observed(&owner, &env, &obs)
            .unwrap();
        let actual = resolver
            .compiled_feature_signature_inputs(&owner, &env, &obs)
            .unwrap();
        assert!(matches!(actual, ErasedSignatures::Owned(_)));
        assert_eq!(
            serde_json::to_vec(&actual).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
        assert!(cache(&resolver).borrow().signatures.is_empty());
        let mut provider = prepared(&bundle, true);
        let id = DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![1],
        };
        provider.provider_relations.insert(
            (id.clone(), id),
            crate::definitions::conversion::ClassifierRelation::Equivalent,
        );
        assert!(matches!(
            provider
                .compiled_feature_signature_inputs(
                    &owner,
                    &Substitution::new(vec![]).unwrap(),
                    &obs
                )
                .unwrap(),
            ErasedSignatures::Owned(_)
        ));
        assert!(cache(&provider).borrow().signatures.is_empty());
        let table = cache(&resolver);
        for i in 0..4096 {
            table.borrow_mut().signatures.insert(
                DeclarationId {
                    source: owner.source.clone(),
                    path: vec![999, i],
                },
                Rc::new(vec![]),
            );
        }
        let empty = Substitution::new(vec![]).unwrap();
        let expected = resolver
            .specialize_signatures_observed(&owner, &empty, &obs)
            .unwrap();
        let actual = resolver
            .compiled_feature_signature_inputs(&owner, &empty, &obs)
            .unwrap();
        assert!(matches!(actual, ErasedSignatures::Owned(_)));
        assert_eq!(
            serde_json::to_vec(&actual).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
        assert_eq!(table.borrow().signatures.len(), 4096);
    }
    struct Meter {
        calls: Cell<usize>,
        bytes: Cell<usize>,
        stop: usize,
        cap: usize,
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("Unexpected signature provider".into())
        }
        fn work(&self, _: usize) -> Result<(), String> {
            self.tick()
        }
        fn retain_storage(&self, n: usize) -> Result<(), String> {
            self.tick()?;
            let total = self.bytes.get() + n;
            if total > self.cap {
                return Err("Signature storage denied".into());
            }
            self.bytes.set(total);
            Ok(())
        }
    }
    impl Meter {
        fn tick(&self) -> Result<(), String> {
            let at = self.calls.get();
            self.calls.set(at + 1);
            if at == self.stop {
                Err("Signature interrupted".into())
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn cold_code_and_owned_adoption_precharge_exact_limits_cancel_and_recover() {
        let (bundle, _) = fixture();
        let owner = DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![0, 1],
        };
        let run = |stop, cap, adopt| {
            let resolver = prepared(&bundle, true);
            let m = Rc::new(Meter {
                calls: Cell::new(0),
                bytes: Cell::new(0),
                stop,
                cap,
            });
            let operation = InvocationContext::new(m.clone());
            let obs = ParserObservation::new(&operation);
            let result = (|| {
                let rows = resolver.compiled_feature_signature_inputs(
                    &owner,
                    &Substitution::new(vec![]).unwrap(),
                    &obs,
                )?;
                if adopt {
                    rows.into_owned_observed(&owner.source, &obs)?;
                }
                Ok::<_, ImportError>(())
            })();
            (obs.finish(result), m.calls.get(), m.bytes.get())
        };
        for adopt in [false, true] {
            let (_, calls, bytes) = run(usize::MAX, usize::MAX, adopt);
            assert!(run(usize::MAX, bytes, adopt).0.is_ok());
            assert!(run(usize::MAX, bytes - 1, adopt).0.is_err());
            for stop in 0..calls {
                assert!(run(stop, usize::MAX, adopt).0.is_err());
            }
            assert!(run(usize::MAX, usize::MAX, adopt).0.is_ok());
        }
        // Retained code does not skip the next attempt's observer, including
        // a complete public copy after a warm lookup.
        let run_warm = |stop, cap, adopt| {
            let resolver = prepared(&bundle, true);
            let initial = InvocationContext::default();
            let initial_obs = ParserObservation::new(&initial);
            let env = Substitution::new(vec![]).unwrap();
            resolver
                .compiled_feature_signature_inputs(&owner, &env, &initial_obs)
                .unwrap();
            let m = Rc::new(Meter {
                calls: Cell::new(0),
                bytes: Cell::new(0),
                stop,
                cap,
            });
            let operation = InvocationContext::new(m.clone());
            let obs = ParserObservation::new(&operation);
            let result = (|| {
                let rows = resolver.compiled_feature_signature_inputs(&owner, &env, &obs)?;
                if adopt {
                    rows.into_owned_observed(&owner.source, &obs)?;
                }
                Ok::<_, ImportError>(())
            })();
            (obs.finish(result), m.calls.get(), m.bytes.get())
        };
        for adopt in [false, true] {
            let (_, calls, bytes) = run_warm(usize::MAX, usize::MAX, adopt);
            assert!(calls > 0);
            assert!(run_warm(usize::MAX, bytes, adopt).0.is_ok());
            if adopt {
                assert!(bytes > 0);
                assert!(run_warm(usize::MAX, bytes - 1, adopt).0.is_err());
            }
            for stop in 0..calls {
                assert!(run_warm(stop, usize::MAX, adopt).0.is_err());
            }
            assert!(run_warm(usize::MAX, usize::MAX, adopt).0.is_ok());
        }
        let resolver = prepared(&bundle, true);
        let operation = InvocationContext::default();
        let obs = ParserObservation::new(&operation);
        let env = Substitution::new(vec![]).unwrap();
        resolver
            .compiled_feature_signature_inputs(&owner, &env, &obs)
            .unwrap();
        for bad in [
            DeclarationId {
                source: crate::definitions::bundle::SourceId {
                    uri: "memory:foreign.ecore".into(),
                    sha256: owner.source.sha256.clone(),
                },
                path: owner.path.clone(),
            },
            DeclarationId {
                source: crate::definitions::bundle::SourceId {
                    uri: owner.source.uri.clone(),
                    sha256: "0".repeat(64),
                },
                path: owner.path.clone(),
            },
            DeclarationId {
                source: owner.source.clone(),
                path: vec![999],
            },
        ] {
            assert!(
                resolver
                    .compiled_feature_signature_inputs(&bad, &env, &obs)
                    .is_err()
            );
        }
    }
    #[test]
    fn nominal_descriptor_data_keeps_identity_caps_and_fresh_observer_checks() {
        let (bundle, types) = fixture();
        let declaration = match types[0].expression() {
            Type::Classifier { declaration, .. } => declaration.clone(),
            _ => panic!("classifier fixture"),
        };
        let span = SourceSpan {
            source: declaration.source.clone(),
            start: 0,
            end: bundle.sources[0].text.len(),
        };
        let run = |stop, cap, warm| {
            let resolver = prepared(&bundle, true);
            if warm {
                let initial = InvocationContext::default();
                resolver
                    .compiled_nominal_type_input(
                        &declaration,
                        true,
                        &span,
                        &ParserObservation::new(&initial),
                    )
                    .unwrap()
                    .unwrap();
            }
            let meter = Rc::new(Meter {
                calls: Cell::new(0),
                bytes: Cell::new(0),
                stop,
                cap,
            });
            let operation = InvocationContext::new(meter.clone());
            let obs = ParserObservation::new(&operation);
            let result = resolver.compiled_nominal_type_input(&declaration, true, &span, &obs);
            (obs.finish(result), meter.calls.get(), meter.bytes.get())
        };
        for warm in [false, true] {
            let (_, calls, bytes) = run(usize::MAX, usize::MAX, warm);
            assert!(bytes > 0 && calls > 0);
            assert!(run(usize::MAX, bytes, warm).0.is_ok());
            assert!(run(usize::MAX, bytes - 1, warm).0.is_err());
            for stop in 0..calls {
                assert!(run(stop, usize::MAX, warm).0.is_err());
            }
            assert!(run(usize::MAX, bytes, warm).0.is_ok());
        }
        let resolver = prepared(&bundle, true);
        let operation = InvocationContext::default();
        let obs = ParserObservation::new(&operation);
        let first = resolver
            .compiled_nominal_type_input(&declaration, true, &span, &obs)
            .unwrap()
            .unwrap();
        let second = resolver
            .compiled_nominal_type_input(&declaration, true, &span, &obs)
            .unwrap()
            .unwrap();
        assert!(Rc::ptr_eq(&first, &second));
        assert_eq!(first.as_ref(), types[0].expression());
        let closed = resolver
            .compiled_nominal_type_input(&declaration, false, &span, &obs)
            .unwrap()
            .unwrap();
        assert_ne!(first, closed);
        let foreign = DeclarationId {
            source: crate::definitions::bundle::SourceId {
                uri: "memory:foreign-descriptor.ecore".into(),
                sha256: declaration.source.sha256.clone(),
            },
            path: declaration.path.clone(),
        };
        let data = resolver
            .compiled_nominal_type_input(&foreign, true, &span, &obs)
            .unwrap()
            .unwrap();
        // Retained descriptor data cannot grant source or application authority.
        let foreign_type = RuntimeType::new(data.as_ref().clone()).unwrap();
        assert!(
            resolver
                .prepare_runtime_members_observed(&foreign_type, &obs)
                .is_err()
        );
        cache(&resolver).borrow_mut().nominal_input_count = 4096;
        assert!(
            resolver
                .compiled_nominal_type_input(&declaration, true, &span, &obs)
                .unwrap()
                .is_some()
        );
        assert!(
            resolver
                .compiled_nominal_type_input(
                    &DeclarationId {
                        source: declaration.source.clone(),
                        path: vec![1]
                    },
                    false,
                    &span,
                    &obs
                )
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn grouped_nominal_inputs_keep_uri_digest_path_and_raw_identity_after_ordered_shifts() {
        let (bundle, types) = fixture();
        let resolver = prepared(&bundle, true);
        let Type::Classifier { declaration, .. } = types[0].expression() else {
            panic!("classifier");
        };
        let span = fallback(declaration);
        let operation = InvocationContext::default();
        let obs = ParserObservation::new(&operation);
        let mut kept = Vec::new();
        for uri in ["memory:z", "memory:a", declaration.source.uri.as_str()] {
            for digest in [
                "f".repeat(64),
                "0".repeat(64),
                declaration.source.sha256.clone(),
            ] {
                for path in [vec![8, 2], vec![0], vec![3, 1]] {
                    for raw in [true, false] {
                        let id = DeclarationId {
                            source: crate::definitions::bundle::SourceId {
                                uri: uri.into(),
                                sha256: digest.clone(),
                            },
                            path: path.clone(),
                        };
                        let data = resolver
                            .compiled_nominal_type_input(&id, raw, &span, &obs)
                            .unwrap()
                            .unwrap();
                        assert_eq!(
                            data.as_ref(),
                            &Type::Classifier {
                                declaration: id.clone(),
                                arguments: vec![],
                                raw
                            }
                        );
                        kept.push((id, raw, data));
                    }
                }
            }
        }
        for (id, raw, initial) in kept {
            let repeated = resolver
                .compiled_nominal_type_input(&id, raw, &span, &obs)
                .unwrap()
                .unwrap();
            assert!(Rc::ptr_eq(&initial, &repeated));
        }
        let inputs = cache(&resolver);
        let table = inputs.borrow();
        assert_eq!(table.nominal_inputs.len(), 9);
        assert!(
            table
                .nominal_inputs
                .windows(2)
                .all(|pair| pair[0].source < pair[1].source)
        );
        assert!(table.nominal_inputs.iter().all(|source| {
            source.declarations.len() == 3
                && source
                    .declarations
                    .windows(2)
                    .all(|pair| pair[0].0 < pair[1].0)
        }));
        assert_eq!(table.nominal_input_count, 54);
    }
    #[test]
    #[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
    fn cold_owned_session_keeps_exact_schema_and_terminal_cancel_recovery() {
        let (mut bundle, _) = fixture();
        let (source, ids) =
            crate::definitions::migration_test_fixture::source_with_ids(bundle.sources.remove(0));
        bundle.sources.push(source);
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
        let operation = InvocationContext::default();
        let entry = bundle.sources[0].id.clone();
        let old = crate::definitions::compiler::materialize_runtime_bundle(
            &bundle,
            &entry,
            "urn:compiled-fields",
            &types,
            None,
        )
        .unwrap();
        let begin = || {
            crate::definitions::compiler::OwnedRuntimeSourceArrayPreparation::begin_with_compiled_signature_inputs(bundle.clone(),entry.clone(),"urn:compiled-fields".into(),&types,None,&operation).unwrap_or_else(|_|panic!("valid source failed"))
        };
        let mut session = begin();
        let mut advances = 0;
        while !session.advance(None, &operation).unwrap() {
            advances += 1;
        }
        assert_eq!(
            serde_json::to_vec(&old).unwrap(),
            serde_json::to_vec(&session.finish().materialization.result.unwrap()).unwrap()
        );
        for prefix in 0..=advances {
            let mut stopped = begin();
            for _ in 0..prefix {
                assert!(!stopped.advance(None, &operation).unwrap());
            }
            let cancelled = InvocationContext::new(Rc::new(Meter {
                calls: Cell::new(0),
                bytes: Cell::new(0),
                stop: 0,
                cap: usize::MAX,
            }));
            assert!(stopped.advance(None, &cancelled).is_err());
            assert!(stopped.advance(None, &operation).is_err());
            assert!(stopped.finish().materialization.result.is_err());
        }
        let mut fresh = begin();
        while !fresh.advance(None, &operation).unwrap() {}
        assert_eq!(
            serde_json::to_vec(&old).unwrap(),
            serde_json::to_vec(&fresh.finish().materialization.result.unwrap()).unwrap()
        );
    }
}
