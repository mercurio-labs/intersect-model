//! Source-program acceptance for symbolic Ecore typing, not symbolic instances.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        SyntaxDiagnostic,
        checking::{TypeReport, check},
        types::Type,
    },
};
const MODEL: &str = r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="symbolic" nsURI="urn:symbolic">
<eClassifiers x:type="e:EClass" name="Base"/>
<eClassifiers x:type="e:EClass" name="Leaf" eSuperTypes="#//Base"/>
<eClassifiers x:type="e:EClass" name="Other"/>
<eClassifiers x:type="e:EClass" name="Box" eSuperTypes="#//Base"><eTypeParameters name="T"><eBounds eClassifier="#//Base"/></eTypeParameters><eStructuralFeatures x:type="e:EReference" name="value" lowerBound="1"><eGenericType eTypeParameter="#//Box/T"/></eStructuralFeatures><eStructuralFeatures x:type="e:EReference" name="optional"><eGenericType eTypeParameter="#//Box/T"/></eStructuralFeatures><eStructuralFeatures x:type="e:EReference" name="items" upperBound="-1" ordered="true" unique="false"><eGenericType eTypeParameter="#//Box/T"/></eStructuralFeatures></eClassifiers>
<eClassifiers x:type="e:EClass" name="Parent"><eTypeParameters name="T"><eBounds eClassifier="#//Base"/></eTypeParameters><eGenericSuperTypes eClassifier="#//Box"><eTypeArguments eTypeParameter="#//Parent/T"/></eGenericSuperTypes></eClassifiers>
<eClassifiers x:type="e:EClass" name="Child"><eTypeParameters name="U"><eBounds eClassifier="#//Base"/></eTypeParameters><eGenericSuperTypes eClassifier="#//Parent"><eTypeArguments eTypeParameter="#//Child/U"/></eGenericSuperTypes></eClassifiers>
<eClassifiers x:type="e:EClass" name="Tight"><eTypeParameters name="T"><eBounds eClassifier="#//Leaf"/></eTypeParameters></eClassifiers>
</e:EPackage>"##;
fn semantic(name: &str, body: &str, model: &DefinitionSource) -> DefinitionSource {
    let mut source = DefinitionSource::new(
        format!("memory:{name}-behavior.rcore"),
        SourceKind::RcoreBehavior,
        format!(
            "rcore 1; package {name} version \"1\"; import model \"urn:symbolic\" as m; {body}"
        ),
    );
    source.dependencies.push(model.id.clone());
    source
}
fn fixture(body: &str) -> (DefinitionBundle, DefinitionSource, DefinitionSource) {
    let model = DefinitionSource::new("memory:symbolic.ecore", SourceKind::Ecore, MODEL);
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    let entry = semantic("symbolic", body, &model);
    (
        DefinitionBundle {
            version: 1,
            sources: vec![model.clone(), entry.clone()],
            namespaces: vec![],
        },
        model,
        entry,
    )
}
fn run(body: &str) -> Result<TypeReport, Vec<SyntaxDiagnostic>> {
    let (bundle, _, entry) = fixture(body);
    check(&bundle, &entry.id)
}
fn good(body: &str) -> TypeReport {
    run(body).unwrap_or_else(|errors| panic!("{body}: {errors:?}"))
}
fn bad(body: &str) {
    let (bundle, _, entry) = fixture(body);
    let errors = check(&bundle, &entry.id).unwrap_err();
    assert!(!errors.is_empty());
    assert!(
        errors
            .iter()
            .all(|e| e.span.source == entry.id && e.span.start < e.span.end),
        "{body}: {errors:?}"
    );
}

