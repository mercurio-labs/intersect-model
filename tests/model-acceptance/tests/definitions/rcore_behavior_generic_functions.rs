//! S04 generic source-program typing; these controls do not execute functions.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        SyntaxDiagnostic,
        checking::{TypeReport, check},
    },
};
fn source(name: &str, body: &str) -> DefinitionSource {
    DefinitionSource::new(
        format!("memory:{name}-behavior.rcore"),
        SourceKind::RcoreBehavior,
        format!("rcore 1; package {name} version \"1\"; {body}"),
    )
}
fn bundle(sources: Vec<DefinitionSource>) -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        sources,
        namespaces: vec![],
    }
}
fn run(body: &str) -> Result<TypeReport, Vec<SyntaxDiagnostic>> {
    let entry = source("generic", body);
    check(&bundle(vec![entry.clone()]), &entry.id)
}
fn good(body: &str) {
    if let Err(errors) = run(body) {
        panic!("Expected generic program to check: {body}\n{errors:?}");
    }
}
fn bad(body: &str) {
    let entry = source("generic", body);
    let errors = check(&bundle(vec![entry.clone()]), &entry.id)
        .expect_err("Invalid generic program must reject");
    assert!(!errors.is_empty());
    for error in errors {
        assert_eq!(error.span.source, entry.id, "{body}: {error:?}");
        assert!(error.span.start < error.span.end, "{body}: {error:?}");
        assert!(
            entry.text.get(error.span.start..error.span.end).is_some(),
            "{body}: {error:?}"
        );
    }
}

#[test]
fn explicit_and_inferred_calls_preserve_concrete_and_scoped_types() {
    good(
        "fn id<T>(x: T) -> T = x; fn explicit() -> Int8 = id::<Int8>(127); fn inferred(x: Bool) -> Bool = id(x); fn nested<U>(x: U) -> U = id(x); fn unrelated<T>(x: T) -> T = x;",
    );
    for body in [
        "fn id<T>(x: T) -> T = x; fn overflow() -> Int8 = id::<Int8>(128);",
        "fn id<T>(x: T) -> T = x; fn wrong() -> Bool = id::<Int>(1);",
        "fn id<T>(x: T) -> T = x; fn arity() -> Int = id::<Int, Bool>(1);",
        "fn pair<T,U>(x: T, y: U) -> T = x; fn partial() -> Int = pair::<Int>(1,true);",
        "fn id<T>(x: T) -> T = x; fn missing() -> Int = id::<Int>();",
        "fn id<T>(x: T) -> T = x; fn extra() -> Int = id(1,2);",
    ] {
        bad(body);
    }
}

#[test]
fn expected_results_infer_formals_through_nested_structural_types() {
    good(
        "fn empty<T>() -> Sequence<T> = []; fn result() -> Sequence<Int8> = empty(); fn nested() -> Sequence<Sequence<Int8>> = empty();",
    );
    good(
        "fn wrap<T>(x: T) -> {items: Sequence<T>, pair: (T, Bool)} = {items: [x], pair: (x, true)}; fn result() -> {pair: (Int8, Bool), items: Sequence<Int8>} = wrap(127);",
    );
    good(
        "fn id<T>(x: T) -> T = x; fn structures(x: Sequence<{pair: (Int8, Bool)}>) -> Sequence<{pair: (Int8, Bool)}> = id(x); fn optional(x: Int8?) -> Int8? = id(x); fn result(x: Result<Int8, String>) -> Result<Int8, String> = id(x);",
    );
    bad("fn empty<T>() -> Sequence<T> = []; fn unresolved() -> Int = { let values = empty(); 1 };");
    bad("fn marker<T>() -> Int = 1; fn unresolved() -> Int = marker();");
    bad("fn wrap<T>(x: T) -> Sequence<T> = [x]; fn wrong(x: Bool) -> Sequence<Int> = wrap(x);");
}

