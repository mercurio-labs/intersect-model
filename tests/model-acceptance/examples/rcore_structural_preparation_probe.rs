//! Verification-only shared native/WASM structural and actual consumer controls.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    conversion::{
        ConversionRegistry, InvocationContext, InvocationObserver, ProviderId, ReferenceContext,
    },
    ecore::{Document, ErrorKind, Node, ObservedImportError, ParseLimits},
    generic_types::{RuntimeType, Type},
    rcore::{DocumentPreparation, DocumentStage},
    rcore_behavior::LexicalPreparation,
    resources::{DeclarationId, ResourceSet, ResourceSetPreparation},
    schema::SharedRuntimeSourceArrayTarget,
};
use intersect_model::model_reference::{ReferenceSchema, ReferenceSchemaFactory};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{cell::Cell, rc::Rc};
fn check(value: bool, message: &str) -> Result<(), String> {
    if value { Ok(()) } else { Err(message.into()) }
}
#[derive(Default)]
struct Meter {
    calls: Cell<usize>,
    work: Cell<usize>,
    bytes: Cell<usize>,
    stop: Option<usize>,
    max_work: Option<usize>,
    max_bytes: Option<usize>,
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected provider".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        let current = self.calls.get();
        self.calls.set(current + 1);
        if self.stop == Some(current) {
            Err("structural interrupted".into())
        } else {
            Ok(())
        }
    }
    fn work(&self, amount: usize) -> Result<(), String> {
        self.checkpoint()?;
        let next = self.work.get() + amount;
        if self.max_work.is_some_and(|limit| next > limit) {
            return Err("structural work denied".into());
        }
        self.work.set(next);
        Ok(())
    }
    fn retain_storage(&self, amount: usize) -> Result<(), String> {
        self.checkpoint()?;
        let next = self.bytes.get() + amount;
        if self.max_bytes.is_some_and(|limit| next > limit) {
            return Err("structural storage denied".into());
        }
        self.bytes.set(next);
        Ok(())
    }
}

