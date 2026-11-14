use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler,
    generic_resolution::Resolver,
    resources::ResourceSet,
};
const MODEL: &str = r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="annotations" nsURI="urn:annotations">
<eAnnotations source="package-doc"><details key="text" value="café 😀"/></eAnnotations>
<eClassifiers xsi:type="e:EClass" name="C">
<eAnnotations source="class-doc" references="#//C/value"><details key="documentation" value="exact bytes"/></eAnnotations>
<eStructuralFeatures xsi:type="e:EAttribute" name="value" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString" derived="true" transient="true" volatile="true" changeable="false"><eAnnotations source="derivation"><details key="body" value="unimplemented domain expression"/></eAnnotations></eStructuralFeatures>
</eClassifiers></e:EPackage>"##;
#[test]
fn annotations_retain_exact_inventory_and_do_not_change_feature_flags() {
    let source = DefinitionSource::new("memory:ecore", SourceKind::Ecore, MODEL);
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let schema =
        compiler::compile_structure_bundle(&bundle, &source.id, "urn:annotations").unwrap();
    let inventory = schema.generic_signatures.as_ref().unwrap();
    assert_eq!(
        serde_json::to_value(inventory).unwrap(),
        serde_json::to_value(Resolver::new(&bundle).unwrap().resolve_all().unwrap()).unwrap()
    );
    assert_eq!(inventory.retained_payloads.len(), 3);
    for span in &inventory.retained_payloads {
        assert_eq!(span.source, source.id);
        assert!(source.text[span.start..span.end].starts_with("annotation"));
    }
    let field = &schema.classes["C"].features["value"];
    assert!(field.derived && field.transient && field.volatile && !field.changeable);
    let resources = ResourceSet::new(&bundle).unwrap();
    assert_eq!(
        schema.classes["C"].feature_declarations["value"],
        resources.resolve(&source.id, "#//C/value").unwrap()
    );
    assert_eq!(bundle.sources[0].text, source.text);
}
#[test]
fn unresolved_references_and_opaque_annotation_payloads_remain_explicit() {
    for changed in [
        MODEL.replace("#//C/value", "#//C/missing"),
        MODEL.replace(
            "<details key=\"text\" value=\"café 😀\"/>",
            "<vendor:data xmlns:vendor='urn:vendor'/>",
        ),
    ] {
        assert!(
            intersect_model::definitions::ecore_document::validate_structure(&changed).is_err()
        );
    }
}

#[test]
fn annotated_import_preserves_exact_limits_cancellation_and_recovery() {
    use intersect_model::definitions::{
        conversion::{InvocationContext, InvocationObserver, ProviderId},
        ecore::{ObservedImportError, ParseLimits},
    };
    use std::{cell::Cell, rc::Rc};
    struct Meter {
        work: Cell<usize>,
        storage: Cell<usize>,
        calls: Cell<usize>,
        work_limit: usize,
        storage_limit: usize,
        cancel: Option<usize>,
    }
    impl Meter {
        fn new(work_limit: usize, storage_limit: usize, cancel: Option<usize>) -> Rc<Self> {
            Rc::new(Self {
                work: Cell::new(0),
                storage: Cell::new(0),
                calls: Cell::new(0),
                work_limit,
                storage_limit,
                cancel,
            })
        }
        fn tick(&self) -> Result<(), String> {
            let call = self.calls.get();
            self.calls.set(call + 1);
            if self.cancel == Some(call) {
                Err("annotation import cancelled".into())
            } else {
                Ok(())
            }
        }
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            panic!("annotation metadata must not invoke providers")
        }
        fn checkpoint(&self) -> Result<(), String> {
            self.tick()
        }
        fn work(&self, units: usize) -> Result<(), String> {
            self.tick()?;
            let total = self.work.get() + units;
            if total > self.work_limit {
                return Err("annotation work denied".into());
            }
            self.work.set(total);
            Ok(())
        }
        fn retain_storage(&self, bytes: usize) -> Result<(), String> {
            self.tick()?;
            let total = self.storage.get() + bytes;
            if total > self.storage_limit {
                return Err("annotation storage denied".into());
            }
            self.storage.set(total);
            Ok(())
        }
    }
    let source = DefinitionSource::new("memory:annotated", SourceKind::Ecore, MODEL);
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let run = |meter| {
        compiler::compile_structure_bundle_with_limits_observed(
            &bundle,
            &source.id,
            "urn:annotations",
            None,
            &InvocationContext::new(meter),
            ParseLimits {
                source_bytes: source.text.len(),
            },
        )
    };
    let baseline = Meter::new(usize::MAX, usize::MAX, None);
    let expected = run(baseline.clone()).unwrap();
    assert_eq!(
        expected,
        compiler::compile_structure_bundle(&bundle, &source.id, "urn:annotations").unwrap()
    );
    assert_eq!(
        expected,
        run(Meter::new(
            baseline.work.get(),
            baseline.storage.get(),
            None
        ))
        .unwrap()
    );
    for meter in [
        Meter::new(baseline.work.get() - 1, usize::MAX, None),
        Meter::new(usize::MAX, baseline.storage.get() - 1, None),
        Meter::new(usize::MAX, usize::MAX, Some(0)),
        Meter::new(usize::MAX, usize::MAX, Some(baseline.calls.get() / 2)),
        Meter::new(usize::MAX, usize::MAX, Some(baseline.calls.get() - 1)),
    ] {
        assert!(matches!(
            run(meter),
            Err(ObservedImportError::Observation(_))
        ));
        assert_eq!(
            expected,
            run(Meter::new(usize::MAX, usize::MAX, None)).unwrap()
        );
    }
    assert_eq!(bundle.sources[0].text, source.text);
}