#[test]
fn repeated_actual_constraints_reject_conflicts_in_all_structural_positions() {
    good(
        "fn same<T>(x: T, y: T) -> T = x; fn ok(x: Int8, y: Int8) -> Int8 = same(x,y); fn pair<T>(x: (T,T)) -> T = x[0]; fn ok_pair() -> Int = pair((1,2));",
    );
    for body in [
        "fn same<T>(x: T,y: T) -> T = x; fn bad() -> Int = same(1,true);",
        "fn same<T>(x: T,y: T) -> T = x; fn bad() -> Int = same(true,1);",
        "fn pair<T>(x: (T,T)) -> T = x[0]; fn bad() -> Int = pair((1,true));",
        "fn record<T>(x: {a: T,b: T}) -> T = x.a; fn bad() -> Int = record({a: 1,b: true});",
        "fn nested<T>(x: Sequence<T>,y: T) -> T = y; fn bad() -> Int = nested([1],true);",
    ] {
        bad(body);
    }
}

#[test]
fn scalar_and_earlier_formal_bounds_are_checked_at_declaration_and_application() {
    good(
        "fn bounded<T: Int8>(x: T) -> T = x; fn ok() -> Int8 = bounded::<Int8>(127); fn inferred(x: Int8) -> Int8 = bounded(x); fn up<T,U:T>(x: U) -> T = x; fn earlier(x: Int8) -> Int8 = up::<Int8,Int8>(x);",
    );
    for body in [
        "fn bounded<T: Int8>(x: T) -> T = x; fn wrong(x: Bool) -> Bool = bounded(x);",
        "fn bounded<T: Int8>(x: T) -> T = x; fn wrong() -> Bool = bounded::<Bool>(true);",
        "fn up<T,U:T>(x: U) -> T = x; fn wrong() -> Int = up::<Int,Bool>(true);",
        "fn forward<T:U,U>(x: T) -> T = x;",
        "fn cyclic<T:T>(x: T) -> T = x;",
    ] {
        bad(body);
    }
}

#[test]
fn universal_bodies_are_checked_even_when_unused_and_recursion_retains_formals() {
    good(
        "fn recur<T>(x: T) -> T = recur(x); fn recur_explicit<T>(x: T) -> T = recur_explicit::<T>(x); fn left<T>(x: T) -> T = right(x); fn right<U>(x: U) -> U = left(x);",
    );
    for body in [
        "fn unused<T>(x: T) -> Bool = x; fn entry() -> Int = 1;",
        "fn unused<T>() -> T = 1; fn entry() -> Int = 1;",
        "fn unrelated<T,U>(x: T) -> U = x;",
        "fn fabricate<T:Int8>() -> T = 1;",
        "fn unused<T>(x: T) -> T = true; fn seemingly_valid() -> Bool = unused::<Bool>(true);",
    ] {
        bad(body);
    }
}

#[test]
fn model_bounds_use_nominal_subtyping_without_generic_ecore_application_claims() {
    let model = DefinitionSource::new(
        "memory:generic-model.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:generic-functions'><eClassifiers x:type='e:EClass' name='Base'/><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Base'/><eClassifiers x:type='e:EClass' name='Other'/></e:EPackage>",
    );
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    for (body, valid) in [
        (
            "fn up<T:m.Base>(x:T) -> m.Base = x; fn ok(x:m.Child) -> m.Base = up(x);",
            true,
        ),
        (
            "fn id<T:m.Base>(x:T) -> T = x; fn ok(x:m.Child) -> m.Child = id::<m.Child>(x);",
            true,
        ),
        (
            "fn up<T,U:T>(x:U) -> T = x; fn ok(x:m.Child) -> m.Base = up::<m.Base,m.Child>(x);",
            true,
        ),
        (
            "fn id<T:m.Base>(x:T) -> T = x; fn wrong(x:m.Other) -> m.Other = id(x);",
            false,
        ),
        (
            "fn id<T:m.Base>(x:T) -> T = x; fn wrong(x:m.Other) -> m.Other = id::<m.Other>(x);",
            false,
        ),
        ("fn down<T:m.Base>(x:m.Base) -> T = x;", false),
    ] {
        let mut entry = source(
            "model_functions",
            &format!("import model \"urn:generic-functions\" as m; {body}"),
        );
        entry.dependencies.push(model.id.clone());
        let result = check(&bundle(vec![model.clone(), entry.clone()]), &entry.id);
        if valid {
            assert!(result.is_ok(), "{body}: {result:?}");
        } else {
            let errors = result.unwrap_err();
            assert!(!errors.is_empty());
            assert!(
                errors
                    .iter()
                    .all(|e| e.span.source == entry.id && e.span.start < e.span.end),
                "{body}: {errors:?}"
            );
        }
    }
}

