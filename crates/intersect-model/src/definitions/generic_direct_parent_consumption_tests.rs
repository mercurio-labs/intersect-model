//! The actual report consumer must advance, rather than drain, parent work.
use super::super::runtime_members_capture_tests::inputs;
use super::*;
use crate::definitions::{conversion::InvocationContext, ecore::ParserObservation};
#[test]
fn runtime_member_reports_retain_parent_work_inside_the_actual_owner() {
    for case in [0, 20, 34] {
        for prepared in [false, true] {
            let (bundle, receiver) = inputs(case);
            let mut resolver = Resolver::new(&bundle).unwrap();
            if prepared {
                resolver
                    .resolve_materialization_inventory_observed(&ParserObservation::new(
                        &InvocationContext::default(),
                    ))
                    .unwrap();
            }
            let previous = resolver.runtime_members(&receiver).unwrap();
            let mut job =
                RuntimeMembersPreparation::begin(&resolver, Cow::Borrowed(&receiver), true);
            let mut parent_steps = 0;
            loop {
                job = std::hint::black_box(job);
                if job.phase == Phase::ParentsPrepare {
                    parent_steps += 1;
                }
                let context = InvocationContext::default();
                if job.advance(&ParserObservation::new(&context)).unwrap() {
                    break;
                }
            }
            assert!(
                parent_steps >= 4,
                "direct-parent work was synchronously drained"
            );
            let context = InvocationContext::default();
            assert_eq!(
                job.finish_owned(&ParserObservation::new(&context)).unwrap(),
                previous
            );
        }
    }
}