#[test]
fn symbolic_feature_reads_preserve_scoped_formals_instead_of_first_bound_erasure() {
    let report = good(
        "fn first<T:m.Base>(x:m.Box<T>) -> T = x.value; fn second<T:m.Base>(x:m.Box<T>) -> T = x.value; fn nested<T:m.Base>(x:m.Box<m.Box<T>>) -> T = x.value.value;",
    );
    let functions = &report.signatures.functions;
    let first = functions[0].generics[0].ty;
    let second = functions[1].generics[0].ty;
    assert_ne!(first, second);
    assert_eq!(functions[0].result, first);
    assert_eq!(functions[1].result, second);
    assert!(matches!(
        report.signatures.types.get(first).unwrap(),
        Type::Parameter { .. }
    ));
    for (function, formal) in [(&functions[0], first), (&functions[1], second)] {
        let Type::SymbolicModelApplication {
            declaration,
            arguments,
        } = report.signatures.types.get(function.parameters[0]).unwrap()
        else {
            panic!("Expected distinct symbolic model application")
        };
        assert_eq!(declaration.path, vec![3]);
        assert_eq!(arguments, &vec![formal]);
    }
    bad("fn wrong<T:m.Base,U:m.Base>(x:m.Box<T>) -> U = x.value;");
    bad("fn erased<T:m.Base>(x:m.Box<m.Base>) -> T = x.value;");
}

#[test]
fn inherited_symbolic_owners_preserve_multiplicity_and_concrete_call_validation() {
    let report = good(
        "fn read<T:m.Base>(x:m.Child<T>) -> T = x.value; fn optional<T:m.Base>(x:m.Child<T>) -> T? = x.optional; fn items<T:m.Base>(x:m.Child<T>) -> Sequence<T> = x.items; fn parent<T:m.Base>(x:m.Child<T>) -> m.Parent<T> = x; fn concrete(x:m.Child<m.Leaf>) -> m.Leaf = read(x); fn explicit(x:m.Child<m.Leaf>) -> m.Leaf = read::<m.Leaf>(x);",
    );
    let members = &report.modules[0].symbolic_members;
    assert_eq!(members.len(), 3);
    for (function_index, field_index) in [(0, 1), (1, 2), (2, 3)] {
        let signature = &report.signatures.functions[function_index];
        let feature = members
            .values()
            .find(|f| f.declaration.path == vec![3, field_index - 1])
            .unwrap();
        assert!(feature.reference);
        assert!(!feature.derived);
        let intersect_model::definitions::generic_types::Type::Classifier {
            declaration: receiver,
            arguments: receiver_arguments,
            raw: false,
        } = &feature.receiver
        else {
            panic!("Expected symbolic Child receiver")
        };
        let intersect_model::definitions::generic_types::Type::Classifier {
            declaration: owner,
            arguments: owner_arguments,
            raw: false,
        } = &feature.owner
        else {
            panic!("Expected exact inherited Box owner")
        };
        assert_eq!(receiver.path, vec![5]);
        assert_eq!(owner.path, vec![3]);
        assert_eq!(feature.declaration.source, owner.source);
        assert_eq!(receiver_arguments, owner_arguments);
        let intersect_model::definitions::generic_types::Type::RigidParameter { identity } =
            &owner_arguments[0]
        else {
            panic!("An inherited symbolic owner must not erase the formal")
        };
        assert_eq!(identity.source, signature.symbol.source);
        assert_eq!(identity.declaration, signature.symbol.declaration);
        assert_eq!(identity.parameter, 0);
        let formal = signature.generics[0].ty;
        match field_index {
            1 => assert_eq!(feature.value, formal),
            2 => assert_eq!(
                report.signatures.types.get(feature.value).unwrap(),
                &Type::Option(formal)
            ),
            3 => assert!(
                matches!(report.signatures.types.get(feature.value).unwrap(),Type::Many{element,lower:0,upper:-1,ordered:true,unique:false} if *element==formal)
            ),
            _ => unreachable!(),
        }
    }

    bad("fn optional<T:m.Base>(x:m.Child<T>) -> T = x.optional;");
    bad("fn items<T:m.Base>(x:m.Child<T>) -> T = x.items;");
    bad("fn invariant<T:m.Base>(x:m.Child<T>) -> m.Parent<m.Base> = x;");
    bad(
        "fn read<T:m.Base>(x:m.Box<T>) -> T = x.value; fn invariant(x:m.Box<m.Leaf>) -> m.Base = read::<m.Base>(x);",
    );
}

