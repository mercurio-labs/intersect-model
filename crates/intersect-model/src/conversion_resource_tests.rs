use super::*;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, NamespaceBinding, SourceKind},
    conversion::{ProviderId, ReferenceContext},
    resources::DeclarationId,
};
use serde_json::json;
use std::sync::Arc;
fn bundle() -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        namespaces: vec![],
        sources: vec![DefinitionSource::new(
            "memory:shared.ecore",
            SourceKind::Ecore,
            r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="p" nsURI="urn:shared"><eClassifiers x:type="e:EDataType" name="Resource" serializable="false" instanceClassName="org.eclipse.emf.ecore.resource.Resource"/><eClassifiers x:type="e:EDataType" name="Set" serializable="false" instanceClassName="org.eclipse.emf.ecore.resource.ResourceSet"/><eClassifiers x:type="e:EDataType" name="Tree" serializable="false" instanceClassName="org.eclipse.emf.common.util.TreeIterator"/></e:EPackage>"#,
        )],
    }
}
fn contract(bundle: &DefinitionBundle, index: usize, selected: bool) -> ConversionContract {
    ConversionContract {
        datatype: DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![index],
        },
        provider: ProviderId {
            name: [
                "ecore.definition-resource",
                "ecore.definition-resource-set",
                "ecore.definition-tree-iterator",
            ][index]
                .into(),
            version: "1".into(),
        },
        serializable: false,
        declarations: if selected {
            vec![DeclarationId {
                source: bundle.sources[0].id.clone(),
                path: vec![],
            }]
        } else {
            vec![]
        },
        members: vec![],
    }
}
#[test]
fn catalog_providers_share_one_resource_tree_without_sharing_scope_or_lifetime() {
    // Preserve the standalone providers' native thread-safety auto-traits.
    fn send_sync<T: Send + Sync>() {}
    send_sync::<crate::resource_conversion::ResourceConverter>();
    send_sync::<crate::tree_iterator_conversion::TreeIteratorConverter>();
    let bundle = bundle();
    let mut budget = CatalogBudget::default();
    let (_, resource) =
        build_provider(&bundle, &contract(&bundle, 0, true), &mut budget, 0).unwrap();
    let (_, empty) = build_provider(&bundle, &contract(&bundle, 1, false), &mut budget, 0).unwrap();
    let (_, tree) = build_provider(&bundle, &contract(&bundle, 2, true), &mut budget, 0).unwrap();
    assert_eq!(budget.resources.len(), 1);
    assert_eq!(Arc::strong_count(&budget.resources[0].1), 4);
    assert!(budget.resources[0].0.sources[0].text.is_empty());
    let weak = Arc::downgrade(&budget.resources[0].1);
    let handle = json!({"resource":bundle.sources[0].id});
    resource.validate(&handle).unwrap();
    empty.validate(&json!({"resources":[]})).unwrap();
    assert!(
        empty
            .validate(&json!({"resources":[bundle.sources[0].id]}))
            .is_err()
    );
    tree.validate(
        &json!({"roots":contract(&bundle, 2, true).declarations,"position":0,"pruned":[]}),
    )
    .unwrap();
    // A nested policy scope may reuse immutable resources but selects its own providers.
    let context = ReferenceContext {
        bundle: bundle.clone(),
        entry: bundle.sources[0].id.clone(),
        namespace: "urn:shared".into(),
        conversions: vec![contract(&bundle, 1, true)],
        applied_conversions: vec![],
        constructors: vec![],
        materialized_types: vec![],
        model: json!(null),
    };
    with_reference_scope(&context, &mut budget, 0, |_| {
        assert_eq!(weak.strong_count(), 5);
        Ok(())
    })
    .unwrap();
    assert_eq!(budget.resources.len(), 1);
    assert_eq!(weak.strong_count(), 4);
    drop(budget);
    assert_eq!(weak.strong_count(), 3);
    resource.validate(&handle).unwrap();
    drop(resource);
    drop(empty);
    drop(tree);
    assert!(weak.upgrade().is_none());
}
#[test]
fn resource_cache_revalidates_content_and_separates_resolution_metadata() {
    let original = bundle();
    let mut budget = CatalogBudget::default();
    let first = budget.resources(&original).unwrap();
    assert!(Arc::ptr_eq(
        &first,
        &budget.resources(&original.clone()).unwrap()
    ));
    let mut forged = original.clone();
    forged.sources[0].text.push(' ');
    assert!(budget.resources(&forged).is_err());
    assert_eq!(budget.resources.len(), 1);
    let mut augmented = original.clone();
    augmented
        .sources
        .push(intersect_model::definitions::compiler::standard_datatype_source());
    assert!(!Arc::ptr_eq(&first, &budget.resources(&augmented).unwrap()));
    let mut dependencies = augmented.clone();
    dependencies.sources[0]
        .dependencies
        .push(augmented.sources[1].id.clone());
    let third = budget.resources(&dependencies).unwrap();
    assert!(!Arc::ptr_eq(&third, &budget.resources(&augmented).unwrap()));
    let mut namespaces = original.clone();
    namespaces.namespaces.push(NamespaceBinding {
        namespace: "urn:shared".into(),
        source: original.sources[0].id.clone(),
    });
    assert!(!Arc::ptr_eq(
        &first,
        &budget.resources(&namespaces).unwrap()
    ));
    assert_eq!(budget.resources.len(), 4);
    assert!(Arc::ptr_eq(&first, &budget.resources(&original).unwrap()));
    let mut invalid = original.clone();
    invalid.version = 999;
    assert!(budget.resources(&invalid).is_err());
    assert_eq!(budget.resources.len(), 4);
}

