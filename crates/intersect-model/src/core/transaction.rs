//! Handwritten deterministic transactions over the Intersect Model.
//! Hosts provide identities/revisions; language packages validate semantics.
use crate::core::model::{Element, Model};
use intersect_model::definitions::schema::Schema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionContext {
    pub id: String,
    pub language_revision: String,
    pub base_generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionStatus {
    Running,
    Prepared,
    Committed,
    RolledBack,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub kind: String,
    pub element: String,
    pub class: String,
    pub feature: Option<String>,
    pub before: Value,
    pub after: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransactionRecord {
    pub context: TransactionContext,
    pub status: TransactionStatus,
    pub changes: Vec<Change>,
    pub reason: Option<String>,
}

impl TransactionRecord {
    /// Hosts can terminate an outstanding transaction after losing its worker.
    /// A committed receipt is only produced by ModelTransaction::commit.
    pub fn abort(&mut self, status: TransactionStatus, reason: String) -> Result<(), String> {
        if !matches!(
            status,
            TransactionStatus::RolledBack | TransactionStatus::Cancelled
        ) {
            return Err("Abort requires rolled_back or cancelled status".into());
        }
        if !matches!(
            self.status,
            TransactionStatus::Running | TransactionStatus::Prepared
        ) {
            return Err("Transaction is already closed".into());
        }
        self.status = status;
        self.reason = Some(reason);
        Ok(())
    }
}

pub struct ModelTransaction {
    context: TransactionContext,
    before: Model,
    staged: Model,
    status: TransactionStatus,
    reason: Option<String>,
}

impl Model {
    pub fn begin_transaction(
        &self,
        schema: &Schema,
        context: TransactionContext,
    ) -> Result<ModelTransaction, String> {
        self.begin_transaction_context(schema, context, None)
    }
    pub fn begin_transaction_with_conversions(
        &self,
        schema: &Schema,
        context: TransactionContext,
        conversions: &intersect_model::definitions::conversion::ConversionRegistry<'_>,
    ) -> Result<ModelTransaction, String> {
        self.begin_transaction_context(schema, context, Some(conversions))
    }
    fn begin_transaction_context(
        &self,
        schema: &Schema,
        context: TransactionContext,
        conversions: Option<&intersect_model::definitions::conversion::ConversionRegistry<'_>>,
    ) -> Result<ModelTransaction, String> {
        match conversions {
            Some(registry) => self.validate_with_conversions(schema, registry)?,
            None => self.validate(schema)?,
        }
        if context.id.is_empty() || context.language_revision.is_empty() {
            return Err("Transaction identity and language revision must be nonempty".into());
        }
        Ok(ModelTransaction {
            context,
            before: self.clone(),
            staged: self.clone(),
            status: TransactionStatus::Running,
            reason: None,
        })
    }
}

impl ModelTransaction {
    pub fn model(&self) -> &Model {
        &self.staged
    }
    pub fn before(&self) -> &Model {
        &self.before
    }
    pub fn context(&self) -> &TransactionContext {
        &self.context
    }
    pub fn status(&self) -> TransactionStatus {
        self.status
    }

    pub fn stage<T>(
        &mut self,
        operation: impl FnOnce(&mut Model) -> Result<T, String>,
    ) -> Result<T, String> {
        if self.status != TransactionStatus::Running {
            return Err("Transaction is not running".into());
        }
        let mut next = self.staged.clone();
        match operation(&mut next) {
            Ok(value) => {
                self.staged = next;
                Ok(value)
            }
            Err(error) => {
                self.abort(TransactionStatus::RolledBack, error.clone())?;
                Err(error)
            }
        }
    }

    pub fn prepare(
        &mut self,
        schema: &Schema,
        validate_language: impl FnOnce(&Model) -> Result<(), String>,
    ) -> Result<(), String> {
        self.prepare_context(schema, validate_language, None)
    }
    pub fn prepare_with_conversions(
        &mut self,
        schema: &Schema,
        validate_language: impl FnOnce(&Model) -> Result<(), String>,
        conversions: &intersect_model::definitions::conversion::ConversionRegistry<'_>,
    ) -> Result<(), String> {
        self.prepare_context(schema, validate_language, Some(conversions))
    }
    fn prepare_context(
        &mut self,
        schema: &Schema,
        validate_language: impl FnOnce(&Model) -> Result<(), String>,
        conversions: Option<&intersect_model::definitions::conversion::ConversionRegistry<'_>>,
    ) -> Result<(), String> {
        if self.status != TransactionStatus::Running {
            return Err("Transaction is not running".into());
        }
        let validation = match conversions {
            Some(registry) => self.staged.validate_with_conversions(schema, registry),
            None => self.staged.validate(schema),
        };
        if let Err(error) = validation.and_then(|()| validate_language(&self.staged)) {
            self.abort(TransactionStatus::RolledBack, error.clone())?;
            return Err(error);
        }
        self.status = TransactionStatus::Prepared;
        Ok(())
    }

    /// Publish once, after preparation. Exact baseline comparison also protects
    /// native callers that accidentally fail to advance their generation.
    pub fn commit(
        &mut self,
        current: &mut Model,
        language_revision: &str,
        generation: u64,
    ) -> Result<TransactionRecord, String> {
        if self.status != TransactionStatus::Prepared {
            return Err("Transaction is not prepared".into());
        }
        if language_revision != self.context.language_revision
            || generation != self.context.base_generation
            || *current != self.before
        {
            let error = "Stale transaction: language or model changed".to_string();
            self.abort(TransactionStatus::RolledBack, error.clone())?;
            return Err(error);
        }
        *current = self.staged.clone();
        self.status = TransactionStatus::Committed;
        Ok(self.record())
    }

    pub fn abort(&mut self, status: TransactionStatus, reason: String) -> Result<(), String> {
        let mut record = self.record();
        record.abort(status, reason)?;
        self.status = record.status;
        self.reason = record.reason;
        Ok(())
    }

    pub fn record(&self) -> TransactionRecord {
        TransactionRecord {
            context: self.context.clone(),
            status: self.status,
            changes: changes(&self.before, &self.staged),
            reason: self.reason.clone(),
        }
    }
}

/// Deterministic net changes, including automatically maintained opposites.
pub fn changes(before: &Model, after: &Model) -> Vec<Change> {
    let old: BTreeMap<_, _> = before.elements.iter().map(|e| (e.id.as_str(), e)).collect();
    let new: BTreeMap<_, _> = after.elements.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut changes = vec![];
    for id in old
        .keys()
        .chain(new.keys())
        .copied()
        .collect::<BTreeSet<_>>()
    {
        match (old.get(id), new.get(id)) {
            (None, Some(e)) => changes.push(element_change("created", e, Value::Null, json!(e))),
            (Some(e), None) => changes.push(element_change("deleted", e, json!(e), Value::Null)),
            (Some(a), Some(b)) => {
                for feature in a
                    .values
                    .keys()
                    .chain(b.values.keys())
                    .collect::<BTreeSet<_>>()
                {
                    let previous = a.values.get(feature).cloned().unwrap_or(Value::Null);
                    let next = b.values.get(feature).cloned().unwrap_or(Value::Null);
                    if previous != next {
                        changes.push(Change {
                            kind: "updated".into(),
                            element: id.into(),
                            class: b.class.clone(),
                            feature: Some(feature.clone()),
                            before: previous,
                            after: next,
                        });
                    }
                }
                if a.class != b.class
                    || a.type_identity != b.type_identity
                    || a.resource != b.resource
                    || a.start != b.start
                    || a.end != b.end
                {
                    changes.push(element_change("identity_changed", b, json!({"class":a.class,"type_identity":a.type_identity,"resource":a.resource,"start":a.start,"end":a.end}), json!({"class":b.class,"type_identity":b.type_identity,"resource":b.resource,"start":b.start,"end":b.end})));
                }
            }
            _ => {}
        }
    }
    changes
}

fn element_change(kind: &str, element: &Element, before: Value, after: Value) -> Change {
    Change {
        kind: kind.into(),
        element: element.id.clone(),
        class: element.class.clone(),
        feature: None,
        before,
        after,
    }
}