#[test]
fn imported_generic_calls_keep_declaration_scope_and_source_diagnostics() {
    let helper = source(
        "helper",
        "pub fn id<T>(x:T) -> T = x; pub fn empty<T>() -> Sequence<T> = [];",
    );
    let mut entry = source(
        "entry",
        "import package helper version \"1\" as h; fn forward<T>(x:T) -> T = h.id(x); fn concrete() -> Int8 = h.id::<Int8>(127); fn expected() -> Sequence<Bool> = h.empty();",
    );
    entry.dependencies.push(helper.id.clone());
    let first = check(&bundle(vec![entry.clone(), helper.clone()]), &entry.id).unwrap();
    let reverse = check(&bundle(vec![helper, entry.clone()]), &entry.id).unwrap();
    assert_eq!(
        serde_json::to_value(first).unwrap(),
        serde_json::to_value(reverse).unwrap()
    );
    let broken = source("helper", "pub fn unused<T>(x:T) -> Bool = x;");
    let mut entry = source(
        "entry",
        "import package helper version \"1\" as h; fn main() -> Int = 1;",
    );
    entry.dependencies.push(broken.id.clone());
    let errors = check(&bundle(vec![entry.clone(), broken.clone()]), &entry.id).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.span.source == broken.id && e.span.start < e.span.end),
        "{errors:?}"
    );
}

#[test]
fn unconstrained_equality_cannot_be_hidden_inside_structural_wrappers() {
    for body in [
        "fn equal<T>(x:T,y:T) -> Bool = x == y;",
        "fn unequal<T>(x:T,y:T) -> Bool = x != y;",
        "fn optional<T>(x:Option<T>,y:Option<T>) -> Bool = x == y;",
        "fn tuple<T>(x:(T,Bool),y:(T,Bool)) -> Bool = x == y;",
        "fn record<T>(x:{value:T},y:{value:T}) -> Bool = x == y;",
        "fn nested<T>(x:{value:Option<(T,Bool)>},y:{value:Option<(T,Bool)>}) -> Bool = x == y;",
    ] {
        bad(body);
    }
    good("fn id<T>(x:T) -> T = x; fn concrete(x:Int8,y:Int8) -> Bool = id(x) == id(y);");
}

#[test]
fn generic_function_value_aliases_require_checked_monomorphic_instantiation() {
    good("fn id<T>(x:T) -> T = x; fn alias() -> Int8 = { let f: (Int8) -> Int8 = id; f(127) };");
    good(
        "fn bounded<T:Int8>(x:T) -> T = x; fn alias() -> Int8 = { let f: (Int8) -> Int8 = bounded; f(127) };",
    );
    for body in [
        "fn id<T>(x:T) -> T = x; fn escape() -> Int = { let f = id; f(1) };",
        "fn id<T>(x:T) -> T = x; fn mismatch() -> Bool = { let f: (Int) -> Bool = id; f(1) };",
        "fn id<T>(x:T) -> T = x; fn wrong_arg() -> Int8 = { let f: (Int8) -> Int8 = id; f(true) };",
        "fn id<T>(x:T) -> T = x; fn overflow() -> Int8 = { let f: (Int8) -> Int8 = id; f(128) };",
        "fn bounded<T:Int8>(x:T) -> T = x; fn bypass() -> Bool = { let f: (Bool) -> Bool = bounded; f(true) };",
        "fn bounded<T:Int8>(x:T) -> T = x; fn escape() -> Bool = { let f = bounded; f(true) };",
    ] {
        bad(body);
    }
}