fn prepared(source: &DefinitionSource) -> Result<DocumentPreparation<'_>, String> {
    let mut lexical = LexicalPreparation::new(source);
    while !lexical.advance(&InvocationContext::default())? {}
    DocumentPreparation::from_lexical(lexical.finish()?, ParseLimits::default())
        .map_err(|e| e.to_string())
}
fn document(source: &DefinitionSource) -> Result<Document, String> {
    let mut pending = prepared(source)?;
    while !pending
        .advance(&InvocationContext::default())
        .map_err(|e| e.to_string())?
    {}
    pending.finish().map_err(|e| e.to_string())
}
fn compatibility() -> Result<(), String> {
    #[derive(Deserialize)]
    struct Capture {
        cases: Vec<Case>,
        successes: usize,
    }
    #[derive(Deserialize)]
    struct Case {
        source: DefinitionSource,
        expected_sha256: String,
        label: String,
        ok: bool,
    }
    let bytes = include_bytes!(
        "../../../crates/intersect-model/assets/fixtures/rcore-structural-baseline.json"
    );
    check(
        format!("{:x}", Sha256::digest(bytes))
            == "132e49675b3f846f335968d0e364ac8ce4f57d9fc0c6064246544eefb51fff7a",
        "Frozen pre-change structural capture changed",
    )?;
    let capture: Capture = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    check(
        capture.cases.len() == 2508 && capture.successes == 457,
        "Incomplete pre-change structural inventory",
    )?;
    let mut successes = 0;
    for case in capture.cases {
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![case.source],
            namespaces: vec![],
        };
        let meter = Rc::new(Meter::default());
        let result = ResourceSet::new_observed(&bundle, &InvocationContext::new(meter.clone()));
        let ok = result.is_ok();
        let rendered =
            |result: Result<ResourceSet, ObservedImportError>| -> Result<String, String> {
                match result {
                    Ok(resources) => {
                        Ok(format!("{:?}", resources.document(&bundle.sources[0].id)?))
                    }
                    Err(error) => Ok(format!("{error:?}")),
                }
            };
        let result = rendered(result)?;
        let encoded = serde_json::to_vec(&serde_json::json!({"ok":ok,"result":result,"work":meter.work.get(),"bytes":meter.bytes.get()})).map_err(|e| e.to_string())?;
        if ok != case.ok || format!("{:x}", Sha256::digest(encoded)) != case.expected_sha256 {
            return Err(format!(
                "Frozen structural case {} changed: {result}; work={}, bytes={}",
                case.label,
                meter.work.get(),
                meter.bytes.get()
            ));
        }
        if ok {
            successes += 1;
        }
        let mut previous = None;
        for retained in [false, true] {
            let meter = Rc::new(Meter::default());
            let output = rendered(run_resource(
                &bundle,
                ParseLimits::default(),
                retained,
                meter.clone(),
            ))?;
            check(
                output == result,
                &format!(
                    "Prepared structural document/error changed for {}",
                    case.label
                ),
            )?;
            let current = (output, meter.work.get(), meter.bytes.get());
            if let Some(previous) = previous {
                // Retained bundle admission has historically reserved its 2,048-byte
                // state before source validation; ordinary begin validates first.
                let equal = current == previous;
                let early_validation = bundle.validate().is_err()
                    && current.0 == previous.0
                    && current.1 == previous.1
                    && current.2 == previous.2 + 2048;
                check(
                    equal || early_validation,
                    &format!(
                        "Retained structural receipts changed for {}: {current:?} vs {previous:?}",
                        case.label
                    ),
                )?;
            }
            previous = Some(current);
        }
    }
    check(
        successes == 457,
        "Lost complete frozen structural documents",
    )
}
const SIMPLE: &str = "rcore 1;package p version \"1\";metamodel \"urn:p\" prefix p {class A {attr x: Int32 [0,1] default -7;}}behavior {{}}";
fn literal_source() -> DefinitionSource {
    DefinitionSource::new("memory:literal.rcore", SourceKind::Rcore, SIMPLE)
}
fn attrs(node: &Node) -> Vec<(&str, &str)> {
    node.attributes
        .iter()
        .map(|a| (a.lexical_name.as_str(), a.value.as_str()))
        .collect()
}
fn literals() -> Result<(), String> {
    let source = literal_source();
    let result = document(&source)?;
    let root = &result.root;
    check(
        root.name.local == "EPackage"
            && root.name.namespace.as_deref() == Some("http://www.eclipse.org/emf/2002/Ecore")
            && root.span.start == 30
            && root.span.end == 100
            && root.opening == root.span
            && root.children.len() == 1
            && attrs(root) == [("name", "p"), ("nsURI", "urn:p"), ("nsPrefix", "p")],
        "Literal root/name/order/span changed",
    )?;
    let class = &root.children[0];
    check(
        class.name.local == "eClassifiers"
            && class.span.start == 58
            && class.span.end == 99
            && class.children.len() == 1
            && attrs(class) == [("xsi:type", "ecore:EClass"), ("name", "A")],
        "Literal class type/span/attributes changed",
    )?;
    let feature = &class.children[0];
    check(
        feature.name.local == "eStructuralFeatures"
            && feature.span.start == 67
            && feature.span.end == 98
            && attrs(feature)
                == [
                    ("xsi:type", "ecore:EAttribute"),
                    ("name", "x"),
                    ("eType", "http://www.eclipse.org/emf/2002/Ecore#//EInt"),
                    ("lowerBound", "0"),
                    ("upperBound", "1"),
                    ("defaultValueLiteral", "-7"),
                ],
        "Literal feature type/default/span changed",
    )?;
    for node in [root, class, feature] {
        check(
            node.span.source == source.id
                && node.attributes.iter().all(|a| a.span == node.span)
                && node.payloads.is_empty(),
            "Literal source/attribute identity lost",
        )?;
    }
    let ty = &class.attributes[0];
    check(
        ty.name.local == "type"
            && ty.name.namespace.as_deref() == Some("http://www.w3.org/2001/XMLSchema-instance")
            && ty.type_name.as_ref().is_some_and(|n| {
                n.local == "EClass"
                    && n.namespace.as_deref() == Some("http://www.eclipse.org/emf/2002/Ecore")
            }),
        "Literal expanded classifier type changed",
    )?;
    check(
        result.source == source && result.misc.is_empty(),
        "Literal document lost source",
    )?;
    Ok(())
}
fn rich_source() -> DefinitionSource {
    DefinitionSource::new(
        "memory:rich.rcore",
        SourceKind::Rcore,
        r##"rcore 1;package p version "1";resource {
package left namespace "urn:left" prefix l {
identity "root";
annotation "root-note" { detail "key" = "value"; reference "#//Base"; annotation "nested" {} }
interface class Base<T extends String> identity "base" instanceClass "Host" instanceType "T" {}
class C<U extends String> extends Base<String>, external "remote#//Base" {
attr label: String [0,1] default "hello" unordered nonunique derived readonly transient volatile unsettable id annotation "feature" {};
ref links: C [0,*] containment noresolve opposite C.links;
attr mode: Mode [1,1] default Mode.A;
attr box: generic Base<? extends String> [0,1];
attr lower: Base<? super Int32> [0,1];
attr parameter: parameter "#//@eClassifiers.1/@eTypeParameters.0" [0,1];
op echo<V extends String>(arg: V [0,*] unordered nonunique) -> U [0,1] throws external "remote#//Exception" annotation "operation" {};
op nothing() -> void;
}
enum Mode identity "mode" nonserializable { A = -2147483648 literal "alpha" annotation "literal" {}; B = 2147483647; annotation "enum" {} }
datatype Opaque<T> identity "opaque" instanceClass "Host" instanceType "Opaque<T>" nonserializable {annotation "datatype" {}}
package sub namespace "urn:sub" prefix s { class Nested extends Base<String> {} }
}
package right namespace "urn:right" prefix r {class Other {attr externalType: external "remote#//Scalar" [0,1];}}
}behavior { fn ignored() { { } } }"##,
    )
}
fn rich_forms() -> Result<(), String> {
    let source = rich_source();
    let result = document(&source)?;
    check(
        result.root.name.local == "XMI" && result.root.children.len() == 2,
        "Lost multi-root resource",
    )?;
    let left = &result.root.children[0];
    let right = &result.root.children[1];
    check(
        attrs(left).contains(&("xmi:id", "root")) && attrs(right).contains(&("nsURI", "urn:right")),
        "Lost package metadata/order",
    )?;
    let base = &left.children[0];
    let class = &left.children[1];
    let enumeration = &left.children[2];
    let datatype = &left.children[3];
    check(
        attrs(base).contains(&("interface", "true"))
            && attrs(base).contains(&("instanceClassName", "Host"))
            && base.children[0].name.local == "eTypeParameters",
        "Lost classifier metadata/generic parameter",
    )?;
    let label = &class.children[0];
    let links = &class.children[1];
    let mode = &class.children[2];
    check(
        attrs(label).contains(&("changeable", "false"))
            && attrs(label).contains(&("ordered", "false"))
            && attrs(label).contains(&("unique", "false"))
            && attrs(label).contains(&("iD", "true"))
            && attrs(label).contains(&("defaultValueLiteral", "hello"))
            && label.children[0].name.local == "eAnnotations",
        "Lost flags/default/annotation",
    )?;
    check(
        attrs(links).contains(&("upperBound", "-1"))
            && attrs(links).contains(&("containment", "true"))
            && attrs(links).contains(&("resolveProxies", "false"))
            && attrs(links).contains(&("eOpposite", "#//C/links"))
            && attrs(mode).contains(&("defaultValueLiteral", "alpha")),
        "Lost reference/opposite/enum default",
    )?;
    check(
        class.children[3].children[0].name.local == "eGenericType"
            && class.children[3].children[0].children[0].children[0]
                .name
                .local
                == "eUpperBound"
            && class.children[4].children[0].children[0].children[0]
                .name
                .local
                == "eLowerBound"
            && attrs(&class.children[5].children[0])
                .contains(&("eTypeParameter", "#//@eClassifiers.1/@eTypeParameters.0")),
        "Lost explicit/wildcard/parameter types",
    )?;
    let operation = &class.children[6];
    check(
        operation.name.local == "eOperations"
            && attrs(operation).contains(&("name", "echo"))
            && operation
                .children
                .iter()
                .any(|n| n.name.local == "eGenericExceptions")
            && operation
                .children
                .iter()
                .any(|n| n.name.local == "eParameters")
            && operation
                .children
                .iter()
                .any(|n| n.name.local == "eTypeParameters"),
        "Lost operation result/arguments/generics/exception",
    )?;
    check(
        attrs(&enumeration.children[0]).contains(&("value", "-2147483648"))
            && attrs(&enumeration.children[1]).contains(&("value", "2147483647"))
            && attrs(datatype).contains(&("serializable", "false"))
            && left.children[4].name.local == "eSubpackages"
            && left.children[5].name.local == "eAnnotations"
            && left.children[5].children[1].name.local == "eAnnotations",
        "Lost enum bounds/datatype/nested package/annotation ordering",
    )?;
    Ok(())
}

