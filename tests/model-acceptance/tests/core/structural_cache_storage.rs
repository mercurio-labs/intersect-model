use intersect_model::core::{
    model::Model,
    storage::{ModelView, StructuralPreparation},
};
use intersect_model::definitions::conversion::{InvocationContext, InvocationObserver, ProviderId};
use std::{cell::Cell, rc::Rc};

struct Probe {
    bytes: Cell<usize>,
    limit: usize,
    checks: Cell<usize>,
    stop: usize,
}
impl InvocationObserver for Probe {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("empty-class structure invokes no provider")
    }
    fn checkpoint(&self) -> Result<(), String> {
        if self.checks.get() == self.stop {
            return Err("structural cancelled".into());
        }
        self.checks.set(self.checks.get() + 1);
        Ok(())
    }
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        let next = self
            .bytes
            .get()
            .checked_add(bytes)
            .filter(|n| *n <= self.limit)
            .ok_or("structural storage denied")?;
        self.bytes.set(next);
        Ok(())
    }
}
fn probe(limit: usize, stop: usize) -> Rc<Probe> {
    Rc::new(Probe {
        bytes: Cell::new(0),
        limit,
        checks: Cell::new(0),
        stop,
    })
}
fn fixture() -> (intersect_model::definitions::schema::Schema, Model) {
    let schema=intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="s" nsURI="urn:structural-cache"><eClassifiers x:type="e:EClass" name="A"/></e:EPackage>"#).unwrap();
    assert_eq!(schema.classes.len(), 1);
    let mut model = Model::default();
    model.create(&schema, "o", "A", "r", 0, 0).unwrap();
    (schema, model)
}

#[test]
fn structural_cache_precharges_literal_keys_map_and_object_boundaries() {
    let (schema, model) = fixture();
    let original = model.clone();
    let view = ModelView::new(&model).unwrap();
    // Empty A map: receiver 65 + effective-kind 65 + ancestry 452 = 582.
    // Two surrounding class keys 130, published cache key 65, object ID/class
    // pair 130: original cumulative 907. Revision/identity/object pages each
    // add a 96-byte row/string slot plus one byte for "o": 3*97 = 291.
    // The first two pages precede cache preparation; the third precedes the
    // object pair. Total 1198; no ownership edges or default payloads.
    let prefixes = [
        0, 97, 194, 259, 324, 421, 551, 648, 680, 809, 906, 971, 1068, 1198,
    ];
    let limits = prefixes
        .into_iter()
        .flat_map(|prefix: usize| [prefix.saturating_sub(1), prefix, prefix + 1])
        .collect::<std::collections::BTreeSet<_>>();
    for limit in limits {
        let budget = probe(limit, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        let mut pending = StructuralPreparation::ownership();
        loop {
            match pending.advance(&view, &schema, None, Some(&observation)) {
                Ok(false) => {}
                Ok(true) => {
                    assert!(limit >= 1198);
                    let index = pending.finish().unwrap();
                    assert_eq!(index.retained_bytes(), 0);
                    assert_eq!(index.parent("o"), None);
                    assert!(index.children("o").is_empty());
                    break;
                }
                Err(error) => {
                    assert!(limit < 1198);
                    assert_eq!(error, "structural storage denied");
                    assert!(
                        pending
                            .advance(&view, &schema, None, Some(&InvocationContext::default()))
                            .is_err()
                    );
                    assert!(pending.finish().is_err());
                    break;
                }
            }
        }
        assert_eq!(
            budget.bytes.get(),
            prefixes.into_iter().filter(|n| *n <= limit).max().unwrap()
        );
        assert_eq!(model, original);
    }
}

#[test]
fn observed_structural_cache_cancels_every_checkpoint_without_publication() {
    let (schema, model) = fixture();
    let original = model.clone();
    let view = ModelView::new(&model).unwrap();
    let baseline = probe(usize::MAX, usize::MAX);
    let observation = InvocationContext::new(baseline.clone());
    let mut pending = StructuralPreparation::ownership();
    while !pending
        .advance(&view, &schema, None, Some(&observation))
        .unwrap()
    {}
    assert_eq!(baseline.bytes.get(), 1198);
    assert_eq!(pending.finish().unwrap().retained_bytes(), 0);
    for stop in 0..baseline.checks.get() {
        let observation = InvocationContext::new(probe(usize::MAX, stop));
        let mut pending = StructuralPreparation::ownership();
        loop {
            match pending.advance(&view, &schema, None, Some(&observation)) {
                Ok(false) => {}
                Ok(true) => panic!("missed cancellation"),
                Err(error) => {
                    assert_eq!(error, "structural cancelled");
                    assert!(pending.advance(&view, &schema, None, None).is_err());
                    assert!(pending.finish().is_err());
                    break;
                }
            }
        }
    }
    let mut recovered = StructuralPreparation::ownership();
    while !recovered.advance(&view, &schema, None, None).unwrap() {}
    assert_eq!(recovered.finish().unwrap().retained_bytes(), 0);
    assert_eq!(model, original);
}

#[test]
fn structural_attribute_codec_and_values_adopt_observed_copies_before_publication() {
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="s" nsURI="urn:structural-codec"><eClassifiers x:type="e:EClass" name="A"><eStructuralFeatures x:type="e:EAttribute" name="title" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/></eClassifiers></e:EPackage>"##).unwrap();
    for present in [false, true] {
        let mut model = Model::default();
        model.create(&schema, "o", "A", "r", 0, 0).unwrap();
        if present {
            model.elements[0]
                .values
                .insert("title".into(), serde_json::json!("é"));
        }
        let original = model.clone();
        let view = ModelView::new(&model).unwrap();
        let run = |observation: &InvocationContext<'_>| {
            let mut pending = StructuralPreparation::ownership();
            loop {
                match pending.advance(&view, &schema, None, Some(observation)) {
                    Ok(false) => {}
                    Ok(true) => {
                        let index = pending.finish().unwrap();
                        assert_eq!(index.retained_bytes(), 0);
                        assert!(index.children("o").is_empty());
                        return Ok(());
                    }
                    Err(error) => {
                        assert!(pending.advance(&view, &schema, None, None).is_err());
                        assert!(pending.finish().is_err());
                        return Err(error);
                    }
                }
            }
        };
        let baseline = probe(usize::MAX, usize::MAX);
        run(&InvocationContext::new(baseline.clone())).unwrap();
        let required = baseline.bytes.get();
        // Aggregate compatibility boundaries include schema/member preparation;
        // independent component byte policies are pinned in definitions controls.
        assert!(required > 1198);
        for limit in [0, required - 1, required, required + 1] {
            let budget = probe(limit, usize::MAX);
            let result = run(&InvocationContext::new(budget.clone()));
            if limit < required {
                assert_eq!(result.unwrap_err(), "structural storage denied");
                assert!(budget.bytes.get() < required);
            } else {
                result.unwrap();
                assert_eq!(budget.bytes.get(), required);
            }
            assert_eq!(model, original);
        }
        for stop in 0..baseline.checks.get() {
            assert_eq!(
                run(&InvocationContext::new(probe(usize::MAX, stop))).unwrap_err(),
                "structural cancelled"
            );
            assert_eq!(model, original);
        }
        let mut recovered = StructuralPreparation::ownership();
        while !recovered.advance(&view, &schema, None, None).unwrap() {}
        assert_eq!(recovered.finish().unwrap().retained_bytes(), 0);
        assert_eq!(model, original);
    }
}