#[test]
fn nominal_kind_bounds_and_exact_constructor_arguments_are_universal_obligations() {
    good(
        "fn bounded<T:m.Leaf>(x:m.Tight<T>) -> m.Tight<T> = x; fn earlier<T:m.Base,U:T>(x:m.Box<U>) -> U = x.value;",
    );
    for body in [
        "fn unconstrained<T>(x:m.Box<T>) -> T = x.value;",
        "fn structural<T:Sequence<Int>>(x:m.Box<T>) -> T = x.value;",
        "fn scalar<T:Int>(x:m.Box<T>) -> T = x.value;",
        "fn insufficient<T:m.Base>(x:m.Tight<T>) -> m.Tight<T> = x;",
        "fn wrong<T:m.Other>(x:m.Box<T>) -> T = x.value;",
        "fn wrong(x:m.Box<Sequence<Int>>) -> Int = 1;",
        "fn wrong<T:m.Base>(x:m.Box<T,T>) -> T = x.value;",
    ] {
        bad(body);
    }
}

#[test]
fn symbolic_source_replay_is_deterministic_and_model_revision_changes_are_rechecked() {
    let body = "fn read<T:m.Base>(x:m.Child<T>) -> T = x.value; fn concrete(x:m.Child<m.Leaf>) -> m.Leaf = read(x);";
    let (bundle, model, entry) = fixture(body);
    let first = check(&bundle, &entry.id).unwrap();
    let mut replay: DefinitionBundle =
        serde_json::from_value(serde_json::to_value(&bundle).unwrap()).unwrap();
    replay.sources.reverse();
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(check(&replay, &entry.id).unwrap()).unwrap()
    );
    let changed = DefinitionSource::new(
        &model.id.uri,
        SourceKind::Ecore,
        MODEL.replace(
            "name=\"value\" lowerBound=\"1\"",
            "name=\"value\" lowerBound=\"0\"",
        ),
    );
    let (changed, _) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(changed);
    assert_ne!(model.id, changed.id);
    let fresh = semantic("symbolic", body, &changed);
    let invalid = DefinitionBundle {
        version: 1,
        sources: vec![changed.clone(), fresh.clone()],
        namespaces: vec![],
    };
    let errors = check(&invalid, &fresh.id).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.span.source == fresh.id && e.span.start < e.span.end),
        "{errors:?}"
    );
    let stale = DefinitionBundle {
        version: 1,
        sources: vec![changed, entry.clone()],
        namespaces: vec![],
    };
    assert!(check(&stale, &entry.id).is_err());
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(check(&bundle, &entry.id).unwrap()).unwrap()
    );
}

#[test]
fn imported_symbolic_functions_keep_same_named_formals_scoped_to_their_sources() {
    let model = DefinitionSource::new("memory:symbolic.ecore", SourceKind::Ecore, MODEL);
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    let helper = semantic(
        "helper",
        "pub fn read<T:m.Base>(x:m.Box<T>) -> T = x.value;",
        &model,
    );
    let mut entry = semantic(
        "entry",
        "import package helper version \"1\" as h; fn read<T:m.Base>(x:m.Box<T>) -> T = h.read(x); fn concrete(x:m.Box<m.Leaf>) -> m.Leaf = h.read(x);",
        &model,
    );
    entry.dependencies.push(helper.id.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![model, helper.clone(), entry.clone()],
        namespaces: vec![],
    };
    let report = check(&bundle, &entry.id).unwrap();
    let generics: Vec<_> = report
        .signatures
        .functions
        .iter()
        .filter_map(|f| f.generics.first())
        .map(|g| g.ty)
        .collect();
    assert_eq!(generics.len(), 2);
    assert_ne!(generics[0], generics[1]);
    let mut reversed = bundle.clone();
    reversed.sources.reverse();
    assert_eq!(
        serde_json::to_value(report).unwrap(),
        serde_json::to_value(check(&reversed, &entry.id).unwrap()).unwrap()
    );
}