#[test]
fn sibling_actuals_join_before_argument_context_in_both_orders() {
    let model = DefinitionSource::new(
        "memory:sibling-model.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='siblings' nsURI='urn:sibling-functions'><eClassifiers x:type='e:EClass' name='Base'/><eClassifiers x:type='e:EClass' name='Left' eSuperTypes='#//Base'/><eClassifiers x:type='e:EClass' name='Right' eSuperTypes='#//Base'/></e:EPackage>",
    );
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    for reverse in [false, true] {
        let arguments = if reverse { "right,left" } else { "left,right" };
        let body = format!(
            "import model \"urn:sibling-functions\" as m; fn same<T>(x:T,y:T) -> T = x; fn contextual(left:m.Left,right:m.Right) -> m.Base = same({arguments}); fn inferred(left:m.Left,right:m.Right) -> m.Base = {{ let joined = same({arguments}); joined }};"
        );
        let mut entry = source("siblings", &body);
        entry.dependencies.push(model.id.clone());
        let result = check(&bundle(vec![model.clone(), entry.clone()]), &entry.id);
        assert!(result.is_ok(), "{body}: {result:?}");
        // Explicit arguments are a fixed contract, not lower bounds to widen.
        let body = format!(
            "import model \"urn:sibling-functions\" as m; fn same<T>(x:T,y:T) -> T = x; fn wrong(left:m.Left,right:m.Right) -> m.Base = same::<m.Left>({arguments});"
        );
        let mut entry = source("siblings", &body);
        entry.dependencies.push(model.id.clone());
        let errors = check(&bundle(vec![model.clone(), entry.clone()]), &entry.id).unwrap_err();
        assert!(!errors.is_empty());
        assert!(
            errors
                .iter()
                .all(|e| e.span.source == entry.id && e.span.start < e.span.end),
            "{errors:?}"
        );
    }
}

#[test]
fn deferred_empty_and_none_actuals_use_later_evidence_without_result_context() {
    good(
        "fn choose<T>(a:Sequence<T>,b:Sequence<T>) -> Sequence<T> = b; fn forward(values:Sequence<Int8>) -> Sequence<Int8> = { let inferred = choose([],values); inferred }; fn reverse(values:Sequence<Int8>) -> Sequence<Int8> = { let inferred = choose(values,[]); inferred };",
    );
    good(
        "fn choose<T>(a:Option<T>,b:Option<T>) -> Option<T> = b; fn forward(value:Int8?) -> Int8? = { let inferred = choose(none,value); inferred }; fn reverse(value:Int8?) -> Int8? = { let inferred = choose(value,none); inferred };",
    );
    bad(
        "fn choose<T>(a:Sequence<T>,b:Sequence<T>) -> Sequence<T> = b; fn missing() -> Int = { let unresolved = choose([],[]); 1 };",
    );
    bad(
        "fn choose<T>(a:Option<T>,b:Option<T>) -> Option<T> = b; fn missing() -> Int = { let unresolved = choose(none,none); 1 };",
    );
}

#[test]
fn callback_variance_bound_propagation_and_many_features_use_shared_model_relations() {
    let model = DefinitionSource::new(
        "memory:inference-model.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='inference' nsURI='urn:inference-functions'><eClassifiers x:type='e:EClass' name='Base'/><eClassifiers x:type='e:EClass' name='Left' eSuperTypes='#//Base'/><eClassifiers x:type='e:EClass' name='Right' eSuperTypes='#//Base'/><eClassifiers x:type='e:EClass' name='Holder'><eStructuralFeatures x:type='e:EReference' name='items' eType='#//Left' upperBound='-1' ordered='true' unique='false'/></eClassifiers></e:EPackage>",
    );
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    for (body, valid) in [
        (
            "fn apply<T>(f:(T)->Unit,x:T) -> T = x; fn callback(x:m.Left) -> Unit = (); fn caller(x:m.Left) -> m.Base = apply(callback,x);",
            true,
        ),
        (
            "fn apply<T>(f:(T)->Unit,x:T) -> T = x; fn callback(x:m.Left) -> Unit = (); fn wrong(x:m.Right) -> m.Base = apply(callback,x);",
            false,
        ),
        (
            "fn up<T,U:T>(x:U) -> T = x; fn caller(x:m.Left) -> m.Left = { let inferred = up(x); inferred };",
            true,
        ),
        (
            "fn read<T>(xs:Sequence<T>) -> T? = xs[0]; fn caller(x:m.Holder) -> m.Left? = { let inferred = read(x.items); inferred };",
            true,
        ),
        (
            "fn read<T>(xs:Sequence<T>) -> T? = xs[0]; fn wrong(x:m.Holder) -> m.Right? = read(x.items);",
            false,
        ),
    ] {
        let mut entry = source(
            "inference",
            &format!("import model \"urn:inference-functions\" as m; {body}"),
        );
        entry.dependencies.push(model.id.clone());
        let result = check(&bundle(vec![model.clone(), entry.clone()]), &entry.id);
        if valid {
            assert!(result.is_ok(), "{body}: {result:?}");
        } else {
            let errors = result.unwrap_err();
            assert!(!errors.is_empty());
            assert!(
                errors
                    .iter()
                    .all(|e| e.span.source == entry.id && e.span.start < e.span.end),
                "{body}: {errors:?}"
            );
        }
    }
}

