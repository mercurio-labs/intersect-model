use super::values::{Data, Id, Node, NodeView};
use super::*;
use intersect_model::definitions::rcore_behavior::ir_v2::{
    FunctionId, InstructionKind, Operand, Terminator, TypeId, TypeNode,
};
use std::collections::BTreeMap;

#[path = "rcore_behavior_functions_construction.rs"]
mod construction;
#[path = "rcore_behavior_functions_invocation.rs"]
mod invocation;
#[path = "rcore_behavior_functions_operands.rs"]
mod operands;

/// Different immutable authorities share dispatch, never context certificates.
#[derive(Clone, Copy)]
pub(super) enum ExecutionImage<'a> {
    Functions(&'a VerifiedExecutable),
    Scalar(&'a intersect_model::definitions::rcore_behavior::ir_v2::VerifiedScalarExecutable<'a>),
}
struct FunctionView<'a> {
    identity: &'a FunctionIdentity,
    parameters: &'a [TypeId],
    captures: &'a [intersect_model::definitions::rcore_behavior::ir_v2::Capture],
    span: &'a SourceSpan,
}
struct BlockView<'a> {
    instructions: InstructionList<'a>,
    terminator: &'a Terminator,
    span: &'a SourceSpan,
}
enum InstructionList<'a> {
    Functions(&'a [intersect_model::definitions::rcore_behavior::ir_v2::Instruction]),
    Scalar(&'a [intersect_model::definitions::rcore_behavior::ir_v2::ScalarInstruction<'a>]),
}
struct InstructionView<'a> {
    ty: TypeId,
    /// None is exclusively a borrowed scalar constant in the trusted lift.
    kind: Option<&'a InstructionKind>,
    span: &'a SourceSpan,
}
impl<'a> InstructionList<'a> {
    fn get(&self, index: usize) -> Option<InstructionView<'a>> {
        match self {
            Self::Functions(items) => items.get(index).map(|i| InstructionView {
                ty: i.ty,
                kind: Some(&i.kind),
                span: &i.span,
            }),
            Self::Scalar(items) => items.get(index).map(|i| InstructionView {
                ty: i.ty,
                span: &i.span,
                kind: match &i.kind {
                    intersect_model::definitions::rcore_behavior::ir_v2::ScalarInstructionKind::Constant(_) => {
                        None
                    }
                    intersect_model::definitions::rcore_behavior::ir_v2::ScalarInstructionKind::Operation(
                        kind,
                    ) => Some(kind),
                },
            }),
        }
    }
}
impl<'a> ExecutionImage<'a> {
    pub fn entry(self) -> FunctionId {
        match self {
            Self::Functions(image) => image.program().entry,
            Self::Scalar(image) => image.program().entry,
        }
    }
    pub(super) fn call_span(self, site: CallSite) -> Option<&'a SourceSpan> {
        self.block(site.function, site.block)?
            .instructions
            .get(site.instruction)
            .map(|instruction| instruction.span)
    }
    fn function(self, index: usize) -> Option<FunctionView<'a>> {
        match self {
            Self::Functions(image) => image.program().functions.get(index).map(|f| FunctionView {
                identity: &f.identity,
                parameters: &f.parameters,
                captures: &f.captures,
                span: &f.span,
            }),
            Self::Scalar(image) => image.program().functions.get(index).map(|f| FunctionView {
                identity: &f.identity,
                parameters: &f.parameters,
                captures: &f.captures,
                span: &f.span,
            }),
        }
    }
    fn block(self, function: usize, index: usize) -> Option<BlockView<'a>> {
        match self {
            Self::Functions(image) => image
                .program()
                .functions
                .get(function)?
                .blocks
                .get(index)
                .map(|b| BlockView {
                    instructions: InstructionList::Functions(&b.instructions),
                    terminator: &b.terminator,
                    span: &b.span,
                }),
            Self::Scalar(image) => image
                .program()
                .functions
                .get(function)?
                .blocks
                .get(index)
                .map(|b| BlockView {
                    instructions: InstructionList::Scalar(&b.instructions),
                    terminator: &b.terminator,
                    span: &b.span,
                }),
        }
    }
    pub(super) fn stored_declaration(
        self,
        site: CallSite,
    ) -> Option<&'a intersect_model::definitions::resources::DeclarationId> {
        let instruction = self
            .block(site.function, site.block)?
            .instructions
            .get(site.instruction)?;
        match instruction.kind? {
            InstructionKind::StoredFeature { declaration, .. } => Some(declaration),
            _ => None,
        }
    }
    pub fn type_node(
        self,
        ty: TypeId,
    ) -> std::result::Result<&'a TypeNode, intersect_model::definitions::rcore_behavior::ir::IrError>
    {
        match self {
            Self::Functions(image) => image.type_node(ty),
            Self::Scalar(image) => image.type_node(ty),
        }
    }
    pub fn permits(
        self,
        actual: TypeId,
        expected: TypeId,
    ) -> std::result::Result<bool, intersect_model::definitions::rcore_behavior::ir::IrError> {
        match self {
            Self::Functions(image) => image.permits(actual, expected),
            Self::Scalar(image) => image.permits(actual, expected),
        }
    }
    pub fn is_scalar(self) -> bool {
        matches!(self, Self::Scalar(_))
    }
}
pub(super) struct Closure {
    pub function: FunctionId,
    pub captures: Vec<Id>,
}
/// Stable location in the immutable image; no borrowed source span survives a slice.
#[derive(Clone, Copy)]
pub(super) struct CallSite {
    function: usize,
    block: usize,
    instruction: usize,
}
// Verified instructions select one calculation until its result is published.
// Sharing their actual storage avoids growing each eagerly allocated Frame
// before the later continuation-specific admission.
enum Calculation {
    Comparison(super::equivalence::InstructionComparison),
    Algebra(super::algebra::AlgebraState),
    Numeric(super::numeric::Instruction),
}
pub(super) struct Frame {
    construction: Option<construction::Construction>,
    invocation: Option<invocation::Invocation>,
    operands: Option<operands::Preparation>,
    pub conversion: Option<super::conversion::Conversion>,
    calculation: Option<Calculation>,
    pub membership: Option<super::membership::MembershipState>,
    pub collection: Option<super::collections::CollectionState>,
    pub stored_read: Option<super::navigation::StoredRead>,
    pub incoming: Option<super::navigation::IncomingRead>,
    pub ownership: Option<super::ownership::OwnershipRead>,
    pub function: FunctionId,
    pub block: usize,
    pub pc: usize,
    pub parameters: Vec<Id>,
    pub registers: Vec<Id>,
    pub captures: Vec<Id>,
    pub call_site: Option<CallSite>,
    pub result: TypeId,
}
impl Frame {
    #[cfg(feature = "allocation-profile")]
    pub(super) fn numeric_progress(&self) -> Option<super::RetainedNumericPhase> {
        match &self.calculation {
            Some(Calculation::Numeric(value)) => value.progress(),
            _ => None,
        }
    }