#[test]
fn symbolic_box_callbacks_and_nominal_application_bounds_retain_exact_arguments() {
    good(
        "fn identity<U:m.Base>(x:m.Box<U>) -> m.Box<U> = x; fn callback<T:m.Base>(x:m.Box<T>) -> m.Box<T> = { let f:(m.Box<T>)->m.Box<T> = identity; f(x) };",
    );
    good(
        "fn read<S:m.Base,T:m.Box<S>>(x:T) -> S = x.value; fn concrete(x:m.Child<m.Leaf>) -> m.Leaf = read(x);",
    );
    bad(
        "fn identity<U:m.Base>(x:m.Box<U>) -> m.Box<U> = x; fn wrong<T:m.Base>(x:m.Box<T>) -> m.Box<T> = { let f:(m.Box<T>)->m.Box<m.Base> = identity; x };",
    );
    bad("fn wrong<S:m.Base,T:m.Box<S>,U:m.Base>(x:T) -> U = x.value;");
    bad("fn fabricate<S:m.Base,T:m.Box<S>>(x:m.Box<S>) -> T = x;");
}

#[test]
fn symbolic_nesting_and_constraint_work_are_bounded_and_fresh_checks_recover() {
    fn nested(depth: usize) -> String {
        let mut ty = "T".to_owned();
        for _ in 0..depth {
            ty = format!("m.Box<{ty}>");
        }
        format!("fn nested<T:m.Base>(x:{ty}) -> {ty} = x;")
    }
    fn chain(count: usize) -> String {
        let mut bounds = vec!["T0:m.Base".to_owned()];
        bounds.extend((1..count).map(|n| format!("T{n}:T{}", n - 1)));
        format!(
            "fn chain<{}>(x:m.Box<T{}>) -> m.Box<T{}> = x;",
            bounds.join(","),
            count - 1,
            count - 1
        )
    }
    good(&nested(12));
    good(&chain(24));
    for (body, need_constraint_limit) in [(nested(80), false), (chain(140), true)] {
        let (bundle, _, entry) = fixture(&body);
        let errors = check(&bundle, &entry.id).unwrap_err();
        assert!(!errors.is_empty());
        assert!(
            errors
                .iter()
                .any(|e| e.message.to_lowercase().contains("limit")
                    || e.message.to_lowercase().contains("budget")
                    || e.message.to_lowercase().contains("nesting")),
            "{errors:?}"
        );
        // The source frontend rejects this chain before the deeper shared
        // resolver ceiling. Do not claim this reaches the 128-level guard.
        if need_constraint_limit {
            assert!(
                errors
                    .iter()
                    .any(|e| e.message == "Nominal generic bound nesting exceeds 64 levels"),
                "{errors:?}"
            );
        }
        assert!(
            errors.iter().all(|e| e.span.source == entry.id),
            "{errors:?}"
        );
        assert_eq!(
            serde_json::to_value(&errors).unwrap(),
            serde_json::to_value(check(&bundle, &entry.id).unwrap_err()).unwrap()
        );
    }
    good(&nested(12));
    good(&chain(24));
}

#[test]
fn same_uri_semantic_edits_refresh_rigid_scope_and_utf8_error_locations() {
    let body = "fn read<T:m.Base>(x:m.Box<T>) -> T = x.value;";
    let (original, model, entry) = fixture(body);
    let before = check(&original, &entry.id).unwrap();
    let edited = semantic(
        "symbolic",
        &format!("// Unicode π changes the exact source revision.\n{body}"),
        &model,
    );
    assert_eq!(entry.id.uri, edited.id.uri);
    assert_ne!(entry.id.sha256, edited.id.sha256);
    let fresh = DefinitionBundle {
        version: 1,
        sources: vec![model.clone(), edited.clone()],
        namespaces: vec![],
    };
    let after = check(&fresh, &edited.id).unwrap();
    let a = &before.signatures.functions[0];
    let b = &after.signatures.functions[0];
    assert_eq!(a.symbol.source, entry.id);
    assert_eq!(b.symbol.source, edited.id);
    assert_ne!(a.symbol, b.symbol);
    let Type::Parameter { symbol, .. } = after.signatures.types.get(b.generics[0].ty).unwrap()
    else {
        panic!("Expected exact scoped formal")
    };
    assert_eq!(symbol, &b.symbol);
    assert!(check(&fresh, &entry.id).is_err());
    let invalid = semantic(
        "symbolic",
        "// π\nfn read<T:m.Base>(x:m.Box<T>) -> Bool = x.value;",
        &model,
    );
    let invalid_bundle = DefinitionBundle {
        version: 1,
        sources: vec![model, invalid.clone()],
        namespaces: vec![],
    };
    let errors = check(&invalid_bundle, &invalid.id).unwrap_err();
    assert!(
        errors.iter().any(|e| e.span.source == invalid.id
            && invalid
                .text
                .get(e.span.start..e.span.end)
                .is_some_and(|fragment| fragment.contains("x.value"))),
        "{errors:?}"
    );
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(check(&original, &entry.id).unwrap()).unwrap()
    );
}