#[test]
fn reference_context_admission_counts_exact_escaped_bytes_before_building() {
    let bundle = bundle();
    let context = ReferenceContext {
        entry: bundle.sources[0].id.clone(),
        bundle,
        namespace: "urn:shared".into(),
        conversions: vec![],
        applied_conversions: vec![],
        constructors: vec![],
        materialized_types: vec![],
        model: json!({"escaped": "\"\n\\", "unicode": "é"}),
    };
    let bytes = serde_json::to_vec(&context).unwrap().len();
    for admitted in [0, bytes - 1, bytes, bytes + 1] {
        let mut budget = CatalogBudget {
            context_bytes: admitted,
            ..Default::default()
        };
        let called = std::cell::Cell::new(false);
        let result = with_reference_scope(&context, &mut budget, 0, |_| {
            called.set(true);
            Ok(())
        });
        assert_eq!(called.get(), admitted >= bytes);
        if admitted < bytes {
            assert_eq!(
                result.unwrap_err(),
                "Reference context byte budget exceeded (16 MiB)"
            );
            assert_eq!(budget.context_bytes, admitted);
            assert!(budget.resources.is_empty());
        } else {
            result.unwrap();
            assert_eq!(budget.context_bytes, admitted - bytes);
        }
    }
}

#[test]
fn constructor_admission_checks_exact_bytes_before_retaining_provider() {
    let bundle = DefinitionBundle {
        version: 1,
        namespaces: vec![],
        sources: vec![DefinitionSource::new(
            "memory:list.ecore",
            SourceKind::Ecore,
            r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="p" nsURI="urn:list"><eClassifiers x:type="e:EDataType" name="List" serializable="false" instanceClassName="org.eclipse.emf.common.util.EList"><eTypeParameters name="T"/></eClassifiers></e:EPackage>"#,
        )],
    };
    let datatype = intersect_model::definitions::compiler::import_datatype_bundle(
        &bundle,
        &DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![0],
        },
    )
    .unwrap();
    let contract = intersect_model::definitions::conversion::ConstructorContract {
        datatype,
        provider: ProviderId {
            name: "ecore.list.constructor".into(),
            version: "1".into(),
        },
    };
    let bytes = serde_json::to_vec(&contract).unwrap().len();
    for admitted in [0, bytes - 1, bytes, bytes + 1] {
        let mut budget = CatalogBudget {
            context_bytes: admitted,
            ..Default::default()
        };
        let result = build_catalog_scope(
            &bundle,
            CatalogSelections {
                contracts: &[],
                applications: &[],
                constructors: std::slice::from_ref(&contract),
            },
            &mut budget,
            0,
        );
        if admitted < bytes {
            assert_eq!(
                result.err().unwrap(),
                "Constructor context byte budget exceeded (16 MiB)"
            );
            assert!(budget.constructors.is_empty());
            assert_eq!(budget.context_bytes, admitted);
        } else {
            assert!(result.is_ok());
            assert_eq!(budget.constructors.len(), 1);
            assert_eq!(budget.context_bytes, admitted - bytes);
        }
    }
}