    pub(super) fn has_numeric(&self) -> bool {
        matches!(self.calculation, Some(Calculation::Numeric(_)))
    }
    pub(super) fn take_numeric(&mut self) -> Option<super::numeric::Instruction> {
        if !self.has_numeric() {
            return None;
        }
        match self.calculation.take() {
            Some(Calculation::Numeric(value)) => Some(value),
            _ => None,
        }
    }
    pub(super) fn set_numeric(&mut self, value: super::numeric::Instruction) {
        self.calculation = Some(Calculation::Numeric(value));
    }
    pub(super) fn numeric_site(&self) -> Option<CallSite> {
        match &self.calculation {
            Some(Calculation::Numeric(value)) => Some(value.site),
            _ => None,
        }
    }

    pub(super) fn has_comparison(&self) -> bool {
        matches!(self.calculation, Some(Calculation::Comparison(_)))
    }
    pub(super) fn has_algebra(&self) -> bool {
        matches!(self.calculation, Some(Calculation::Algebra(_)))
    }
    pub(super) fn take_comparison(&mut self) -> Option<super::equivalence::InstructionComparison> {
        if !self.has_comparison() {
            return None;
        }
        match self.calculation.take() {
            Some(Calculation::Comparison(value)) => Some(value),
            _ => None,
        }
    }
    pub(super) fn take_algebra(&mut self) -> Option<super::algebra::AlgebraState> {
        if !self.has_algebra() {
            return None;
        }
        match self.calculation.take() {
            Some(Calculation::Algebra(value)) => Some(value),
            _ => None,
        }
    }
    pub(super) fn set_comparison(&mut self, value: super::equivalence::InstructionComparison) {
        self.calculation = Some(Calculation::Comparison(value));
    }
    pub(super) fn set_algebra(&mut self, value: super::algebra::AlgebraState) {
        self.calculation = Some(Calculation::Algebra(value));
    }
}
/// Evaluation-owned mutable state. It contains no borrowed authority
/// or serializable executable handles. Optional evidence owns source locations.
pub(super) struct ExecutionState<
    I: super::inputs::InputFamily = super::inputs::NoBorrowedModelInputs,
> {
    pub observation: super::observation::Recorder,
    pub consumed: Consumption,
    pub numeric: NumericBudget,
    pub nodes: Vec<Node>,
    pub closures: Vec<Closure>,
    pub frames: Vec<Frame>,
    pub constants: BTreeMap<(usize, usize, usize), Id>,
    pub preparing: Option<FunctionId>,
    pub provider_receipts: std::rc::Rc<super::providers::Receipts>,
    pub domains: I::Domains,
    pub contexts: BTreeMap<String, TypeId>,
    pub reference_inputs: BTreeMap<String, (usize, usize)>,
    // Exact prepared inputs moved from an owning session, keyed by original index.
    pub owned_reference_inputs:
        Option<BTreeMap<usize, intersect_model::definitions::conversion::ReferenceContext>>,
    pub datatypes: BTreeMap<TypeId, intersect_model::definitions::datatypes::Datatype>,
}
impl<I: super::inputs::InputFamily> ExecutionState<I> {
    fn with_domains<J: super::inputs::InputFamily>(self, domains: J::Domains) -> ExecutionState<J> {
        ExecutionState {
            observation: self.observation,
            consumed: self.consumed,
            numeric: self.numeric,
            nodes: self.nodes,
            closures: self.closures,
            frames: self.frames,
            constants: self.constants,
            preparing: self.preparing,
            provider_receipts: self.provider_receipts,
            domains,
            contexts: self.contexts,
            reference_inputs: self.reference_inputs,
            owned_reference_inputs: self.owned_reference_inputs,
            datatypes: self.datatypes,
        }
    }
}
/// A transient view borrowing the same authority and suspended execution data.
pub(super) struct State<
    'run,
    'a,
    'ctx,
    I: super::inputs::InputFamily = super::inputs::NoBorrowedModelInputs,
