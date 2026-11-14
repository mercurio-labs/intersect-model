use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    generic_resolution::{ApplicationBounds, Resolver},
    resources::DeclarationId,
};
fn run(
    body: &str,
    path: &[usize],
) -> Result<ApplicationBounds, intersect_model::definitions::ecore::ImportError> {
    let source = DefinitionSource::new(
        "memory:bounds",
        SourceKind::Ecore,
        format!(
            r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="g" nsURI="urn:g">{body}</e:EPackage>"#
        ),
    );
    let id = DeclarationId {
        source: source.id.clone(),
        path: path.to_vec(),
    };
    Resolver::new(&DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    })
    .unwrap()
    .application_bounds(&id)
}
const SIMPLE: &str = r##"
<eClassifiers x:type="e:EClass" name="Base"/>
<eClassifiers x:type="e:EClass" name="Good" eSuperTypes="#//Base"/>
<eClassifiers x:type="e:EClass" name="Bad"/>
<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"><eBounds eClassifier="#//Base"/></eTypeParameters></eClassifiers>
<eClassifiers x:type="e:EClass" name="Use"><eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eClassifier="#//Box"><eTypeArguments eClassifier="#//Good"/></eGenericType></eStructuralFeatures></eClassifiers>
"##;
#[test]
fn nominal_bounds_accept_subtypes_reject_unrelated_and_preserve_evidence() {
    let report = run(SIMPLE, &[4, 0, 0]).unwrap();
    assert!(report.valid);
    assert_eq!(report.checks.len(), 1);
    assert_eq!(report.checks[0].parameter.path, vec![3, 0]);
    assert_eq!(
        report.checks[0].declared_bound.declaration.path,
        vec![3, 0, 0]
    );
    let bad = SIMPLE.replace(
        r##"<eTypeArguments eClassifier="#//Good"/>"##,
        r##"<eTypeArguments eClassifier="#//Bad"/>"##,
    );
    assert!(!run(&bad, &[4, 0, 0]).unwrap().valid);
    let any = SIMPLE.replace(
        r##"<eTypeArguments eClassifier="#//Good"/>"##,
        "<eTypeArguments/>",
    );
    assert!(run(&any, &[4, 0, 0]).unwrap().valid);
}
#[test]
fn every_intersection_bound_is_required_and_full_environment_precedes_checks() {
    let body = SIMPLE.replace(
        r##"<eBounds eClassifier="#//Base"/>"##,
        r##"<eBounds eClassifier="#//Base"/><eBounds eClassifier="#//Bad"/>"##,
    );
    assert!(!run(&body, &[4, 0, 0]).unwrap().valid);
    assert!(
        run(
            &body.replace(
                r##"eSuperTypes="#//Base""##,
                r##"eSuperTypes="#//Base #//Bad""##
            ),
            &[4, 0, 0]
        )
        .unwrap()
        .valid
    );
    let body = SIMPLE.replace(r##"</eTypeParameters></eClassifiers>"##,r##"</eTypeParameters><eTypeParameters name="U"><eBounds eTypeParameter="#//Box/T"/></eTypeParameters></eClassifiers>"##)
        .replace(r##"<eTypeArguments eClassifier="#//Good"/>"##,r##"<eTypeArguments eClassifier="#//Base"/><eTypeArguments eClassifier="#//Good"/>"##);
    let report = run(&body, &[4, 0, 0]).unwrap();
    assert!(report.valid);
    assert_eq!(report.checks.len(), 2);
    assert!(
        !run(
            &body.replace(
                r##"<eTypeArguments eClassifier="#//Good"/>"##,
                r##"<eTypeArguments eClassifier="#//Bad"/>"##
            ),
            &[4, 0, 0]
        )
        .unwrap()
        .valid
    );
}
const RECURSIVE: &str = r##"
<eClassifiers x:type="e:EClass" name="Comparable"><eTypeParameters name="E"/></eClassifiers>
<eClassifiers x:type="e:EClass" name="Good"><eGenericSuperTypes eClassifier="#//Comparable"><eTypeArguments eClassifier="#//Good"/></eGenericSuperTypes></eClassifiers>
<eClassifiers x:type="e:EClass" name="Bad"/>
<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"><eBounds eClassifier="#//Comparable"><eTypeArguments eTypeParameter="#//Box/T"/></eBounds></eTypeParameters></eClassifiers>
<eClassifiers x:type="e:EClass" name="Use"><eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eClassifier="#//Box"><eTypeArguments eClassifier="#//Good"/></eGenericType></eStructuralFeatures></eClassifiers>
"##;
#[test]
fn recursive_f_bounds_check_instantiated_ancestry_without_infinite_expansion() {
    assert!(run(RECURSIVE, &[4, 0, 0]).unwrap().valid);
    let bad = RECURSIVE.replace(
        r##"<eGenericType eClassifier="#//Box"><eTypeArguments eClassifier="#//Good"/>"##,
        r##"<eGenericType eClassifier="#//Box"><eTypeArguments eClassifier="#//Bad"/>"##,
    );
    assert!(!run(&bad, &[4, 0, 0]).unwrap().valid);
    let wrong = RECURSIVE.replace(r##"<eGenericSuperTypes eClassifier="#//Comparable"><eTypeArguments eClassifier="#//Good"/>"##,
        r##"<eGenericSuperTypes eClassifier="#//Comparable"><eTypeArguments eClassifier="#//Bad"/>"##);
    assert!(!run(&wrong, &[4, 0, 0]).unwrap().valid);
}
#[test]
fn argument_matching_is_invariant_with_pinned_upper_and_lower_wildcard_rules() {
    // Good inherits Comparable<Good> and Good is a subtype of Bad in this control.
    let source = RECURSIVE.replace(
        r##"</eGenericSuperTypes></eClassifiers>"##,
        r##"</eGenericSuperTypes><eGenericSuperTypes eClassifier="#//Bad"/></eClassifiers>"##,
    );
    let invariant = source.replace(
        r##"<eTypeArguments eTypeParameter="#//Box/T"/>"##,
        r##"<eTypeArguments eClassifier="#//Bad"/>"##,
    );
    assert!(!run(&invariant, &[4, 0, 0]).unwrap().valid);
    let upper = source.replace(
        r##"<eTypeArguments eTypeParameter="#//Box/T"/>"##,
        r##"<eTypeArguments><eUpperBound eClassifier="#//Bad"/></eTypeArguments>"##,
    );
    assert!(run(&upper, &[4, 0, 0]).unwrap().valid);
    let lower_equal = source.replace(
        r##"<eTypeArguments eTypeParameter="#//Box/T"/>"##,
        r##"<eTypeArguments><eLowerBound eClassifier="#//Good"/></eTypeArguments>"##,
    );
    assert!(run(&lower_equal, &[4, 0, 0]).unwrap().valid);
    let lower_other = source.replace(
        r##"<eTypeArguments eTypeParameter="#//Box/T"/>"##,
        r##"<eTypeArguments><eLowerBound eClassifier="#//Bad"/></eTypeArguments>"##,
    );
    assert!(!run(&lower_other, &[4, 0, 0]).unwrap().valid);
}
#[test]
fn nested_and_ancestor_applications_are_checked_and_unknown_provider_relations_are_explicit() {
    let inherited = SIMPLE.replace(r##"<eClassifiers x:type="e:EClass" name="Bad"/>"##,
        r##"<eClassifiers x:type="e:EClass" name="Bad"><eGenericSuperTypes eClassifier="#//Box"><eTypeArguments eClassifier="#//Use"/></eGenericSuperTypes></eClassifiers>"##)
        .replace(r##"<eGenericType eClassifier="#//Box"><eTypeArguments eClassifier="#//Good"/></eGenericType>"##,
            r##"<eGenericType eClassifier="#//Bad"/>"##);
    assert!(!run(&inherited, &[4, 0, 0]).unwrap().valid);
    let nested = SIMPLE.replace(r##"<eTypeArguments eClassifier="#//Good"/>"##,
        r##"<eTypeArguments eClassifier="#//Box"><eTypeArguments eClassifier="#//Bad"/></eTypeArguments>"##);
    let checked = run(&nested, &[4, 0, 0]).unwrap();
    assert!(!checked.valid);
    assert_eq!(checked.checks.len(), 2);
    let raw = SIMPLE.replace(r##"<eTypeArguments eClassifier="#//Good"/>"##, "");
    let report = run(&raw, &[4, 0, 0]).unwrap();
    assert!(report.valid);
    assert!(report.checks.is_empty());
    assert_eq!(report.warnings[0].code, "raw_type");
    let provider = SIMPLE.replace(
        r##"<eBounds eClassifier="#//Base"/>"##,
        r##"<eBounds eClassifier="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>"##,
    );
    assert!(
        run(&provider, &[4, 0, 0])
            .unwrap_err()
            .message
            .contains("assignability contract")
    );
}

const OPEN: &str = r##"
<eClassifiers x:type="e:EClass" name="Base"/>
<eClassifiers x:type="e:EClass" name="Good" eSuperTypes="#//Base"/>
<eClassifiers x:type="e:EClass" name="Other"/>
<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"><eBounds eClassifier="#//Base"/></eTypeParameters></eClassifiers>
<eClassifiers x:type="e:EClass" name="Use"><eTypeParameters name="U"><eBounds eClassifier="#//Good"/></eTypeParameters><eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eClassifier="#//Box"><eTypeArguments eTypeParameter="#//Use/U"/></eGenericType></eStructuralFeatures></eClassifiers>
"##;

#[test]
fn open_actual_uses_any_declared_bound_without_erasure() {
    let report = run(OPEN, &[4, 1, 0]).unwrap();
    assert!(report.valid);
    assert_eq!(report.checks.len(), 1);
    assert!(matches!(&report.checks[0].argument,
        intersect_model::definitions::generic_types::Type::Parameter { declaration } if declaration.path == [4, 0]));
    let bound = r##"<eBounds eClassifier="#//Good"/>"##;
    for replacement in ["", r##"<eBounds eClassifier="#//Other"/>"##] {
        assert!(
            !run(&OPEN.replace(bound, replacement), &[4, 1, 0])
                .unwrap()
                .valid
        );
    }
    // Any actual bound can establish classifier boundedness.
    let intersection = OPEN.replace(
        bound,
        &format!(r##"<eBounds eClassifier="#//Other"/>{bound}"##),
    );
    assert!(run(&intersection, &[4, 1, 0]).unwrap().valid);
}

#[test]
fn distinct_parameter_bounds_follow_pinned_all_actual_bounds_rule() {
    let body = r##"
<eClassifiers x:type="e:EClass" name="Base"/>
<eClassifiers x:type="e:EClass" name="Good" eSuperTypes="#//Base"/>
<eClassifiers x:type="e:EClass" name="Other"/>
<eClassifiers x:type="e:EClass" name="Pair"><eTypeParameters name="A"/><eTypeParameters name="B"><eBounds eTypeParameter="#//Pair/A"/></eTypeParameters></eClassifiers>
<eClassifiers x:type="e:EClass" name="Use"><eTypeParameters name="A"><eBounds eClassifier="#//Base"/></eTypeParameters><eTypeParameters name="B"><eBounds eClassifier="#//Good"/></eTypeParameters><eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eClassifier="#//Pair"><eTypeArguments eTypeParameter="#//Use/A"/><eTypeArguments eTypeParameter="#//Use/B"/></eGenericType></eStructuralFeatures></eClassifiers>
"##;
    assert!(run(body, &[4, 2, 0]).unwrap().valid);
    for replacement in [
        "",
        r##"<eBounds eClassifier="#//Good"/><eBounds eClassifier="#//Other"/>"##,
    ] {
        let changed = body.replace(r##"<eBounds eClassifier="#//Good"/>"##, replacement);
        assert!(!run(&changed, &[4, 2, 0]).unwrap().valid);
    }
    // Exact self-identity is valid even with no declared bounds.
    let same = body
        .replace(
            r##"<eTypeArguments eTypeParameter="#//Use/B"/>"##,
            r##"<eTypeArguments eTypeParameter="#//Use/A"/>"##,
        )
        .replace(r##"<eBounds eClassifier="#//Base"/>"##, "");
    assert!(run(&same, &[4, 2, 0]).unwrap().valid);
    // A concrete subtype of A's bound does not prove it is an instance of A.
    let concrete = body.replace(
        r##"<eTypeArguments eTypeParameter="#//Use/B"/>"##,
        r##"<eTypeArguments eClassifier="#//Good"/>"##,
    );
    assert!(!run(&concrete, &[4, 2, 0]).unwrap().valid);
}

#[test]
fn open_recursive_bounds_terminate_and_nested_parameters_remain_invariant() {
    let body = r##"
<eClassifiers x:type="e:EClass" name="Comparable"><eTypeParameters name="T"/></eClassifiers>
<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"><eBounds eClassifier="#//Comparable"><eTypeArguments eTypeParameter="#//Box/T"/></eBounds></eTypeParameters></eClassifiers>
<eClassifiers x:type="e:EClass" name="Use"><eTypeParameters name="U"><eBounds eClassifier="#//Comparable"><eTypeArguments eTypeParameter="#//Use/U"/></eBounds></eTypeParameters><eTypeParameters name="V"><eBounds eClassifier="#//Comparable"><eTypeArguments eTypeParameter="#//Use/V"/></eBounds></eTypeParameters><eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eClassifier="#//Box"><eTypeArguments eTypeParameter="#//Use/U"/></eGenericType></eStructuralFeatures></eClassifiers>
"##;
    assert!(run(body, &[2, 2, 0]).unwrap().valid);
    let changed = body.replace(r##"<eBounds eClassifier="#//Comparable"><eTypeArguments eTypeParameter="#//Use/U"/></eBounds>"##,
        r##"<eBounds eClassifier="#//Comparable"><eTypeArguments eTypeParameter="#//Use/V"/></eBounds>"##);
    assert!(!run(&changed, &[2, 2, 0]).unwrap().valid);
}

#[test]
fn raw_and_applied_bounds_use_argument_counts_without_unchecked_erasure() {
    for (raw_argument, raw_bound, valid) in [
        (true, true, true),
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let argument = if raw_argument {
            ""
        } else {
            r##"<eTypeArguments eClassifier="#//Value"/>"##
        };
        let bound = if raw_bound {
            ""
        } else {
            r##"<eTypeArguments eClassifier="#//Value"/>"##
        };
        let body = format!(
            r##"
<eClassifiers x:type="e:EClass" name="Value"/>
<eClassifiers x:type="e:EClass" name="Base"><eTypeParameters name="T"/></eClassifiers>
<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="U"><eBounds eClassifier="#//Base">{bound}</eBounds></eTypeParameters></eClassifiers>
<eClassifiers x:type="e:EClass" name="Use"><eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eClassifier="#//Box"><eTypeArguments eClassifier="#//Base">{argument}</eTypeArguments></eGenericType></eStructuralFeatures></eClassifiers>
"##
        );
        let report = run(&body, &[3, 0, 0]).unwrap();
        assert_eq!(report.valid, valid);
        assert_eq!(report.checks.len(), 1);
        assert_eq!(report.warnings.is_empty(), !raw_argument && !raw_bound);
        assert!(report.warnings.iter().all(|w| w.code == "raw_type"));
    }
}

#[test]
fn raw_ancestors_check_open_bounds_and_concrete_ancestry_without_invented_arguments() {
    let body = r##"
<eClassifiers x:type="e:EClass" name="Base"/>
<eClassifiers x:type="e:EClass" name="Good" eSuperTypes="#//Base"/>
<eClassifiers x:type="e:EClass" name="Carrier"><eTypeParameters name="C"><eBounds eClassifier="#//Base"/></eTypeParameters></eClassifiers>
<eClassifiers x:type="e:EClass" name="Raw"><eTypeParameters name="T"><eBounds eClassifier="#//Good"/></eTypeParameters><eGenericSuperTypes eClassifier="#//Carrier"><eTypeArguments eTypeParameter="#//Raw/T"/></eGenericSuperTypes></eClassifiers>
<eClassifiers x:type="e:EClass" name="Use"><eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eClassifier="#//Raw"/></eStructuralFeatures></eClassifiers>
"##;
    let report = run(body, &[4, 0, 0]).unwrap();
    assert!(report.valid);
    assert_eq!(report.checks.len(), 1);
    assert_eq!(report.warnings.len(), 1);
    let invalid = body.replace(r##"<eBounds eClassifier="#//Good"/>"##, "");
    let report = run(&invalid, &[4, 0, 0]).unwrap();
    assert!(!report.valid);
    assert_eq!(report.warnings.len(), 1);

    let concrete_parent = SIMPLE.replace(
        r##"<eClassifiers x:type="e:EClass" name="Good" eSuperTypes="#//Base"/>"##,
        r##"<eClassifiers x:type="e:EClass" name="Good" eSuperTypes="#//Base"><eTypeParameters name="T"/></eClassifiers>"##);
    let report = run(&concrete_parent, &[4, 0, 0]).unwrap();
    assert!(report.valid);
    assert_eq!(report.warnings.len(), 1);
}

#[test]
fn mixed_paths_try_every_bound_candidate_without_erasing_raw_arguments() {
    let fixture = include_str!("../../../../fixtures/ecore-generics/mixed-ancestry.ecore");
    let body = fixture
        .split_once('>')
        .unwrap()
        .1
        .rsplit_once("</e:EPackage>")
        .unwrap()
        .0;
    for parents in ["#//Raw #//Applied", "#//Applied #//Raw"] {
        let body = body.replace("#//Raw #//Applied", parents);
        let report = run(&body, &[7, 0, 0]).unwrap();
        assert!(report.valid);
        assert_eq!(report.warnings.len(), 1);
        let different = body.replace(
            r##"<eBounds eClassifier="#//Base"><eTypeArguments eClassifier="#//Value"/></eBounds>"##,
            r##"<eBounds eClassifier="#//Base"><eTypeArguments eClassifier="#//Other"/></eBounds>"##);
        assert!(!run(&different, &[7, 0, 0]).unwrap().valid);
    }
}

#[test]
fn native_reference_scalar_bounds_accept_object_and_reject_unrelated_types() {
    for (actual, bound, valid) in [
        ("EString", "EJavaObject", true),
        ("EByteArray", "EJavaObject", true),
        ("EBigDecimal", "EJavaObject", true),
        ("EDate", "EJavaObject", true),
        ("EIntegerObject", "EJavaObject", true),
        ("EJavaObject", "EString", false),
        ("EIntegerObject", "ELongObject", false),
        ("EString", "EDate", false),
    ] {
        let body = format!(
            r##"
<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"><eBounds eClassifier="http://www.eclipse.org/emf/2002/Ecore#//{bound}"/></eTypeParameters></eClassifiers>
<eClassifiers x:type="e:EClass" name="Use"><eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eClassifier="#//Box"><eTypeArguments eClassifier="http://www.eclipse.org/emf/2002/Ecore#//{actual}"/></eGenericType></eStructuralFeatures></eClassifiers>
"##
        );
        assert_eq!(
            run(&body, &[1, 0, 0]).unwrap().valid,
            valid,
            "{actual} -> {bound}"
        );
    }
}

#[test]
fn scalar_aliases_match_nested_arguments_without_merging_declaration_ids() {
    let body = r##"
<eClassifiers x:type="e:EDataType" name="A" instanceClassName="java.lang.String"/>
<eClassifiers x:type="e:EDataType" name="B" instanceTypeName="java.lang.String"/>
<eClassifiers x:type="e:EClass" name="Holder"><eTypeParameters name="T"/></eClassifiers>
<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"><eBounds eClassifier="#//Holder"><eTypeArguments eClassifier="#//B"/></eBounds></eTypeParameters></eClassifiers>
<eClassifiers x:type="e:EClass" name="Use"><eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eClassifier="#//Box"><eTypeArguments eClassifier="#//Holder"><eTypeArguments eClassifier="#//A"/></eTypeArguments></eGenericType></eStructuralFeatures></eClassifiers>
"##;
    let report = run(body, &[4, 0, 0]).unwrap();
    assert!(report.valid);
    assert_ne!(
        report.checks[0].argument,
        report.checks[0].substituted_bound
    );
    let mismatch = body.replace(
        r#"instanceTypeName="java.lang.String""#,
        r#"instanceTypeName="java.lang.Integer""#,
    );
    assert!(!run(&mismatch, &[4, 0, 0]).unwrap().valid);
    let unknown = body.replace(
        r#"instanceTypeName="java.lang.String""#,
        r#"instanceTypeName="example.Unknown""#,
    );
    assert!(
        run(&unknown, &[4, 0, 0])
            .unwrap_err()
            .message
            .contains("provider contract")
    );
}
