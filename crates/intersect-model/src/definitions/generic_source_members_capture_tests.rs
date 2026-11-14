use super::*;
use crate::definitions::{
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::ParserObservation,
};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, rc::Rc};
struct Meter(RefCell<Vec<(char, usize)>>);
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("source projection cannot invoke providers")
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.0.borrow_mut().push(('w', n));
        Ok(())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.0.borrow_mut().push(('s', n));
        Ok(())
    }
}
pub(super) fn inputs(case: usize) -> (DefinitionBundle, Type, SourceSpan) {
    let (bundle, receiver) = super::runtime_members_capture_tests::inputs(case.min(35));
    let expression = match case {
        36 => Type::Classifier {
            declaration: DeclarationId {
                source: bundle.sources[0].id.clone(),
                path: vec![1],
            },
            arguments: vec![Type::Parameter {
                declaration: DeclarationId {
                    source: bundle.sources[0].id.clone(),
                    path: vec![1, 0],
                },
            }],
            raw: false,
        },
        37 => Type::Parameter {
            declaration: DeclarationId {
                source: bundle.sources[0].id.clone(),
                path: vec![1, 0],
            },
        },
        38 => Type::Wildcard {
            upper: None,
            lower: None,
        },
        _ => receiver.expression().clone(),
    };
    let span = SourceSpan {
        source: bundle.sources[0].id.clone(),
        start: 17,
        end: 19,
    };
    (bundle, expression, span)
}
fn row(case: usize, profile: usize) -> serde_json::Value {
    let (bundle, expression, span) = inputs(case);
    let runtime = profile % 2 == 1;
    let views = (profile / 2) % 2 == 1;
    let prepared = profile / 4;
    let mut resolver = Resolver::new(&bundle).unwrap();
    if prepared > 0
        && let Err(error) = resolver.resolve_materialization_inventory_observed(
            &ParserObservation::new(&InvocationContext::default()),
        )
    {
        return serde_json::json!({"case":case,"profile":profile,"setup_error":format!("{error:?}")});
    }
    if prepared == 2 {
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        drop(resolver.project_member_sources_observed(
            &expression,
            &span,
            runtime,
            views,
            &observation,
        ));
    }
    let meter = Rc::new(Meter(RefCell::new(vec![])));
    let context = InvocationContext::new(meter.clone());
    let observation = ParserObservation::new(&context);
    let result = observation.finish(resolver.project_member_sources_observed(
        &expression,
        &span,
        runtime,
        views,
        &observation,
    ));
    let output = match result {
        Ok(report) => {
            serde_json::json!({"kind":if matches!(report,SourceMembers::Views(_)){"views"}else{"owned"},"members":(0..report.len()).map(|i|serde_json::to_value(report.get(i).unwrap()).unwrap()).collect::<Vec<_>>()})
        }
        Err(error) => serde_json::json!({"error":format!("{error:?}")}),
    };
    let events = meter.0.borrow();
    serde_json::json!({"case":case,"profile":profile,"output":output,"events_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&*events).unwrap())),"work":events.iter().filter(|e|e.0=='w').map(|e|e.1).sum::<usize>(),"storage":events.iter().filter(|e|e.0=='s').map(|e|e.1).sum::<usize>(),"erasure_cache":format!("{:?}",resolver.prepared_erased_signatures.get().map(|v|v.borrow())),"ancestry_cache":format!("{:?}",resolver.prepared_ancestry.get().map(|v|v.borrow()))})
}
#[test]
fn source_members_capture_complete_projections_errors_fees_and_caches() {
    let rows = (0..39)
        .flat_map(|case| (0..12).map(move |profile| row(case, profile)))
        .collect::<Vec<_>>();
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("generic_source_members_baseline.json")).unwrap();
    assert_eq!(serde_json::to_value(rows).unwrap(), expected);
}