> {
    pub image: ExecutionImage<'a>,
    pub context: Option<&'a ExecutionContext<'ctx>>,
    pub limits: &'a ExecutionLimits,
    pub control: Option<&'a dyn ExecutionControl>,
    pub data: &'run mut ExecutionState<I>,
}
impl<'a, 'ctx, I: super::inputs::InputFamily> State<'_, 'a, 'ctx, I> {
    pub(super) fn invocation_observation<'observation>(
        &self,
        span: Option<&SourceSpan>,
    ) -> Result<intersect_model::definitions::conversion::InvocationContext<'observation>>
    where
        I: 'observation,
        'a: 'observation,
    {
        I::observation(
            &self.data.domains,
            &self.data.provider_receipts,
            self.control,
        )
        .map_err(|error| self.provider_error(error, span))
    }
    pub fn checkpoint(&self, span: Option<&SourceSpan>) -> Result<()> {
        if self
            .control
            .is_some_and(|control| control.cancellation_requested())
        {
            return Err(self.error(
                ExecutionErrorKind::Cancelled,
                "Evaluation cancelled by host",
                span,
            ));
        }
        Ok(())
    }
    pub fn function_image(&self) -> Result<&'a VerifiedExecutable> {
        match self.image {
            ExecutionImage::Functions(image) => Ok(image),
            ExecutionImage::Scalar(_) => {
                Err(self.invalid("Scalar capability has no function context", None))
            }
        }
    }
    pub fn function_context(&self) -> Result<&'a ExecutionContext<'ctx>> {
        self.context
            .ok_or_else(|| self.invalid("Scalar capability has no nominal context", None))
    }

    pub fn error(
        &self,
        kind: ExecutionErrorKind,
        message: impl Into<String>,
        span: Option<&SourceSpan>,
    ) -> ExecutionError {
        if self.image.is_scalar() {
            let mut consumed = self.data.consumed.clone();
            consumed.numeric_work = self.data.numeric.work_used();
            return ExecutionError {
                observation: self.data.observation.snapshot().map(Box::new),
                kind,
                message: Box::new(message.into()),
                arithmetic_kind: None,
                dependency: None,
                span: span.cloned().map(Box::new),
                trace: vec![],
                trace_truncated: false,
                selected_providers: vec![],
                consumed_providers: vec![],
                provider_evidence_truncated: false,
                consumed: Box::new(consumed),
            };
        }
        #[derive(Serialize)]
        struct BorrowedFrame<'s> {
            function: &'s FunctionIdentity,
            definition: &'s SourceSpan,
            call_site: Option<&'s SourceSpan>,
        }
        let image = self.image;
        let mut remaining = self.limits.max_trace_bytes;
        let mut truncated = false;
        let leaf_span =
            span.and_then(
                |span| match super::preflight::serialized_size(span, remaining) {
                    Ok(size) => {
                        remaining -= size;
                        Some(Box::new(span.clone()))
                    }
                    Err(_) => {
                        truncated = true;
                        None
                    }
                },
            );
        let mut trace = Vec::new();
        let fallback = self
            .data
            .frames
            .is_empty()
            .then_some((self.data.preparing.unwrap_or(image.entry()), None));
        let candidates = self
            .data
            .frames
            .iter()
            .rev()
            .map(|f| {
                (
                    f.function,
                    f.call_site.and_then(|site| image.call_span(site)),
                )
            })
            .chain(fallback);
        for (index, (id, call_site)) in candidates.enumerate() {
            if index >= self.limits.max_trace_frames {
                truncated = true;
                continue;
            }
            if let Some(function) = image.function(id.0 as usize) {
                let item = BorrowedFrame {
                    function: function.identity,
                    definition: function.span,
                    call_site,
                };
                let size = super::preflight::serialized_size(&item, remaining);
                match size {
                    Ok(size) => {
                        remaining -= size;
                        trace.push(TraceFrame {
                            function: function.identity.clone(),
                            definition: function.span.clone(),
                            call_site: call_site.cloned(),
                        })
                    }
                    Err(_) => truncated = true,
                }
            }
        }
        let (selected_providers, consumed_providers, provider_evidence_truncated) =
            self.data.provider_receipts.error_evidence(&mut remaining);
        truncated |= provider_evidence_truncated;
        let mut consumed = self.data.consumed.clone();
        consumed.numeric_work = self.data.numeric.work_used();
        self.data.provider_receipts.counters(&mut consumed);
        ExecutionError {
            observation: self.data.observation.snapshot().map(Box::new),
            kind,
            message: Box::new(message.into()),
            arithmetic_kind: None,
            dependency: None,
            span: leaf_span,
            trace,
            trace_truncated: truncated,
            selected_providers,
            consumed_providers,
            provider_evidence_truncated,
            consumed: Box::new(consumed),
        }
    }
    pub fn missing_dependency(
        &self,
        cause: DependencyCause,
        span: Option<&SourceSpan>,
    ) -> ExecutionError {
        let mut error = self.error(
            ExecutionErrorKind::MissingDependency,
            "Required execution dependency is unavailable",
            span,
        );
        error.dependency = Some(Box::new(cause));
        error
    }
    pub fn check_provider_status(&self, span: Option<&SourceSpan>) -> Result<()> {
        if self.data.provider_receipts.cancelled() {
            return Err(self.error(
                ExecutionErrorKind::Cancelled,
                "Execution cancelled during provider work",
                span,
            ));
        }
        if let Some(kind) = self.data.provider_receipts.missing() {
            return Err(self.missing_dependency(DependencyCause::Provider(kind), span));
        }
        if let Some(message) = self.data.provider_receipts.resource_message() {
            return Err(self.resource(message, span));
        }
        Ok(())
    }
    pub fn provider_error(&self, message: String, span: Option<&SourceSpan>) -> ExecutionError {
        if self.data.provider_receipts.cancelled() {
            return self.error(
                ExecutionErrorKind::Cancelled,
                "Execution cancelled during provider work",
                span,
            );
        }
        if let Some(kind) = self.data.provider_receipts.missing() {
            return self.missing_dependency(DependencyCause::Provider(kind), span);
        }
        if let Some(cause) = self.data.provider_receipts.resource_message() {
            return self.resource(cause, span);
        }
        self.error(ExecutionErrorKind::Nominal, message, span)
    }
    pub fn invalid(&self, message: &str, span: Option<&SourceSpan>) -> ExecutionError {
        self.error(ExecutionErrorKind::InvalidProgram, message, span)
    }
    pub fn resource(&self, message: &str, span: Option<&SourceSpan>) -> ExecutionError {
        self.error(ExecutionErrorKind::ResourceExhausted, message, span)
    }
    pub fn arithmetic(
        &self,
        e: crate::rcore_behavior_scalar::ArithmeticError,
        span: &SourceSpan,
    ) -> ExecutionError {
        if e.kind == ArithmeticErrorKind::Cancelled {
            return self.error(ExecutionErrorKind::Cancelled, e.message, Some(span));
        }
        let kind = if e.kind == ArithmeticErrorKind::ResourceLimit {
            ExecutionErrorKind::ResourceExhausted
        } else {
            ExecutionErrorKind::Arithmetic
        };
        let mut result = self.error(kind, e.message, Some(span));
        result.arithmetic_kind = Some(e.kind);
        result
    }
    pub fn type_node(&self, ty: TypeId) -> Result<&TypeNode> {
        self.image
            .type_node(ty)
            .map_err(|e| self.invalid(&e.message, None))
    }
    pub fn node(&self, id: Id) -> Result<NodeView<'_>> {
        self.node_in(&self.data.nodes, id)
    }
    pub(super) fn node_in<'nodes>(
        &self,
        nodes: &'nodes [Node],
        id: Id,
    ) -> Result<NodeView<'nodes>> {
        let header = nodes
            .get(id)
            .ok_or_else(|| self.invalid("Invalid runtime value handle", None))?;
        let origin = if let Data::View(origin) = header.data {
            origin
        } else {
            id
        };
        let payload = nodes
            .get(origin)
            .ok_or_else(|| self.invalid("Invalid runtime view payload", None))?;
        if matches!(payload.data, Data::View(_))
            || header.bytes != payload.bytes
            || header.depth != payload.depth
        {
            return Err(self.invalid("Invalid runtime view chain or measurements", None));
        }
        Ok(NodeView {
            ty: header.ty,
            data: &payload.data,
            bytes: header.bytes,
            depth: header.depth,
            origin,
        })
    }
    pub fn permit(
        &self,
        actual: TypeId,
        expected: TypeId,
        span: Option<&SourceSpan>,
    ) -> Result<()> {
        if self
            .image
            .permits(actual, expected)
            .map_err(|e| self.invalid(&e.message, span))?
        {
            Ok(())
        } else {
            Err(self.error(
                ExecutionErrorKind::Arguments,
                "Runtime value does not conform to checked destination type",
                span,
            ))
        }
    }
    pub fn step(&mut self, span: &SourceSpan) -> Result<()> {
        self.step_at(Some(span))
    }
    pub(super) fn step_at(&mut self, span: Option<&SourceSpan>) -> Result<()> {
        self.data.consumed.steps = self
            .data
            .consumed
            .steps
            .checked_add(1)
            .filter(|n| *n <= self.limits.scalar.max_steps)
            .ok_or_else(|| {
                self.resource(
                    if self.image.is_scalar() {
                        "Scalar execution step budget exceeded"
                    } else {
                        "Function execution step budget exceeded"
                    },
                    span,
                )
            })?;
        Ok(())
    }
    pub fn transfer(&mut self, id: Id, span: Option<&SourceSpan>) -> Result<()> {
        if self.image.is_scalar() {
            return self.scalar_transfer(id, span);
        }
        let bytes = self.node(id)?.bytes;
        self.transfer_bytes(bytes, span)
    }
    pub(super) fn transfer_in(
        &mut self,
        nodes: &[Node],
        id: Id,
        span: Option<&SourceSpan>,
    ) -> Result<()> {
        let bytes = self.node_in(nodes, id)?.bytes;
        self.transfer_bytes(bytes, span)
    }
    fn transfer_bytes(&mut self, bytes: usize, span: Option<&SourceSpan>) -> Result<()> {
        self.data.consumed.value_bytes = self
            .data
            .consumed
            .value_bytes
            .checked_add(bytes)
            .filter(|n| *n <= self.limits.scalar.max_value_bytes)
            .ok_or_else(|| self.resource("Cumulative function value byte budget exceeded", span))?;
        Ok(())
    }
    fn operand(&self, value: Operand, span: &SourceSpan) -> Result<Id> {
        let frame = self
            .data
            .frames
            .last()
            .ok_or_else(|| self.invalid("Missing runtime frame", Some(span)))?;
        let id = match value {
            Operand::Parameter(i) => frame.parameters.get(i as usize),
            Operand::Register(i) => frame.registers.get(i as usize),
            Operand::Capture(i) => frame.captures.get(i as usize),
        };
        id.copied()
            .ok_or_else(|| self.invalid("Verified operand unavailable", Some(span)))
    }
    fn scalar_operands(&mut self, values: &[Operand], span: &SourceSpan) -> Result<Vec<Id>> {
        if !self.image.is_scalar() {
            return Err(self.invalid("Scalar operand path requires scalar authority", Some(span)));
        }
        let mut result = Vec::new();
        for operand in values {
            let id = self.operand(*operand, span)?;
            self.transfer(id, Some(span))?;
            result.push(id);
        }
        Ok(result)
    }
    pub(super) fn push_scalar_entry(
        &mut self,
        function: FunctionId,
        parameters: Vec<Id>,
        captures: Vec<Id>,
        call_site: Option<CallSite>,
        result: TypeId,
    ) -> Result<()> {
        let call_span = call_site.and_then(|site| self.image.call_span(site));
        if !self.image.is_scalar() {
            return Err(self.invalid("Scalar frame entry requires scalar authority", call_span));
        }
        let target = self.frame_signature(function, parameters.len(), captures.len(), call_span)?;
        for (id, ty) in parameters.iter().zip(target.parameters) {
            self.permit(self.node(*id)?.ty, *ty, call_span)?;
        }
        for (id, capture) in captures.iter().zip(target.captures) {
            self.permit(self.node(*id)?.ty, capture.ty, call_span)?;
        }
        self.enter_frame(function, parameters, captures, call_site, result);
        Ok(())
    }
    fn frame_signature(
        &self,
        function: FunctionId,
        parameters: usize,
        captures: usize,
        call_span: Option<&SourceSpan>,
    ) -> Result<FunctionView<'a>> {
        if self.data.frames.len() >= self.limits.max_frames {
            return Err(self.resource("Function frame depth limit exceeded", call_span));
        }
        let target = self
            .image
            .function(function.0 as usize)
            .ok_or_else(|| self.invalid("Unknown function", call_span))?;
        if parameters != target.parameters.len() || captures != target.captures.len() {
            return Err(self.invalid("Function argument or capture arity mismatch", call_span));
        }
        Ok(target)
    }
    fn enter_frame(
        &mut self,
        function: FunctionId,
        parameters: Vec<Id>,
        captures: Vec<Id>,
        call_site: Option<CallSite>,
        result: TypeId,
    ) {
        self.data.frames.push(Frame {
            invocation: None,
            operands: None,
            construction: None,
            conversion: None,
            calculation: None,
            membership: None,
            collection: None,
            stored_read: None,
            incoming: None,
            ownership: None,
            function,
            block: 0,
            pc: 0,
            parameters,
            registers: vec![],
            captures,
            call_site,
            result,
        });
    }
    fn jump(
        &mut self,
        block: u32,
        operands: &[Operand],
        payload: Option<Id>,
        span: &SourceSpan,
        choice: u8,
    ) -> Result<()> {
        if !self.image.is_scalar() {
            return self.start_edge_operands(choice, payload);
        }
        let mut arguments = Vec::new();
        if let Some(id) = payload {
            self.transfer(id, Some(span))?;
            arguments.push(id)
        }
        arguments.extend(self.scalar_operands(operands, span)?);
        let index = self
            .data
            .frames
            .len()
            .checked_sub(1)
            .ok_or_else(|| self.invalid("Missing jump frame", Some(span)))?;
        let frame = &mut self.data.frames[index];
        frame.block = block as usize;
        frame.pc = 0;
        frame.parameters = arguments;
        frame.registers.clear();
        Ok(())
    }
    pub(super) fn run(&mut self) -> Result<Id> {
        self.run_with_quantum(std::num::NonZeroUsize::MAX)
    }

    fn run_with_quantum(&mut self, quantum: std::num::NonZeroUsize) -> Result<Id> {
        loop {
            // Each slice gets a fresh view. Only the evaluation-owned data
            // survives; no view or source-span borrow is stored in it.
            let progress = State {
                image: self.image,
                context: self.context,
                limits: self.limits,
                control: self.control,
                data: &mut *self.data,
            }
            .run_slice(quantum)?;
            if let Some(result) = progress {
                return Ok(result);
            }
        }
    }

    /// Advance at most `quantum` instructions/terminators without resetting any
    /// semantic budget. A pending slice owns all frames and intermediate values;
    /// it is neither a successful result nor an execution failure.
    fn run_slice(&mut self, quantum: std::num::NonZeroUsize) -> Result<Option<Id>> {
        for _ in 0..quantum.get() {
            if self.data.frames.last().is_some_and(Frame::has_numeric) {
                self.advance_numeric()?;
                continue;
            }
            if self
                .data
                .frames
                .last()
                .is_some_and(|frame| frame.operands.is_some())
            {
                self.advance_operands()?;
                continue;
            }
            if self
                .data
                .frames
                .last()
                .is_some_and(|frame| frame.invocation.is_some())
            {
                self.advance_invocation()?;
                continue;
            }
            if self
                .data
                .frames
                .last()
                .is_some_and(|frame| frame.construction.is_some())
            {
                self.advance_construction()?;
                continue;
            }
            if self
                .data
                .frames
                .last()
                .is_some_and(|frame| frame.conversion.is_some())
            {
                self.advance_conversion()?;
                continue;
            }
            if self
                .data
                .frames
                .last()
                .is_some_and(|frame| frame.has_algebra())
            {
                self.advance_algebra()?;
                continue;
            }
            if self
                .data
                .frames
                .last()
                .is_some_and(|frame| frame.has_comparison())
            {
                self.advance_comparison()?;
                continue;
            }
            if self
                .data
                .frames
                .last()
                .is_some_and(|frame| frame.membership.is_some())
            {
                self.advance_membership()?;
                continue;
            }
            if self
                .data
                .frames
                .last()
                .is_some_and(|frame| frame.collection.is_some())
            {
                self.advance_collection()?;
                continue;
            }
            if self
                .data
                .frames
                .last()
                .is_some_and(|frame| frame.stored_read.is_some())
            {
                self.advance_stored_read()?;
                continue;
            }
            if self
                .data
                .frames
                .last()
                .is_some_and(|frame| frame.incoming.is_some())
            {
                self.advance_incoming()?;
                continue;
            }
            if self
                .data
                .frames
                .last()
                .is_some_and(|frame| frame.ownership.is_some())
            {
                self.advance_ownership()?;
                continue;
            }
            let frame = self
                .data
                .frames
                .last()
                .ok_or_else(|| self.invalid("Missing execution frame", None))?;
            let fi = frame.function.0 as usize;
            let bi = frame.block;
            let pc = frame.pc;
            let image = self.image;
            let block = image
                .block(fi, bi)
                .ok_or_else(|| self.invalid("Missing verified block", None))?;
            if let Some(ins) = block.instructions.get(pc) {
                self.step(ins.span)?;
                if let Some(frame) = self.data.frames.last_mut() {
                    frame.pc += 1;
                }
                let value = match ins.kind {
                    None => self.scalar_constant(bi, pc, ins.span)?,
                    Some(kind) => match kind {
                        InstructionKind::New(_) => {
                            return Err(self.invalid(
                                "`new` runs only for implicit elements on the direct executor",
                                Some(ins.span),
                            ));
                        }
                        InstructionKind::Constant(_) => {
                            let id = *self.data.constants.get(&(fi, bi, pc)).ok_or_else(|| {
                                self.invalid("Unprepared constant", Some(ins.span))
                            })?;
                            self.transfer(id, Some(ins.span))?;
                            id
                        }
                        InstructionKind::Unary { op, value } => {
                            let id = self.operand(*value, ins.span)?;
                            if !self.image.is_scalar() {
                                self.start_numeric_unary(
                                    *op,
                                    id,
                                    ins.ty,
                                    CallSite {
                                        function: fi,
                                        block: bi,
                                        instruction: pc,
                                    },
                                )?;
                                continue;
                            }
                            self.unary(*op, id, ins.ty, ins.span)?
                        }
                        InstructionKind::Binary { op, left, right } => {
                            let a = self.operand(*left, ins.span)?;
                            let b = self.operand(*right, ins.span)?;
                            if !self.image.is_scalar() {
                                if matches!(op,intersect_model::definitions::rcore_behavior::ir::BinaryOp::Equal|intersect_model::definitions::rcore_behavior::ir::BinaryOp::NotEqual) {
                                    if matches!(self.node(a)?.data,Data::Scalar(_)) {
                                        match self.start_numeric_binary(*op,a,b,ins.ty,CallSite {function:fi,block:bi,instruction:pc})? {Some(id)=>id,None=>continue}
                                    }else if matches!(self.node(a)?.data,Data::Some(_)|Data::Ok(_)|Data::Err(_)|Data::Tuple(_)|Data::Record(_)|Data::Collection(_)) {
                                        self.start_comparison(a,b,ins.ty,*op==intersect_model::definitions::rcore_behavior::ir::BinaryOp::Equal,CallSite {function:fi,block:bi,instruction:pc})?;continue;
                                    }else{self.binary(*op,a,b,ins.ty,ins.span)?}
                                }else{
                                    if self.start_numeric_binary(*op,a,b,ins.ty,CallSite {function:fi,block:bi,instruction:pc})?.is_some() {return Err(self.invalid("Unexpected early arithmetic result",Some(ins.span)));}
                                    continue;
                                }
                            } else {
                                self.binary(*op, a, b, ins.ty, ins.span)?
                            }
                        }
                        InstructionKind::View { value } => {
                            let id = self.operand(*value, ins.span)?;
                            self.view(id, ins.ty, Some(ins.span))?
                        }
                        InstructionKind::MakeClosure { .. } | InstructionKind::Call { .. } => {
                            self.start_instruction_operands(CallSite {
                                function: fi,
                                block: bi,
                                instruction: pc,
                            })?;
                            continue;
                        }
                        InstructionKind::None => self.alloc(ins.ty, Data::None, Some(ins.span))?,
                        InstructionKind::Some(value)
                        | InstructionKind::Ok(value)
                        | InstructionKind::Err(value) => {
                            let id = self.operand(*value, ins.span)?;
                            let data = match kind {
                                InstructionKind::Some(_) => Data::Some(id),
                                InstructionKind::Ok(_) => Data::Ok(id),
                                _ => Data::Err(id),
                            };
                            self.alloc(ins.ty, data, Some(ins.span))?
                        }
                        InstructionKind::Tuple(_)
                        | InstructionKind::Collection(_)
                        | InstructionKind::Record(_) => {
                            self.start_construction(CallSite {
                                function: fi,
                                block: bi,
                                instruction: pc,
                            })?;
                            continue;
                        }
                        InstructionKind::CollectionCall {
                            operation,
                            value,
                            callback,
                            seed,
                        } => {
                            let input = self.operand(*value, ins.span)?;
                            let callback = self.operand(*callback, ins.span)?;
                            let seed = seed
                                .map(|value| self.operand(value, ins.span))
                                .transpose()?;
                            self.start_collection(
                                *operation,
                                input,
                                Some(callback),
                                seed,
                                ins.ty,
                                CallSite {
                                    function: fi,
                                    block: bi,
                                    instruction: pc,
                                },
                            )?;
                            continue;
                        }
                        InstructionKind::CollectionAlgebra {
                            operation,
                            value,
                            other,
                        } => {
                            let value = self.operand(*value, ins.span)?;
                            let other = other
                                .map(|operand| self.operand(operand, ins.span))
                                .transpose()?;
                            self.start_algebra(
                                *operation,
                                value,
                                other,
                                ins.ty,
                                CallSite {
                                    function: fi,
                                    block: bi,
                                    instruction: pc,
                                },
                            )?;
                            continue;
                        }
                        InstructionKind::CollectionMembership {
                            operation,
                            value,
                            other,
                            ..
                        } => {
                            let value = self.operand(*value, ins.span)?;
                            let other = self.operand(*other, ins.span)?;
                            self.start_membership(
                                *operation,
                                value,
                                other,
                                ins.ty,
                                CallSite {
                                    function: fi,
                                    block: bi,
                                    instruction: pc,
                                },
                            )?;
                            continue;
                        }
                        InstructionKind::CollectionFlatten { value } => {
                            let input = self.operand(*value, ins.span)?;
                            self.start_collection(
                                intersect_model::definitions::rcore_behavior::ir_v2::CollectionOp::FlatMap,
                                input,
                                None,
                                None,
                                ins.ty,
                                CallSite {
                                    function: fi,
                                    block: bi,
                                    instruction: pc,
                                },
                            )?;
                            continue;
                        }
                        InstructionKind::CollectionSize { value }
                        | InstructionKind::CollectionIsEmpty { value } => {
                            let id = self.operand(*value, ins.span)?;
                            let Data::Collection(items) = &self.node(id)?.data else {
                                return Err(self.invalid(
                                    "Collection measure requires collection data",
                                    Some(ins.span),
                                ));
                            };
                            let (scalar, payload) = if matches!(
                                kind,
                                InstructionKind::CollectionSize { .. }
                            ) {
                                let length = i64::try_from(items.len()).map_err(|_| {
                                    self.resource("Collection size exceeds Int64", Some(ins.span))
                                })?;
                                (
                                    intersect_model::definitions::rcore_behavior::types::Scalar::Int64,
                                    serde_json::json!({"kind":"int64", "value":length.to_string()}),
                                )
                            } else {
                                (
                                    intersect_model::definitions::rcore_behavior::types::Scalar::Bool,
                                    serde_json::json!(items.is_empty()),
                                )
                            };
                            self.alloc(
                                ins.ty,
                                Data::Scalar(intersect_model::definitions::rcore_behavior::ir::Value {
                                    ty: intersect_model::definitions::rcore_behavior::ir::ValueType::Scalar(
                                        scalar,
                                    ),
                                    payload,
                                }),
                                Some(ins.span),
                            )?
                        }
                        InstructionKind::Text { op, value } => {
                            use intersect_model::definitions::rcore_behavior::ir_v2::TextOp;
                            let operand = self.operand(*value, ins.span)?;
                            let input = match (op, &self.node(operand)?.data) {
                                (TextOp::Hex, Data::Scalar(value)) => {
                                    value.payload.as_str().map(str::to_owned)
                                }
                                (TextOp::Identity, Data::Model(payload)) => payload
                                    .get("element")
                                    .and_then(serde_json::Value::as_str)
                                    .map(str::to_owned),
                                (TextOp::Resource, Data::Model(_)) => {
                                    return Err(self.error(
                                        ExecutionErrorKind::Unsupported,
                                        "resource is available only on the direct executor",
                                        Some(ins.span),
                                    ));
                                }
                                _ => None,
                            };
                            let input = input.ok_or_else(|| {
                                self.invalid("text primitive operand", Some(ins.span))
                            })?;
                            let text = if *op == TextOp::Hex {
                                let mut hex = String::with_capacity(input.len() * 2);
                                for byte in input.bytes() {
                                    self.step(ins.span)?;
                                    hex.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
                                    hex.push(char::from(HEX_DIGITS[usize::from(byte & 15)]));
                                }
                                hex
                            } else {
                                input
                            };
                            self.alloc(
                                ins.ty,
                                Data::Scalar(intersect_model::definitions::rcore_behavior::ir::Value {
                                    ty: intersect_model::definitions::rcore_behavior::ir::ValueType::Scalar(
                                        intersect_model::definitions::rcore_behavior::types::Scalar::String,
                                    ),
                                    payload: serde_json::Value::String(text),
                                }),
                                Some(ins.span),
                            )?
                        }
                        InstructionKind::Split { value, separator } => {
                            let text = self.operand(*value, ins.span)?;
                            let separator = self.operand(*separator, ins.span)?;
                            let string = |data: &Data| match data {
                                Data::Scalar(value) => value.payload.as_str().map(str::to_owned),
                                _ => None,
                            };
                            let text = string(self.node(text)?.data).ok_or_else(|| {
                                self.invalid("split requires a String", Some(ins.span))
                            })?;
                            let separator =
                                string(self.node(separator)?.data).ok_or_else(|| {
                                    self.invalid(
                                        "split requires a String separator",
                                        Some(ins.span),
                                    )
                                })?;
                            if separator.is_empty() {
                                return Err(self.error(
                                    ExecutionErrorKind::Arguments,
                                    "split separator must be nonempty",
                                    Some(ins.span),
                                ));
                            }
                            let intersect_model::definitions::rcore_behavior::ir_v2::TypeNode::Collection {
                                element,
                                ..
                            } = *self.type_node(ins.ty)?
                            else {
                                return Err(self.invalid("split result is not a collection", Some(ins.span)));
                            };
                            let mut items = Vec::new();
                            for piece in text.split(separator.as_str()) {
                                self.step(ins.span)?;
                                items.push(self.alloc(
                                    element,
                                    Data::Scalar(intersect_model::definitions::rcore_behavior::ir::Value {
                                        ty: intersect_model::definitions::rcore_behavior::ir::ValueType::Scalar(
                                            intersect_model::definitions::rcore_behavior::types::Scalar::String,
                                        ),
                                        payload: serde_json::Value::String(piece.to_owned()),
                                    }),
                                    Some(ins.span),
                                )?);
                            }
                            self.alloc(ins.ty, Data::Collection(items), Some(ins.span))?
                        }
                        InstructionKind::ConvertCollection { value } => {
                            let id = self.operand(*value, ins.span)?;
                            self.start_conversion(
                                id,
                                ins.ty,
                                CallSite {
                                    function: fi,
                                    block: bi,
                                    instruction: pc,
                                },
                            )?;
                            continue;
                        }
                        InstructionKind::TupleField { value, index } => {
                            let id = self.operand(*value, ins.span)?;
                            let Data::Tuple(items) = &self.node(id)?.data else {
                                return Err(
                                    self.invalid("Tuple projection requires tuple", Some(ins.span))
                                );
                            };
                            let field = *items.get(*index as usize).ok_or_else(|| {
                                self.invalid("Tuple projection index out of bounds", Some(ins.span))
                            })?;
                            self.view(field, ins.ty, Some(ins.span))?
                        }
                        InstructionKind::Ownership {
                            operation,
                            value,
                            selected,
                        } => {
                            let id = self.operand(*value, ins.span)?;
                            self.start_ownership(
                                *operation,
                                id,
                                *selected,
                                ins.ty,
                                CallSite {
                                    function: fi,
                                    block: bi,
                                    instruction: pc,
                                },
                            )?;
                            continue;
                        }
                        InstructionKind::Incoming {
                            value,
                            source,
                            declaration,
                            ..
                        } => {
                            let id = self.operand(*value, ins.span)?;
                            self.start_incoming(
                                id,
                                *source,
                                declaration,
                                ins.ty,
                                CallSite {
                                    function: fi,
                                    block: bi,
                                    instruction: pc,
                                },
                            )?;
                            continue;
                        }
                        InstructionKind::StoredFeature { value, .. } => {
                            let id = self.operand(*value, ins.span)?;
                            self.start_stored_read(
                                id,
                                ins.ty,
                                CallSite {
                                    function: fi,
                                    block: bi,
                                    instruction: pc,
                                },
                            )?;
                            continue;
                        }
                        InstructionKind::RecordField { value, field } => {
                            let id = self.operand(*value, ins.span)?;
                            let Data::Record(fields) = &self.node(id)?.data else {
                                return Err(self
                                    .invalid("Record projection requires record", Some(ins.span)));
                            };
                            let field = *fields.get(field).ok_or_else(|| {
                                self.invalid("Unknown record field", Some(ins.span))
                            })?;
                            self.view(field, ins.ty, Some(ins.span))?
                        }
                        InstructionKind::Index { value, index } => {
                            let id = self.operand(*value, ins.span)?;
                            let index = self.operand(*index, ins.span)?;
                            self.index(id, index, ins.ty, ins.span)?
                        }
                    },
                };
                if self.image.is_scalar() {
                    self.scalar_instruction_result(value, ins.ty, ins.span)?;
                } else {
                    self.permit(self.node(value)?.ty, ins.ty, Some(ins.span))?;
                }
                if let Some(frame) = self.data.frames.last_mut() {
                    frame.registers.push(value)
                }
            } else {
                self.step(block.span)?;
                match block.terminator {
                    Terminator::Absurd { .. } => {
                        return Err(
                            self.invalid("Reached impossible uninhabited branch", Some(block.span))
                        );
                    }
                    Terminator::Return(value) => {
                        let id = self.operand(*value, block.span)?;
                        let expected =
                            self.data.frames.last().map(|f| f.result).ok_or_else(|| {
                                self.invalid("Missing return frame", Some(block.span))
                            })?;
                        let id = if self.image.is_scalar() {
                            if self.node(id)?.ty != expected {
                                return Err(self.invalid(
                                    "Scalar return type differs from signature",
                                    Some(block.span),
                                ));
                            }
                            id
                        } else {
                            self.view(id, expected, Some(block.span))?
                        };
                        self.data.frames.pop();
                        if self.data.frames.is_empty() {
                            return Ok(Some(id));
                        }
                        if let Some(frame) = self.data.frames.last_mut() {
                            if let Some(collection) = &mut frame.collection {
                                collection.returned = Some(id);
                            } else {
                                frame.registers.push(id)
                            }
                        }
                    }
                    Terminator::Jump(edge) => {
                        self.jump(edge.block, &edge.arguments, None, block.span, 0)?
                    }
                    Terminator::Branch {
                        condition,
                        then_edge,
                        else_edge,
                    } => {
                        let id = self.operand(*condition, block.span)?;
                        let Data::Scalar(value) = &self.node(id)?.data else {
                            return Err(self.invalid("Branch requires Bool", Some(block.span)));
                        };
                        let condition = value.payload.as_bool().ok_or_else(|| {
                            self.invalid("Branch requires Bool payload", Some(block.span))
                        })?;
                        let edge = if condition { then_edge } else { else_edge };
                        self.jump(
                            edge.block,
                            &edge.arguments,
                            None,
                            block.span,
                            u8::from(!condition),
                        )?
                    }
                    Terminator::Option {
                        subject,
                        none,
                        some,
                    } => {
                        let id = self.operand(*subject, block.span)?;
                        match self.node(id)?.data {
                            Data::None => {
                                self.jump(none.block, &none.arguments, None, block.span, 0)?
                            }
                            Data::Some(value) => {
                                self.jump(some.block, &some.arguments, Some(*value), block.span, 1)?
                            }
                            _ => {
                                return Err(
                                    self.invalid("Option branch requires Option", Some(block.span))
                                );
                            }
                        }
                    }
                    Terminator::Result { subject, ok, error } => {
                        let id = self.operand(*subject, block.span)?;
                        match self.node(id)?.data {
                            Data::Ok(value) => {
                                self.jump(ok.block, &ok.arguments, Some(*value), block.span, 0)?
                            }
                            Data::Err(value) => self.jump(
                                error.block,
                                &error.arguments,
                                Some(*value),
                                block.span,
                                1,
                            )?,
                            _ => {
                                return Err(
                                    self.invalid("Result branch requires Result", Some(block.span))
                                );
                            }
                        }
                    }
                }
            }
        }
        Ok(None)
    }
}
pub(super) fn evaluate(
    image: &VerifiedExecutable,
    arguments: &[ValueWire],
    context: &ExecutionContext<'_>,
    limits: &ExecutionLimits,
) -> Result<Evaluation> {
    evaluate_with_quantum(
        image,
        arguments,
        context,
        limits,
        std::num::NonZeroUsize::MAX,
    )
}

fn evaluate_with_quantum(
    image: &VerifiedExecutable,
    arguments: &[ValueWire],
    context: &ExecutionContext<'_>,
    limits: &ExecutionLimits,
    quantum: std::num::NonZeroUsize,
) -> Result<Evaluation> {
    let mut session = EvaluationSession::new(image, arguments, context, limits)?;
    loop {
        match session.resume(quantum) {
            SessionProgress::Pending => {}
            SessionProgress::Complete(outcome) => return *outcome,
            SessionProgress::Finished => return Err(session.finished_error()),
        }
    }
}

#[path = "rcore_behavior_functions_session.rs"]
mod session;
pub(crate) use session::BorrowedEvaluationSession;
pub use session::{EvaluationSession, SessionProgress};

#[cfg(test)]
#[path = "rcore_behavior_functions_slices_tests.rs"]
mod slice_tests;

// No larger eager machine allocation is introduced before the instruction's
// original admission. Numeric storage shares an exclusive calculation slot.
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<Frame>() <= 6960);
#[cfg(target_arch = "wasm32")]
const _: () = assert!(std::mem::size_of::<Frame>() <= 3544);

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
