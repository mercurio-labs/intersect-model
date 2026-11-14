//! Constructor policies are separate from argument comparison and raw value codecs.
use super::*;
use crate::definitions::conversion::{
    ClassifierRelation as R, ConstructorContract, ConstructorRegistry,
};

impl Resolver {
    /// Snapshot checked executable policies without requiring concrete applications.
    /// The registry has re-imported each contract; this context must contain the
    /// same exact source identities and formal syntax. Ordinary policies are kept.
    pub fn with_constructors(
        mut self,
        registry: &ConstructorRegistry<'_>,
    ) -> Result<Self, ImportError> {
        let location = fallback(&DeclarationId {
            source: self.standard_id.clone(),
            path: vec![],
        });
        let contracts = registry
            .registered_contracts()
            .map_err(|e| error(&location, e))?;
        self.prepared_ancestry.take();
        self.prepared_erased_signatures.take();
        self.constructor_contracts.clear();
        self.constructor_relations.clear();
        self.consumed_constructor_relations.get_mut().clear();
        for contract in &contracts {
            let id = contract.datatype.declaration();
            if self.classifier(id)? != TargetKind::Datatype
                || self.formals(id)? != contract.datatype.parameters()
            {
                return Err(error(
                    &fallback(id),
                    "Constructor policy differs from resolver source context",
                ));
            }
            self.constructor_contracts
                .insert(id.clone(), contract.clone());
        }
        for a in &contracts {
            for b in &contracts {
                let a = a.datatype.declaration();
                let b = b.datatype.declaration();
                if a >= b {
                    continue;
                }
                if let Some(relation) = registry
                    .constructor_relation(a, b)
                    .map_err(|e| error(&fallback(a), e))?
                {
                    let reverse = match relation {
                        R::Equivalent => R::Equivalent,
                        R::Subtype => R::Supertype,
                        R::Supertype => R::Subtype,
                        R::Unrelated => R::Unrelated,
                    };
                    self.constructor_relations
                        .insert((a.clone(), b.clone()), relation);
                    self.constructor_relations
                        .insert((b.clone(), a.clone()), reverse);
                }
            }
        }
        registry
            .registered_contracts()
            .map_err(|e| error(&location, e))?;
        Ok(self)
    }

    /// Exact consumed constructor policies, distinct from ordinary codec dependencies.
    pub fn consumed_constructor_contracts(&self) -> Vec<ConstructorContract> {
        self.consumed_constructor_relations
            .borrow()
            .iter()
            .filter_map(|id| self.constructor_contracts.get(id).cloned())
            .collect()
    }

