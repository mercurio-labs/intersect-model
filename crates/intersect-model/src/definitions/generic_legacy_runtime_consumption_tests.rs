//! Real runtime reports must expose each new individual agreement step.
use super::*;
use crate::definitions::{
    bundle::{DefinitionSource, SourceKind},
    conversion::InvocationContext,
    ecore::ParserObservation,
    resources::ResourceSet,
};
#[test]
fn retained_legacy_runtime_and_source_member_owners_advance_before_publication() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("generic_legacy_baseline_owners.json")).unwrap();
    let fixture = fixture.as_array().unwrap().last().unwrap();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![DefinitionSource::new(
            fixture["uri"].as_str().unwrap(),
            SourceKind::Rcore,
            fixture["text"].as_str().unwrap(),
        )],
        namespaces: vec![],
    };
    let declaration = ResourceSet::new(&bundle)
        .unwrap()
        .resolve(&bundle.sources[0].id, fixture["receiver"].as_str().unwrap())
        .unwrap();
    let receiver = RuntimeType::new(Type::Classifier {
        declaration,
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    let resolver = Resolver::new(&bundle).unwrap();
    let mut job = RuntimeMembersPreparation::begin(&resolver, Cow::Borrowed(&receiver), true);
    let mut sources = 0;
    let mut signatures = 0;
    let mut parents = 0;
    loop {
        job = std::hint::black_box(job);
        if job.is_silent_agreement_step() {
            match job.phase {
                Phase::Sources => sources += 1,
                Phase::SignaturePrepare => signatures += 1,
                Phase::ParentsPrepare => parents += 1,
                _ => panic!("unrecognized retained owner"),
            }
        }
        let context = InvocationContext::default();
        if job.advance(&ParserObservation::new(&context)).unwrap() {
            break;
        }
    }
    assert!(
        sources >= 6,
        "source member owner synchronously drained agreement"
    );
    assert!(
        signatures >= 6,
        "runtime signature owner synchronously drained agreement"
    );
    assert!(
        parents >= 6,
        "runtime parent owner synchronously drained agreement"
    );
    let context = InvocationContext::default();
    assert!(job.finish_owned(&ParserObservation::new(&context)).is_ok());
}
