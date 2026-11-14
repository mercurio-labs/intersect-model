use intersect_model::core::{
    model::Model,
    storage::{ModelView, StructuralPreparation},
};
use intersect_model::definitions::conversion::{InvocationContext, InvocationObserver, ProviderId};
use serde_json::json;
use std::{cell::Cell, rc::Rc};
struct Probe {
    work: Cell<usize>,
    stop: usize,
}

#[test]
fn present_structural_values_retain_each_validation_step_and_cancel_without_publication() {
    let fields=(0..12).map(|n|format!(r#"<eStructuralFeatures x:type="e:EAttribute" name="f{n:02}" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>"#)).collect::<String>();
    let schema=intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&format!(r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:present-structural-values"><eClassifiers x:type="e:EClass" name="Root">{fields}<eStructuralFeatures x:type="e:EReference" name="children" containment="true" upperBound="-1" eType="#//Child"/></eClassifiers><eClassifiers x:type="e:EClass" name="Child"/></e:EPackage>"##)).unwrap();
    let mut model = Model::default();
    model.create(&schema, "root", "Root", "r", 0, 0).unwrap();
    model.create(&schema, "child", "Child", "r", 0, 0).unwrap();
    model.elements[0]
        .values
        .insert("children".into(), json!(["child"]));
    // Raw fixture metadata must match the containment supplied above.
    model.resources.get_mut("r").unwrap().roots = std::sync::Arc::new(vec!["root".into()]);
    for n in 0..12 {
        model.elements[0]
            .values
            .insert(format!("f{n:02}"), json!(format!("literal {n}")));
    }
    let before = model.clone();
    let view = ModelView::new(&model).unwrap();
    let probe = Rc::new(Probe {
        work: Cell::new(0),
        stop: usize::MAX,
    });
    let observation = InvocationContext::new(probe.clone());
    let mut pending = StructuralPreparation::ownership();
    let mut steps = 0;
    // Source/default/codec atoms report internal work. This fixture envelope
    // does not claim a universal host quantum; continuations remain S09 carryover.
    let mut largest_quantum = 0;
    loop {
        let prior = probe.work.get();
        let done = pending
            .advance(&view, &schema, None, Some(&observation))
            .unwrap();
        largest_quantum = largest_quantum.max(probe.work.get() - prior);
        steps += 1;
        assert!(steps < 2000);
        if done {
            break;
        }
    }
    assert_eq!(
        largest_quantum, 48,
        "present structural source/codec fixture quantum"
    );
    let total = probe.work.get();
    let owners = pending.finish().unwrap();
    assert_eq!(owners.parent("child"), Some("root"));
    assert_eq!(owners.children("root"), &["child"]);
    for stop in 0..total {
        let probe = Rc::new(Probe {
            work: Cell::new(0),
            stop,
        });
        let observation = InvocationContext::new(probe.clone());
        let mut pending = StructuralPreparation::ownership();
        let error = loop {
            match pending.advance(&view, &schema, None, Some(&observation)) {
                Ok(false) => {}
                Ok(true) => panic!("incomplete structural admission published"),
                Err(error) => break error,
            }
        };
        assert!(
            error.ends_with("qualification cancelled"),
            "{stop}: {error}"
        );
        assert!(probe.work.get() <= stop);
        assert_eq!(
            pending.advance(&view, &schema, None, None).unwrap_err(),
            "Structural preparation has failed"
        );
        assert!(pending.finish().is_err());
        assert_eq!(model, before);
    }
    let probe = Rc::new(Probe {
        work: Cell::new(0),
        stop: total,
    });
    let observation = InvocationContext::new(probe.clone());
    let mut pending = StructuralPreparation::ownership();
    while !pending
        .advance(&view, &schema, None, Some(&observation))
        .unwrap()
    {}
    assert_eq!(probe.work.get(), total);
    assert_eq!(pending.finish().unwrap().parent("child"), Some("root"));
    assert_eq!(model, before);
    drop(view);
    model.elements[0].values.insert("f11".into(), json!(17));
    let invalid_before = model.clone();
    let view = ModelView::new(&model).unwrap();
    let mut pending = StructuralPreparation::ownership();
    let error = loop {
        match pending.advance(&view, &schema, None, None) {
            Ok(false) => {}
            Ok(true) => panic!("invalid final scalar accepted"),
            Err(error) => break error,
        }
    };
    assert_eq!(
        error,
        "Invalid EString value for Root.f11: Invalid EString wire value"
    );
    assert!(pending.finish().is_err());
    assert_eq!(model, invalid_before);
}
impl InvocationObserver for Probe {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("builtin qualification invokes no provider")
    }
    fn work(&self, units: usize) -> Result<(), String> {
        if self.work.get() + units > self.stop {
            return Err("qualification cancelled".into());
        }
        self.work.set(self.work.get() + units);
        Ok(())
    }
}
#[test]
fn structural_admission_retains_attribute_setup_and_publishes_only_complete_ownership() {
    let fields=(0..12).map(|n|format!(r#"<eStructuralFeatures x:type="e:EAttribute" name="f{n:02}" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>"#)).collect::<String>();
    let schema=intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&format!(r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:structural-attribute"><eClassifiers x:type="e:EClass" name="Root">{fields}<eStructuralFeatures x:type="e:EReference" name="children" containment="true" upperBound="-1" eType="#//Child"/></eClassifiers><eClassifiers x:type="e:EClass" name="Child"/></e:EPackage>"##)).unwrap();
    let mut model = Model::default();
    model.create(&schema, "root", "Root", "r", 0, 0).unwrap();
    model.create(&schema, "child", "Child", "r", 0, 0).unwrap();
    model.elements[0]
        .values
        .insert("children".into(), json!(["child"]));
    // Raw fixture metadata must match the containment supplied above.
    model.resources.get_mut("r").unwrap().roots = std::sync::Arc::new(vec!["root".into()]);
    let before = model.clone();
    let view = ModelView::new(&model).unwrap();
    let probe = Rc::new(Probe {
        work: Cell::new(0),
        stop: usize::MAX,
    });
    let observation = InvocationContext::new(probe.clone());
    let mut pending = StructuralPreparation::ownership();
    let mut advances = 0;
    // Source/default/codec atoms report internal work. This fixture envelope
    // does not claim a universal host quantum; continuations remain S09 carryover.
    let mut largest_quantum = 0;
    loop {
        let before_work = probe.work.get();
        let done = pending
            .advance(&view, &schema, None, Some(&observation))
            .unwrap();
        largest_quantum = largest_quantum.max(probe.work.get() - before_work);
        advances += 1;
        if done {
            break;
        }
        assert!(advances < 2000);
    }
    assert_eq!(
        largest_quantum, 34,
        "absent structural source/codec fixture quantum"
    );
    assert!(advances > 12 * 8);
    let total = probe.work.get();
    let ownership = pending.finish().unwrap();
    assert_eq!(ownership.parent("child"), Some("root"));
    assert_eq!(ownership.children("root"), &["child"]);
    assert_eq!(ownership.parent("root"), None);
    for stop in [0, 1, total / 4, total / 2, total - 1] {
        let probe = Rc::new(Probe {
            work: Cell::new(0),
            stop,
        });
        let observation = InvocationContext::new(probe.clone());
        let mut pending = StructuralPreparation::ownership();
        let error = loop {
            match pending.advance(&view, &schema, None, Some(&observation)) {
                Ok(false) => {}
                Ok(true) => panic!("insufficient budget accepted"),
                Err(error) => break error,
            }
        };
        assert_eq!(error, "qualification cancelled");
        assert_eq!(probe.work.get(), stop);
        assert_eq!(
            pending.advance(&view, &schema, None, None).unwrap_err(),
            "Structural preparation has failed"
        );
        assert!(pending.finish().is_err());
        assert_eq!(model, before);
    }
    let exact = Rc::new(Probe {
        work: Cell::new(0),
        stop: total,
    });
    let observation = InvocationContext::new(exact.clone());
    let mut pending = StructuralPreparation::ownership();
    while !pending
        .advance(&view, &schema, None, Some(&observation))
        .unwrap()
    {}
    assert!(
        pending
            .advance(&view, &schema, None, Some(&observation))
            .unwrap()
    );
    assert_eq!(exact.work.get(), total);
    assert_eq!(pending.finish().unwrap().parent("child"), Some("root"));
    assert_eq!(model, before);
}
