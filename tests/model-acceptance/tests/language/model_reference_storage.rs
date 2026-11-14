use intersect_model::core::model::Model;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::compile_structure_bundle_with_conversions,
    conversion::{
        ConversionRegistry, InvocationContext, InvocationObserver, ProviderId, ReferenceContext,
    },
    generic_types::{RuntimeType, Type},
    resources::DeclarationId,
};
use intersect_model::model_reference::ReferenceDomain;
use std::{cell::Cell, rc::Rc};

struct Budget {
    used: Cell<usize>,
    limit: usize,
}
impl InvocationObserver for Budget {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("membership invokes no provider")
    }
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        let next = self
            .used
            .get()
            .checked_add(bytes)
            .filter(|n| *n <= self.limit)
            .ok_or("membership storage denied")?;
        self.used.set(next);
        Ok(())
    }
}

#[test]
fn denied_ancestry_storage_cannot_be_interpreted_as_false_membership() {
    let source = DefinitionSource::new(
        "file:///membership.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="t" nsURI="urn:membership">
<eClassifiers xsi:type="e:EClass" name="A"/>
<eClassifiers xsi:type="e:EClass" name="B" eSuperTypes="#//A"/>
</e:EPackage>"##,
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let class = DeclarationId {
        source: source.id.clone(),
        path: vec![0],
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let registry = ConversionRegistry::default();
    let schema =
        compile_structure_bundle_with_conversions(&bundle, &source.id, "urn:membership", &registry)
            .unwrap();
    let mut model = Model::default();
    model.create(&schema, "b", "B", "r", 0, 0).unwrap();
    let context = ReferenceContext {
        bundle,
        entry: source.id,
        namespace: "urn:membership".into(),
        conversions: vec![],
        materialized_types: vec![],
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::to_value(&model).unwrap(),
    };
    let original = ReferenceDomain::new(class.clone(), context.clone(), &registry).unwrap();
    let target = RuntimeType::new(Type::Classifier {
        declaration: class,
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    // Separate declaration copy: 10 nodes/155 metadata bytes = 1435.
    // Classifier Type: 18 nodes/230 metadata bytes = 2534. Schema validation
    // state128/B cursor65, then A cursor/selection/B cursor195, gives4345 before
    // membership's130 class keys +1033 chain ancestry: total5508.
    for limit in [
        0, 1434, 1435, 1436, 3968, 3969, 3970, 4096, 4097, 4098, 4356, 4357, 4358, 5519, 5520, 5521,
    ] {
        let budget = Rc::new(Budget {
            used: Cell::new(0),
            limit,
        });
        let observation = InvocationContext::new(budget.clone());
        let mut pending = match original.begin_retarget_runtime(target.clone(), &observation) {
            Ok(value) => value,
            Err(error) => {
                assert!(limit < 3969);
                assert_eq!(error, "membership storage denied");
                assert!(budget.used.get() <= limit);
                assert_eq!(original.context(), &context);
                continue;
            }
        };
        loop {
            match pending.advance(&observation) {
                Ok(false) => {}
                Ok(true) => {
                    assert!(limit >= 5520);
                    let accepted = pending.finish(&observation).unwrap();
                    assert_eq!(
                        accepted.resolve(&accepted.bind("b").unwrap()).unwrap().id,
                        "b"
                    );
                    assert_eq!(budget.used.get(), 5520);
                    assert_eq!(accepted.context(), &context);
                    break;
                }
                Err(error) => {
                    assert!(limit < 5520);
                    assert_eq!(error, "membership storage denied");
                    assert!(pending.advance(&InvocationContext::default()).is_err());
                    assert!(pending.finish(&InvocationContext::default()).is_err());
                    break;
                }
            }
        }
        assert!(budget.used.get() <= limit);
        assert_eq!(
            original.resolve(&original.bind("b").unwrap()).unwrap().id,
            "b"
        );
        assert_eq!(original.context(), &context);
    }
    let recovered = original
        .retarget_runtime_observed(target, &InvocationContext::default())
        .unwrap();
    assert_eq!(
        recovered.resolve(&recovered.bind("b").unwrap()).unwrap().id,
        "b"
    );
    assert_eq!(serde_json::to_value(model).unwrap(), context.model);
}