#[test]
fn invalid_imported_nominal_bound_reports_the_helper_source_and_bound_span() {
    let model = DefinitionSource::new("memory:symbolic.ecore", SourceKind::Ecore, MODEL);
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    let helper = semantic(
        "helper",
        "// π keeps this a UTF-8 source-location control.\npub fn invalid<T:m.Tight<m.Base>>(x:T) -> T = x;",
        &model,
    );
    let mut entry = semantic(
        "entry",
        "import package helper version \"1\" as h; fn main() -> Int = 1;",
        &model,
    );
    entry.dependencies.push(helper.id.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![entry.clone(), helper.clone(), model],
        namespaces: vec![],
    };
    let errors = check(&bundle, &entry.id).unwrap_err();
    assert!(!errors.is_empty());
    assert!(
        errors
            .iter()
            .all(|e| e.span.source == helper.id && e.span.start < e.span.end),
        "{errors:?}"
    );
    assert!(
        errors.iter().any(|e| helper
            .text
            .get(e.span.start..e.span.end)
            .is_some_and(|text| text.contains("m.Tight<m.Base>"))),
        "{errors:?}"
    );
    let mut reverse = bundle.clone();
    reverse.sources.reverse();
    assert_eq!(
        serde_json::to_value(&errors).unwrap(),
        serde_json::to_value(check(&reverse, &entry.id).unwrap_err()).unwrap()
    );
}

#[test]
fn symbolic_owner_primitive_feature_is_not_misclassified_as_a_generic_argument() {
    let marker =
        "</eStructuralFeatures></eClassifiers>\n<eClassifiers x:type=\"e:EClass\" name=\"Parent\">";
    let replacement = "</eStructuralFeatures><eStructuralFeatures x:type=\"e:EAttribute\" name=\"count\" lowerBound=\"1\" eType=\"e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EInt\"/></eClassifiers>\n<eClassifiers x:type=\"e:EClass\" name=\"Parent\">";
    assert!(MODEL.contains(marker));
    let model = DefinitionSource::new(
        "memory:symbolic.ecore",
        SourceKind::Ecore,
        MODEL.replace(marker, replacement),
    );
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    for (body, valid) in [
        (
            "fn count<T:m.Base>(x:m.Box<T>) -> Int32 = x.count; fn inherited<T:m.Base>(x:m.Child<T>) -> Int32 = x.count; fn concrete(x:m.Child<m.Leaf>) -> Int32 = count(x);",
            true,
        ),
        ("fn fabricate<T:m.Base>(x:m.Box<T>) -> T = x.count;", false),
    ] {
        let entry = semantic("primitive", body, &model);
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![model.clone(), entry.clone()],
            namespaces: vec![],
        };
        let result = check(&bundle, &entry.id);
        if valid {
            assert!(result.is_ok(), "{body}: {result:?}");
        } else {
            let errors = result.unwrap_err();
            assert!(
                errors
                    .iter()
                    .all(|e| e.span.source == entry.id && e.span.start < e.span.end),
                "{errors:?}"
            );
        }
    }
}

