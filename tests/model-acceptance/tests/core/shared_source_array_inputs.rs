use intersect_model::core::{
    model::Model,
    storage::{ModelView, StoredFeatureProjection, StructuralPreparation},
};
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler,
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    generic_types::{RuntimeType, Type},
    resources::DeclarationId,
    schema::{RuntimeAlternative, RuntimeSourceArraySchemaInputs, Schema, SchemaRepresentation},
};
use serde_json::json;
use std::{cell::Cell, rc::Rc};
struct Fixture {
    owned: Schema,
    shared: RuntimeSourceArraySchemaInputs,
    model: Model,
    root: String,
    child: String,
}
fn fixture() -> Fixture {
    let source = DefinitionSource::new(
        "memory:shared-core.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:shared-core"><eClassifiers x:type="e:EClass" name="Root"><eTypeParameters name="T"><eBounds eClassifier="#//Root"/></eTypeParameters><eStructuralFeatures x:type="e:EReference" name="r0" containment="true" upperBound="-1"><eGenericType eTypeParameter="#//Root/T"/></eStructuralFeatures><eStructuralFeatures x:type="e:EReference" name="r1"><eGenericType eTypeParameter="#//Root/T"/></eStructuralFeatures><eStructuralFeatures x:type="e:EAttribute" name="label" unsettable="true" defaultValueLiteral="x" eType="http://www.eclipse.org/emf/2002/Ecore#//EString"/></eClassifiers><eClassifiers x:type="e:EClass" name="C0" eSuperTypes="#//Root"/><eClassifiers x:type="e:EClass" name="C1" eSuperTypes="#//Root"/><eClassifiers x:type="e:EClass" name="C2" eSuperTypes="#//Root"/></e:EPackage>"##,
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
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
        .collect::<Vec<_>>();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let owned =
        compiler::materialize_runtime_bundle(&bundle, &source.id, "urn:shared-core", &types, None)
            .unwrap();
    let root = owned.class_key_for_type(&types[0]).unwrap().to_owned();
    let child = owned.class_key_for_type(&types[1]).unwrap().to_owned();
    let shared = compiler::materialize_runtime_source_array_inputs_observed(
        &bundle,
        &source.id,
        "urn:shared-core",
        &types,
        None,
        &InvocationContext::default(),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&shared).unwrap(),
        serde_json::to_vec(&owned).unwrap()
    );
    let mut model = Model::default();
    model.create(&owned, "a", &root, "r", 0, 0).unwrap();
    model.create(&owned, "b", &child, "r", 0, 0).unwrap();
    model.elements[0].values.insert("r0".into(), json!(["b"]));
    model.elements[1].values.insert("r1".into(), json!("a"));
    // The raw fixture supplies its resource roots alongside containment.
    model.resources.get_mut("r").unwrap().roots = std::sync::Arc::new(vec!["a".into()]);
    Fixture {
        owned,
        shared,
        model,
        root,
        child,
    }
}
struct Probe {
    bytes: Cell<usize>,
    checks: Cell<usize>,
    work: Cell<usize>,
    max_bytes: usize,
    stop: usize,
}
impl InvocationObserver for Probe {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("builtin fixture cannot invoke providers")
    }
    fn checkpoint(&self) -> Result<(), String> {
        if self.checks.get() == self.stop {
            return Err("shared core cancelled".into());
        }
        self.checks.set(self.checks.get() + 1);
        Ok(())
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.work.set(self.work.get() + n);
        Ok(())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        let next = self.bytes.get() + n;
        if next > self.max_bytes {
            return Err("shared core storage denied".into());
        }
        self.bytes.set(next);
        Ok(())
    }
}
type Summary = Result<(Option<String>, Vec<String>, usize), String>;
fn run<A: RuntimeAlternative>(
    schema: &SchemaRepresentation<A>,
    model: &Model,
    max_bytes: usize,
    stop: usize,
) -> (Summary, usize, usize, usize) {
    let probe = Rc::new(Probe {
        bytes: Cell::new(0),
        checks: Cell::new(0),
        work: Cell::new(0),
        max_bytes,
        stop,
    });
    let observation = InvocationContext::new(probe.clone());
    let view = ModelView::new(model).unwrap();
    let mut pending = StructuralPreparation::ownership();
    let result = (|| {
        while !pending.advance(&view, schema, None, Some(&observation))? {}
        let index = pending.finish()?;
        Ok((
            index.parent("b").map(str::to_owned),
            index.children("a").to_vec(),
            index.retained_bytes(),
        ))
    })();
    (
        result,
        probe.bytes.get(),
        probe.checks.get(),
        probe.work.get(),
    )
}
#[test]
fn array_view_core_reads_preserve_complete_results_and_schema_errors() {
    let f = fixture();
    let original = f.model.clone();
    let view = ModelView::new(&f.model).unwrap();
    let a = run(&f.owned, &f.model, usize::MAX, usize::MAX);
    let b = run(&f.shared, &f.model, usize::MAX, usize::MAX);
    assert_eq!(a.0, b.0);
    assert_eq!(a.0.as_ref().unwrap().0.as_deref(), Some("a"));
    assert_eq!(a.0.as_ref().unwrap().1, vec!["b"]);
    assert_eq!(
        intersect_model::core::storage::validate(&view, &f.shared),
        Ok(())
    );
    for (receiver, object, name) in [
        (&f.root, "a", "r0"),
        (&f.child, "b", "r1"),
        (&f.child, "b", "label"),
    ] {
        let member = f.owned.effective_feature(receiver, name).unwrap().identity;
        let a = StoredFeatureProjection::new(&view, &f.owned, receiver, &member, object).unwrap();
        let b = StoredFeatureProjection::new(&view, &f.shared, receiver, &member, object).unwrap();
        assert_eq!(a.definition(), b.definition());
        assert_eq!(a.identity(), b.identity());
        if name == "r0" {
            assert_eq!(a.collection_page(0, 10), b.collection_page(0, 10));
            assert_eq!(b.collection_page(0, 10).unwrap(), vec![json!("b")]);
        } else {
            assert_eq!(a.singular(), b.singular());
        }
    }
    for mode in [0, 1, 2] {
        let mut model = f.model.clone();
        match mode {
            0 => {
                model.elements[1]
                    .values
                    .insert("r1".into(), json!("missing"));
            }
            1 => {
                model.elements[1].values.insert("r0".into(), json!(["a"]));
                // Both nodes are contained: test the cycle, not stale roots.
                model.resources.get_mut("r").unwrap().roots = std::sync::Arc::new(Vec::new());
            }
            _ => {
                model.elements[1]
                    .values
                    .insert("label".into(), json!("changed"));
            }
        }
        let a = run(&f.owned, &model, usize::MAX, usize::MAX);
        let b = run(&f.shared, &model, usize::MAX, usize::MAX);
        assert_eq!(a.0, b.0);
        assert!(a.0.is_err());
    }
    assert_eq!(f.model, original);
}

#[test]
fn array_view_core_reads_precharge_exact_limits_cancel_every_transition_and_recover() {
    let f = fixture();
    let original = f.model.clone();
    let owned = run(&f.owned, &f.model, usize::MAX, usize::MAX);
    let baseline = run(&f.shared, &f.model, usize::MAX, usize::MAX);
    assert!(baseline.0.is_ok());
    assert_eq!(baseline.0, owned.0);
    assert!(baseline.1 < owned.1);
    assert!(baseline.2 < owned.2);
    assert!(baseline.3 < owned.3);
    for stop in 0..baseline.2 {
        let result = run(&f.shared, &f.model, usize::MAX, stop);
        assert_eq!(result.0, Err("shared core cancelled".into()));
    }
    for limit in [0, baseline.1 - 1, baseline.1] {
        let result = run(&f.shared, &f.model, limit, usize::MAX);
        if limit < baseline.1 {
            assert_eq!(result.0, Err("shared core storage denied".into()));
        } else {
            assert_eq!(result, baseline);
        }
    }
    assert_eq!(run(&f.shared, &f.model, usize::MAX, usize::MAX), baseline);
    assert_eq!(f.model, original);
}
