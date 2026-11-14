use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        model_types::ModelTypes,
        types::{Scalar, Type, TypeTable},
    },
    resources::DeclarationId,
};

fn fixture() -> (DefinitionBundle, Vec<DeclarationId>) {
    let source = DefinitionSource::new(
        "memory:model.rcore",
        SourceKind::Rcore,
        r#"rcore 1; package model version "1";
metamodel "urn:model" prefix model {
 class Root {
  attr name: EString [1,1]; attr count: EInt [0,1];
  ref children: Root [0,*] unordered nonunique; ref parent: Root [0,1];
  attr flag: EBoolean [1,1] derived;
 }
 class Left extends Root {} class Right extends Root {}
 class Leaf extends Left, Right {} class OtherLeaf extends Left, Right {}
 class Unrelated {}
}"#,
    );
    let classes = (0..6)
        .map(|i| DeclarationId {
            source: source.id.clone(),
            path: vec![i],
        })
        .collect();
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        classes,
    )
}

#[test]
fn model_conformance_and_join_reuse_all_parents_without_choosing_ambiguous_branches() {
    let (bundle, c) = fixture();
    let mut model = ModelTypes::new(&bundle).unwrap();
    for parent in &c[..4] {
        assert!(model.conforms(&c[3], parent).unwrap());
    }
    assert!(!model.conforms(&c[0], &c[3]).unwrap());
    assert!(!model.conforms(&c[1], &c[2]).unwrap());
    assert_eq!(model.common_supertype(&c[1], &c[2]).unwrap(), c[0]);
    assert_eq!(model.common_supertype(&c[3], &c[2]).unwrap(), c[2]);
    assert!(
        model
            .common_supertype(&c[3], &c[4])
            .unwrap_err()
            .contains("Ambiguous")
    );
    assert!(
        model
            .common_supertype(&c[3], &c[5])
            .unwrap_err()
            .contains("no common")
    );
    let mut fake = c[0].clone();
    fake.path = vec![99];
    assert!(model.conforms(&fake, &fake).is_err());
}

#[test]
fn inherited_reads_retain_declaring_feature_identity_and_multiplicity() {
    let (bundle, c) = fixture();
    let mut model = ModelTypes::new(&bundle).unwrap();
    let mut table = TypeTable::default();
    let name = model.feature(&mut table, &c[3], "name").unwrap();
    let declared = model.feature(&mut table, &c[0], "name").unwrap();
    assert_eq!(name.declaration, declared.declaration);
    assert_eq!(name.owner, declared.owner);
    assert_eq!(name.value, declared.value);
    assert_eq!(name.derived, declared.derived);
    assert_eq!(name.domains, declared.domains);
    assert_eq!(name.assignment_exact, declared.assignment_exact);
    let receiver =
        |id: &DeclarationId| intersect_model::definitions::generic_types::Type::Classifier {
            declaration: id.clone(),
            arguments: vec![],
            raw: false,
        };
    assert_eq!(name.receiver.expression(), &receiver(&c[3]));
    assert_eq!(declared.receiver.expression(), &receiver(&c[0]));
    assert_eq!(name.owner.expression(), &receiver(&c[0]));
    assert_ne!(name.receiver, declared.receiver);
    assert!(!name.schema_revision.is_empty());
    assert!(!declared.schema_revision.is_empty());
    assert_eq!(name.declaration.path, vec![0, 0]);
    assert_eq!(
        table.get(name.value).unwrap(),
        &Type::Scalar(Scalar::String)
    );
    let count = model.feature(&mut table, &c[3], "count").unwrap();
    let Type::Option(int) = table.get(count.value).unwrap() else {
        panic!("optional")
    };
    assert_eq!(table.get(*int).unwrap(), &Type::Scalar(Scalar::Int32));
    let children = model.feature(&mut table, &c[3], "children").unwrap();
    let Type::Many {
        element,
        lower,
        upper,
        ordered,
        unique,
    } = table.get(children.value).unwrap()
    else {
        panic!("many")
    };
    assert_eq!((*lower, *upper, *ordered, *unique), (0, -1, false, false));
    assert_eq!(table.get(*element).unwrap(), &Type::Model(c[0].clone()));
    let parent = model.feature(&mut table, &c[3], "parent").unwrap();
    assert!(matches!(table.get(parent.value).unwrap(), Type::Option(_)));
    let flag = model.feature(&mut table, &c[3], "flag").unwrap();
    assert!(flag.derived);
    assert_eq!(table.get(flag.value).unwrap(), &Type::Scalar(Scalar::Bool));
    assert!(model.feature(&mut table, &c[3], "missing").is_err());
}