#[test]
fn inherited_symbolic_and_closed_reads_preserve_all_independent_behavior_flags() {
    for mask in 0..32 {
        let flags = [
            "derived",
            "changeable",
            "transient",
            "volatile",
            "unsettable",
        ];
        let attributes = flags
            .iter()
            .enumerate()
            .map(|(bit, flag)| format!(" {flag}=\"{}\"", mask & (1 << bit) != 0))
            .collect::<String>();
        let model = DefinitionSource::new(
            "memory:symbolic.ecore",
            SourceKind::Ecore,
            MODEL.replace("name=\"value\"", &format!("name=\"value\"{attributes}")),
        );
        let (model, _) =
            intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
        let entry = semantic(
            "flags",
            "fn symbolic<T:m.Base>(x:m.Child<T>) -> T = x.value; fn closed(x:m.Child<m.Leaf>) -> m.Leaf = x.value; fn raw(x:m.Child) -> m.Base = x.value;",
            &model,
        );
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![model.clone(), entry.clone()],
            namespaces: vec![],
        };
        let report = check(&bundle, &entry.id).unwrap();
        let module = &report.modules[0];
        assert_eq!(module.symbolic_members.len(), 1);
        assert_eq!(module.members.len(), 2);
        let symbolic = module.symbolic_members.values().next().unwrap();
        let closed = module.members.values().next().unwrap();
        assert_eq!(symbolic.declaration, closed.declaration);
        assert_eq!(closed.declaration.source, model.id);
        assert_eq!(closed.declaration.path, vec![3, 0]);
        let features = std::iter::once(serde_json::to_value(symbolic).unwrap()).chain(
            module.members.values().map(|member| {
                assert_eq!(member.declaration, symbolic.declaration);
                serde_json::to_value(member).unwrap()
            }),
        );
        for feature in features {
            for (bit, flag) in flags.iter().enumerate() {
                assert_eq!(
                    feature[*flag],
                    mask & (1 << bit) != 0,
                    "mask={mask}, flag={flag}"
                );
            }
        }
    }
    // Omission uses the same Ecore defaults through both projection paths.
    let report = good(
        "fn symbolic<T:m.Base>(x:m.Child<T>) -> T = x.value; fn closed(x:m.Child<m.Leaf>) -> m.Leaf = x.value; fn raw(x:m.Child) -> m.Base = x.value;",
    );
    let module = &report.modules[0];
    for feature in [
        serde_json::to_value(module.symbolic_members.values().next().unwrap()).unwrap(),
        serde_json::to_value(module.members.values().next().unwrap()).unwrap(),
    ] {
        assert_eq!(feature["changeable"], true);
        for flag in ["derived", "transient", "volatile", "unsettable"] {
            assert_eq!(feature[flag], false);
        }
    }
}

#[test]
fn migration_rejects_invalid_behavior_flag_lexemes_and_preserves_originals() {
    for flag in [
        "derived",
        "changeable",
        "transient",
        "volatile",
        "unsettable",
    ] {
        let original = DefinitionSource::new(
            "memory:symbolic.ecore",
            SourceKind::Ecore,
            MODEL.replace(
                "name=\"value\"",
                &format!("name=\"value\" {flag}=\"invalid\""),
            ),
        );
        let report =
            intersect_model::migration::migrate_ecore(original.clone(), "memory:symbolic.rcore")
                .unwrap();
        assert_eq!(report.original, original);
        assert!(report.converted.is_none());
        assert!(
            report.issues.iter().any(|issue| issue.blocks_conversion
                && issue.message.contains(flag)
                && issue.span.source == original.id),
            "{flag}: {:?}",
            report.issues
        );
    }
}

#[test]
fn symbolic_inverse_queries_preserve_reference_parameter_identity() {
    for field in ["value", "optional", "items"] {
        good(&format!(
            "fn query<T:m.Base>(x:T)->OrderedSet<m.Box<T>> =incoming::<m.Box<T>>(x,\"{field}\");"
        ));
        good(&format!(
            "fn query<T:m.Base>(x:T)->OrderedSet<m.Child<T>> =incoming::<m.Child<T>>(x,\"{field}\");"
        ));
        bad(&format!(
            "fn query<T:m.Base,U:m.Base>(x:U)->OrderedSet<m.Box<T>> =incoming::<m.Box<T>>(x,\"{field}\");"
        ));
        bad(&format!(
            "fn query<T:m.Base>(x:m.Base)->OrderedSet<m.Box<T>> =incoming::<m.Box<T>>(x,\"{field}\");"
        ));
    }
}