fn boundaries_and_failures() -> Result<(), String> {
    let header = "rcore 1;package p version \"1\";metamodel \"urn:p\" prefix p {";
    for (name, positive, negative, message) in [
        (
            "type",
            format!("class C {{attr x: {}String [0,1];}}", "generic ".repeat(63)),
            format!("class C {{attr x: {}String [0,1];}}", "generic ".repeat(64)),
            "Type nesting budget exceeded",
        ),
        (
            "annotation",
            format!("{}{}", "annotation \"a\" {".repeat(64), "}".repeat(64)),
            format!("{}{}", "annotation \"a\" {".repeat(65), "}".repeat(65)),
            "Annotation nesting budget exceeded",
        ),
        (
            "package",
            format!(
                "{}class C {{}}{}",
                "package sub namespace \"urn:sub\" prefix s {".repeat(63),
                "}".repeat(63)
            ),
            format!(
                "{}class C {{}}{}",
                "package sub namespace \"urn:sub\" prefix s {".repeat(64),
                "}".repeat(64)
            ),
            "Package nesting budget exceeded",
        ),
    ] {
        let source = DefinitionSource::new(
            "memory:depth.rcore",
            SourceKind::Rcore,
            format!("{header}{positive}}}"),
        );
        document(&source).map_err(|e| format!("Positive {name} depth: {e}"))?;
        let bad = DefinitionSource::new(
            "memory:depth.rcore",
            SourceKind::Rcore,
            format!("{header}{negative}}}"),
        );
        let mut pending = prepared(&bad)?;
        let error = loop {
            match pending.advance(&InvocationContext::default()) {
                Ok(false) => {}
                Ok(true) => return Err(format!("Exceeded {name} nesting admitted")),
                Err(e) => break e,
            }
        };
        check(
            matches!(&error,ObservedImportError::Import(e) if e.kind==ErrorKind::InvalidDefinition && e.message==message && e.span.source==bad.id)
                && pending.advance(&InvocationContext::default()) == Err(error.clone())
                && matches!(pending.finish(),Err(e) if e==error),
            "Nesting failure lost diagnostic or terminal semantics",
        )?;
    }
    for depth in [64, 65] {
        let source = DefinitionSource::new(
            "memory:behavior.rcore",
            SourceKind::Rcore,
            format!(
                "{header}}}behavior {}{}",
                "{".repeat(depth),
                "}".repeat(depth)
            ),
        );
        let result = document(&source);
        check(
            result.is_ok() == (depth == 64),
            "Behavior range nesting policy changed",
        )?;
        if depth == 65 {
            check(
                result
                    .err()
                    .is_some_and(|e| e.contains("Behavior nesting budget exceeded")),
                "Wrong behavior nesting diagnostic",
            )?;
        }
    }
    let source = literal_source();
    for exact in [true, false] {
        let mut lexical = LexicalPreparation::new(&source);
        while !lexical.advance(&InvocationContext::default())? {}
        let result = DocumentPreparation::from_lexical(
            lexical.finish()?,
            ParseLimits {
                source_bytes: source.text.len() - usize::from(!exact),
            },
        );
        if exact {
            check(result.is_ok(), "Exact structural source limit rejected")?;
        } else {
            check(
                matches!(result,Err(e) if e.kind==ErrorKind::ResourceLimit && e.message=="Rcore source size budget exceeded" && e.span.start==0 && e.span.end==113 && e.span.source==source.id),
                "One-short structural size policy changed",
            )?;
        }
    }
    for (suffix, message) in [
        (
            "class C {attr x: Missing [0,1];}",
            "Unknown structural type Missing",
        ),
        (
            "class C {attr x: String [1,0];}",
            "Lower bound exceeds upper bound",
        ),
        (
            "class C {attr x: Int32 [0,2147483648];}",
            "Multiplicity exceeds Ecore integer range",
        ),
        ("class C {} class C {}", "Duplicate classifier/package name"),
    ] {
        let source = DefinitionSource::new(
            "memory:failure.rcore",
            SourceKind::Rcore,
            format!("{header}{suffix}}}"),
        );
        check(
            document(&source).err().is_some_and(|e| e.contains(message)),
            "Literal structural syntax/tree rejection changed",
        )?;
    }
    Ok(())
}
fn progress_and_interruptions() -> Result<(), String> {
    for source in [literal_source(), rich_source()] {
        progress_source(source)?;
    }
    Ok(())
}
fn progress_source(mut source: DefinitionSource) -> Result<(), String> {
    source.text = format!("{} /*{}*/", source.text, "λ🚀".repeat(200));
    source.id = DefinitionSource::new(source.id.uri.clone(), source.kind, source.text.clone()).id;
    let dep = DefinitionSource::new(
        format!("dependency:{}", "α".repeat(300)),
        SourceKind::Xtext,
        "revision",
    )
    .id;
    source.dependencies = vec![dep.clone(), dep];
    let input = source.text.as_ptr();
    let deps = source.dependencies.as_ptr();
    let expected = format!("{:?}", document(&source)?);
    let mut total = 0;
    for quantum in [1, 3, 256] {
        let mut pending = prepared(&source)?;
        let mut previous = pending.progress();
        let mut steps = 0;
        let mut complete = false;
        let mut stages = [false; 4];
        while !complete {
            for _ in 0..quantum {
                let meter = Rc::new(Meter::default());
                let weak = Rc::downgrade(&meter);
                let context = InvocationContext::new(meter.clone());
                complete = pending.advance(&context).map_err(|e| e.to_string())?;
                drop(context);
                drop(meter);
                check(
                    weak.upgrade().is_none(),
                    "Document retained an invocation observer",
                )?;
                let next = pending.progress();
                steps += 1;
                check(
                    next.consumed_tokens - previous.consumed_tokens + next.tree_transitions
                        - previous.tree_transitions
                        <= 1
                        && next.copied_text_bytes - previous.copied_text_bytes <= 256,
                    "One structural advance exceeded its token/tree/copy transition",
                )?;
                stages[match next.stage {
                    DocumentStage::Syntax => 0,
                    DocumentStage::Tree => 1,
                    DocumentStage::SourceCopy => 2,
                    DocumentStage::Complete => 3,
                }] = true;
                previous = next;
                check(
                    pending.source().text.as_ptr() == input && source.dependencies.as_ptr() == deps,
                    "Retained structural input allocation changed",
                )?;
                if complete {
                    break;
                }
            }
        }
        check(
            stages.into_iter().all(|s| s)
                && previous.consumed_tokens > 30
                && previous.tree_transitions > 15
                && previous.copied_text_bytes == source.text.len(),
            "Missing structural progress stage",
        )?;
        check(
            format!("{:?}", pending.finish().map_err(|e| e.to_string())?) == expected,
            "Resume quantum changed complete document",
        )?;
        if quantum == 1 {
            total = steps;
        }
    }
    check(
        prepared(&source)?.finish().is_err(),
        "Incomplete structural output published",
    )?;
    for prefix in 0..total {
        let mut pending = prepared(&source)?;
        for _ in 0..prefix {
            check(
                !pending
                    .advance(&InvocationContext::default())
                    .map_err(|e| e.to_string())?,
                "Early structural completion",
            )?;
        }
        let failed = InvocationContext::new(Rc::new(Meter {
            stop: Some(0),
            ..Default::default()
        }));
        let error = pending
            .advance(&failed)
            .err()
            .ok_or("Missed structural cancellation")?;
        check(
            matches!(&error,ObservedImportError::Observation(m) if m=="structural interrupted")
                && pending.advance(&InvocationContext::default()) == Err(error.clone())
                && matches!(pending.finish(),Err(e) if e==error),
            "Structural cancellation resumed or published",
        )?;
    }
    check(
        format!("{:?}", document(&source)?) == expected
            && source.text.as_ptr() == input
            && source.dependencies.as_ptr() == deps,
        "Fresh structural recovery changed input",
    )?;
    Ok(())
}
const NS: &str = "urn:structural";
fn fixture() -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        namespaces: vec![],
        sources: vec![DefinitionSource::new(
            "memory:structural.rcore",
            SourceKind::Rcore,
            format!(
                "rcore 1; package structural version \"1\"; /*{}*/ metamodel \"urn:structural\" prefix lx {{ class Item {{}} }}",
                "λ🚀".repeat(150)
            ),
        )],
    }
}
fn run_resource(
    bundle: &DefinitionBundle,
    limits: ParseLimits,
    retained: bool,
    meter: Rc<Meter>,
) -> Result<ResourceSet, ObservedImportError> {
    let operation = InvocationContext::new(meter);
    let mut pending = if retained {
        ResourceSetPreparation::begin_with_retained_bundle(bundle, limits, &operation)
    } else {
        ResourceSetPreparation::begin(bundle, limits, &operation)
    }?;
    while !pending.advance(&operation)? {}
    pending.finish()
}
fn resources() -> Result<(), String> {
    let bundle = fixture();
    let old = Rc::new(Meter::default());
    let meter = Rc::new(Meter::default());
    let expected = run_resource(&bundle, ParseLimits::default(), false, old.clone())
        .map_err(|e| e.to_string())?;
    let current = run_resource(&bundle, ParseLimits::default(), true, meter.clone())
        .map_err(|e| e.to_string())?;
    check(
        format!("{current:?}") == format!("{expected:?}")
            && current.document(&bundle.sources[0].id)?.source == bundle.sources[0]
            && (meter.work.get(), meter.bytes.get()) == (old.work.get(), old.bytes.get()),
        "Retained Rcore documents or logical receipts changed",
    )?;
    for (work, bytes, success) in [
        (meter.work.get(), meter.bytes.get(), true),
        (meter.work.get() - 1, meter.bytes.get(), false),
        (meter.work.get(), meter.bytes.get() - 1, false),
    ] {
        check(
            run_resource(
                &bundle,
                ParseLimits::default(),
                true,
                Rc::new(Meter {
                    max_work: Some(work),
                    max_bytes: Some(bytes),
                    ..Default::default()
                }),
            )
            .is_ok()
                == success,
            "Adjacent Rcore resource limits changed",
        )?;
    }
    for stop in 0..meter.calls.get() {
        check(
            matches!(run_resource(&bundle, ParseLimits::default(), true, Rc::new(Meter {
            stop: Some(stop), ..Default::default()
        })), Err(ObservedImportError::Observation(message)) if message == "structural interrupted"),
            "Rcore resource missed an observer interruption",
        )?;
    }
    for invalid in [false, true] {
        let mut bad = bundle.clone();
        if invalid {
            bad.sources[0] = DefinitionSource::new(
                "memory:bad.rcore",
                SourceKind::Rcore,
                "rcore 1;package bad version \"1\";metamodel \"\\uD800\" prefix b { class Item {} }",
            );
        }
        let limits = if invalid {
            ParseLimits::default()
        } else {
            ParseLimits {
                source_bytes: bad.sources[0].text.len() - 1,
            }
        };
        let old = run_resource(&bad, limits, false, Rc::new(Meter::default())).unwrap_err();
        let new = run_resource(&bad, limits, true, Rc::new(Meter::default())).unwrap_err();
        check(
            old == new,
            "Rcore resource diagnostic kind/span/priority changed",
        )?;
        let ObservedImportError::Import(error) = new else {
            return Err("Lost Rcore import diagnostic".into());
        };
        check(
            error.span.source == bad.sources[0].id
                && if invalid {
                    error.kind == ErrorKind::InvalidDefinition
                        && error.message == "Invalid string escape or control character"
                        && error.span.start == 42
                        && error.span.end == 50
                } else {
                    error.kind == ErrorKind::ResourceLimit
                        && error.message == "Rcore source size budget exceeded"
                        && error.span.start == 0
                        && error.span.end == bad.sources[0].text.len()
                },
            "Literal Rcore resource rejection changed",
        )?;
    }
    Ok(())
}
fn factory() -> Result<(), String> {
    let bundle = fixture();
    let entry = bundle.sources[0].id.clone();
    let types = vec![RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: entry.clone(),
            path: vec![0],
        },
        arguments: vec![],
        raw: false,
    })?];
    let expected = intersect_model::definitions::compiler::materialize_runtime_bundle(
        &bundle, &entry, NS, &types, None,
    )
    .map_err(|e| e.to_string())?;
    check(
        expected.classes.len() == 1,
        "Authored Item schema did not compile",
    )?;
    let registry = ConversionRegistry::default();
    for prefix in [None, Some(0), Some(16), Some(64), Some(256), Some(512)] {
        let input = bundle.clone();
        let pointer = input.sources[0].text.as_ptr();
        let mut context = ReferenceContext {
            bundle: input,
            entry: entry.clone(),
            namespace: NS.into(),
            conversions: vec![],
            materialized_types: types.clone(),
            applied_conversions: vec![],
            constructors: vec![],
            model: serde_json::json!({}),
        };
        let operation = InvocationContext::default();
        let mut pending =
            SharedRuntimeSourceArrayTarget::begin_compilation(&mut context, &registry, &operation)?
                .ok_or("Actual factory did not retain compilation")?;
        if let Some(prefix) = prefix {
            for _ in 0..prefix {
                check(
                    !SharedRuntimeSourceArrayTarget::advance_compilation(
                        &mut pending,
                        &registry,
                        &operation,
                    )?,
                    "Unexpected early schema publication",
                )?;
            }
            let cancelled = InvocationContext::new(Rc::new(Meter {
                stop: Some(0),
                ..Default::default()
            }));
            check(
                SharedRuntimeSourceArrayTarget::advance_compilation(
                    &mut pending,
                    &registry,
                    &cancelled,
                )
                .is_err()
                    && SharedRuntimeSourceArrayTarget::advance_compilation(
                        &mut pending,
                        &registry,
                        &operation,
                    )
                    .is_err(),
                "Actual structural factory cancellation was not terminal",
            )?;
        } else {
            while !SharedRuntimeSourceArrayTarget::advance_compilation(
                &mut pending,
                &registry,
                &operation,
            )? {}
        }
        let result =
            SharedRuntimeSourceArrayTarget::finish_compilation(pending, &mut context, &operation);
        check(
            context.bundle == bundle && context.bundle.sources[0].text.as_ptr() == pointer,
            "Actual factory lost exact source allocations",
        )?;
        if prefix.is_some() {
            check(
                result.is_err(),
                "Cancelled structural factory published a schema",
            )?;
        } else {
            let ReferenceSchema::Checked(schema) = result? else {
                return Err("Actual factory lost checked completion".into());
            };
            check(
                serde_json::to_value(schema.schema()).map_err(|e| e.to_string())?
                    == serde_json::to_value(&expected).map_err(|e| e.to_string())?,
                "Actual structural factory changed the complete schema",
            )?;
        }
    }
    Ok(())
}

