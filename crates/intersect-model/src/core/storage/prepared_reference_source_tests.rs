//! Selected source metadata never substitutes for live target/domain checks.
use super::*;
use crate::{
    core::query::QueryBudget,
    definitions::{
        conversion::{InvocationObserver, ProviderId},
        schema::{FeatureMapPreparation, PreparationError, ReferenceConformancePreparation},
    },
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

struct Meter {
    budget: RefCell<QueryBudget>,
    bytes: Cell<usize>,
    copies: Cell<usize>,
    cancelled: Cell<bool>,
}
impl Meter {
    fn new(limit: usize) -> Rc<Self> {
        Rc::new(Self {
            budget: RefCell::new(QueryBudget::new(limit)),
            bytes: Cell::new(0),
            copies: Cell::new(0),
            cancelled: Cell::new(false),
        })
    }
    fn work(&self) -> usize {
        self.budget.borrow().reads
    }
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> StorageResult<()> {
        Err("No provider expected".into())
    }
    fn checkpoint(&self) -> StorageResult<()> {
        if self.cancelled.get() {
            Err("Selected reference cancelled".into())
        } else {
            Ok(())
        }
    }
    fn work(&self, n: usize) -> StorageResult<()> {
        for _ in 0..n {
            self.budget.borrow_mut().charge()?;
        }
        Ok(())
    }
    fn retain_storage(&self, bytes: usize) -> StorageResult<()> {
        self.copies.set(self.copies.get() + 1);
        self.bytes.set(
            self.bytes
                .get()
                .checked_add(bytes)
                .ok_or("Test storage overflow")?,
        );
        Ok(())
    }
}
fn schema() -> Schema {
    crate::definitions::compiler::inspect_ecore_schema(r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="selected" nsURI="urn:selected-source">
<eClassifiers xsi:type="e:EClass" name="Base"><eStructuralFeatures xsi:type="e:EReference" name="r" eType="#//Target"/><eStructuralFeatures xsi:type="e:EAttribute" name="a" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/></eClassifiers>
<eClassifiers xsi:type="e:EClass" name="Receiver" eSuperTypes="#//Base"/>
<eClassifiers xsi:type="e:EClass" name="Target"/>
<eClassifiers xsi:type="e:EClass" name="Good" eSuperTypes="#//Target"/>
<eClassifiers xsi:type="e:EClass" name="Other"/>
<eClassifiers xsi:type="e:EClass" name="Broken"/>
</e:EPackage>"##).unwrap()
}
fn fields(schema: &Schema, receiver: &str) -> CachedFeatures {
    let mut p = FeatureMapPreparation::new(receiver);
    while !p.advance(schema).unwrap() {}
    CachedFeatures::Owned(p.finish().unwrap())
}
fn start(
    schema: &Schema,
    fields: &CachedFeatures,
    receiver: &str,
    name: &str,
    actual: &str,
    selected: bool,
    observation: &InvocationContext<'_>,
) -> StorageResult<ReferenceConformancePreparation> {
    if selected {
        let field = fields.get(name).ok_or("Missing test member")?;
        let source = fields
            .selected_reference(receiver, name, field, schema, observation)?
            .ok_or("Expected source selection")?;
        ReferenceConformancePreparation::from_selected_source_observed(
            receiver,
            name,
            actual,
            source.owner,
            source.declared,
            observation,
        )
        .map_err(|e| e.to_string())
    } else {
        ReferenceConformancePreparation::new_observed(receiver, name, actual, observation)
            .map_err(|e| e.to_string())
    }
}
fn run(
    schema: &Schema,
    fields: &CachedFeatures,
    receiver: &str,
    actual: &str,
    selected: bool,
    meter: &Rc<Meter>,
) -> StorageResult<bool> {
    let observation = InvocationContext::new(meter.clone());
    let mut p = start(
        schema,
        fields,
        receiver,
        "r",
        actual,
        selected,
        &observation,
    )?;
    loop {
        observation.work(1)?;
        match p.advance_observed(schema, &observation) {
            Ok(false) => {}
            Ok(true) => return p.finish(),
            Err(e) => {
                assert!(p.advance(schema).is_err());
                assert!(p.finish().is_err());
                return Err(e.to_string());
            }
        }
    }
}
fn equal(schema: &Schema, receiver: &str, actual: &str, expected: StorageResult<bool>) {
    let fields = fields(schema, receiver);
    let old = run(
        schema,
        &fields,
        receiver,
        actual,
        false,
        &Meter::new(usize::MAX),
    );
    let selected = run(
        schema,
        &fields,
        receiver,
        actual,
        true,
        &Meter::new(usize::MAX),
    );
    assert_eq!(old, expected);
    assert_eq!(selected, old);
}

#[test]
fn selected_reference_whole_results_preserve_inherited_target_ancestry_and_raw_bounds() {
    let mut s = schema();
    for (actual, expected) in [
        ("Target", true),
        ("Good", true),
        ("Other", false),
        ("Missing", false),
    ] {
        equal(&s, "Receiver", actual, Ok(expected));
    }
    s.classes.get_mut("Broken").unwrap().parents = vec!["Missing".into()];
    equal(&s, "Receiver", "Broken", Ok(false));
    s.raw_reference_bounds
        .insert("Base".into(), [("r".into(), vec!["Good".into()])].into());
    equal(&s, "Receiver", "Target", Ok(false));
    equal(&s, "Receiver", "Good", Ok(true));
    // Receiver constraints take precedence over owner fallback exactly as before.
    s.raw_reference_bounds.insert(
        "Receiver".into(),
        [("r".into(), vec!["Other".into()])].into(),
    );
    equal(&s, "Receiver", "Good", Ok(false));
    // Invalid ancestry in a raw bound preserves the old predicate rejection.
    s.raw_reference_bounds
        .get_mut("Receiver")
        .unwrap()
        .insert("r".into(), vec!["Missing".into()]);
    equal(&s, "Receiver", "Good", Ok(false));
}

fn domain_template() -> crate::definitions::schema::CheckedRuntimeFeatureDomain {
    use crate::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        generic_types::{RuntimeType, Type},
    };
    let source = DefinitionSource::new(
        "memory:selected-domain",
        SourceKind::Ecore,
        include_str!("../../../assets/fixtures/ecore-generics/raw-feature-domains.ecore"),
    );
    // Same fixture migration as definitions' private test helper, through the
    // existing public migration API; do not widen that helper's visibility.
    let uri = format!("{}.rcore", source.id.uri);
    let report = crate::migration::migrate_ecore(source, &uri).unwrap();
    assert!(report.issues.iter().all(|issue| !issue.blocks_conversion));
    let ids = report
        .correspondence
        .into_iter()
        .map(|entry| (entry.original.path, entry.converted))
        .collect::<BTreeMap<_, _>>();
    let source = report.converted.unwrap();
    let raw = RuntimeType::new(Type::Classifier {
        declaration: ids[&vec![8]].clone(),
        arguments: vec![],
        raw: true,
    })
    .unwrap();
    let schema = crate::definitions::compiler::materialize_runtime_types(
        &DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        &[raw],
        None,
    )
    .unwrap();
    schema
        .runtime_feature_domains
        .into_values()
        .next()
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
}
#[test]
fn selected_reference_reuses_runtime_alternative_and_unresolved_precedence() {
    let mut s = schema();
    let mut d = domain_template();
    d.source.projection.source.name = "r".into();
    d.source.reference = true;
    d.alternatives = vec!["Good".into()];
    d.unresolved.clear();
    s.runtime_feature_domains
        .insert("Receiver".into(), vec![d.clone()]);
    equal(&s, "Receiver", "Good", Ok(true));
    equal(&s, "Receiver", "Target", Ok(false));
    equal(&s, "Receiver", "Missing", Ok(false));
    let mut second = d.clone();
    second.alternatives = vec!["Other".into()];
    s.runtime_feature_domains
        .get_mut("Receiver")
        .unwrap()
        .push(second);
    equal(&s, "Receiver", "Good", Ok(false));
    s.runtime_feature_domains.get_mut("Receiver").unwrap().pop();
    // A matching runtime domain bypasses nominal/raw fallback in the OLD worker.
    // This control prevents the new initializer from changing that precedence.
    s.raw_reference_bounds.insert(
        "Receiver".into(),
        [("r".into(), vec!["Other".into()])].into(),
    );
    equal(&s, "Receiver", "Good", Ok(true));
    d.alternatives.clear();
    d.unresolved.push(
        crate::definitions::schema::UnresolvedRuntimeFeatureCandidate {
            candidate: d.source.erasure.clone(),
            target: "Good".into(),
            reason: "source membership deliberately unproven".into(),
        },
    );
    s.runtime_feature_domains.insert("Receiver".into(), vec![d]);
    equal(
        &s,
        "Receiver",
        "Good",
        Err(
            "Unresolved runtime feature membership: source membership deliberately unproven".into(),
        ),
    );
}

#[test]
fn selected_reference_source_selection_rejects_foreign_member_and_declaration() {
    let s = schema();
    let f = fields(&s, "Receiver");
    let r = f.get("r").unwrap();
    let o = InvocationContext::default();
    assert_eq!(
        f.selected_reference("Base", "r", r, &s, &o).err().unwrap(),
        "Pending reference source receiver/member changed"
    );
    assert!(
        f.selected_reference("Receiver", "missing", r, &s, &o)
            .is_err()
    );
    assert!(
        f.selected_reference("Receiver", "r", &r.clone(), &s, &o)
            .is_err()
    );
    assert!(
        f.selected_reference("Receiver", "a", f.get("a").unwrap(), &s, &o)
            .is_err()
    );
    let mut changed = s.clone();
    changed
        .classes
        .get_mut("Base")
        .unwrap()
        .feature_declarations
        .get_mut("r")
        .unwrap()
        .path
        .push(99);
    assert_eq!(
        f.selected_reference("Receiver", "r", r, &changed, &o)
            .err()
            .unwrap(),
        "Pending reference source declaration changed"
    );
    let mut missing = s.clone();
    missing.classes.remove("Base");
    assert_eq!(
        f.selected_reference("Receiver", "r", r, &missing, &o)
            .err()
            .unwrap(),
        "Missing pending reference source class"
    );
    assert!(
        ReferenceConformancePreparation::from_selected_source_observed(
            "Receiver",
            "r",
            "Good",
            "Base",
            f.get("a").unwrap(),
            &o
        )
        .is_err()
    );
}

#[test]
fn selected_reference_keeps_declaration_owner_and_target_when_effective_default_differs() {
    let mut s = schema();
    let mut d = domain_template();
    d.source.projection.source.name = "r".into();
    d.source.reference = true;
    d.alternatives = vec!["Good".into()];
    d.unresolved.clear();
    d.default = serde_json::json!("runtime-source-default");
    s.runtime_feature_domains.insert("Receiver".into(), vec![d]);
    let f = fields(&s, "Receiver");
    let projected = f.get("r").unwrap();
    let selected = f
        .selected_reference(
            "Receiver",
            "r",
            projected,
            &s,
            &InvocationContext::default(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        projected.default,
        serde_json::json!("runtime-source-default")
    );
    assert_eq!(selected.owner, "Base");
    assert_eq!(selected.declared.target, "Target");
    assert_eq!(selected.declared.default, serde_json::Value::Null);
    assert!(std::ptr::eq(
        selected.declared,
        &s.classes["Base"].features["r"]
    ));
    assert!(!std::ptr::eq(selected.declared, projected));
    // The shared runtime-domain worker still decides actual targets, regardless
    // of receiver-specific defaults. This is source metadata, not graph validity.
    equal(&s, "Receiver", "Good", Ok(true));
    equal(&s, "Receiver", "Target", Ok(false));
}

#[test]
fn selected_reference_budget_denial_precedes_selection_and_retained_copies_and_recovers() {
    let s = schema();
    let f = fields(&s, "Receiver");
    let baseline = Meter::new(usize::MAX);
    assert!(run(&s, &f, "Receiver", "Good", true, &baseline).unwrap());
    let exact = baseline.work();
    let old = Meter::new(usize::MAX);
    assert!(run(&s, &f, "Receiver", "Good", false, &old).unwrap());
    println!(
        "OLD_REFERENCE_COMPONENT {}",
        serde_json::json!({"work":old.work(),"retained_bytes":old.bytes.get(),"retained_callbacks":old.copies.get(),"holder_size":std::mem::size_of::<ReferenceConformancePreparation>(),"holder_alignment":std::mem::align_of::<ReferenceConformancePreparation>(),"whole_result":true})
    );
    println!(
        "SELECTED_REFERENCE_COMPONENT {}",
        serde_json::json!({"work":exact,"retained_bytes":baseline.bytes.get(),"retained_callbacks":baseline.copies.get(),"whole_result":true,"scope":"one selected source reference component, source map paid separately"})
    );
    for limit in [0, 1, 2, 3, 4, 5, 6, exact - 1, exact] {
        let m = Meter::new(limit);
        let result = run(&s, &f, "Receiver", "Good", true, &m);
        if limit == exact {
            assert_eq!(result, Ok(true));
            assert_eq!(m.work(), exact);
        } else {
            assert_eq!(
                result,
                Err("Query work budget exhausted; no complete result".into())
            );
        }
        assert!(m.work() <= limit);
        if limit < 7 {
            assert_eq!(m.copies.get(), 0);
        }
    }
    let cancelled = Meter::new(usize::MAX);
    let o = InvocationContext::new(cancelled.clone());
    let mut p = start(&s, &f, "Receiver", "r", "Good", true, &o).unwrap();
    cancelled.cancelled.set(true);
    let e = p.advance_observed(&s, &o).unwrap_err();
    assert_eq!(
        e,
        PreparationError::Observation("Selected reference cancelled".into())
    );
    assert!(p.advance(&s).is_err());
    assert!(p.finish().is_err());
    assert!(run(&s, &f, "Receiver", "Good", true, &Meter::new(exact)).unwrap());
}