#[test]
fn shallow_wide_inference_exhausts_its_work_budget_and_fresh_checks_recover() {
    fn program(width: usize) -> String {
        let template = vec!["T"; width].join(",");
        let actual = vec!["Int"; width].join(",");
        format!("fn wide<T>(x:({template})) -> T = x[0]; fn caller(x:({actual})) -> Int = wide(x);")
    }
    // Wide, shallow input exercises finite inference work, not parser recursion.
    // This is exhaustion coverage, not a claim about an exact source-width limit.
    good(&program(512));
    let large = program(9000);
    let entry = source("generic", &large);
    let input = bundle(vec![entry.clone()]);
    let first = check(&input, &entry.id).unwrap_err();
    assert!(
        first
            .iter()
            .any(|e| e.message == "Generic inference work limit exceeded"),
        "{first:?}"
    );
    assert!(
        first
            .iter()
            .all(|e| e.span.source == entry.id && e.span.start < e.span.end)
    );
    let repeated = check(&input, &entry.id).unwrap_err();
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(&repeated).unwrap()
    );
    good(&program(512));
    good("fn id<T>(x:T) -> T = x; fn recover() -> Bool = id(true);");
}

#[test]
fn imported_function_value_aliases_share_checked_monomorphization() {
    let helper = source(
        "helper",
        "pub fn id<T>(x:T) -> T = x; pub fn bounded<T:Int8>(x:T) -> T = x;",
    );
    for (body, valid) in [
        (
            "fn alias() -> Int8 = { let f: (Int8)->Int8 = h.id; f(127) };",
            true,
        ),
        (
            "fn alias() -> Int8 = { let f: (Int8)->Int8 = h.bounded; f(127) };",
            true,
        ),
        (
            "fn apply<T>(f:(T)->T,x:T)->T=f(x); fn use_import()->Int={let result=apply(h.id,1); result};",
            true,
        ),
        (
            "fn apply<T>(x:T,f:(T)->T)->T=f(x); fn use_import()->Int={let result=apply(1,h.id); result};",
            true,
        ),
        ("fn escape() -> Int = { let f = h.id; f(1) };", false),
        (
            "fn mismatch() -> Bool = { let f: (Int)->Bool = h.id; f(1) };",
            false,
        ),
        (
            "fn bypass() -> Bool = { let f: (Bool)->Bool = h.bounded; f(true) };",
            false,
        ),
        (
            "fn escape() -> Bool = { let f = h.bounded; f(true) };",
            false,
        ),
    ] {
        let mut entry = source(
            "entry",
            &format!("import package helper version \"1\" as h; {body}"),
        );
        entry.dependencies.push(helper.id.clone());
        let result = check(&bundle(vec![helper.clone(), entry.clone()]), &entry.id);
        if valid {
            assert!(result.is_ok(), "{body}: {result:?}");
        } else {
            let errors = result.unwrap_err();
            assert!(!errors.is_empty());
            assert!(
                errors
                    .iter()
                    .all(|e| e.span.source == entry.id && e.span.start < e.span.end),
                "{body}: {errors:?}"
            );
        }
    }
}

#[test]
fn partially_known_nested_arguments_supply_evidence_for_their_empty_components() {
    // Required positive: inference must collect the known tuple component before
    // checking its nested empty literal. An insufficient-context diagnostic here
    // is an implementation gap, not successful S04 qualification.
    good(
        "fn pair<T>(x:(T,Sequence<T>)) -> T = x[0]; fn inferred() -> Int = { let value = pair((1,[])); value };",
    );
    good(
        "fn pair<T>(x:(Sequence<T>,T)) -> T = x[1]; fn inferred() -> Int = { let value = pair(([],1)); value };",
    );
    bad(
        "fn pair<T>(x:(T,Sequence<T>)) -> T = x[0]; fn conflict() -> Int = { let value = pair((1,[true])); value };",
    );
}

