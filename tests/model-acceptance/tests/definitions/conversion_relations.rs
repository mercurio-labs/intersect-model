use intersect_model::definitions::{
    bundle::{DefinitionSource, SourceKind},
    conversion::{
        ClassifierRelation as R, ConversionContract, ConversionProvider, ConversionRegistry,
        ProviderId,
    },
    datatypes::Datatype,
    resources::DeclarationId,
};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};

struct Provider {
    contract: ConversionContract,
    changed: ConversionContract,
    mutate: Cell<bool>,
    did_mutate: Cell<bool>,
    answer: Result<Option<R>, String>,
    observed: RefCell<Vec<ConversionContract>>,
}
impl Provider {
    fn new(index: usize, answer: Result<Option<R>, String>) -> Self {
        let source = DefinitionSource::new(
            "memory:relations",
            SourceKind::Ecore,
            include_str!("../../../../fixtures/ecore-generics/provider-alias-bounds.ecore"),
        );
        let contract = ConversionContract {
            datatype: DeclarationId {
                source: source.id,
                path: vec![index],
            },
            provider: ProviderId {
                name: "test.domain".into(),
                version: format!("{index}"),
            },
            serializable: true,
            declarations: vec![],
            members: vec![],
        };
        let mut changed = contract.clone();
        changed.provider.version = "changed".into();
        Self {
            contract,
            changed,
            mutate: Cell::new(false),
            did_mutate: Cell::new(false),
            answer,
            observed: RefCell::new(vec![]),
        }
    }
    fn datatype(&self) -> Datatype {
        Datatype::new(
            self.contract.datatype.clone(),
            "Domain".into(),
            true,
            None,
            None,
        )
        .unwrap()
    }
}
impl ConversionProvider for Provider {
    fn contract(&self) -> &ConversionContract {
        if self.did_mutate.get() {
            &self.changed
        } else {
            &self.contract
        }
    }
    fn classifier_relation(&self, target: &ConversionContract) -> Result<Option<R>, String> {
        self.observed.borrow_mut().push(target.clone());
        if self.mutate.get() {
            self.did_mutate.set(true);
        }
        self.answer.clone()
    }
    fn parse(&self, text: &str) -> Result<Value, String> {
        Ok(json!(text))
    }
    fn format(&self, value: &Value) -> Result<String, String> {
        Ok(value.to_string())
    }
    fn validate(&self, _: &Value) -> Result<(), String> {
        Ok(())
    }
    fn default_value(&self) -> Result<Option<Value>, String> {
        Ok(None)
    }
}

#[test]
fn reciprocal_relations_preserve_direction_and_exact_contracts() {
    for (forward, reverse) in [
        (R::Equivalent, R::Equivalent),
        (R::Subtype, R::Supertype),
        (R::Supertype, R::Subtype),
        (R::Unrelated, R::Unrelated),
    ] {
        let a = Provider::new(0, Ok(Some(forward)));
        let b = Provider::new(1, Ok(Some(reverse)));
        let mut registry = ConversionRegistry::default();
        registry.register(&a.datatype(), &a).unwrap();
        registry.register(&b.datatype(), &b).unwrap();
        assert_eq!(
            registry
                .classifier_relation(&a.contract.datatype, &b.contract.datatype)
                .unwrap(),
            Some(forward)
        );
        assert_eq!(
            registry
                .classifier_relation(&b.contract.datatype, &a.contract.datatype)
                .unwrap(),
            Some(reverse)
        );
        assert!(
            a.observed
                .borrow()
                .iter()
                .all(|contract| contract == &b.contract)
        );
        assert!(
            b.observed
                .borrow()
                .iter()
                .all(|contract| contract == &a.contract)
        );
        let value = registry
            .bind(&a.contract.datatype, json!("payload"))
            .unwrap();
        assert!(
            registry
                .validate_for(&b.datatype(), &value)
                .unwrap_err()
                .contains("destination datatype")
        );
    }
}

#[test]
fn missing_support_does_not_mean_unrelated_or_accept_one_sided_assertions() {
    for (left, right) in [
        (None, None),
        (Some(R::Equivalent), None),
        (None, Some(R::Unrelated)),
    ] {
        let a = Provider::new(0, Ok(left));
        let b = Provider::new(1, Ok(right));
        let mut registry = ConversionRegistry::default();
        assert!(
            registry
                .classifier_relation(&a.contract.datatype, &a.contract.datatype)
                .is_err()
        );
        registry.register(&a.datatype(), &a).unwrap();
        registry.register(&b.datatype(), &b).unwrap();
        assert_eq!(
            registry
                .classifier_relation(&a.contract.datatype, &a.contract.datatype)
                .unwrap(),
            Some(R::Equivalent)
        );
        assert_eq!(
            registry
                .classifier_relation(&a.contract.datatype, &b.contract.datatype)
                .unwrap(),
            None
        );
    }
}

#[test]
fn contradictory_and_failed_providers_are_errors() {
    for (answer, message) in [
        (Ok(Some(R::Subtype)), "disagree"),
        (Err("provider failed".into()), "provider failed"),
    ] {
        let a = Provider::new(0, Ok(Some(R::Equivalent)));
        let b = Provider::new(1, answer);
        let mut registry = ConversionRegistry::default();
        registry.register(&a.datatype(), &a).unwrap();
        registry.register(&b.datatype(), &b).unwrap();
        assert!(
            registry
                .classifier_relation(&a.contract.datatype, &b.contract.datatype)
                .unwrap_err()
                .contains(message)
        );
    }
}

