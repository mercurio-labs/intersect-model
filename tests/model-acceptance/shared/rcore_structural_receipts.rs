//! Verification-only actual factory receipt oracle, shared by native and WASM.
use intersect_model::{
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        conversion::{
            ConversionRegistry, InvocationContext, InvocationObserver, ProviderId, ReferenceContext,
        },
        generic_types::{RuntimeType, Type},
        resources::DeclarationId,
        schema::SharedRuntimeSourceArrayTarget,
    },
    model_reference::{ReferenceSchema, ReferenceSchemaFactory},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{cell::Cell, rc::Rc};
#[derive(Default)]
struct Meter {
    work: Cell<usize>,
    bytes: Cell<usize>,
    max_work: Option<usize>,
    max_bytes: Option<usize>,
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected semantic invocation during structural factory preparation".into())
    }
    fn work(&self, n: usize) -> Result<(), String> {
        let value = self.work.get() + n;
        if self.max_work.is_some_and(|max| value > max) {
            return Err("Factory work denied".into());
        }
        self.work.set(value);
        Ok(())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        let value = self.bytes.get() + n;
        if self.max_bytes.is_some_and(|max| value > max) {
            return Err("Factory storage denied".into());
        }
        self.bytes.set(value);
        Ok(())
    }
}
fn input(case: usize) -> Result<(DefinitionBundle, Vec<usize>), String> {
    let (text,path)=match case {
        0=>(format!("rcore 1;package structural version \"1\"; /*{}*/ metamodel \"urn:structural\" prefix s {{class Item {{}}}}","λ🚀".repeat(150)),vec![0]),
        1=>("rcore 1;package structural version \"1\";metamodel \"urn:structural\" prefix s {class Base {} class Item extends Base {attr label: String [0,1] default \"hi\";ref next: Item [0,*];op echo(arg: String [0,1])->String [0,1];}enum Mode {A=0;B=1;}datatype Opaque instanceClass \"Host\" nonserializable {} package nested namespace \"urn:nested\" prefix n {class Nested {}}}".into(),vec![1]),
        2=>("rcore 1;package structural version \"1\";resource {package left namespace \"urn:structural\" prefix s {class Item {attr count: Int32 [0,1];}} package right namespace \"urn:right\" prefix r {class Other {}}}".into(),vec![0,0]),
        _=>return Err("Unknown structural receipt fixture".into()),
    };
    Ok((
        DefinitionBundle {
            version: 1,
            sources: vec![DefinitionSource::new(
                format!("memory:structural-factory-{case}.rcore"),
                SourceKind::Rcore,
                text,
            )],
            namespaces: vec![],
        },
        path,
    ))
}
fn run(case: usize, max_work: Option<usize>, max_bytes: Option<usize>) -> Result<Value, String> {
    let (bundle, path) = input(case)?;
    let entry = bundle.sources[0].id.clone();
    let ty = RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: entry.clone(),
            path,
        },
        arguments: vec![],
        raw: false,
    })?;
    let mut context = ReferenceContext {
        bundle,
        entry: entry.clone(),
        namespace: "urn:structural".into(),
        conversions: vec![],
        materialized_types: vec![ty],
        applied_conversions: vec![],
        constructors: vec![],
        model: json!({}),
    };
    let original = context.bundle.clone();
    let pointer = context.bundle.sources[0].text.as_ptr();
    let meter = Rc::new(Meter {
        max_work,
        max_bytes,
        ..Default::default()
    });
    let operation = InvocationContext::new(meter.clone());
    let registry = ConversionRegistry::default();
    let result = match SharedRuntimeSourceArrayTarget::begin_compilation(
        &mut context,
        &registry,
        &operation,
    ) {
        Ok(Some(mut pending)) => {
            let advanced = loop {
                match SharedRuntimeSourceArrayTarget::advance_compilation(
                    &mut pending,
                    &registry,
                    &operation,
                ) {
                    Ok(false) => {}
                    Ok(true) => break Ok(()),
                    Err(e) => break Err(e),
                }
            };
            let finished = SharedRuntimeSourceArrayTarget::finish_compilation(
                pending,
                &mut context,
                &operation,
            );
            advanced.and(finished)
        }
        Ok(None) => Err("Actual structural factory did not retain preparation".into()),
        Err(e) => Err(e),
    };
    if context.bundle != original || context.bundle.sources[0].text.as_ptr() != pointer {
        return Err("Actual factory did not recover original exact source allocations".into());
    }
    let ReferenceSchema::Checked(schema) = result? else {
        return Err("Actual factory did not produce a checked schema".into());
    };
    let encoded = serde_json::to_vec(schema.schema()).map_err(|e| e.to_string())?;
    Ok(
        json!({"case":case,"source":entry,"work":meter.work.get(),"bytes":meter.bytes.get(),"schema_sha256":format!("{:x}",Sha256::digest(encoded))}),
    )
}
pub fn receipts() -> Result<Value, String> {
    (0..3)
        .map(|case| run(case, None, None))
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}
pub fn verify(expected: &str) -> Result<(), String> {
    let expected: Value = serde_json::from_str(expected).map_err(|e| e.to_string())?;
    let current = receipts()?;
    if current != expected {
        return Err(format!(
            "Pre-change actual factory output/receipts changed: {current}"
        ));
    }
    for case in 0..3 {
        let work = current[case]["work"]
            .as_u64()
            .ok_or("Missing frozen factory work")? as usize;
        let bytes = current[case]["bytes"]
            .as_u64()
            .ok_or("Missing frozen factory storage")? as usize;
        for (work, bytes, ok) in [
            (work, bytes, true),
            (work - 1, bytes, false),
            (work, bytes - 1, false),
        ] {
            if run(case, Some(work), Some(bytes)).is_ok() != ok {
                return Err("Pre-change actual factory exact/one-short ceilings changed".into());
            }
        }
    }
    Ok(())
}
