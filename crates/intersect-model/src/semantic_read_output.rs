//! Representation adapters AFTER the one checked semantic read preparation.
use super::*;
use storage::{SemanticFeatureSelection, ValueView};

/// Shape-preserving owned transport for reference consumers. It grants no
/// reference-kind, target, graph or source authority. Malformed items retain
/// their positions so the consumer's original validation order is unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReferenceReadItem {
    Identity(String),
    Other,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReferenceReadValue {
    Null,
    Identity(String),
    Collection(Vec<ReferenceReadItem>),
    Other,
}
fn items(
    length: usize,
    observation: &InvocationContext<'_>,
) -> QueryResult<Vec<ReferenceReadItem>> {
    let bytes = length
        .checked_mul(crate::logical_size!(ReferenceReadItem, 24))
        .ok_or("Reference output size overflow")?;
    observation.retain_storage(bytes)?;
    observation.work(1)?;
    observation.checkpoint()?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(length)
        .map_err(|_| "Unable to allocate reference output")?;
    Ok(result)
}
impl ReferenceReadValue {
    /// Move identities from an existing getter result; this performs no read.
    pub fn from_json(value: Value) -> QueryResult<Self> {
        Self::owned(value, &InvocationContext::default())
    }
    /// Move an existing getter result while charging the same caller for the
    /// new transport container/processing, not for strings already owned.
    pub fn from_json_with_work(value: Value, budget: &mut QueryBudget) -> QueryResult<Self> {
        let observation = InvocationContext::new(Rc::new(BudgetObservation(RefCell::new(budget))));
        Self::owned(value, &observation)
    }
    fn owned(value: Value, observation: &InvocationContext<'_>) -> QueryResult<Self> {
        observation.work(1)?;
        Ok(match value {
            Value::Null => Self::Null,
            Value::String(id) => Self::Identity(id),
            Value::Array(values) => {
                let mut result = items(values.len(), observation)?;
                for value in values {
                    observation.work(1)?;
                    result.push(match value {
                        Value::String(id) => ReferenceReadItem::Identity(id),
                        _ => ReferenceReadItem::Other,
                    });
                }
                Self::Collection(result)
            }
            _ => Self::Other,
        })
    }
    fn borrowed(value: ValueView<'_>, observation: &InvocationContext<'_>) -> QueryResult<Self> {
        let identity = |text: &str| -> QueryResult<String> {
            crate::definitions::retained_storage::reserve_serialized(text, observation)?;
            observation.checkpoint()?;
            Ok(text.to_owned())
        };
        Ok(if value.is_null() {
            Self::Null
        } else if let Some(text) = value.as_str() {
            Self::Identity(identity(text)?)
        } else if let Some(values) = value.collection() {
            let mut result = items(values.len(), observation)?;
            for value in values.iter() {
                observation.work(1)?;
                result.push(match value.as_str() {
                    Some(text) => ReferenceReadItem::Identity(identity(text)?),
                    // Unresolved/opaque backing representations must keep the
                    // existing JSON spelling/shape, never invent malformedness.
                    None => match value.to_json_observed(observation)? {
                        Value::String(text) => ReferenceReadItem::Identity(text),
                        _ => ReferenceReadItem::Other,
                    },
                });
            }
            Self::Collection(result)
        } else {
            // Includes '?spelling', missing numeric identities -> Null, and
            // Cell::Other JSON strings/arrays. Only canonical borrowed forms
            // avoid materialization; compatibility remains the old projection.
            Self::owned(value.to_json_observed(observation)?, observation)?
        })
    }
}
pub(super) trait ReadOutput {
    type Output;
    const OBSERVE_COMPUTED: bool = false;
    fn stored(
        self,
        selected: &SemanticFeatureSelection,
        store: &dyn storage::ModelRead,
        schema: &Schema,
        object: &str,
        observation: &InvocationContext<'_>,
        fully_observed: bool,
    ) -> QueryResult<Self::Output>;
    fn computed(
        self,
        value: Value,
        observation: &InvocationContext<'_>,
    ) -> QueryResult<Self::Output>;
}
pub(super) struct JsonOutput;
impl ReadOutput for JsonOutput {
    type Output = Value;
    fn stored(
        self,
        selected: &SemanticFeatureSelection,
        store: &dyn storage::ModelRead,
        schema: &Schema,
        object: &str,
        observation: &InvocationContext<'_>,
        fully_observed: bool,
    ) -> QueryResult<Value> {
        selected.stored(store, schema, object, observation, fully_observed)
    }
    fn computed(self, value: Value, _: &InvocationContext<'_>) -> QueryResult<Value> {
        Ok(value)
    }
}
pub(super) struct ReferenceOutput;
impl ReadOutput for ReferenceOutput {
    type Output = ReferenceReadValue;
    const OBSERVE_COMPUTED: bool = true;
    fn stored(
        self,
        selected: &SemanticFeatureSelection,
        store: &dyn storage::ModelRead,
        schema: &Schema,
        object: &str,
        observation: &InvocationContext<'_>,
        fully_observed: bool,
    ) -> QueryResult<ReferenceReadValue> {
        let mut result = None;
        let mut protocol_failed = false;
        let inspection = selected.inspect_stored(
            store,
            schema,
            object,
            observation,
            fully_observed,
            &mut |value| {
                if result.is_some() {
                    protocol_failed = true;
                    return Err("Stored reference adapter visited more than once".into());
                }
                result = Some(ReferenceReadValue::borrowed(value, observation));
                match &result {
                    Some(Err(error)) => Err(error.clone()),
                    _ => Ok(()),
                }
            },
        );
        // A raw adapter cannot swallow a callback failure and publish a prior
        // value, or revive the invocation by visiting a second valid value.
        match result {
            Some(Err(error)) => Err(error),
            _ if protocol_failed => Err("Stored reference adapter visited more than once".into()),
            Some(Ok(value)) => {
                inspection?;
                Ok(value)
            }
            None => {
                inspection?;
                Err("Stored reference adapter did not visit a value".into())
            }
        }
    }
    fn computed(
        self,
        value: Value,
        observation: &InvocationContext<'_>,
    ) -> QueryResult<ReferenceReadValue> {
        ReferenceReadValue::owned(value, observation)
    }
}