#[path = "../shared/rcore_structural_receipts.rs"]
mod factory_receipts;
fn frozen_factory_receipts() -> Result<(), String> {
    let expected = include_str!(
        "../../../crates/intersect-model/assets/fixtures/rcore-structural-factory-baseline.json"
    );
    check(
        format!("{:x}", Sha256::digest(expected.as_bytes()))
            == "f34cd83e4a6cc4699835503c84e6fdfbbfb8733d4317a6b308bef3664426e587",
        "Frozen pre-change factory receipt artifact changed",
    )?;
    factory_receipts::verify(expected)
}
fn verify() -> Result<u32, String> {
    compatibility()?;
    literals()?;
    rich_forms()?;
    boundaries_and_failures()?;
    progress_and_interruptions()?;
    resources()?;
    factory()?;
    frozen_factory_receipts()?;
    Ok(8)
}
#[cfg_attr(not(test), unsafe(no_mangle))]
pub extern "C" fn verify_rcore_structural_preparation() -> u32 {
    verify().unwrap_or(0)
}
fn main() -> Result<(), String> {
    println!("{} retained Rcore structural groups passed", verify()?);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frozen_resource_documents_errors_and_receipts() {
        compatibility().unwrap();
    }
    #[test]
    fn literal_tree_spans_and_attributes() {
        literals().unwrap();
    }
    #[test]
    fn complete_existing_structural_categories() {
        rich_forms().unwrap();
    }
    #[test]
    fn independent_nesting_size_and_terminal_failure_boundaries() {
        boundaries_and_failures().unwrap();
    }
    #[test]
    fn retained_progress_quantums_cancellation_and_source_identity() {
        progress_and_interruptions().unwrap();
    }
    #[test]
    fn exact_resource_fees_and_all_observer_interruptions() {
        resources().unwrap();
    }
    #[test]
    fn actual_factory_schema_and_input_recovery() {
        factory().unwrap();
    }
}