#[test]
fn rigid_bounds_expose_index_call_and_scalar_capabilities_without_fabricating_formals() {
    good(
        "fn head<T:Sequence<Int>>(x:T) -> Int? = x[0]; fn component<T:(Int,Bool)>(x:T) -> Bool = x[1]; fn apply<F:(Int)->Bool>(f:F) -> Bool = f(1);",
    );
    good(
        "fn plus<T:Int8>(x:T) -> Int8 = x + 1; fn negate<T:Int>(x:T) -> Int = -x; fn compare<T:Int>(x:T) -> Bool = x < 1; fn boolean<T:Bool>(x:T) -> Bool = !x;",
    );
    for body in [
        "fn fabricate<T:Sequence<Int>>(x:T) -> T = [1];",
        "fn fabricate<T:(Int,Bool)>(x:T) -> T = (1,true);",
        "fn fabricate<T:Int8>(x:T) -> T = x + 1;",
        "fn fabricate<F:(Int)->Bool>(f:F) -> F = |x:Int| true;",
        "fn wrong<T:Sequence<Int>>(x:T) -> Bool? = x[0];",
        "fn unordered<T:Set<Int>>(x:T) -> Int? = x[0];",
        "fn past<T:(Int,Bool)>(x:T) -> Bool = x[2];",
        "fn wrong<F:(Int)->Bool>(f:F) -> Bool = f(true);",
        "fn wrong<F:(Int)->Bool>(f:F) -> Int = f(1);",
    ] {
        bad(body);
    }
}

#[test]
fn rigid_option_and_result_bounds_preserve_match_payloads_and_exhaustiveness() {
    good(
        "fn optional<T:Option<Int>>(x:T) -> Int = match x { Some(n) => n, none => 0 }; fn result<T:Result<Int,String>>(x:T) -> Int = match x { Ok(n) => n, Err(problem) => if problem == \"\" { 0 } else { 1 } };",
    );
    for body in [
        "fn missing<T:Option<Int>>(x:T) -> Int = match x { Some(n) => n };",
        "fn missing<T:Result<Int,String>>(x:T) -> Int = match x { Ok(n) => n };",
        "fn wrong<T:Option<Int>>(x:T) -> Bool = match x { Some(n) => n, none => false };",
        "fn fabricate<T:Option<Int>>(x:T) -> T = match x { Some(n) => Some(n), none => none };",
        "fn fabricate<T:Result<Int,String>>(x:T) -> T = match x { Ok(n) => Ok(n), Err(message) => Err(message) };",
    ] {
        bad(body);
    }
}

#[test]
fn rigid_optional_model_bound_supports_safe_access_with_exact_scalar_result() {
    let model = DefinitionSource::new(
        "memory:optional-bound.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='optional' nsURI='urn:optional-bound'><eClassifiers x:type='e:EClass' name='Base'><eStructuralFeatures x:type='e:EAttribute' name='name' lowerBound='1' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString'/></eClassifiers></e:EPackage>",
    );
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    for (body, valid) in [
        ("fn name<T:Option<m.Base>>(x:T) -> String? = x?.name;", true),
        (
            "fn name<T:Option<m.Base>>(x:T) -> String = match x { Some(value) => value.name, none => \"absent\" };",
            true,
        ),
        (
            "fn unsafe<T:Option<m.Base>>(x:T) -> String = x.name;",
            false,
        ),
        ("fn wrong<T:Option<m.Base>>(x:T) -> Bool? = x?.name;", false),
        ("fn fabricate<T:Option<m.Base>>(x:T) -> T = none;", false),
    ] {
        let mut entry = source(
            "optional_bound",
            &format!("import model \"urn:optional-bound\" as m; {body}"),
        );
        entry.dependencies.push(model.id.clone());
        let result = check(&bundle(vec![model.clone(), entry.clone()]), &entry.id);
        if valid {
            assert!(result.is_ok(), "{body}: {result:?}");
        } else {
            let errors = result.unwrap_err();
            assert!(!errors.is_empty());
            assert!(
                errors
                    .iter()
                    .all(|e| e.span.source == entry.id && e.span.start < e.span.end),
                "{body}: {errors:?}"
            );
        }
    }
}

