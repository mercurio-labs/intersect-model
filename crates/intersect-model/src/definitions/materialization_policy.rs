//! Fixed conversion selection inputs, never a retained executable registry or proof.
use super::*;
use crate::definitions::conversion::{
    AppliedConversionContract, ConstructorContract, ConversionContract, ConversionRegistry,
};

#[derive(Default)]
pub(super) struct CompilationPolicyInputs {
    ordinary: Vec<ConversionContract>,
    applied: Vec<AppliedConversionContract>,
    constructors: Vec<ConstructorContract>,
    compilation_dependencies: Vec<DeclarationId>,
    constructor_dependencies: Vec<DeclarationId>,
}
fn error(span: &SourceSpan, message: impl Into<String>) -> ImportError {
    ImportError::new(ErrorKind::InvalidDefinition, span, message)
}
fn retain<'a, T: Clone + serde::Serialize + 'a>(
    values: impl Iterator<Item = Result<&'a T, String>>,
    span: &SourceSpan,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<Vec<T>, ImportError> {
    let mut retained = Vec::new();
    for value in values {
        observation.work(1, &span.source)?;
        let value = value.map_err(|message| error(span, message))?;
        observation.serialized(value, &span.source)?;
        observation.reserve(32, &span.source)?;
        retained.push(value.clone());
    }
    Ok(retained)
}
fn compare<'a, T: PartialEq + 'a>(
    expected: &[T],
    mut current: impl Iterator<Item = Result<&'a T, String>>,
    span: &SourceSpan,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    for expected in expected {
        observation.work(1, &span.source)?;
        let actual = current
            .next()
            .ok_or_else(|| error(span, "Compilation provider selections changed"))?
            .map_err(|message| error(span, message))?;
        if expected != actual {
            return Err(error(span, "Compilation provider selections changed"));
        }
    }
    if let Some(extra) = current.next() {
        observation.work(1, &span.source)?;
        extra.map_err(|message| error(span, message))?;
        return Err(error(span, "Compilation provider selections changed"));
    }
    Ok(())
}
impl CompilationPolicyInputs {
    pub(super) fn capture(
        registry: Option<&ConversionRegistry<'_>>,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        let Some(registry) = registry else {
            return Ok(Self::default());
        };
        Ok(Self {
            ordinary: retain(registry.registered_contract_refs(), span, observation)?,
            applied: retain(
                registry.applied().registered_contract_refs(),
                span,
                observation,
            )?,
            constructors: retain(
                registry.constructors().registered_contract_refs(),
                span,
                observation,
            )?,
            compilation_dependencies: retain(
                registry
                    .compilation_dependencies()
                    .map_err(|message| error(span, message))?
                    .map(|value| value.map(|contract| &contract.datatype)),
                span,
                observation,
            )?,
            constructor_dependencies: retain(
                registry
                    .constructor_dependencies()
                    .map_err(|message| error(span, message))?
                    .map(|contract| Ok(contract.datatype.declaration())),
                span,
                observation,
            )?,
        })
    }
    pub(super) fn verify(
        &self,
        registry: Option<&ConversionRegistry<'_>>,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        // Metadata equality grants no relation/value authority. Existing source,
        // relation and conversion consumers still run their own semantic checks.
        let empty = ConversionRegistry::default();
        let registry = registry.unwrap_or(&empty);
        compare(
            &self.ordinary,
            registry.registered_contract_refs(),
            span,
            observation,
        )?;
        compare(
            &self.applied,
            registry.applied().registered_contract_refs(),
            span,
            observation,
        )?;
        compare(
            &self.constructors,
            registry.constructors().registered_contract_refs(),
            span,
            observation,
        )?;
        compare(
            &self.compilation_dependencies,
            registry
                .compilation_dependencies()
                .map_err(|message| error(span, message))?
                .map(|value| value.map(|contract| &contract.datatype)),
            span,
            observation,
        )?;
        compare(
            &self.constructor_dependencies,
            registry
                .constructor_dependencies()
                .map_err(|message| error(span, message))?
                .map(|contract| Ok(contract.datatype.declaration())),
            span,
            observation,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::{
        bundle::{DefinitionSource, SourceKind},
        conversion::{
            AppliedConversionProvider, ClassifierRelation, ConstructorProvider, ConversionProvider,
            InvocationContext, InvocationObserver, ProviderId,
        },
        generic_types::{ClosedType, Type},
    };
    use std::{cell::Cell, rc::Rc};
    struct Provider {
        contract: ConversionContract,
        changed: ConversionContract,
        stale: Cell<bool>,
    }
    impl ConversionProvider for Provider {
        fn contract(&self) -> &ConversionContract {
            if self.stale.get() {
                &self.changed
            } else {
                &self.contract
            }
        }
        fn parse(&self, _: &str) -> Result<Value, String> {
            Err("Unexpected semantic call".into())
        }
        fn format(&self, _: &Value) -> Result<String, String> {
            Err("Unexpected semantic call".into())
        }
        fn validate(&self, _: &Value) -> Result<(), String> {
            Err("Unexpected semantic call".into())
        }
        fn default_value(&self) -> Result<Option<Value>, String> {
            Err("Unexpected semantic call".into())
        }
    }
    struct Applied {
        contract: AppliedConversionContract,
        implementation: Provider,
    }
    impl AppliedConversionProvider for Applied {
        fn contract(&self) -> &AppliedConversionContract {
            &self.contract
        }
        fn implementation(&self) -> &dyn ConversionProvider {
            &self.implementation
        }
    }
    struct Constructor(ConstructorContract);
    impl ConstructorProvider for Constructor {
        fn contract(&self) -> &ConstructorContract {
            &self.0
        }
        fn constructor_relation(
            &self,
            _: &ConstructorContract,
        ) -> Result<Option<ClassifierRelation>, String> {
            Err("Unexpected relation invocation".into())
        }
    }
    struct Fixture {
        bundle: DefinitionBundle,
        ordinary: Rc<Provider>,
        applied: Applied,
        constructor: Constructor,
    }
    impl Fixture {
        fn new() -> Self {
            let source = DefinitionSource::new(
                "memory:policy-inputs.ecore",
                SourceKind::Ecore,
                "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:policy-inputs'><eClassifiers x:type='e:EDataType' name='Text' instanceClassName='java.lang.String'/><eClassifiers x:type='e:EDataType' name='Box' instanceClassName='demo.Box'><eTypeParameters name='T'/></eClassifiers></e:EPackage>",
            );
            let id = |index| DeclarationId {
                source: source.id.clone(),
                path: vec![index],
            };
            let contract = |index| ConversionContract {
                datatype: id(index),
                provider: ProviderId {
                    name: "test.policy".into(),
                    version: "1".into(),
                },
                serializable: true,
                declarations: vec![],
                members: vec![],
            };
            let provider = |index| {
                let contract = contract(index);
                let mut changed = contract.clone();
                changed.provider.version = "changed".into();
                Provider {
                    contract,
                    changed,
                    stale: Cell::new(false),
                }
            };
            let applied = Applied {
                contract: AppliedConversionContract {
                    application: ClosedType::new(Type::Classifier {
                        declaration: id(1),
                        arguments: vec![Type::Classifier {
                            declaration: id(0),
                            arguments: vec![],
                            raw: false,
                        }],
                        raw: false,
                    })
                    .unwrap(),
                    conversion: contract(1),
                },
                implementation: provider(1),
            };
            let ordinary = Rc::new(provider(0));
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source],
                namespaces: vec![],
            };
            let constructor = Constructor(ConstructorContract {
                datatype: crate::definitions::compiler::import_datatype_bundle(
                    &bundle,
                    &DeclarationId {
                        source: bundle.sources[0].id.clone(),
                        path: vec![1],
                    },
                )
                .unwrap(),
                provider: ProviderId {
                    name: "test.constructor".into(),
                    version: "1".into(),
                },
            });
            Self {
                bundle,
                ordinary,
                applied,
                constructor,
            }
        }
        fn registry(&self, selections: bool) -> ConversionRegistry<'_> {
            self.registry_selections(selections, selections)
        }
        fn registry_selections(
            &self,
            compilation: bool,
            constructor: bool,
        ) -> ConversionRegistry<'_> {
            let mut registry = ConversionRegistry::default();
            let raw = &self.constructor.0.datatype;
            let datatype = crate::definitions::compiler::import_datatype_bundle(
                &self.bundle,
                &self.ordinary.contract.datatype,
            )
            .unwrap();
            registry
                .register_shared(&datatype, self.ordinary.clone())
                .unwrap();
            registry
                .register_constructor(&self.bundle, &self.constructor)
                .unwrap();
            let resolver = Resolver::new(&self.bundle).unwrap();
            let checked = resolver
                .instantiate_datatype(&self.applied.contract.application)
                .unwrap();
            registry
                .register_application(raw, &checked, &self.applied)
                .unwrap();
            if compilation {
                registry
                    .retain_compilation_dependency(&self.ordinary.contract.datatype)
                    .unwrap();
            }
            if constructor {
                registry
                    .retain_constructor_dependency(&self.constructor.0)
                    .unwrap();
            }
            registry
        }
        fn span(&self) -> SourceSpan {
            SourceSpan {
                source: self.bundle.sources[0].id.clone(),
                start: 0,
                end: 0,
            }
        }
    }
    #[derive(Default)]
    struct Meter {
        bytes: Cell<usize>,
        limit: usize,
        cancelled: Cell<bool>,
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("Unexpected provider invocation".into())
        }
        fn work(&self, _: usize) -> Result<(), String> {
            if self.cancelled.get() {
                Err("Policy cancelled".into())
            } else {
                Ok(())
            }
        }
        fn retain_storage(&self, bytes: usize) -> Result<(), String> {
            let total = self.bytes.get() + bytes;
            if total > self.limit {
                return Err("Policy storage denied".into());
            }
            self.bytes.set(total);
            Ok(())
        }
    }
    #[test]
    fn full_selections_recheck_fresh_registries_dependencies_and_mutated_contracts() {
        let fixture = Fixture::new();
        let span = fixture.span();
        let operation = InvocationContext::default();
        let observation = crate::definitions::ecore::ParserObservation::new(&operation);
        let inputs = {
            let registry = fixture.registry(true);
            CompilationPolicyInputs::capture(Some(&registry), &span, &observation).unwrap()
        };
        assert_eq!(
            (
                inputs.ordinary.len(),
                inputs.applied.len(),
                inputs.constructors.len(),
                inputs.compilation_dependencies.len(),
                inputs.constructor_dependencies.len()
            ),
            (1, 1, 1, 1, 1)
        );
        inputs
            .verify(Some(&fixture.registry(true)), &span, &observation)
            .unwrap();
        assert!(
            inputs
                .verify(Some(&fixture.registry(false)), &span, &observation)
                .is_err()
        );
        assert!(inputs.verify(None, &span, &observation).is_err());
        let registry = fixture.registry(true);
        fixture.ordinary.stale.set(true);
        assert!(inputs.verify(Some(&registry), &span, &observation).is_err());
        fixture.ordinary.stale.set(false);
        inputs
            .verify(Some(&fixture.registry(true)), &span, &observation)
            .unwrap();
        assert!(
            inputs
                .verify(
                    Some(&fixture.registry_selections(true, false)),
                    &span,
                    &observation
                )
                .is_err()
        );
        assert!(
            inputs
                .verify(
                    Some(&fixture.registry_selections(false, true)),
                    &span,
                    &observation
                )
                .is_err()
        );
        let mut changed = Fixture::new();
        changed.applied.contract.conversion.provider.version = "2".into();
        changed.applied.implementation.contract.provider.version = "2".into();
        assert!(
            inputs
                .verify(Some(&changed.registry(true)), &span, &observation)
                .is_err()
        );
        let mut changed = Fixture::new();
        changed.constructor.0.provider.version = "2".into();
        assert!(
            inputs
                .verify(Some(&changed.registry(true)), &span, &observation)
                .is_err()
        );
        let empty = CompilationPolicyInputs::default();
        assert!(empty.verify(Some(&registry), &span, &observation).is_err());
        empty
            .verify(Some(&ConversionRegistry::default()), &span, &observation)
            .unwrap();
    }
    #[test]
    fn selection_capture_precharges_storage_releases_observers_and_recovers_after_denial() {
        let fixture = Fixture::new();
        let span = fixture.span();
        let registry = fixture.registry(true);
        let meter = Rc::new(Meter {
            limit: usize::MAX,
            ..Default::default()
        });
        let weak = Rc::downgrade(&meter);
        let (inputs, bytes) = {
            let operation = InvocationContext::new(meter.clone());
            let observation = crate::definitions::ecore::ParserObservation::new(&operation);
            let inputs =
                CompilationPolicyInputs::capture(Some(&registry), &span, &observation).unwrap();
            (inputs, meter.bytes.get())
        };
        assert!(bytes > 0);
        drop(meter);
        assert!(weak.upgrade().is_none());
        let denied = Rc::new(Meter {
            limit: bytes - 1,
            ..Default::default()
        });
        let operation = InvocationContext::new(denied);
        let observation = crate::definitions::ecore::ParserObservation::new(&operation);
        assert!(matches!(
            observation.finish(CompilationPolicyInputs::capture(
                Some(&registry),
                &span,
                &observation
            )),
            Err(crate::definitions::ecore::ObservedImportError::Observation(
                _
            ))
        ));
        let meter = Rc::new(Meter {
            limit: usize::MAX,
            cancelled: Cell::new(true),
            ..Default::default()
        });
        let operation = InvocationContext::new(meter);
        let observation = crate::definitions::ecore::ParserObservation::new(&operation);
        assert!(matches!(
            observation.finish(inputs.verify(Some(&registry), &span, &observation)),
            Err(crate::definitions::ecore::ObservedImportError::Observation(
                _
            ))
        ));
        let operation = InvocationContext::default();
        let observation = crate::definitions::ecore::ParserObservation::new(&operation);
        CompilationPolicyInputs::capture(Some(&registry), &span, &observation)
            .unwrap()
            .verify(Some(&registry), &span, &observation)
            .unwrap();
    }
    #[test]
    fn selection_inputs_outlive_registry_and_release_the_executable_implementation() {
        let operation = InvocationContext::default();
        let observation = crate::definitions::ecore::ParserObservation::new(&operation);
        let (inputs, weak) = {
            let fixture = Fixture::new();
            let weak = Rc::downgrade(&fixture.ordinary);
            let registry = fixture.registry(true);
            let inputs =
                CompilationPolicyInputs::capture(Some(&registry), &fixture.span(), &observation)
                    .unwrap();
            (inputs, weak)
        };
        assert!(weak.upgrade().is_none());
        let fresh = Fixture::new();
        inputs
            .verify(Some(&fresh.registry(true)), &fresh.span(), &observation)
            .unwrap();
    }
}
