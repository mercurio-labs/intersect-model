//! Shared staged context preparation. Execution starts only after all contexts pass.
use super::{machine::State, *};
use serde_json::Value;
// Affine identity input produced only from an already registered, owned
// context. It is not deserializable and exposes no mutation or public constructor.
pub(crate) struct PreparedReferenceInput {
    context: intersect_model::definitions::conversion::ReferenceContext,
    digest: String,
}
impl PreparedReferenceInput {
    pub(super) fn from_registered_owned(
        context: intersect_model::definitions::conversion::ReferenceContext,
        digest: String,
        index: usize,
        registered: &std::collections::BTreeMap<String, (usize, usize)>,
    ) -> std::result::Result<Self, String> {
        if registered.get(&digest).map(|row| row.0) != Some(index) {
            return Err("Owned context does not match prepared identity registration".into());
        }
        Ok(Self { context, digest })
    }
    pub(crate) fn into_parts(
        self,
    ) -> (
        intersect_model::definitions::conversion::ReferenceContext,
        String,
    ) {
        (self.context, self.digest)
    }
}

enum Stage {
    Bundle,
    Reference,
    Model,
    SourceBytes,
    Policies,
    Sources,
    Namespaces,
    Register,
}
pub(super) struct ContextPreparation {
    stage: Stage,
    index: usize,
    item: usize,
    candidate: usize,
    total: usize,
    bytes: usize,
    model: Option<ModelPreparation>,
}
impl ContextPreparation {
    pub(super) fn new() -> Self {
        Self {
            stage: Stage::Bundle,
            index: 0,
            item: 0,
            candidate: 0,
            total: 0,
            bytes: 0,
            model: None,
        }
    }
    #[cfg(test)]
    pub(super) fn in_model(&self) -> bool {
        matches!(self.stage, Stage::Model)
    }
    #[cfg(test)]
    pub(super) fn model_nodes(&self) -> Option<usize> {
        self.model.as_ref().map(|model| model.nodes)
    }
    #[cfg(test)]
    pub(super) fn membership_cursor(&self) -> Option<(bool, usize, usize)> {
        match self.stage {
            Stage::Sources => Some((false, self.item, self.candidate)),
            Stage::Namespaces => Some((true, self.item, self.candidate)),
            _ => None,
        }
    }
    #[cfg(test)]
    pub(super) fn policy_cursor(&self) -> Option<usize> {
        matches!(self.stage, Stage::Policies).then_some(self.item)
    }
    pub(super) fn advance<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
    ) -> Result<bool> {
        state.checkpoint(None)?;
        if matches!(self.stage, Stage::Bundle) {
            state.validate_bundle_context()?;
            self.stage = Stage::Reference;
            return Ok(false);
        }
        let Some(context) = state.function_context()?.references.get(self.index) else {
            return Ok(true);
        };
        match self.stage {
            Stage::Bundle => return Err(state.invalid("Unexpected context stage", None)),
            Stage::Reference => {
                let observation = state.invocation_observation(None)?;
                intersect_model::definitions::metadata_limits::measure_with_work(
                    context,
                    state.limits.max_value_nodes,
                    state.limits.max_value_depth,
                    state.limits.max_context_bytes,
                    &|units| observation.work(units),
                )
                .map_err(|e| {
                    if let Err(error) = state.check_provider_status(None) {
                        error
                    } else {
                        state.resource(&e, None)
                    }
                })?;
                self.model = Some(ModelPreparation::new(state)?);
                self.stage = Stage::Model;
            }
            Stage::Model => {
                let model = self
                    .model
                    .as_mut()
                    .ok_or_else(|| state.invalid("Missing model preparation", None))?;
                if let Some(bytes) = model.advance(state, &context.model)? {
                    self.bytes = bytes;
                    self.model = None;
                    self.item = 0;
                    if context.bundle.sources.len() > state.limits.max_value_nodes {
                        return Err(state.resource("Context source count limit exceeded", None));
                    }
                    self.stage = Stage::SourceBytes;
                }
            }
            Stage::SourceBytes => {
                if let Some(source) = context.bundle.sources.get(self.item) {
                    self.bytes = self
                        .bytes
                        .checked_add(source.text.len())
                        .filter(|n| *n <= state.limits.max_context_bytes)
                        .ok_or_else(|| {
                            state.resource("Context source byte limit exceeded", None)
                        })?;
                    self.item += 1;
                } else {
                    self.item = 0;
                    self.stage = Stage::Policies;
                }
            }
            Stage::Policies => {
                if state.validate_reference_policy(context, self.item, None)? {
                    self.item = 0;
                    self.stage = Stage::Sources;
                } else {
                    self.item += 1;
                }
            }
            Stage::Sources => {
                if advance_exact_source_rows(
                    &context.bundle.sources,
                    &state.function_context()?.bundle.sources,
                    &mut self.item,
                    &mut self.candidate,
                )
                .map_err(|_| {
                    state.error(
                        ExecutionErrorKind::Nominal,
                        "Reference context contains foreign source/dependency identity",
                        None,
                    )
                })? {
                    self.item = 0;
                    self.stage = Stage::Namespaces;
                }
            }
            Stage::Namespaces => {
                if let Some(binding) = context.bundle.namespaces.get(self.item) {
                    let candidate = state
                        .function_context()?
                        .bundle
                        .namespaces
                        .get(self.candidate);
                    let Some(candidate) = candidate else {
                        return Err(state.error(
                            ExecutionErrorKind::Nominal,
                            "Reference context namespace differs from execution context",
                            None,
                        ));
                    };
                    if candidate == binding {
                        self.item += 1;
                        self.candidate = 0;
                    } else {
                        self.candidate += 1;
                    }
                } else {
                    self.stage = Stage::Register;
                }
            }
            Stage::Register => {
                let bytes = state.reference_context_bytes(context, None)?;
                self.total = self
                    .total
                    .checked_add(bytes)
                    .filter(|n| {
                        *n <= state
                            .limits
                            .max_context_bytes
                            .saturating_sub(state.data.consumed.context_bytes)
                    })
                    .ok_or_else(|| state.resource("Reference input byte limit exceeded", None))?;
                let observation = state.invocation_observation(None)?;
                let digest = context
                    .digest_with_work(&|units| observation.work(units))
                    .map_err(|e| {
                        if let Err(error) = state.check_provider_status(None) {
                            error
                        } else {
                            state.error(ExecutionErrorKind::Nominal, e, None)
                        }
                    })?;
                if state
                    .data
                    .reference_inputs
                    .insert(digest, (self.index, bytes))
                    .is_some()
                {
                    return Err(state.error(
                        ExecutionErrorKind::Nominal,
                        "Duplicate reference context identity",
                        None,
                    ));
                }
                self.index += 1;
                self.stage = Stage::Reference;
            }
        }
        Ok(false)
    }
}
enum Route {
    Index(usize),
    Field(String),
}
enum Frame {
    Visit,
    Leave,
    Array(usize),
    Object(Vec<String>),
}
struct ModelPreparation {
    frames: Vec<Frame>,
    path: Vec<Route>,
    pending: usize,
    nodes: usize,
    bytes: usize,
}
impl ModelPreparation {
    fn new<I: super::inputs::InputFamily>(state: &mut State<'_, '_, '_, I>) -> Result<Self> {
        state.retained_storage(64, None)?;
        let mut value = Self {
            frames: Vec::new(),
            path: Vec::new(),
            pending: 1,
            nodes: 0,
            bytes: 0,
        };
        value.push(state, Frame::Visit)?;
        Ok(value)
    }
    fn push<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        frame: Frame,
    ) -> Result<()> {
        if self.frames.len()
            >= state
                .limits
                .max_value_depth
                .saturating_add(2)
                .saturating_mul(3)
        {
            return Err(state.resource("Context frame limit exceeded", None));
        }
        if self.frames.len() == self.frames.capacity() {
            state.retained_storage(128, None)?;
            self.frames
                .try_reserve_exact(1)
                .map_err(|_| state.resource("Context frame allocation failed", None))?;
        }
        self.frames.push(frame);
        Ok(())
    }
    fn descend<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        route: Route,
    ) -> Result<()> {
        if self.path.len() == self.path.capacity() {
            state.retained_storage(64, None)?;
            self.path
                .try_reserve_exact(1)
                .map_err(|_| state.resource("Context path allocation failed", None))?;
        }
        self.path.push(route);
        self.push(state, Frame::Leave)?;
        self.push(state, Frame::Visit)
    }
    fn value<'a, I: super::inputs::InputFamily>(
        &self,
        state: &State<'_, '_, '_, I>,
        root: &'a Value,
    ) -> Result<&'a Value> {
        let mut value = root;
        for route in &self.path {
            value = match (route, value) {
                (Route::Index(index), Value::Array(values)) => values
                    .get(*index)
                    .ok_or_else(|| state.invalid("Context model item unavailable", None))?,
                (Route::Field(key), Value::Object(values)) => values
                    .get(key)
                    .ok_or_else(|| state.invalid("Context model field unavailable", None))?,
                _ => return Err(state.invalid("Context model path changed", None)),
            };
        }
        Ok(value)
    }
    fn advance<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        root: &Value,
    ) -> Result<Option<usize>> {
        state.checkpoint(None)?;
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| state.invalid("Context model already prepared", None))?;
        match frame {
            Frame::Leave => {
                self.path.pop();
            }
            Frame::Array(remaining) => {
                if remaining > 0 {
                    let index = remaining - 1;
                    self.push(state, Frame::Array(index))?;
                    self.descend(state, Route::Index(index))?;
                }
            }
            Frame::Object(mut keys) => {
                if let Some(key) = keys.pop() {
                    self.push(state, Frame::Object(keys))?;
                    self.descend(state, Route::Field(key))?;
                }
            }
            Frame::Visit => {
                self.pending = self.pending.saturating_sub(1);
                self.nodes = self
                    .nodes
                    .checked_add(1)
                    .filter(|n| *n <= state.limits.max_value_nodes)
                    .ok_or_else(|| state.resource("Model context node limit exceeded", None))?;
                if self.path.len() > state.limits.max_value_depth {
                    return Err(state.resource("Model context depth limit exceeded", None));
                }
                let value = self.value(state, root)?;
                let size = match value {
                    Value::String(s) => s.len(),
                    Value::Array(v) => v.len(),
                    Value::Object(v) => v
                        .keys()
                        .map(|s| s.len())
                        .try_fold(0usize, |a, b| a.checked_add(b))
                        .ok_or_else(|| state.resource("Context size overflow", None))?,
                    _ => 32,
                };
                self.bytes = self
                    .bytes
                    .checked_add(size)
                    .filter(|n| *n <= state.limits.max_context_bytes)
                    .ok_or_else(|| state.resource("Context byte limit exceeded", None))?;
                match value {
                    Value::Array(values) => {
                        if self.pending.saturating_add(values.len()) > state.limits.max_value_nodes
                        {
                            return Err(state.resource("Context node limit exceeded", None));
                        }
                        self.pending += values.len();
                        self.push(state, Frame::Array(values.len()))?;
                    }
                    Value::Object(values) => {
                        if self.pending.saturating_add(values.len()) > state.limits.max_value_nodes
                        {
                            return Err(state.resource("Context node limit exceeded", None));
                        }
                        self.pending += values.len();
                        // serde_json's public map has no retained positional cursor.
                        // Snapshot keys once after whole-context metadata admission;
                        // key indexing remains an explicitly bounded atomic phase.
                        let storage = values
                            .len()
                            .checked_mul(24)
                            .and_then(|n| n.checked_add(size))
                            .ok_or_else(|| state.resource("Context key storage overflow", None))?;
                        state.retained_storage(storage, None)?;
                        let mut keys = Vec::new();
                        keys.try_reserve_exact(values.len())
                            .map_err(|_| state.resource("Context key allocation failed", None))?;
                        keys.extend(values.keys().cloned());
                        self.push(state, Frame::Object(keys))?;
                    }
                    _ => {}
                }
            }
        }
        if self.frames.is_empty() {
            Ok(Some(self.bytes))
        } else {
            Ok(None)
        }
    }
}

/// One existing exact candidate comparison. The caller owns its original step
/// admission; this helper neither charges again nor constructs authority.
pub(crate) fn advance_exact_source_rows<T: PartialEq>(
    required: &[T],
    available: &[T],
    item: &mut usize,
    candidate: &mut usize,
) -> std::result::Result<bool, ()> {
    let Some(source) = required.get(*item) else {
        return Ok(true);
    };
    let actual = available.get(*candidate).ok_or(())?;
    if actual == source {
        *item += 1;
        *candidate = 0;
    } else {
        *candidate += 1;
    }
    Ok(false)
}