#[test]
fn assignability_checks_wrappers_functions_and_cardinality_without_implicit_lifts() {
    let (bundle, c) = fixture();
    let mut model = ModelTypes::new(&bundle).unwrap();
    let mut t = TypeTable::default();
    let root = t.intern(Type::Model(c[0].clone())).unwrap();
    let leaf = t.intern(Type::Model(c[3].clone())).unwrap();
    let int = t.intern(Type::Scalar(Scalar::Int32)).unwrap();
    let long = t.intern(Type::Scalar(Scalar::Int64)).unwrap();
    let optional_root = t.intern(Type::Option(root)).unwrap();
    let optional_leaf = t.intern(Type::Option(leaf)).unwrap();
    let result_root = t
        .intern(Type::Result {
            ok: root,
            error: int,
        })
        .unwrap();
    let result_leaf = t
        .intern(Type::Result {
            ok: leaf,
            error: int,
        })
        .unwrap();
    assert!(model.assignable(&t, optional_leaf, optional_root).unwrap());
    assert!(model.assignable(&t, result_leaf, result_root).unwrap());
    for (a, b) in [
        (optional_root, optional_leaf),
        (leaf, optional_root),
        (optional_leaf, root),
        (result_leaf, optional_root),
        (result_leaf, leaf),
        (int, long),
    ] {
        assert!(!model.assignable(&t, a, b).unwrap());
    }
    let general = t
        .intern(Type::Function {
            parameters: vec![root],
            result: leaf,
        })
        .unwrap();
    let narrow = t
        .intern(Type::Function {
            parameters: vec![leaf],
            result: root,
        })
        .unwrap();
    assert!(model.assignable(&t, general, narrow).unwrap());
    assert!(!model.assignable(&t, narrow, general).unwrap());
    let broad = t
        .intern(Type::Many {
            element: root,
            lower: 0,
            upper: -1,
            ordered: true,
            unique: true,
        })
        .unwrap();
    let bounded = t
        .intern(Type::Many {
            element: leaf,
            lower: 2,
            upper: 4,
            ordered: true,
            unique: true,
        })
        .unwrap();
    let bag = t
        .intern(Type::Many {
            element: leaf,
            lower: 2,
            upper: 4,
            ordered: false,
            unique: false,
        })
        .unwrap();
    assert!(model.assignable(&t, bounded, broad).unwrap());
    assert!(!model.assignable(&t, broad, bounded).unwrap());
    assert!(!model.assignable(&t, bag, broad).unwrap());
    assert!(!model.assignable(&t, bounded, root).unwrap());
    assert!(!model.assignable(&t, bounded, optional_root).unwrap());
}

#[test]
fn same_name_revisions_remain_distinct_and_context_edits_match_fresh_checks() {
    let (mut bundle, c) = fixture();
    let changed = DefinitionSource::new(
        "memory:model.rcore",
        SourceKind::Rcore,
        bundle.sources[0]
            .text
            .replace("class Root", "abstract class Root"),
    );
    let new_root = DeclarationId {
        source: changed.id.clone(),
        path: vec![0],
    };
    let new_leaf = DeclarationId {
        source: changed.id.clone(),
        path: vec![3],
    };
    bundle.sources.push(changed.clone());
    let mut together = ModelTypes::new(&bundle).unwrap();
    assert!(!together.conforms(&c[3], &new_root).unwrap());
    assert!(!together.conforms(&new_leaf, &c[0]).unwrap());
    assert!(together.conforms(&new_leaf, &new_root).unwrap());
    let fresh_bundle = DefinitionBundle {
        version: 1,
        sources: vec![changed],
        namespaces: vec![],
    };
    let mut fresh = ModelTypes::new(&fresh_bundle).unwrap();
    assert_eq!(
        together.common_supertype(&new_leaf, &new_root).unwrap(),
        fresh.common_supertype(&new_leaf, &new_root).unwrap()
    );
    let mut old_table = TypeTable::default();
    let mut new_table = TypeTable::default();
    let together_feature = together.feature(&mut old_table, &new_leaf, "name").unwrap();
    let fresh_feature = fresh.feature(&mut new_table, &new_leaf, "name").unwrap();
    assert_ne!(
        together_feature.schema_revision,
        fresh_feature.schema_revision
    );
    assert_eq!(together_feature.declaration, fresh_feature.declaration);
    assert_eq!(together_feature.receiver, fresh_feature.receiver);
    assert_eq!(together_feature.owner, fresh_feature.owner);
    assert_eq!(together_feature.domains, fresh_feature.domains);
    assert_eq!(
        together_feature.assignment_exact,
        fresh_feature.assignment_exact
    );
    assert_eq!(together_feature.derived, fresh_feature.derived);
    assert_eq!(together_feature.value, fresh_feature.value);
    assert_eq!(
        old_table.get(together_feature.value).unwrap(),
        new_table.get(fresh_feature.value).unwrap()
    );
    let mut rebuilt = ModelTypes::new(&bundle).unwrap();
    assert_eq!(
        together_feature,
        rebuilt
            .feature(&mut TypeTable::default(), &new_leaf, "name")
            .unwrap()
    );
}