    /// Compares classifier domains only. Callers own argument checking or explicit
    /// erasure policy; a positive result is never an application equality result.
    pub(super) fn classifier_domain_relation(
        &self,
        actual: &DeclarationId,
        bound: &DeclarationId,
        equivalent: bool,
    ) -> Result<Option<bool>, ImportError> {
        self.classifier_domain_relation_observed(
            actual,
            bound,
            equivalent,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    pub(super) fn classifier_domain_relation_observed(
        &self,
        actual: &DeclarationId,
        bound: &DeclarationId,
        equivalent: bool,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Option<bool>, ImportError> {
        observation.work(1, &actual.source)?;
        for ((candidate_actual, candidate_bound), relation) in &self.constructor_relations {
            observation.work(1, &actual.source)?;
            if candidate_actual == actual && candidate_bound == bound {
                Self::observe_relation_consumption(
                    &self.consumed_constructor_relations,
                    actual,
                    observation,
                )?;
                Self::observe_relation_consumption(
                    &self.consumed_constructor_relations,
                    bound,
                    observation,
                )?;
                return Ok(Some(
                    *relation == R::Equivalent || (!equivalent && *relation == R::Subtype),
                ));
            }
        }
        self.native_scalar_relation_observed(actual, bound, equivalent, observation)
    }
    pub(super) fn observe_relation_consumption(
        consumed: &std::cell::RefCell<BTreeSet<DeclarationId>>,
        declaration: &DeclarationId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        observation.work(1, &declaration.source)?;
        if !consumed.borrow().contains(declaration) {
            observation.serialized(declaration, &declaration.source)?;
            observation.reserve(128, &declaration.source)?;
            consumed.borrow_mut().insert(declaration.clone());
        }
        Ok(())
    }
}

#[cfg(test)]
mod observation_controls {
    use super::*;
    use crate::definitions::{
        compiler::xml_projection_controls::assert_lowering_receipt, conversion::InvocationContext,
        ecore::ParserObservation,
    };

    fn fixture(body: &str) -> (DefinitionBundle, DeclarationId, DeclarationId) {
        let source = crate::definitions::bundle::DefinitionSource::new(
            "memory:x",
            SourceKind::Ecore,
            format!(
                "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'>{body}</e:EPackage>"
            ),
        );
        let a = DeclarationId {
            source: source.id.clone(),
            path: vec![0],
        };
        let b = DeclarationId {
            source: source.id.clone(),
            path: vec![1],
        };
        (
            DefinitionBundle {
                version: 1,
                sources: vec![source],
                namespaces: vec![],
            },
            a,
            b,
        )
    }

    #[test]
    fn relation_table_observation_distinguishes_fresh_and_cached_consumption() {
        let (bundle, a, b) = fixture(
            "<eClassifiers x:type='e:EClass' name='A'/><eClassifiers x:type='e:EClass' name='B'/>",
        );
        // These direct table fixtures verify lookup/retention, not registry policy
        // qualification. Existing public registry controls verify contract import.
        // Unique consumed IDs each cost1413/16 + node128 + contains visit1.
        // Two ordered rows precede lookup success. Scalar fallback on two classes
        // costs2826/44 before provider selection; repeats skip copied set entries.
        for (constructor, repeated, bytes, work, prefixes) in [
            (true, false, 3082, 37, vec![1413, 1541, 2954, 3082]),
            (true, true, 3082, 42, vec![1413, 1541, 2954, 3082]),
            (
                false,
                false,
                5908,
                86,
                vec![1413, 2826, 4239, 4367, 5780, 5908],
            ),
            (
                false,
                true,
                8734,
                140,
                vec![1413, 2826, 4239, 4367, 5780, 5908, 7321, 8734],
            ),
        ] {
            for equivalent in [false, true] {
                let expected = Some(!equivalent);
                assert_lowering_receipt(&expected, bytes, work, &prefixes, |probe| {
                    let mut resolver = Resolver::new(&bundle).unwrap();
                    let table = if constructor {
                        &mut resolver.constructor_relations
                    } else {
                        &mut resolver.provider_relations
                    };
                    table.insert((a.clone(), a.clone()), R::Unrelated);
                    table.insert((a.clone(), b.clone()), R::Subtype);
                    if constructor {
                        resolver
                            .provider_relations
                            .insert((a.clone(), b.clone()), R::Unrelated);
                    }
                    let context = InvocationContext::new(probe.clone());
                    let observation = ParserObservation::new(&context);
                    let result =
                        observation.finish(resolver.classifier_domain_relation_observed(
                            &a,
                            &b,
                            equivalent,
                            &observation,
                        ))?;
                    let consumed = if constructor {
                        &resolver.consumed_constructor_relations
                    } else {
                        &resolver.consumed_provider_relations
                    };
                    assert_eq!(consumed.borrow().iter().collect::<Vec<_>>(), vec![&a, &b]);
                    if constructor {
                        assert!(resolver.consumed_provider_relations.borrow().is_empty());
                    }
                    if repeated {
                        assert_eq!(
                            observation.finish(resolver.classifier_domain_relation_observed(
                                &a,
                                &b,
                                equivalent,
                                &observation
                            ))?,
                            result
                        );
                        assert_eq!(consumed.borrow().iter().collect::<Vec<_>>(), vec![&a, &b]);
                    }
                    Ok(result)
                });
            }
        }
    }

    #[test]
    fn borrowed_native_relations_share_scalar_bindings_and_preserve_raw_domains() {
        // Parent copies1413/16 plus four classifier visits. Per native class:
        // entry1, six metadata-attribute visits, payload1, name visits2/payload1
        // =1413/31. Object skips the name scan, giving1413/28.
        for (body, expected, bytes, work, prefixes) in [
            (
                "<eClassifiers x:type='e:EClass' name='A'/><eClassifiers x:type='e:EClass' name='B'/>",
                None,
                2826,
                50,
                vec![1413, 2826],
            ),
            (
                "<eClassifiers x:type='e:EDataType' name='A' instanceClassName='int'/><eClassifiers x:type='e:EDataType' name='B' instanceClassName='int'/>",
                Some(true),
                2826,
                78,
                vec![1413, 2826],
            ),
            (
                "<eClassifiers x:type='e:EDataType' name='A' instanceClassName='int'/><eClassifiers x:type='e:EDataType' name='B' instanceClassName='java.lang.Object'/>",
                Some(false),
                2826,
                75,
                vec![1413, 2826],
            ),
            (
                "<eClassifiers x:type='e:EDataType' name='A' instanceClassName='java.lang.String'/><eClassifiers x:type='e:EDataType' name='B' instanceClassName='java.lang.Object'/>",
                Some(true),
                2826,
                75,
                vec![1413, 2826],
            ),
            (
                "<eClassifiers x:type='e:EDataType' name='A' instanceClassName='org.example.Custom'/><eClassifiers x:type='e:EDataType' name='B' instanceClassName='java.lang.Object'/>",
                None,
                2826,
                75,
                vec![1413, 2826],
            ),
            (
                "<eClassifiers x:type='e:EDataType' name='A' instanceClassName='int' instanceTypeName='long'/><eClassifiers x:type='e:EDataType' name='B' instanceClassName='java.lang.Object'/>",
                None,
                2826,
                74,
                vec![1413, 2826],
            ),
            (
                "<eClassifiers x:type='e:EDataType' name='A' instanceTypeName='java.lang.String'/><eClassifiers x:type='e:EDataType' name='B' instanceClassName='java.lang.Object'/>",
                Some(true),
                2826,
                75,
                vec![1413, 2826],
            ),
            (
                "<eClassifiers x:type='e:EDataType' name='A' instanceClassName='java.lang.String' instanceTypeName='java.lang.String'/><eClassifiers x:type='e:EDataType' name='B' instanceClassName='java.lang.Object'/>",
                Some(true),
                2826,
                77,
                vec![1413, 2826],
            ),
            (
                "<eClassifiers x:type='e:EDataType' name='A' instanceClassName='java.lang.String'><eTypeParameters name='T'/></eClassifiers><eClassifiers x:type='e:EDataType' name='B' instanceClassName='java.lang.Object'/>",
                None,
                10914,
                174,
                vec![
                    1413, 2826, 2858, 4431, 5844, 5935, 7508, 9083, 9148, 9276, 9341, 9469, 9501,
                    10914,
                ],
            ),
        ] {
            let (bundle, a, b) = fixture(body);
            assert_lowering_receipt(&expected, bytes, work, &prefixes, |probe| {
                let resolver = Resolver::new(&bundle).unwrap();
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                let result = observation.finish(resolver.classifier_domain_relation_observed(
                    &a,
                    &b,
                    false,
                    &observation,
                ))?;
                assert!(resolver.consumed_constructor_relations.borrow().is_empty());
                assert!(resolver.consumed_provider_relations.borrow().is_empty());
                Ok(result)
            });
        }
    }
}
