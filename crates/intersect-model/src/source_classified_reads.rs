//! Generic observation cursor for source-authored selection over checked reads.
//! Cursor entities describe observations, never a substitute language model.
use crate::{
    core::query::QueryBudget,
    definitions::{
        bundle::{DefinitionSource, SourceKind},
        conversion::InvocationContext,
        rcore_behavior::{
            ir_v2::{DataWire, EntrySelection, Limits, TypeNode},
            modules::SymbolId,
            types::Scalar,
        },
        resources::DeclarationId,
        scalars::{Builtin, int64_wire},
        schema::MemberIdentity,
    },
    rcore::{RcorePackage, compile_rcore_at},
    rcore_behavior_functions::ExecutionLimits,
    scope_runtime::{ScopePage, ScopeRead, ScopeValue},
    semantic_bindings::BorrowedBindingWork,
    semantic_reads::ClassifiedValue,
    source_model_functions::SourceModelFunction,
};
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    sync::OnceLock,
};

fn protocol() -> Result<&'static RcorePackage, String> {
    static PROTOCOL: OnceLock<Result<RcorePackage, String>> = OnceLock::new();
    PROTOCOL
        .get_or_init(|| {
            compile_rcore_at(
                "intersect-model:/protocols/classified-read.rcore",
                include_str!("classified_read_protocol.rcore"),
            )
            .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)
}
/// Source body ABI: `(Cursor) -> Option<Int64>`. Only a genuinely observed
/// matching occurrence can be returned. Source code owns traversal/selection.
pub struct SourceClassifiedRead {
    function: SourceModelFunction<'static>,
    max_observations: usize,
}
impl SourceClassifiedRead {
    pub fn compile(
        uri: &str,
        text: &str,
        declaration: usize,
        limits: ExecutionLimits,
    ) -> Result<Self, String> {
        let package = protocol()?;
        let mut source = DefinitionSource::new(uri, SourceKind::RcoreBehavior, text);
        source.dependencies.push(package.entry().clone());
        let selection = EntrySelection {
            symbol: SymbolId {
                source: source.id.clone(),
                declaration,
            },
            types: vec![],
            arguments: vec![],
        };
        let max_observations = limits.max_model_elements;
        let function = SourceModelFunction::from_rcore(
            package,
            vec![source],
            &selection,
            &Limits::default(),
            limits,
            Default::default(),
        )?;
        let image = function.executable();
        let entry = &image.program().functions[image.program().entry.0 as usize];
        let receiver = match image
            .type_node(entry.parameters[0])
            .map_err(|error| error.to_string())?
        {
            TypeNode::Model(receiver) => package.schema().class_key_for_type(receiver)?,
            _ => return Err("Classified source requires its Cursor receiver".into()),
        };
        let result = match image
            .type_node(entry.result)
            .map_err(|error| error.to_string())?
        {
            TypeNode::Option(inner) => {
                image.type_node(*inner).map_err(|error| error.to_string())?
            }
            _ => return Err("Classified source requires Option<Int64> result".into()),
        };
        if entry.parameters.len() != 1
            || receiver
                != package.schema().class_key(
                    package.entry(),
                    "urn:intersect:classified-read-protocol",
                    "Cursor",
                )?
            || *result != TypeNode::Scalar(Scalar::Int64)
        {
            return Err("Classified source requires (Cursor) -> Option<Int64>".into());
        }
        Ok(Self {
            function,
            max_observations,
        })
    }
    pub fn invoke(
        &self,
        read: impl FnMut(usize, &mut QueryBudget) -> Result<ClassifiedValue, String>,
        budget: &mut QueryBudget,
    ) -> Result<Option<(usize, Value)>, String> {
        let work = BorrowedBindingWork::bind(budget);
        let cursor = CursorRead {
            read: RefCell::new(read),
            work: &work,
            values: RefCell::new(Vec::new()),
            ended: Cell::new(false),
            failed: Cell::new(false),
            failure: RefCell::new(None),
            max: self.max_observations,
        };
        let result = self
            .function
            .invoke_with_work(&cursor, "cursor", vec![], &work);
        if let Some(error) = cursor.failure.borrow_mut().take() {
            return Err(error);
        }
        let value = result?;
        work.charge(1).map_err(|error| error.to_string())?;
        let selected =
            match value.data {
                DataWire::None => None,
                DataWire::Some(value) => {
                    let DataWire::Scalar(value) = value.data else {
                        return Err("Malformed classified source index".into());
                    };
                    let text = Builtin::ELong
                        .format(&value.payload)?
                        .ok_or("Missing classified source index")?;
                    Some(text.parse::<usize>().map_err(
                        |_| "Classified source index is negative or exceeds the host range",
                    )?)
                }
                _ => return Err("Malformed classified source result".into()),
            };
        let result = if let Some(index) = selected {
            let value = cursor
                .values
                .borrow_mut()
                .get_mut(index)
                .and_then(Option::take)
                .ok_or("Classified source selected an unobserved or nonmatching occurrence")?;
            Some((index, value))
        } else {
            None
        };
        work.check().map_err(|error| error.to_string())?;
        Ok(result)
    }
}
struct CursorRead<'a, 'caller, F> {
    read: RefCell<F>,
    work: &'a BorrowedBindingWork<'caller>,
    values: RefCell<Vec<Option<Value>>>,
    ended: Cell<bool>,
    failed: Cell<bool>,
    failure: RefCell<Option<String>>,
    max: usize,
}
impl<F: FnMut(usize, &mut QueryBudget) -> Result<ClassifiedValue, String>> CursorRead<'_, '_, F> {
    fn index(&self, object: &str, observation: &InvocationContext<'_>) -> Result<usize, String> {
        observation.work(1 + object.len().div_ceil(64))?;
        let index = object
            .strip_prefix("observation:")
            .ok_or("Unknown observation identity")?
            .parse::<usize>()
            .map_err(|_| "Malformed observation identity")?;
        if index >= self.values.borrow().len() {
            return Err("Unobserved cursor identity".into());
        }
        Ok(index)
    }
    fn probe(&self, index: usize, observation: &InvocationContext<'_>) -> Result<Value, String> {
        self.check(observation)?;
        if index == self.values.borrow().len() && !self.ended.get() {
            if index >= self.max {
                return Err("Classified observation limit exceeded".into());
            }
            observation.work(1)?;
            let value = self.work.read_callback(|budget| {
                let mut read = self
                    .read
                    .try_borrow_mut()
                    .map_err(|_| "Classified callback reentry")?;
                read(index, budget)
            });
            let value = match value {
                Ok(value) => value,
                Err(error) => {
                    self.failed.set(true);
                    *self.failure.borrow_mut() = Some(error);
                    return Err("Classified read callback failed".into());
                }
            };
            match value {
                ClassifiedValue::Absent => self.ended.set(true),
                value => {
                    let value = match value {
                        ClassifiedValue::Match(value) => Some(value),
                        ClassifiedValue::NonMatch => None,
                        ClassifiedValue::Absent => return Err("Invalid cursor state".into()),
                    };
                    let mut values = self.values.borrow_mut();
                    if values.len() == values.capacity() {
                        let count = 64usize.min(self.max.saturating_sub(values.len()));
                        let bytes = values
                            .len()
                            .checked_add(count)
                            .and_then(|n| n.checked_mul(crate::logical_size!(Option<Value>, 32)))
                            .ok_or("Classified observation storage overflow")?;
                        observation.retain_storage(bytes)?;
                        values
                            .try_reserve_exact(count)
                            .map_err(|_| "Classified observation allocation failed")?;
                    }
                    values.push(value);
                }
            }
        }
        if index < self.values.borrow().len() {
            observation.retain_storage(32)?;
            let id = format!("observation:{index}");
            Ok(Value::String(id))
        } else if self.ended.get() {
            Ok(Value::Null)
        } else {
            Err("Classified cursor requested a nonsequential observation".into())
        }
    }
}
impl<F: FnMut(usize, &mut QueryBudget) -> Result<ClassifiedValue, String>> ScopeRead
    for CursorRead<'_, '_, F>
{
    fn check(&self, observation: &InvocationContext<'_>) -> Result<(), String> {
        observation.work(1)?;
        self.work.check().map_err(|error| error.to_string())?;
        if self.failed.get() {
            return Err("Classified observation cursor has failed".into());
        }
        Ok(())
    }
    fn class(&self, object: &str, observation: &InvocationContext<'_>) -> Result<String, String> {
        self.check(observation)?;
        let name = if object == "cursor" {
            "Cursor"
        } else {
            self.index(object, observation)?;
            "Observation"
        };
        let package = protocol()?;
        let class = package.schema().class_key(
            package.entry(),
            "urn:intersect:classified-read-protocol",
            name,
        )?;
        observation.retain_storage(class.len())?;
        Ok(class.into())
    }
    fn singular(
        &self,
        object: &str,
        member: &DeclarationId,
        observation: &InvocationContext<'_>,
    ) -> Result<ScopeValue, String> {
        let class = self.class(object, observation)?;
        let schema = protocol()?.schema();
        observation.work(4)?;
        observation.retain_storage(
            4 * crate::logical_size!(crate::definitions::schema::EffectiveFeature<'_>, 128),
        )?;
        let name = schema.effective_features(&class)?.into_iter().find(|field| matches!(&field.identity, MemberIdentity::Source { declaration } if declaration == member))
            .map(|field| field.feature.name.as_str()).ok_or("Unknown classified cursor member")?;
        let value = if object == "cursor" && name == "head" {
            self.probe(0, observation)?
        } else {
            let index = self.index(object, observation)?;
            match name {
                "matches" => Value::Bool(self.values.borrow()[index].is_some()),
                "position" => {
                    int64_wire(i64::try_from(index).map_err(|_| "Classified index exceeds Int64")?)
                }
                "next" => self.probe(
                    index.checked_add(1).ok_or("Classified index overflow")?,
                    observation,
                )?,
                _ => return Err("Unsupported classified cursor member".into()),
            }
        };
        self.check(observation)?;
        Ok(ScopeValue::Value(value))
    }
    fn page(
        &self,
        _: &str,
        _: &DeclarationId,
        _: usize,
        _: &InvocationContext<'_>,
    ) -> Result<ScopePage, String> {
        Err("Classified cursor exposes singular observation links".into())
    }
}
