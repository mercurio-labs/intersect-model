use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function,
        ir::IrErrorKind,
        ir_v2::{self, EntrySelection, Limits, TypeNode},
        modules::SymbolId,
    },
};

#[test]
fn exact_type_table_count_and_function_type_depth_reverify_and_recover() {
    let source = DefinitionSource::new(
        "memory:type-boundaries-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1;package type_boundaries version \"1\";fn run(x:Option<Option<Int8>>)->Option<Option<Int8>> =identity(x);fn identity(x:Option<Option<Int8>>)->Option<Option<Int8>> =x;",
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let selection = EntrySelection {
        symbol: SymbolId {
            source: source.id.clone(),
            declaration: 0,
        },
        types: vec![],
        arguments: vec![],
    };
    let image = compile_function(&bundle, &source.id, &selection, &Limits::default()).unwrap();
    let program = image.program();
    // The retained callable takes and returns Option<Option<Int8>>. Its
    // structural depth is four: callable -> Option -> Option -> scalar.
    assert!(
        program
            .types
            .iter()
            .any(|ty| matches!(ty, TypeNode::Function { .. }))
    );
    assert!(program.types.len() >= 4);
    let exact = Limits {
        max_types: program.types.len(),
        max_type_depth: 4,
        ..Default::default()
    };
    assert!(ir_v2::verify(program.clone(), &bundle, &exact).is_ok());
    for limits in [
        Limits {
            max_types: program.types.len() - 1,
            ..exact.clone()
        },
        Limits {
            max_type_depth: 3,
            ..exact.clone()
        },
    ] {
        let error = ir_v2::verify(program.clone(), &bundle, &limits).unwrap_err();
        assert_eq!(error.kind, IrErrorKind::ResourceExhausted);
        assert_eq!(
            error.message,
            if limits.max_types < program.types.len() {
                "Function IR work limit exceeded"
            } else {
                "Function type depth limit exceeded"
            }
        );
        let recovered = ir_v2::verify(program.clone(), &bundle, &exact).unwrap();
        assert_eq!(recovered.program(), program);
    }
}
