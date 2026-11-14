//! Verification-only consumer of retained canonical traversal and its real job adapter.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceId, SourceKind},
    compiler::OwnedRuntimeSourceArrayPreparation,
    conversion::{
        ConversionRegistry, InvocationContext, InvocationObserver, ProviderId, ReferenceContext,
    },
    ecore::{ErrorKind, ObservedImportError},
    generic_types::{RuntimeType, Type},
    resources::DeclarationId,
    schema::SharedRuntimeSourceArrayTarget,
};
use intersect_model::model_reference::{ReferenceSchema, ReferenceSchemaFactory};
use std::{cell::Cell, rc::Rc};
const NS: &str = "urn:canonical";
const MESSAGE: &str =
    "Ecore sources are migration inputs; migrate the complete bundle before compiling structure";
fn check(value: bool, message: &str) -> Result<(), String> {
    if value { Ok(()) } else { Err(message.into()) }
}
fn fixture() -> Result<(DefinitionBundle, SourceId, Vec<RuntimeType>), String> {
    let source = DefinitionSource::new(
        "canonical.rcore",
        SourceKind::Rcore,
        "rcore 1; package canonical version \"1\"; metamodel \"urn:canonical\" prefix c { class Item {} }",
    );
    let ty = RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: source.id.clone(),
            path: vec![0],
        },
        arguments: vec![],
        raw: false,
    })?;
    let entry = source.id.clone();
    let mut bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    for index in 0..16 {
        bundle.sources.push(DefinitionSource::new(
            format!("unused:{index}"),
            SourceKind::Xtext,
            "not parsed as a grammar",
        ));
    }
    Ok((bundle, entry, vec![ty]))
}
fn begin(
    bundle: DefinitionBundle,
    entry: SourceId,
    types: &[RuntimeType],
    operation: &InvocationContext<'_>,
    retained: bool,
) -> Result<OwnedRuntimeSourceArrayPreparation, String> {
    let constructor = if retained {
        OwnedRuntimeSourceArrayPreparation::begin_with_retained_canonical_sources
    } else {
        OwnedRuntimeSourceArrayPreparation::begin_with_compiled_signature_inputs
    };
    constructor(bundle, entry, NS.into(), types, None, operation).map_err(|e| {
        match e.materialization.result {
            Err(error) => error.to_string(),
            Ok(_) => "Failed constructor published a schema".into(),
        }
    })
}
fn complete(
    mut session: OwnedRuntimeSourceArrayPreparation,
    operation: &InvocationContext<'_>,
) -> Result<intersect_model::definitions::compiler::OwnedRuntimeSourceArrayPreparationResult, String>
{
    while !session
        .advance(None, operation)
        .map_err(|e| e.to_string())?
    {}
    Ok(session.finish())
}
#[derive(Default)]
struct Meter {
    cancel: Cell<bool>,
    calls: Cell<usize>,
    work: Cell<usize>,
    bytes: Cell<usize>,
    work_limit: Option<usize>,
    byte_limit: Option<usize>,
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected provider".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        self.calls.set(self.calls.get() + 1);
        if self.cancel.get() {
            Err("canonical source cancelled".into())
        } else {
            Ok(())
        }
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.checkpoint()?;
        let value = self.work.get() + n;
        if self.work_limit.is_some_and(|limit| value > limit) {
            return Err("canonical source work denied".into());
        }
        self.work.set(value);
        Ok(())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.checkpoint()?;
        let value = self.bytes.get() + n;
        if self.byte_limit.is_some_and(|limit| value > limit) {
            return Err("canonical source storage denied".into());
        }
        self.bytes.set(value);
        Ok(())
    }
}
fn verify() -> Result<u32, String> {
    let (bundle, entry, types) = fixture()?;
    // Literal diagnostics and exact candidate boundaries, before any validation.
    for position in [0, 3, 17] {
        let mut bad = bundle.clone();
        bad.version = 0;
        bad.sources[0].id.sha256 = "forged".into();
        let first = DefinitionSource::new("unmigrated:first", SourceKind::Ecore, "<bad/>");
        bad.sources.insert(position, first.clone());
        bad.sources.push(DefinitionSource::new(
            "unmigrated:second",
            SourceKind::Ecore,
            "later",
        ));
        check(
            begin(
                bad.clone(),
                entry.clone(),
                &types,
                &InvocationContext::default(),
                false,
            )
            .is_err(),
            "Legacy canonical rejection was deferred",
        )?;
        let pointer = bad.sources[position].text.as_ptr();
        let mut pending = begin(
            bad.clone(),
            entry.clone(),
            &types,
            &InvocationContext::default(),
            true,
        )?;
        for _ in 0..position {
            check(
                !pending
                    .advance(None, &InvocationContext::default())
                    .map_err(|e| e.to_string())?,
                "Early canonical publication",
            )?;
        }
        let error = pending
            .advance(None, &InvocationContext::default())
            .err()
            .ok_or("Ecore accepted")?;
        check(
            matches!(&error,ObservedImportError::Import(e) if e.kind==ErrorKind::Unsupported && e.span.source==first.id && e.span.start==0 && e.span.end==6 && e.message==MESSAGE),
            "Canonical diagnostic identity, kind, span or priority changed",
        )?;
        check(
            pending.advance(None, &InvocationContext::default()) == Err(error),
            "Canonical failure was not terminal",
        )?;
        check(
            pending.finish().materialization.bundle == bad,
            "Invalid source owner changed",
        )?;
        let mut moved = begin(
            bad,
            entry.clone(),
            &types,
            &InvocationContext::default(),
            true,
        )?;
        for _ in 0..=position {
            let _ = moved.advance(None, &InvocationContext::default());
        }
        check(
            moved.finish().materialization.bundle.sources[position]
                .text
                .as_ptr()
                == pointer,
            "Invalid source allocation was copied",
        )?;
    }
    // Every canonical boundary, fresh cancellation observers and exact recovery.
    for boundary in 0..=bundle.sources.len() + 1 {
        let moved = bundle.clone();
        let pointer = moved.sources[0].text.as_ptr();
        let mut pending = begin(
            moved,
            entry.clone(),
            &types,
            &InvocationContext::default(),
            true,
        )?;
        for _ in 0..boundary {
            check(
                !pending
                    .advance(None, &InvocationContext::default())
                    .map_err(|e| e.to_string())?,
                "Early scan publication",
            )?;
        }
        let meter = Rc::new(Meter {
            cancel: Cell::new(true),
            ..Meter::default()
        });
        let operation = InvocationContext::new(meter.clone());
        let error = ObservedImportError::Observation("canonical source cancelled".into());
        check(
            pending.advance(None, &operation) == Err(error.clone()),
            "Cancellation lost its typed diagnostic",
        )?;
        drop(operation);
        check(
            Rc::strong_count(&meter) == 1,
            "Observer retained by compiler",
        )?;
        check(
            pending.advance(None, &InvocationContext::default()) == Err(error),
            "Cancelled compiler restarted",
        )?;
        let failed = pending.finish();
        check(
            failed.materialization.bundle == bundle
                && failed.materialization.bundle.sources[0].text.as_ptr() == pointer
                && failed.materialization.result.is_err(),
            "Cancelled owner recovery changed",
        )?;
    }
    // Complete schema/receipts and adjacent work/storage limits.
    let old = Rc::new(Meter::default());
    let old_operation = InvocationContext::new(old.clone());
    let old_result = complete(
        begin(bundle.clone(), entry.clone(), &types, &old_operation, false)?,
        &old_operation,
    )?;
    let expected = serde_json::to_value(
        old_result
            .materialization
            .result
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let meter = Rc::new(Meter::default());
    let operation = InvocationContext::new(meter.clone());
    let current = complete(
        begin(bundle.clone(), entry.clone(), &types, &operation, true)?,
        &operation,
    )?;
    check(
        serde_json::to_value(current.materialization.result.map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?
            == expected,
        "Retained schema differs from existing compiler",
    )?;
    check(
        (meter.work.get(), meter.bytes.get()) == (old.work.get(), old.bytes.get()),
        "Canonical traversal changed logical fees",
    )?;
    for (work, bytes, success) in [
        (meter.work.get(), meter.bytes.get(), true),
        (meter.work.get() - 1, meter.bytes.get(), false),
        (meter.work.get(), meter.bytes.get() - 1, false),
    ] {
        let limits = Rc::new(Meter {
            work_limit: Some(work),
            byte_limit: Some(bytes),
            ..Meter::default()
        });
        let context = InvocationContext::new(limits);
        let mut pending = begin(bundle.clone(), entry.clone(), &types, &context, true)?;
        let outcome = loop {
            match pending.advance(None, &context) {
                Ok(false) => {}
                Ok(true) => break true,
                Err(_) => break false,
            }
        };
        check(
            outcome == success,
            "Adjacent compiler resource limit changed",
        )?;
        let result = pending.finish();
        check(
            result.materialization.bundle == bundle
                && result.materialization.result.is_ok() == success,
            "Limited attempt changed input recovery or publication",
        )?;
    }
    // The actual model-context factory selects retained canonical admission.
    for invalid in [false, true] {
        let mut input = bundle.clone();
        if invalid {
            input.sources.push(DefinitionSource::new(
                "unmigrated:job",
                SourceKind::Ecore,
                "<job/>",
            ));
        }
        let pointer = input.sources[0].text.as_ptr();
        let original = input.clone();
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
        let registry = ConversionRegistry::default();
        let operation = InvocationContext::default();
        let mut pending =
            SharedRuntimeSourceArrayTarget::begin_compilation(&mut context, &registry, &operation)?
                .ok_or("Actual factory did not retain compilation")?;
        check(
            context.bundle.sources.is_empty(),
            "Actual factory did not transfer source ownership",
        )?;
        let succeeded = loop {
            match SharedRuntimeSourceArrayTarget::advance_compilation(
                &mut pending,
                &registry,
                &operation,
            ) {
                Ok(false) => {}
                Ok(true) => break true,
                Err(_) => break false,
            }
        };
        check(
            succeeded != invalid,
            "Actual job canonical admission changed",
        )?;
        let finished =
            SharedRuntimeSourceArrayTarget::finish_compilation(pending, &mut context, &operation);
        check(
            context.bundle == original && context.bundle.sources[0].text.as_ptr() == pointer,
            "Actual job lost exact source owner",
        )?;
        if invalid {
            check(finished.is_err(), "Actual job published an invalid schema")?;
        } else {
            let ReferenceSchema::Checked(schema) = finished? else {
                return Err("Actual job lost checked schema authority".into());
            };
            check(
                serde_json::to_value(schema.schema()).map_err(|e| e.to_string())? == expected,
                "Actual job schema changed",
            )?;
        }
    }
    Ok(4)
}
#[unsafe(no_mangle)]
pub extern "C" fn verify_canonical_source_preparation() -> u32 {
    verify().unwrap_or(0)
}
fn main() -> Result<(), String> {
    println!("{} canonical source preparation groups passed", verify()?);
    Ok(())
}