#[test]
fn partially_contextual_structures_do_not_freeze_a_provisional_sibling_lower_bound() {
    let model = DefinitionSource::new(
        "memory:partial-siblings.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='partial' nsURI='urn:partial-siblings'><eClassifiers x:type='e:EClass' name='Base'/><eClassifiers x:type='e:EClass' name='Left' eSuperTypes='#//Base'/><eClassifiers x:type='e:EClass' name='Right' eSuperTypes='#//Base'/></e:EPackage>",
    );
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    for swapped in [false, true] {
        let (first, second) = if swapped {
            ("right", "left")
        } else {
            ("left", "right")
        };
        for (signature, arguments) in [
            ("a:T,b:(T,Sequence<T>)", format!("{first},({second},[])")),
            ("b:(T,Sequence<T>),a:T", format!("({second},[]),{first}")),
            (
                "a:T,b:{value:T,items:Sequence<T>}",
                format!("{first},{{value:{second},items:[]}}"),
            ),
            (
                "b:{value:T,items:Sequence<T>},a:T",
                format!("{{items:[],value:{second}}},{first}"),
            ),
        ] {
            let body = format!(
                "import model \"urn:partial-siblings\" as m; fn select<T>({signature}) -> T = a; fn joined(left:m.Left,right:m.Right) -> m.Base = {{ let inferred = select({arguments}); inferred }};"
            );
            let mut entry = source("partial_siblings", &body);
            entry.dependencies.push(model.id.clone());
            let result = check(&bundle(vec![model.clone(), entry.clone()]), &entry.id);
            assert!(result.is_ok(), "{body}: {result:?}");
            let explicit = body.replace("select(", "select::<m.Left>(");
            let mut entry = source("partial_siblings", &explicit);
            entry.dependencies.push(model.id.clone());
            let errors = check(&bundle(vec![model.clone(), entry.clone()]), &entry.id).unwrap_err();
            assert!(!errors.is_empty());
            assert!(
                errors
                    .iter()
                    .all(|e| e.span.source == entry.id && e.span.start < e.span.end),
                "{explicit}: {errors:?}"
            );
        }
    }
}

#[test]
fn inference_uses_rigid_structural_capabilities_without_erasing_identity_actuals() {
    good(
        "fn first<T>(xs:Sequence<T>) -> T? = xs[0]; fn wrapper<S:Sequence<Int>>(s:S) -> Int? = first(s); fn id<T>(x:T) -> T = x; fn retain<S:Sequence<Int>>(s:S) -> S = id(s);",
    );
    good(
        "fn first<T>(x:(T,Bool)) -> T = x[0]; fn wrapper<S:(Int,Bool)>(s:S) -> Int = first(s); fn id<T>(x:T) -> T = x; fn retain<S:(Int,Bool)>(s:S) -> S = id(s);",
    );
    good(
        "fn field<T>(x:{value:T}) -> T = x.value; fn wrapper<S:{value:Int}>(s:S) -> Int = field(s); fn id<T>(x:T) -> T = x; fn retain<S:{value:Int}>(s:S) -> S = id(s);",
    );
    good(
        "fn apply<T>(f:(Int)->T) -> T = f(1); fn wrapper<S:(Int)->Bool>(s:S) -> Bool = apply(s); fn id<T>(x:T) -> T = x; fn retain<S:(Int)->Bool>(s:S) -> S = id(s);",
    );
    bad("fn id<T>(x:T) -> T = x; fn fabricate<S:Sequence<Int>>(s:S) -> S = id([1]);");
    bad(
        "fn first<T>(xs:Sequence<T>) -> T? = xs[0]; fn wrong<S:Sequence<Int>>(s:S) -> Bool? = first(s);",
    );
}