#[test]
fn external_parent_feature_identity_and_invalid_graphs_use_shared_ecore_rules() {
    let (base_bundle, _) = fixture();
    let base = DefinitionSource::new(
        "file:///types/base.rcore",
        SourceKind::Rcore,
        base_bundle.sources[0].text.clone(),
    );
    let mut child = DefinitionSource::new(
        "file:///types/child.rcore",
        SourceKind::Rcore,
        r##"rcore 1; package child version "1"; metamodel "urn:child" prefix child { class Root extends external "base.rcore#//Leaf" {} }"##,
    );
    child.dependencies.push(base.id.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![child.clone(), base.clone()],
        namespaces: vec![],
    };
    let mut model = ModelTypes::new(&bundle).unwrap();
    let child_id = DeclarationId {
        source: child.id,
        path: vec![0],
    };
    let base_id = DeclarationId {
        source: base.id.clone(),
        path: vec![0],
    };
    assert!(model.conforms(&child_id, &base_id).unwrap());
    assert!(!model.conforms(&base_id, &child_id).unwrap());
    let mut table = TypeTable::default();
    let name = model.feature(&mut table, &child_id, "name").unwrap();
    assert_eq!(
        name.declaration,
        DeclarationId {
            source: base.id,
            path: vec![0, 0]
        }
    );
    for (old, new, expected) in [
        ("class Root {", "class Root extends Leaf {", "Cyclic"),
        (
            "class Left extends Root {}",
            "class Left extends Root { attr name: EString [0,1]; }",
            "redefinition",
        ),
    ] {
        let invalid = DefinitionSource::new(
            "memory:invalid.rcore",
            SourceKind::Rcore,
            base_bundle.sources[0].text.replace(old, new),
        );
        let invalid_class = DeclarationId {
            source: invalid.id.clone(),
            path: vec![3],
        };
        let invalid_bundle = DefinitionBundle {
            version: 1,
            sources: vec![invalid],
            namespaces: vec![],
        };
        let error = ModelTypes::new(&invalid_bundle)
            .unwrap()
            .feature(&mut TypeTable::default(), &invalid_class, "name")
            .unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn compound_joins_preserve_wrappers_bounds_and_ambiguity() {
    let (bundle, c) = fixture();
    let mut model = ModelTypes::new(&bundle).unwrap();
    let mut table = TypeTable::default();
    let root = table.intern(Type::Model(c[0].clone())).unwrap();
    let left = table.intern(Type::Model(c[1].clone())).unwrap();
    let right = table.intern(Type::Model(c[2].clone())).unwrap();
    let optional_root = table.intern(Type::Option(root)).unwrap();
    let optional_left = table.intern(Type::Option(left)).unwrap();
    let optional_right = table.intern(Type::Option(right)).unwrap();
    assert_eq!(
        model
            .join(&mut table, optional_left, optional_right)
            .unwrap(),
        optional_root
    );
    assert_eq!(
        model
            .join(&mut table, optional_right, optional_left)
            .unwrap(),
        optional_root
    );
    let result_left = table
        .intern(Type::Result {
            ok: optional_left,
            error: right,
        })
        .unwrap();
    let result_right = table
        .intern(Type::Result {
            ok: optional_right,
            error: left,
        })
        .unwrap();
    let expected = table
        .intern(Type::Result {
            ok: optional_root,
            error: root,
        })
        .unwrap();
    assert_eq!(
        model.join(&mut table, result_left, result_right).unwrap(),
        expected
    );
    let a = table
        .intern(Type::Many {
            element: left,
            lower: 2,
            upper: 4,
            ordered: true,
            unique: true,
        })
        .unwrap();
    let b = table
        .intern(Type::Many {
            element: right,
            lower: 0,
            upper: -1,
            ordered: true,
            unique: true,
        })
        .unwrap();
    let expected = table
        .intern(Type::Many {
            element: root,
            lower: 0,
            upper: -1,
            ordered: true,
            unique: true,
        })
        .unwrap();
    assert_eq!(model.join(&mut table, a, b).unwrap(), expected);
    let a = table
        .intern(Type::Function {
            parameters: vec![root],
            result: optional_left,
        })
        .unwrap();
    let b = table
        .intern(Type::Function {
            parameters: vec![root],
            result: optional_right,
        })
        .unwrap();
    let expected = table
        .intern(Type::Function {
            parameters: vec![root],
            result: optional_root,
        })
        .unwrap();
    assert_eq!(model.join(&mut table, a, b).unwrap(), expected);
    let mut deep_left = left;
    let mut deep_right = right;
    let mut deep_root = root;
    for _ in 1..64 {
        deep_left = table.intern(Type::Option(deep_left)).unwrap();
        deep_right = table.intern(Type::Option(deep_right)).unwrap();
        deep_root = table.intern(Type::Option(deep_root)).unwrap();
    }
    assert_eq!(
        model.join(&mut table, deep_left, deep_right).unwrap(),
        deep_root
    );
    let leaf = table.intern(Type::Model(c[3].clone())).unwrap();
    let other = table.intern(Type::Model(c[4].clone())).unwrap();
    let leaf = table.intern(Type::Option(leaf)).unwrap();
    let other = table.intern(Type::Option(other)).unwrap();
    assert!(
        model
            .join(&mut table, leaf, other)
            .unwrap_err()
            .contains("Ambiguous")
    );
    assert!(model.join(&mut table, optional_left, right).is_err());
}
