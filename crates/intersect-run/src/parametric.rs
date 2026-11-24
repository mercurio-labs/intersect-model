//! Ordered independent runs over language-supplied parameter sets.
use crate::{Input, Profile, RunError, RunOutcome, Semantics, Trace, TraceMetadata, run_traced};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

/// Parameter names and portable values. The adapter checks types and units.
pub type Parameters = BTreeMap<String, Value>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunCase {
    pub id: String,
    pub parameters: Parameters,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaseError {
    Empty,
    CaseLimit { limit: usize },
    InvalidName,
    DuplicateCase(String),
    EmptyAxis(String),
    FixedAxisConflict(String),
}

impl fmt::Display for CaseError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => out.write_str("Provide at least one run case"),
            Self::CaseLimit { limit } => write!(out, "The run case limit is {limit}"),
            Self::InvalidName => out.write_str("Case IDs and parameter names must be nonempty"),
            Self::DuplicateCase(id) => write!(out, "Duplicate run case ID {id}"),
            Self::EmptyAxis(name) => write!(out, "Parameter axis {name} has no values"),
            Self::FixedAxisConflict(name) => {
                write!(out, "Parameter {name} is both fixed and swept")
            }
        }
    }
}

impl Error for CaseError {}

/// An admitted collection of cases with stable identities and bounded cardinality.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunCases(Vec<RunCase>);

impl RunCases {
    /// Retains the caller's case order.
    pub fn explicit(cases: Vec<RunCase>, limit: usize) -> Result<Self, CaseError> {
        if cases.is_empty() {
            return Err(CaseError::Empty);
        }
        if cases.len() > limit {
            return Err(CaseError::CaseLimit { limit });
        }
        let mut ids = BTreeSet::new();
        for case in &cases {
            if case.id.trim().is_empty()
                || case.parameters.keys().any(|name| name.trim().is_empty())
            {
                return Err(CaseError::InvalidName);
            }
            if !ids.insert(&case.id) {
                return Err(CaseError::DuplicateCase(case.id.clone()));
            }
        }
        Ok(Self(cases))
    }

    /// Expands the Cartesian product in parameter-name order. The last axis
    /// varies fastest, each axis retains its supplied value order, and no axes
    /// yields one case with the fixed parameters.
    pub fn grid(
        fixed: Parameters,
        axes: BTreeMap<String, Vec<Value>>,
        limit: usize,
    ) -> Result<Self, CaseError> {
        if fixed
            .keys()
            .chain(axes.keys())
            .any(|name| name.trim().is_empty())
        {
            return Err(CaseError::InvalidName);
        }
        let mut count = 1usize;
        if count > limit {
            return Err(CaseError::CaseLimit { limit });
        }
        for (name, values) in &axes {
            if fixed.contains_key(name) {
                return Err(CaseError::FixedAxisConflict(name.clone()));
            }
            if values.is_empty() {
                return Err(CaseError::EmptyAxis(name.clone()));
            }
            count = count
                .checked_mul(values.len())
                .filter(|count| *count <= limit)
                .ok_or(CaseError::CaseLimit { limit })?;
        }
        let mut cases = Vec::with_capacity(count);
        for index in 0..count {
            let mut offset = index;
            let mut parameters = fixed.clone();
            for (name, values) in axes.iter().rev() {
                parameters.insert(name.clone(), values[offset % values.len()].clone());
                offset /= values.len();
            }
            cases.push(RunCase {
                id: format!("case-{}", index + 1),
                parameters,
            });
        }
        Ok(Self(cases))
    }

    pub fn as_slice(&self) -> &[RunCase] {
        &self.0
    }
}

#[derive(Debug)]
pub struct CaseOutcome<'a, State> {
    pub case: &'a RunCase,
    pub result: Result<(RunOutcome<State>, Trace), RunError>,
}

/// Executes cases lazily. Each case constructs its own semantics and inputs and
/// starts from the profile's original seed and budgets. A failed case yields its
/// error alongside its parameters. The iterator can proceed to subsequent cases.
pub fn run_cases<'a, S, I>(
    cases: &'a RunCases,
    profile: &'a Profile,
    metadata: TraceMetadata,
    mut prepare: impl FnMut(&Parameters) -> Result<(S, I), RunError> + 'a,
) -> impl ExactSizeIterator<Item = CaseOutcome<'a, S::State>> + 'a
where
    S: Semantics + 'a,
    I: IntoIterator<Item = Input<S::Event>> + 'a,
{
    cases.as_slice().iter().map(move |case| {
        let result = prepare(&case.parameters).and_then(|(semantics, inputs)| {
            run_traced(&semantics, profile, inputs, metadata.clone())
        });
        CaseOutcome { case, result }
    })
}