#[test]
fn deferred_none_ok_and_err_calls_obtain_complete_sibling_or_result_context() {
    good(
        "fn choose<T>(a:T,b:T) -> T = a; fn before(x:Int?) -> Int? = { let inferred = choose(None(),x); inferred }; fn after(x:Int?) -> Int? = { let inferred = choose(x,None()); inferred };",
    );
    good(
        "fn choose<T>(a:T,b:T) -> T = a; fn before(x:Result<Int,String>) -> Result<Int,String> = { let inferred = choose(Ok(1),x); inferred }; fn after(x:Result<Int,String>) -> Result<Int,String> = { let inferred = choose(x,Ok(1)); inferred };",
    );
    good(
        "fn choose<T>(a:T,b:T) -> T = a; fn before(x:Result<Int,String>) -> Result<Int,String> = { let inferred = choose(Err(\"error\"),x); inferred }; fn after(x:Result<Int,String>) -> Result<Int,String> = { let inferred = choose(x,Err(\"error\")); inferred };",
    );
    good(
        "fn id<T>(x:T) -> T = x; fn none_result() -> Int? = id(None()); fn ok_result() -> Result<Int,String> = id(Ok(1)); fn err_result() -> Result<Int,String> = id(Err(\"error\"));",
    );
    bad("fn id<T>(x:T) -> T = x; fn missing() -> Int = { let unresolved = id(None()); 1 };");
    bad("fn id<T>(x:T) -> T = x; fn missing() -> Int = { let unresolved = id(Ok(1)); 1 };");
    bad(
        "fn choose<T>(a:T,b:T) -> T = a; fn wrong(x:Result<Int,String>) -> Result<Int,String> = choose(Ok(true),x);",
    );
}

#[test]
fn nested_rigid_option_patterns_expand_shape_but_keep_whole_capture_identity() {
    good(
        "fn nested<T:Option<Int>>(x:Option<T>) -> Int = match x { Some(Some(value)) => value, Some(none) => 0, none => 1 };",
    );
    good(
        "fn capture<T:Option<Int>>(x:Option<T>,fallback:T) -> T = match x { Some(value) => value, none => fallback };",
    );
    for body in [
        "fn missing<T:Option<Int>>(x:Option<T>) -> Int = match x { Some(Some(value)) => value, none => 0 };",
        "fn wrong<T:Option<Int>>(x:Option<T>) -> Bool = match x { Some(Some(value)) => value, Some(none) => false, none => false };",
        "fn fabricate<T:Option<Int>>(x:Option<T>) -> T = match x { Some(Some(value)) => Some(value), Some(none) => none, none => none };",
    ] {
        bad(body);
    }
}

#[test]
fn deferred_context_crosses_expression_wrappers_and_nested_generic_calls() {
    good(
        "fn same<T>(a:T,b:T) -> T = a; fn block(xs:Sequence<Int>) -> Sequence<Int> = { let inferred = same({ let n = 1; [] },xs); inferred }; fn conditional(xs:Sequence<Int>) -> Sequence<Int> = { let inferred = same(if true { [] } else { xs },xs); inferred };",
    );
    good(
        "fn same<T>(a:T,b:T) -> T = a; fn matched(xs:Sequence<Int>,flag:Bool) -> Sequence<Int> = { let inferred = same(match flag { true => [], false => [] },xs); inferred };",
    );
    good(
        "fn apply<T>(f:(Int)->T,x:T) -> T = x; fn lambda(x:Option<Int>) -> Option<Int> = { let inferred = apply(|n:Int| none,x); inferred };",
    );
    good(
        "fn empty<T>() -> Sequence<T> = []; fn same<T>(a:T,b:T) -> T = a; fn before(xs:Sequence<Int8>) -> Sequence<Int8> = { let inferred = same(empty(),xs); inferred }; fn after(xs:Sequence<Int8>) -> Sequence<Int8> = { let inferred = same(xs,empty()); inferred };",
    );
    for body in [
        "fn same<T>(a:T,b:T) -> T = a; fn wrong(xs:Sequence<Int>) -> Sequence<Int> = same({ let n = 1; [true] },xs);",
        "fn same<T>(a:T,b:T) -> T = a; fn wrong(xs:Sequence<Int>) -> Sequence<Int> = same(if true { [] } else { [true] },xs);",
        "fn same<T>(a:T,b:T) -> T = a; fn wrong(xs:Sequence<Int>,flag:Bool) -> Sequence<Int> = same(match flag { true => [], false => [true] },xs);",
        "fn apply<T>(f:(Int)->T,x:T) -> T = x; fn wrong(x:Option<Int>) -> Option<Int> = apply(|n:Int| Some(true),x);",
        "fn empty<T>() -> Sequence<T> = []; fn same<T>(a:T,b:T) -> T = a; fn wrong(xs:Sequence<Int>) -> Sequence<Int> = same(empty::<Bool>(),xs);",
    ] {
        bad(body);
    }
}