#[test]
fn contract_changes_during_either_callback_invalidate_the_answer() {
    for change_source in [true, false] {
        let a = Provider::new(0, Ok(Some(R::Equivalent)));
        let b = Provider::new(1, Ok(Some(R::Equivalent)));
        a.mutate.set(change_source);
        b.mutate.set(!change_source);
        let mut registry = ConversionRegistry::default();
        registry.register(&a.datatype(), &a).unwrap();
        registry.register(&b.datatype(), &b).unwrap();
        assert!(
            registry
                .classifier_relation(&a.contract.datatype, &b.contract.datatype)
                .unwrap_err()
                .contains("changed its registered contract")
        );
    }
}

#[test]
fn resolver_consumes_directional_relations_without_treating_unknown_as_false() {
    use intersect_model::definitions::{bundle::DefinitionBundle, generic_resolution::Resolver};
    for (actual, bound, expected) in [
        (R::Equivalent, R::Equivalent, true),
        (R::Subtype, R::Supertype, true),
        (R::Supertype, R::Subtype, false),
        (R::Unrelated, R::Unrelated, false),
    ] {
        let a = Provider::new(0, Ok(Some(bound)));
        let b = Provider::new(1, Ok(Some(actual)));
        let source = DefinitionSource::new(
            "memory:relations",
            SourceKind::Ecore,
            include_str!("../../../../fixtures/ecore-generics/provider-alias-bounds.ecore"),
        );
        let selected = DeclarationId {
            source: source.id.clone(),
            path: vec![3, 0, 0],
        };
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        };
        let mut registry = ConversionRegistry::default();
        registry.register(&a.datatype(), &a).unwrap();
        registry.register(&b.datatype(), &b).unwrap();
        let resolver = Resolver::new(&bundle)
            .unwrap()
            .with_conversions(&registry)
            .unwrap();
        assert_eq!(
            resolver.application_bounds(&selected).unwrap().valid,
            expected
        );
    }
}

#[test]
fn generic_selection_rejects_inconsistent_provider_equivalence_in_any_group_order() {
    use intersect_model::definitions::{
        bundle::DefinitionBundle,
        generic_resolution::Resolver,
        generic_types::{ClosedType, Type},
    };
    struct Matrix(ConversionContract);
    impl ConversionProvider for Matrix {
        fn contract(&self) -> &ConversionContract {
            &self.0
        }
        fn classifier_relation(&self, target: &ConversionContract) -> Result<Option<R>, String> {
            let pair = (self.0.datatype.path[0], target.datatype.path[0]);
            Ok(Some(if matches!(pair, (0, 2) | (2, 0)) {
                R::Unrelated
            } else {
                R::Equivalent
            }))
        }
        fn parse(&self, text: &str) -> Result<Value, String> {
            Ok(json!(text))
        }
        fn format(&self, value: &Value) -> Result<String, String> {
            Ok(value.to_string())
        }
        fn validate(&self, _: &Value) -> Result<(), String> {
            Ok(())
        }
        fn default_value(&self) -> Result<Option<Value>, String> {
            Ok(None)
        }
    }
    for parents in ["#//C0 #//C1 #//C2", "#//C0 #//C2 #//C1"] {
        let types = (0..3)
            .map(|n| format!(r#"<eClassifiers x:type="e:EDataType" name="D{n}"/>"#))
            .collect::<String>();
        let classes = (0..3).map(|n|format!(r##"<eClassifiers x:type="e:EClass" name="C{n}"><eOperations name="f"><eParameters name="p" eType="#//D{n}"/></eOperations></eClassifiers>"##)).collect::<String>();
        let source = DefinitionSource::new(
            "memory:inconsistent",
            SourceKind::Ecore,
            format!(
                r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="p" nsURI="urn:p">{types}{classes}<eClassifiers x:type="e:EClass" name="Root" eSuperTypes="{parents}"/></e:EPackage>"#
            ),
        );
        let id = |n| DeclarationId {
            source: source.id.clone(),
            path: vec![n],
        };
        let implementations = (0..3)
            .map(|n| {
                Matrix(ConversionContract {
                    datatype: id(n),
                    provider: ProviderId {
                        name: "test.matrix".into(),
                        version: "1".into(),
                    },
                    serializable: true,
                    declarations: vec![],
                    members: vec![],
                })
            })
            .collect::<Vec<_>>();
        let requested = ClosedType::new(Type::Classifier {
            declaration: id(6),
            arguments: vec![],
            raw: false,
        })
        .unwrap();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        };
        let mut registry = ConversionRegistry::default();
        for implementation in &implementations {
            let datatype = intersect_model::definitions::compiler::import_datatype_bundle(
                &bundle,
                &implementation.0.datatype,
            )
            .unwrap();
            registry.register(&datatype, implementation).unwrap();
        }
        let resolver = Resolver::new(&bundle)
            .unwrap()
            .with_conversions(&registry)
            .unwrap();
        let error = resolver
            .resolved_generic_operations(&requested)
            .unwrap_err();
        assert!(
            error
                .message
                .contains("Inconsistent operation signature equivalence"),
            "{}",
            error.message
        );
    }
}

#[test]
fn exact_contract_lookup_checks_identity_and_provider_snapshot_without_invocation() {
    let provider = Provider::new(0, Ok(None));
    let mut registry = ConversionRegistry::default();
    assert!(!registry.contains_contract(&provider.contract).unwrap());
    registry.register(&provider.datatype(), &provider).unwrap();
    assert!(registry.contains_contract(&provider.contract).unwrap());
    assert!(!registry.contains_contract(&provider.changed).unwrap());
    assert!(provider.observed.borrow().is_empty());
    provider.did_mutate.set(true);
    assert!(
        registry
            .contains_contract(&provider.contract)
            .unwrap_err()
            .contains("changed")
    );
}
