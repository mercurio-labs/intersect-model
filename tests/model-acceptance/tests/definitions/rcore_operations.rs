use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    generic_types::{RuntimeType, Type},
    rcore::lower_structure,
    rcore_behavior::{
        function_lowering::compile_function,
        ir_v2::{EntrySelection, Limits},
        modules::SymbolId,
    },
    rcore_operations::verify_binding,
};
fn check(
    declaration: &str,
    body: &str,
) -> Result<
    intersect_model::definitions::rcore_operations::OperationBinding,
    Vec<intersect_model::definitions::rcore_behavior::SyntaxDiagnostic>,
> {
    let source = DefinitionSource::new(
        "memory:model.rcore",
        SourceKind::Rcore,
        format!(
            "rcore 1; package demo version \"1\"; metamodel \"urn:ops\" prefix d {{ class Node {{ ref links:Node [0,*] nonunique; {declaration} }} class Other {{}} }}"
        ),
    );
    let model = source;
    let mut entry = DefinitionSource::new(
        "memory:behavior.rcore",
        SourceKind::RcoreBehavior,
        format!(
            "rcore 1; package demo.behavior version \"1\"; import model \"urn:ops\" as m; {body}"
        ),
    );
    entry.dependencies.push(model.id.clone());
    let selection = EntrySelection {
        symbol: SymbolId {
            source: entry.id.clone(),
            declaration: 1,
        },
        types: vec![],
        arguments: vec![],
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![model.clone(), entry.clone()],
        namespaces: vec![],
    };
    let (_, operations) =
        intersect_model::definitions::rcore::resolve_structure(&bundle, &model.id).unwrap();
    let image = compile_function(&bundle, &entry.id, &selection, &Limits::default()).unwrap();
    let receiver = RuntimeType::new(Type::Classifier {
        declaration: operations[0].owner.clone(),
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    let bound = verify_binding(
        &bundle,
        &image,
        &receiver,
        &operations[0].declaration,
        std::slice::from_ref(&receiver),
        &Limits::default(),
    );
    if let Ok(binding) = &bound {
        assert_eq!(binding.implementation_span.source, entry.id);
        assert_eq!(
            binding.declaration_span.source,
            operations[0].declaration.source
        );
        assert_eq!(binding.operation.declaration, operations[0].declaration);
        assert_eq!(binding.receiver, receiver);
        let mut edited = bundle.clone();
        edited.sources.retain(|s| s.id != entry.id);
        assert!(
            verify_binding(
                &edited,
                &image,
                &receiver,
                &binding.operation.declaration,
                std::slice::from_ref(&receiver),
                &Limits::default()
            )
            .is_err()
        );
    }
    bound
}
#[test]
fn receiver_collection_optional_scalar_and_parameter_signatures_are_bound_exactly() {
    for (op, body) in [
        (
            "op reachable()->Node [0,*];",
            "fn main(node:m.Node)->OrderedSet<m.Node> {closure([node],|n|n.links)}",
        ),
        (
            "op itself()->Node [1,1];",
            "fn main(node:m.Node)->m.Node {node}",
        ),
        (
            "op maybe()->Node [0,1];",
            "fn main(node:m.Node)->m.Node? {Some(node)}",
        ),
        (
            "op echo(text:String [1,1])->String [1,1];",
            "fn main(node:m.Node,text:String)->String {text}",
        ),
        (
            "op optional(text:String [0,1])->String [0,1];",
            "fn main(node:m.Node,text:String?)->String? {text}",
        ),
    ] {
        let result = check(op, body);
        assert!(result.is_ok(), "{op}: {result:?}");
    }
}
#[test]
fn mismatched_receiver_arity_parameter_result_and_collection_contracts_reject() {
    for (op, body) in [
        (
            "op reachable()->Node [0,*];",
            "fn main(node:m.Other)->OrderedSet<m.Node> {OrderedSet([])}",
        ),
        (
            "op reachable()->Node [0,*];",
            "fn main()->OrderedSet<m.Node> {OrderedSet([])}",
        ),
        (
            "op reachable()->Node [0,*];",
            "fn main(node:m.Node)->Sequence<m.Node> {[node]}",
        ),
        (
            "op reachable()->Node [0,*] unordered;",
            "fn main(node:m.Node)->OrderedSet<m.Node> {OrderedSet([node])}",
        ),
        (
            "op reachable()->Node [1,*];",
            "fn main(node:m.Node)->OrderedSet<m.Node> {OrderedSet([node])}",
        ),
        (
            "op echo(text:String [1,1])->String [1,1];",
            "fn main(node:m.Node,text:Bool)->String {\"wrong\"}",
        ),
        (
            "op echo()->String [1,1];",
            "fn main(node:m.Node)->Bool {true}",
        ),
        (
            "op itself()->Node [1,1];",
            "fn main(node:m.Node)->m.Node? {Some(node)}",
        ),
    ] {
        let errors = check(op, body).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.span.source.uri == "memory:behavior.rcore"),
            "{op}: {errors:?}"
        );
    }
}
#[test]
fn malformed_operations_are_explicit_and_original_spans_survive_lowering() {
    for op in [
        "op a()->Missing [0,*];",
        "op a(x:Node [2,1])->Node [1,1];",
        "op a(x:Node [1,1],x:Node [1,1])->Node [1,1];",
        "op a()->Node [0,*] containment;",
        "op a()->Node [1,1] {self}",
        "op a()->Node [1,1]; op a()->Node [1,1];",
    ] {
        let source = DefinitionSource::new(
            "memory:bad.rcore",
            SourceKind::Rcore,
            format!(
                "// café😀\nrcore 1; package demo version \"1\"; metamodel \"urn:ops\" prefix d {{ class Node {{{op}}} }}"
            ),
        );
        for error in lower_structure(source.clone(), "memory:g.ecore").unwrap_err() {
            assert_eq!(error.span.source, source.id);
            assert!(source.text.is_char_boundary(error.span.start));
            assert!(source.text.is_char_boundary(error.span.end));
        }
    }
}
