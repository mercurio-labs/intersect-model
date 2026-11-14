//! Literal predecessor holders are verification-only; production defaults are
//! monomorphized to exactly the old map/cursor types, without extra fields.
use super::*;
#[allow(dead_code)]
mod predecessor {
    use super::*;
    struct OldExecutionState {
        _observation: crate::rcore_behavior_functions::observation::Recorder,
        _consumed: Consumption,
        _numeric: NumericBudget,
        _nodes: Vec<Node>,
        _closures: Vec<Closure>,
        _frames: Vec<Frame>,
        _constants: BTreeMap<(usize, usize, usize), Id>,
        _preparing: Option<FunctionId>,
        _provider_receipts: std::rc::Rc<crate::rcore_behavior_functions::providers::Receipts>,
        _domains: BTreeMap<(String, TypeId), crate::model_reference::RuntimeReferenceDomain>,
        _contexts: BTreeMap<String, TypeId>,
        _reference_inputs: BTreeMap<String, (usize, usize)>,
        // Exact prepared inputs moved from an owning session, keyed by original index.
        _owned_reference_inputs:
            Option<BTreeMap<usize, intersect_model::definitions::conversion::ReferenceContext>>,
        _datatypes: BTreeMap<TypeId, intersect_model::definitions::datatypes::Datatype>,
    }
    struct OldState<'run, 'a, 'ctx> {
        _image: ExecutionImage<'a>,
        _context: Option<&'a ExecutionContext<'ctx>>,
        _limits: &'a ExecutionLimits,
        _control: Option<&'a dyn ExecutionControl>,
        _data: &'run mut OldExecutionState,
    }
    enum OldSessionWork<'a> {
        Idle,
        Context(crate::rcore_behavior_functions::context::ContextPreparation),
        Shape(
            (
                intersect_model::definitions::rcore_behavior::ir_v2::RetainedValueShapeCursor<'a>,
                intersect_model::definitions::rcore_behavior::ir_v2::ValueBudget,
            ),
        ),
        Decode(crate::rcore_behavior_functions::decoding::Decoding<'a>),
        Encode(crate::rcore_behavior_functions::encoding::Encoding<'a>),
    }
    struct OldSessionState<'a> {
        _source: &'a dyn SessionAuthority,
        _references: SliceInput<'a, intersect_model::definitions::conversion::ReferenceContext>,
        _execution: OldExecutionState,
        _decoded_arguments: Vec<Id>,
        _work: OldSessionWork<'a>,
        _phase: Phase,
    }
    #[test]
    fn default_input_family_retains_exact_predecessor_holder_layouts() {
        use std::mem::{align_of, size_of};
        assert_eq!(
            size_of::<crate::rcore_behavior_functions::inputs::NoBorrowedModelInputs>(),
            0
        );
        assert_eq!(size_of::<ExecutionState>(), size_of::<OldExecutionState>());
        assert_eq!(
            align_of::<ExecutionState>(),
            align_of::<OldExecutionState>()
        );
        assert_eq!(
            size_of::<State<'static, 'static, 'static>>(),
            size_of::<OldState<'static, 'static, 'static>>()
        );
        assert_eq!(
            align_of::<State<'static, 'static, 'static>>(),
            align_of::<OldState<'static, 'static, 'static>>()
        );
        assert_eq!(
            size_of::<SessionWork<'static>>(),
            size_of::<OldSessionWork<'static>>()
        );
        assert_eq!(
            align_of::<SessionWork<'static>>(),
            align_of::<OldSessionWork<'static>>()
        );
        assert_eq!(
            size_of::<SessionState<'static>>(),
            size_of::<OldSessionState<'static>>()
        );
        assert_eq!(
            align_of::<SessionState<'static>>(),
            align_of::<OldSessionState<'static>>()
        );
        println!(
            "BORROWED_DEFAULT_LAYOUTS {}",
            serde_json::json!({
                "execution":size_of::<ExecutionState>(),"state":size_of::<State<'static,'static,'static>>(),
                "session_work":size_of::<SessionWork<'static>>(),"session_state":size_of::<SessionState<'static>>()
            })
        );
    }
}
