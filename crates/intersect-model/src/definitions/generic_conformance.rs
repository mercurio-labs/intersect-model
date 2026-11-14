//! Nominal application bounds with explicit raw-argument warnings. No ambient provider or Java assignability.
use super::*;
use crate::definitions::generic_types::{ParameterBinding, Substitution, Type, TypeUse};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundCheck {
    pub application: Type,
    pub parameter: DeclarationId,
    pub argument: Type,
    pub declared_bound: TypeUse,
    pub substituted_bound: Type,
    pub satisfied: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationBounds {
    pub selected: TypeUse,
    pub checks: Vec<BoundCheck>,
    /// Bound satisfaction; warnings (including omitted raw arguments) are separate.
    pub valid: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
}
#[derive(Debug)]
struct SharedCheckWarning {
    code: &'static str,
    message: String,
    span: std::rc::Rc<SourceSpan>,
}
#[derive(Debug)]
enum CheckWarnings {
    // Membership predicates consume validity only; no report is published.
    Discarded,
    Owned(Vec<Warning>),
    Shared(Vec<SharedCheckWarning>),
}
impl CheckWarnings {
    fn len(&self) -> usize {
        match self {
            Self::Discarded => 0,
            Self::Owned(rows) => rows.len(),
            Self::Shared(rows) => rows.len(),
        }
    }
    fn at(&self, index: usize) -> Option<(&str, &str, &SourceSpan)> {
        match self {
            Self::Discarded => None,
            Self::Owned(rows) => rows
                .get(index)
                .map(|row| (row.code.as_str(), row.message.as_str(), &row.span)),
            Self::Shared(rows) => rows
                .get(index)
                .map(|row| (row.code, row.message.as_str(), row.span.as_ref())),
        }
    }
    fn into_owned_observed(
        self,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<Warning>, ImportError> {
        match self {
            Self::Discarded => Err(error(
                span,
                "Membership-only checks cannot publish a warning report",
            )),
            Self::Owned(rows) => Ok(rows),
            Self::Shared(rows) => {
                let mut result = Vec::new();
                for row in rows {
                    observation.work(1, &row.span.source)?;
                    observation.string(row.code.len(), &row.span.source)?;
                    observation.serialized(row.span.as_ref(), &row.span.source)?;
                    observation.reserve(128 + 32, &row.span.source)?;
                    result.push(Warning {
                        code: row.code.to_owned(),
                        message: row.message,
                        span: row.span.as_ref().clone(),
                    });
                }
                Ok(result)
            }
        }
    }
}
impl PartialEq for CheckWarnings {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && (0..self.len()).all(|index| self.at(index) == other.at(index))
    }
}
impl Eq for CheckWarnings {}
// Small attempts retain pointer slots in a vector; broad traversals keep the
// existing ordered-set behavior. Neither representation survives an attempt.
enum SharedApplicationVisits {
    Small(Vec<std::rc::Rc<Type>>),
    Inline {
        entries: [Option<std::rc::Rc<Type>>; 16],
        len: usize,
    },
    Large(BTreeSet<std::rc::Rc<Type>>),
}
impl Default for SharedApplicationVisits {
    fn default() -> Self {
        Self::Small(Vec::new())
    }
}
impl SharedApplicationVisits {
    fn inline() -> Self {
        Self::Inline {
            entries: std::array::from_fn(|_| None),
            len: 0,
        }
    }
    fn contains_observed(
        &self,
        ty: &Type,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        match self {
            Self::Small(rows) => {
                for input in rows {
                    observation.work(1, &span.source)?;
                    if input.as_ref() == ty {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Self::Inline { entries, len } => {
                for input in entries.iter().take(*len).flatten() {
                    observation.work(1, &span.source)?;
                    if input.as_ref() == ty {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Self::Large(rows) => {
                observation.work(1, &span.source)?;
                Ok(rows.contains(ty))
            }
        }
    }
    fn insert_observed(
        &mut self,
        input: std::rc::Rc<Type>,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        match self {
            Self::Small(rows) if rows.len() < 16 => {
                observation.reserve(32, &span.source)?;
                rows.try_reserve_exact(1)
                    .map_err(|_| error(span, "Application visit allocation failed"))?;
                rows.push(input);
            }
            Self::Small(rows) => {
                // Precharge the complete replacement before moving any input.
                observation.reserve(
                    observation.size(
                        rows.len().checked_add(1).and_then(|n| n.checked_mul(128)),
                        &span.source,
                    )?,
                    &span.source,
                )?;
                observation.work(rows.len(), &span.source)?;
                let mut promoted: BTreeSet<_> = std::mem::take(rows).into_iter().collect();
                promoted.insert(input);
                *self = Self::Large(promoted);
            }
            Self::Inline { entries, len } if *len < entries.len() => {
                // The slot is part of this local value; no vector allocation.
                // Observe the actual slot write before changing local progress.
                observation.work(1, &span.source)?;
                entries[*len] = Some(input);
                *len += 1;
            }
            Self::Inline { entries, len } => {
                observation.reserve(
                    observation.size(
                        len.checked_add(1).and_then(|n| n.checked_mul(128)),
                        &span.source,
                    )?,
                    &span.source,
                )?;
                observation.work(*len, &span.source)?;
                let mut promoted = BTreeSet::new();
                for entry in entries.iter_mut() {
                    if let Some(input) = entry.take() {
                        promoted.insert(input);
                    }
                }
                promoted.insert(input);
                *self = Self::Large(promoted);
            }
            Self::Large(rows) => {
                observation.reserve(128, &span.source)?;
                rows.insert(input);
            }
        }
        Ok(())
    }
    #[cfg(test)]
    fn get(&self, ty: &Type) -> Option<&std::rc::Rc<Type>> {
        match self {
            Self::Small(rows) => rows.iter().find(|input| input.as_ref() == ty),
            Self::Inline { entries, len } => entries
                .iter()
                .take(*len)
                .flatten()
                .find(|input| input.as_ref() == ty),
            Self::Large(rows) => rows.get(ty),
        }
    }
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        match self {
            Self::Small(rows) => rows.is_empty(),
            Self::Inline { len, .. } => *len == 0,
            Self::Large(rows) => rows.is_empty(),
        }
    }
}
// Fresh, allocation-free classifier checks scoped to one membership attempt.
// Source/path selection executes even on a hit. Borrowed node identity can only
// match within this immutable resolver; no entry survives into another attempt.
struct AttemptClassifiers<'source> {
    resolver: &'source Resolver,
    entries: [Option<(&'source Node, TargetKind)>; 4],
}
impl<'source> AttemptClassifiers<'source> {
    fn new(resolver: &'source Resolver) -> Self {
        Self {
            resolver,
            entries: std::array::from_fn(|_| None),
        }
    }
    fn kind(
        &mut self,
        id: &DeclarationId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<TargetKind, ImportError> {
        let node = self.resolver.node_observed(id, observation)?;
        let mut vacant = None;
        for (index, entry) in self.entries.iter().enumerate() {
            let Some((checked_node, kind)) = entry else {
                vacant = Some(index);
                break;
            };
            observation.work(1, &id.source)?;
            if std::ptr::eq(*checked_node, node) {
                return Ok(kind.clone());
            }
        }
        observation.work(1, &id.source)?;
        let kind = self
            .resolver
            .classifier_node_observed(id, node, observation)?;
        if let Some(index) = vacant {
            self.entries[index] = Some((node, kind.clone()));
        }
        Ok(kind)
    }
}
// One temporary source input at a time. Keep ordinary owned values on the
// stack; introducing an Rc/Box there would change their allocation receipts.
#[allow(clippy::large_enum_variant)]
enum OperationParameterInput {
    Owned(ResolvedParameter),
    Shared(std::rc::Rc<ResolvedParameter>),
}
impl std::ops::Deref for OperationParameterInput {
    type Target = ResolvedParameter;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(value) => value,
            Self::Shared(value) => value,
        }
    }
}
const _: () = assert!(std::mem::size_of::<OperationParameterInput>() <= 1024);
// Private to one operation application. Exact source inputs are retained only;
// substitutions, ancestry, bounds and aggregate work budgets remain fresh.
struct OperationParameterInputs<'source> {
    resolver: &'source Resolver,
    rows:
        std::collections::BTreeMap<DeclarationId, (DeclarationId, std::rc::Rc<ResolvedParameter>)>,
}
struct Checks<'source> {
    classifiers: Option<AttemptClassifiers<'source>>,
    parameter_inputs: Option<OperationParameterInputs<'source>>,
    seen: BTreeSet<Type>,
    // Fresh per-attempt visitation; shared entries contain complete inputs only.
    shared_seen: Option<SharedApplicationVisits>,
    bounds: Vec<BoundCheck>,
    warnings: CheckWarnings,
    remaining: usize,
}
fn unsupported(span: &SourceSpan, message: &str) -> ImportError {
    ImportError::new(ErrorKind::Unsupported, span, message)
}
fn budget(remaining: &mut usize, depth: usize, span: &SourceSpan) -> Result<(), ImportError> {
    if *remaining == 0 || depth > 128 {
        return Err(ImportError::new(
            ErrorKind::ResourceLimit,
            span,
            "Generic conformance budget exceeded",
        ));
    }
    *remaining -= 1;
    Ok(())
}
impl Resolver {
    /// Validate nominal applications and their ancestor applications.
    /// Raw argument omissions are warnings; unknown provider relations remain errors.
    pub fn application_bounds(
        &self,
        usage: &DeclarationId,
    ) -> Result<ApplicationBounds, ImportError> {
        let selected = self.type_expression(usage)?;
        if !matches!(selected.expression, Type::Classifier { .. }) {
            return Err(error(
                &selected.span,
                "Application checking requires a classifier application",
            ));
        }
        let checks = self.check_expression(&selected.expression, &selected.span)?;
        let valid = checks.bounds.iter().all(|check| check.satisfied);
        let warnings = checks.warnings.into_owned_observed(
            &selected.span,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )?;
        Ok(ApplicationBounds {
            selected,
            checks: checks.bounds,
            valid,
            warnings,
        })
    }
    pub(super) fn check_expression_bounds(
        &self,
        expression: &Type,
        span: &SourceSpan,
    ) -> Result<Vec<BoundCheck>, ImportError> {
        Ok(self.check_expression(expression, span)?.bounds)
    }
    pub(super) fn runtime_application_checks(
        &self,
        expression: &Type,
        span: &SourceSpan,
    ) -> Result<(Vec<BoundCheck>, Vec<Warning>), ImportError> {
        self.runtime_application_checks_observed(
            expression,
            span,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    pub(crate) fn runtime_application_checks_observed(
        &self,
        expression: &Type,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(Vec<BoundCheck>, Vec<Warning>), ImportError> {
        let checks = self.check_expression_observed(expression, span, observation)?;
        Ok((
            checks.bounds,
            checks.warnings.into_owned_observed(span, observation)?,
        ))
    }
    fn check_expression(
        &self,
        expression: &Type,
        span: &SourceSpan,
    ) -> Result<Checks<'_>, ImportError> {
        self.check_expression_observed(
            expression,
            span,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    fn check_expression_observed(
        &self,
        expression: &Type,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Checks<'_>, ImportError> {
        self.check_expression_with_warnings_observed(expression, span, observation, true)
    }
    fn check_expression_with_warnings_observed(
        &self,
        expression: &Type,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        retain_warnings: bool,
    ) -> Result<Checks<'_>, ImportError> {
        self.check_expression_attempt_observed(expression, span, observation, retain_warnings, None)
    }
    fn check_expression_attempt_observed<'source>(
        &'source self,
        expression: &Type,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        retain_warnings: bool,
        classifiers: Option<AttemptClassifiers<'source>>,
    ) -> Result<Checks<'source>, ImportError> {
        let inline_visits = classifiers.is_some();
        let mut checks = Checks {
            classifiers,
            parameter_inputs: None,
            seen: BTreeSet::new(),
            shared_seen: self
                .prepared_parameter_bound_inputs
                .borrow()
                .is_some()
                .then(|| {
                    if inline_visits {
                        SharedApplicationVisits::inline()
                    } else {
                        SharedApplicationVisits::default()
                    }
                }),
            bounds: vec![],
            warnings: CheckWarnings::Owned(Vec::new()),
            remaining: 16_384,
        };
        if !retain_warnings {
            checks.warnings = CheckWarnings::Discarded;
        } else if checks.shared_seen.is_some() {
            checks.warnings = CheckWarnings::Shared(Vec::new());
        }
        self.check_application_observed(expression, span, 0, &mut checks, observation)?;
        Ok(checks)
    }
    pub(super) fn check_argument_kind(
        &self,
        ty: &Type,
        span: &SourceSpan,
    ) -> Result<(), ImportError> {
        if let Type::Classifier { declaration, .. } = ty
            && self.primitive_classifier(declaration)?
        {
            return Err(error(
                span,
                "Primitive classifier is only allowed as a typed-element generic type",
            ));
        }
        Ok(())
    }
    fn check_application(
        &self,
        ty: &Type,
        span: &SourceSpan,
        depth: usize,
        checks: &mut Checks<'_>,
    ) -> Result<(), ImportError> {
        self.check_application_observed(
            ty,
            span,
            depth,
            checks,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    fn shared_warning_span(
        &self,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<std::rc::Rc<SourceSpan>, ImportError> {
        let table = self
            .prepared_parameter_bound_inputs
            .borrow()
            .as_ref()
            .cloned()
            .ok_or_else(|| error(span, "Missing warning source ownership context"))?;
        let mut table = table
            .try_borrow_mut()
            .map_err(|_| error(span, "Warning source metadata is already borrowed"))?;
        // Order by every source/span field; lookup borrows the complete query.
        let mut left = 0;
        let mut right = table.spans.len();
        while left < right {
            observation.work(1, &span.source)?;
            let middle = left + (right - left) / 2;
            let input = table
                .spans
                .get(middle)
                .ok_or_else(|| error(span, "Missing warning source input"))?;
            let order = input
                .source
                .cmp(&span.source)
                .then_with(|| input.start.cmp(&span.start))
                .then_with(|| input.end.cmp(&span.end));
            match order {
                std::cmp::Ordering::Less => left = middle + 1,
                std::cmp::Ordering::Greater => right = middle,
                std::cmp::Ordering::Equal => {
                    observation.reserve(32, &span.source)?;
                    return Ok(std::rc::Rc::clone(input));
                }
            }
        }
        let retained = table.spans.len() < 4096;
        observation.serialized(span, &span.source)?;
        observation.reserve(32, &span.source)?;
        if retained {
            observation.reserve(32, &span.source)?;
            table
                .spans
                .try_reserve_exact(1)
                .map_err(|_| error(span, "Warning source input allocation failed"))?;
        }
        let input = std::rc::Rc::new(span.clone());
        if retained {
            observation.work(table.spans.len() - left, &span.source)?;
            table.spans.insert(left, std::rc::Rc::clone(&input));
        }
        Ok(input)
    }
    fn shared_application_input(
        &self,
        ty: &Type,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<std::rc::Rc<Type>, ImportError> {
        if let Type::Classifier {
            declaration,
            arguments,
            raw,
        } = ty
            && arguments.is_empty()
            && let Some(input) =
                self.compiled_nominal_type_input(declaration, *raw, span, observation)?
        {
            return Ok(input);
        }
        let table = self
            .prepared_parameter_bound_inputs
            .borrow()
            .as_ref()
            .cloned()
            .ok_or_else(|| error(span, "Missing application input ownership context"))?;
        observation.work(1, &span.source)?;
        let mut table = table
            .try_borrow_mut()
            .map_err(|_| error(span, "Application input metadata is already borrowed"))?;
        if let Some(input) = table.expressions.get(ty) {
            observation.reserve(32, &span.source)?;
            return Ok(std::rc::Rc::clone(input));
        }
        // Retain exact whole expressions, without recording any check outcome.
        // Bounded metadata retention falls back to the original owned input.
        let retained = table.expressions.len() < 4096;
        observation.serialized(ty, &span.source)?;
        observation.reserve(32, &span.source)?;
        if retained {
            observation.reserve(128 + 32, &span.source)?;
        }
        let input = std::rc::Rc::new(ty.clone());
        if retained {
            table.expressions.insert(std::rc::Rc::clone(&input));
        }
        Ok(input)
    }
    fn visit_application_input(
        &self,
        ty: &Type,
        span: &SourceSpan,
        checks: &mut Checks<'_>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        if let Some(seen) = &mut checks.shared_seen {
            if seen.contains_observed(ty, span, observation)? {
                return Ok(true);
            }
            let input = self.shared_application_input(ty, span, observation)?;
            seen.insert_observed(input, span, observation)?;
        } else {
            if checks.seen.contains(ty) {
                return Ok(true);
            }
            observation.serialized(ty, &span.source)?;
            observation.reserve(128, &span.source)?;
            checks.seen.insert(ty.clone());
        }
        Ok(false)
    }
    fn operation_parameter_input(
        &self,
        id: &DeclarationId,
        checks: &mut Checks<'_>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<OperationParameterInput, ImportError> {
        let Some(inputs) = &mut checks.parameter_inputs else {
            return self
                .resolve_parameter(id)
                .map(OperationParameterInput::Owned);
        };
        if !std::ptr::eq(self, inputs.resolver) {
            return Err(error(
                &fallback(id),
                "Operation parameter inputs belong to another source resolver",
            ));
        }
        observation.work(1, &id.source)?;
        if let Some((owner, value)) = inputs.rows.get(id) {
            // Recheck exact source ownership; retained rows are source inputs,
            // never application-bound, classifier or provider outcomes.
            self.parameter_owner_input_observed(id, owner, observation)?;
            observation.reserve(32, &id.source)?;
            return Ok(OperationParameterInput::Shared(std::rc::Rc::clone(value)));
        }
        let owner = self.parameter_owner_observed(id, observation)?;
        let parameter = self.resolve_parameter_context(id, observation)?;
        observation.serialized(&parameter, &id.source)?;
        observation.reserve(128 + 32, &id.source)?;
        let value = std::rc::Rc::new(parameter);
        if inputs.rows.len() < 4096 {
            observation.serialized(id, &id.source)?;
            observation.serialized(&owner, &id.source)?;
            observation.reserve(128 + 32, &id.source)?;
            inputs
                .rows
                .insert(id.clone(), (owner, std::rc::Rc::clone(&value)));
        }
        Ok(OperationParameterInput::Shared(value))
    }
    fn check_application_observed(
        &self,
        ty: &Type,
        span: &SourceSpan,
        depth: usize,
        checks: &mut Checks<'_>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        observation.work(1, &span.source)?;
        budget(&mut checks.remaining, depth, span)?;
        if self.visit_application_input(ty, span, checks, observation)? {
            return Ok(());
        }
        let (declaration, arguments, raw) = match ty {
            Type::RigidParameter { identity } => {
                for bound in self.rigid_parameter_bounds(identity, span)? {
                    observation.work(1, &span.source)?;
                    self.check_application_observed(bound, span, depth + 1, checks, observation)?;
                }
                return Ok(());
            }
            Type::Classifier {
                declaration,
                arguments,
                raw,
            } => (declaration, arguments, raw),
            Type::Parameter { declaration } => {
                for bound in self.parameter_bound_types(declaration)? {
                    observation.work(1, &span.source)?;
                    self.check_application_observed(
                        &bound.expression,
                        &bound.span,
                        depth + 1,
                        checks,
                        observation,
                    )?;
                }
                return Ok(());
            }
            Type::Wildcard { upper, lower } => {
                for bound in [upper, lower].into_iter().flatten() {
                    self.check_argument_kind(bound, span)?;
                    observation.work(1, &span.source)?;
                    self.check_application_observed(bound, span, depth + 1, checks, observation)?;
                }
                return Ok(());
            }
        };
        let kind = match &mut checks.classifiers {
            Some(classifiers) => classifiers.kind(declaration, observation)?,
            None => self.classifier_observed(declaration, observation)?,
        };
        let formals = self.formal_info_observed(declaration, observation)?;
        if *raw {
            if formals.is_empty() || !arguments.is_empty() {
                return Err(error(span, "Invalid raw application"));
            }
            if !matches!(checks.warnings, CheckWarnings::Discarded) {
                let code = "raw_type";
                if matches!(checks.warnings, CheckWarnings::Owned(_)) {
                    observation.string(code.len(), &span.source)?;
                }
                let message = observation.formatted(
                    &format_args!(
                        "Raw application of {} {:?} omits {} type arguments",
                        declaration.source.uri,
                        declaration.path,
                        formals.len()
                    ),
                    &span.source,
                )?;
                match &mut checks.warnings {
                    CheckWarnings::Discarded => {}
                    CheckWarnings::Owned(rows) => {
                        observation.serialized(span, &span.source)?;
                        observation.reserve(128 + 32, &span.source)?;
                        rows.push(Warning {
                            code: code.to_owned(),
                            message,
                            span: span.clone(),
                        });
                    }
                    CheckWarnings::Shared(rows) => {
                        let span = self.shared_warning_span(span, observation)?;
                        observation.reserve(128 + 32, &span.source)?;
                        rows.push(SharedCheckWarning {
                            code,
                            message,
                            span,
                        });
                    }
                }
            }
        } else if formals.len() != arguments.len() {
            return Err(error(span, "Application argument count mismatch"));
        }
        let mut bindings = Vec::new();
        for (formal, argument) in formals.iter().zip(arguments) {
            observation.work(1, &span.source)?;
            observation.serialized(&formal.declaration, &span.source)?;
            observation.serialized(argument, &span.source)?;
            observation.reserve(128 + 32, &span.source)?;
            bindings.push(ParameterBinding {
                parameter: formal.declaration.clone(),
                argument: argument.clone(),
            });
        }
        let environment = observation.preparation_at(span, |context| {
            Substitution::new_observed(bindings, context)
        })?;
        for argument in arguments {
            observation.work(1, &span.source)?;
            self.check_argument_kind(argument, span)?;
            self.check_application_observed(argument, span, depth + 1, checks, observation)?;
        }
        for (formal, argument) in formals.iter().zip(arguments) {
            observation.work(1, &span.source)?;
            let parameter =
                self.operation_parameter_input(&formal.declaration, checks, observation)?;
            for declared in &parameter.bounds {
                observation.work(1, &span.source)?;
                let declared_bound = self.type_expression(&declared.syntax.declaration)?;
                let substituted_bound =
                    observation.preparation_at(&declared_bound.span, |context| {
                        environment.apply_simultaneous_observed(&declared_bound.expression, context)
                    })?;
                observation.serialized(ty, &span.source)?;
                observation.serialized(&formal.declaration, &span.source)?;
                observation.serialized(argument, &span.source)?;
                observation.reserve(128, &span.source)?;
                self.verify_bound_observed(
                    BoundCheck {
                        application: ty.clone(),
                        parameter: formal.declaration.clone(),
                        argument: argument.clone(),
                        declared_bound,
                        substituted_bound,
                        satisfied: false,
                    },
                    depth + 1,
                    checks,
                    observation,
                )?;
            }
        }
        if kind == TargetKind::Class {
            let ancestry = self.ancestry_view_observed(ty, span, observation)?;
            for ancestor in ancestry.as_slice() {
                observation.work(1, &span.source)?;
                self.check_application_observed(
                    &ancestor.expression,
                    span,
                    depth + 1,
                    checks,
                    observation,
                )?;
            }
        }
        Ok(())
    }
    fn verify_bound(
        &self,
        check: BoundCheck,
        depth: usize,
        checks: &mut Checks<'_>,
    ) -> Result<(), ImportError> {
        self.verify_bound_observed(
            check,
            depth,
            checks,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    fn verify_bound_observed(
        &self,
        mut check: BoundCheck,
        depth: usize,
        checks: &mut Checks<'_>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        observation.work(1, &check.declared_bound.span.source)?;
        self.check_application_observed(
            &check.substituted_bound,
            &check.declared_bound.span,
            depth,
            checks,
            observation,
        )?;
        // Ecore accepts an unconstrained wildcard as a substitution.
        check.satisfied = if matches!(
            &check.argument,
            Type::Wildcard {
                upper: None,
                lower: None
            }
        ) {
            true
        } else {
            observation.work(1, &check.declared_bound.span.source)?;
            self.nominal_bounded_observed(
                &check.argument,
                &check.substituted_bound,
                &check.declared_bound.span,
                depth,
                &mut checks.remaining,
                observation,
            )?
        };
        observation.reserve(32, &check.declared_bound.span.source)?;
        checks.bounds.push(check);
        Ok(())
    }
    pub(super) fn runtime_operation_bindings(
        &self,
        projection: &RuntimeMemberProjection,
        arguments: &[crate::definitions::generic_types::RuntimeType],
    ) -> Result<(Substitution, Vec<BoundCheck>, Vec<Warning>), ImportError> {
        let member = &projection.source;
        if member.formals.len() != arguments.len() {
            return Err(error(&member.span, "Operation argument count mismatch"));
        }
        let method_bindings: Vec<_> = member
            .formals
            .iter()
            .zip(arguments)
            .map(|(formal, argument)| ParameterBinding {
                parameter: formal.declaration.declaration.clone(),
                argument: argument.expression().clone(),
            })
            .collect();
        let environment =
            Substitution::new(method_bindings.clone()).map_err(|e| error(&member.span, e))?;
        let mut bindings = projection.bindings.clone();
        bindings.extend(method_bindings);
        let mut state = RuntimeDomainChecks {
            classifiers: None,
            observation: None,
            borrow_input_types: false,
            remaining: 16_384,
            environment: Substitution::new(bindings).map_err(|e| error(&member.span, e))?,
            span: std::borrow::Cow::Owned(member.span.clone()),
            active: std::collections::BTreeMap::new(),
            active_inputs: Vec::new(),
        };
        let mut actual_checks = Checks {
            classifiers: None,
            parameter_inputs: Some(OperationParameterInputs {
                resolver: self,
                rows: std::collections::BTreeMap::new(),
            }),
            seen: BTreeSet::new(),
            shared_seen: None,
            bounds: vec![],
            warnings: CheckWarnings::Owned(Vec::new()),
            remaining: state.remaining,
        };
        for argument in arguments {
            self.check_argument_kind(argument.expression(), &member.span)?;
            // Keep per-actual report contents, but charge all actuals to one budget.
            actual_checks.seen.clear();
            let first_check = actual_checks.bounds.len();
            self.check_application(argument.expression(), &member.span, 0, &mut actual_checks)?;
            if actual_checks.bounds[first_check..]
                .iter()
                .any(|check| !check.satisfied)
            {
                return Err(error(
                    &member.span,
                    "Runtime method argument violates generic bounds",
                ));
            }
        }
        state.remaining = actual_checks.remaining;
        let mut checks = actual_checks.bounds;
        let warnings = actual_checks.warnings.into_owned_observed(
            &member.span,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )?;
        for (formal, argument) in member.formals.iter().zip(arguments) {
            for (syntax, projected) in formal.declaration.bounds.iter().zip(&formal.bounds) {
                let declared_bound = self.type_expression(&syntax.declaration)?;
                let substituted_bound = environment
                    .apply_simultaneous(projected)
                    .map_err(|e| error(&declared_bound.span, e))?;
                let satisfied = self.runtime_domain_match(
                    argument.expression(),
                    &substituted_bound,
                    false,
                    &mut state,
                    (0, 0),
                    None,
                )?;
                if !satisfied {
                    return Err(error(
                        &declared_bound.span,
                        "Runtime operation application violates generic bounds",
                    ));
                }
                checks.push(BoundCheck {
                    application: member.owner.clone(),
                    parameter: formal.declaration.declaration.clone(),
                    argument: argument.expression().clone(),
                    declared_bound,
                    substituted_bound,
                    satisfied,
                });
            }
        }
        Ok((environment, checks, warnings))
    }

    pub(super) fn operation_bindings(
        &self,
        member: &SpecializedMember,
        arguments: &[crate::definitions::generic_types::ClosedType],
    ) -> Result<(Substitution, Vec<BoundCheck>), ImportError> {
        if member.formals.len() != arguments.len() {
            return Err(error(&member.span, "Operation argument count mismatch"));
        }
        let environment = Substitution::new(
            member
                .formals
                .iter()
                .zip(arguments)
                .map(|(formal, argument)| ParameterBinding {
                    parameter: formal.declaration.declaration.clone(),
                    argument: argument.expression().clone(),
                })
                .collect(),
        )
        .map_err(|e| error(&member.span, e))?;
        let mut checks = Checks {
            classifiers: None,
            parameter_inputs: None,
            seen: BTreeSet::new(),
            shared_seen: None,
            bounds: vec![],
            warnings: CheckWarnings::Owned(Vec::new()),
            remaining: 16_384,
        };
        for argument in arguments {
            self.check_argument_kind(argument.expression(), &member.span)?;
            self.check_application(argument.expression(), &member.span, 0, &mut checks)?;
        }
        for (formal, argument) in member.formals.iter().zip(arguments) {
            for (syntax, projected) in formal.declaration.bounds.iter().zip(&formal.bounds) {
                let declared_bound = self.type_expression(&syntax.declaration)?;
                let substituted_bound = environment
                    .apply_simultaneous(projected)
                    .map_err(|e| error(&declared_bound.span, e))?;
                self.verify_bound(
                    BoundCheck {
                        application: member.owner.clone(),
                        parameter: formal.declaration.declaration.clone(),
                        argument: argument.expression().clone(),
                        declared_bound,
                        substituted_bound,
                        satisfied: false,
                    },
                    0,
                    &mut checks,
                )?;
            }
        }
        Ok((environment, checks.bounds))
    }
    pub(super) fn parameter_bound_types(
        &self,
        declaration: &DeclarationId,
    ) -> Result<Vec<TypeUse>, ImportError> {
        self.parameter_bound_types_context(
            declaration,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    fn parameter_bound_types_context(
        &self,
        declaration: &DeclarationId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<TypeUse>, ImportError> {
        let parameter = self.resolve_parameter_context(declaration, observation)?;
        let mut bounds = Vec::new();
        for bound in parameter.bounds {
            observation.work(1, &declaration.source)?;
            observation.reserve(32, &declaration.source)?;
            bounds.push(self.type_expression_context(&bound.syntax.declaration, observation)?);
        }
        Ok(bounds)
    }
    pub(crate) fn enable_parameter_bound_inputs(
        &self,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        if self.prepared_parameter_bound_inputs.borrow().is_none() {
            observation.reserve(128 + 32 + 32 + 32 + 128 + 32, &self.standard_id)?;
            *self.prepared_parameter_bound_inputs.borrow_mut() = Some(std::rc::Rc::new(
                std::cell::RefCell::new(Default::default()),
            ));
        }
        Ok(())
    }
    fn parameter_bound_inputs_observed(
        &self,
        declaration: &DeclarationId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<ParameterBounds, ImportError> {
        let Some(table) = self
            .prepared_parameter_bound_inputs
            .borrow()
            .as_ref()
            .cloned()
        else {
            return self
                .parameter_bound_types_context(declaration, observation)
                .map(ParameterBounds::Owned);
        };
        // Source ownership is checked on every read; stored values are input data,
        // never a bounds/conformance outcome or a provider invocation result.
        if let Some(inputs) = table.borrow().get(declaration).cloned() {
            self.parameter_owner_input_observed(declaration, &inputs.owner, observation)?;
            for bound in &inputs.parameter.bounds {
                observation.work(1, &declaration.source)?;
                self.node_observed(&bound.syntax.declaration, observation)?;
            }
            observation.reserve(32, &declaration.source)?;
            return Ok(ParameterBounds::Shared(inputs));
        }
        let owner = self.parameter_owner_observed(declaration, observation)?;
        let parameter = self.resolve_parameter_context(declaration, observation)?;
        let mut expressions = Vec::new();
        for bound in &parameter.bounds {
            observation.work(1, &declaration.source)?;
            observation.reserve(32, &declaration.source)?;
            expressions.push(self.type_expression_context(&bound.syntax.declaration, observation)?);
        }
        observation.serialized(declaration, &declaration.source)?;
        observation.reserve(128 + 32 + 32, &declaration.source)?;
        let inputs = std::rc::Rc::new(ParameterBoundInputs {
            parameter,
            expressions,
            owner,
        });
        table
            .borrow_mut()
            .insert(declaration.clone(), inputs.clone());
        Ok(ParameterBounds::Shared(inputs))
    }
    pub(crate) fn nominal_bounded_observed(
        &self,
        actual: &Type,
        bound: &Type,
        span: &SourceSpan,
        depth: usize,
        remaining: &mut usize,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        observation.work(1, &span.source)?;
        budget(remaining, depth, span)?;
        if actual == bound {
            return Ok(true);
        }
        match (actual, bound) {
            (Type::RigidParameter { identity }, _) => {
                for declared in self.rigid_parameter_bounds(identity, span)? {
                    observation.work(1, &span.source)?;
                    if self.nominal_bounded_observed(
                        declared,
                        bound,
                        span,
                        depth + 1,
                        remaining,
                        observation,
                    )? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            (_, Type::RigidParameter { .. }) => Ok(false),
            (
                Type::Parameter {
                    declaration: actual_parameter,
                },
                Type::Parameter {
                    declaration: bound_parameter,
                },
            ) => {
                // Distinct parameters retain nominal identities. Ecore requires
                // each actual bound to be bounded by the target parameter or
                // one of its declared bounds; an unconstrained actual fails.
                let actual_bounds = self.parameter_bound_types(actual_parameter)?;
                let bound_bounds = self.parameter_bound_types(bound_parameter)?;
                if actual_bounds.is_empty() {
                    return Ok(false);
                }
                for actual_bound in actual_bounds {
                    observation.work(1, &span.source)?;
                    if self.nominal_bounded_observed(
                        &actual_bound.expression,
                        bound,
                        span,
                        depth + 1,
                        remaining,
                        observation,
                    )? {
                        continue;
                    }
                    let mut satisfied = false;
                    for bound_bound in &bound_bounds {
                        observation.work(1, &span.source)?;
                        if self.nominal_bounded_observed(
                            &actual_bound.expression,
                            &bound_bound.expression,
                            span,
                            depth + 1,
                            remaining,
                            observation,
                        )? {
                            satisfied = true;
                            break;
                        }
                    }
                    if !satisfied {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (Type::Parameter { declaration }, Type::Classifier { .. }) => {
                for declared in self.parameter_bound_types(declaration)? {
                    observation.work(1, &span.source)?;
                    if self.nominal_bounded_observed(
                        &declared.expression,
                        bound,
                        span,
                        depth + 1,
                        remaining,
                        observation,
                    )? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            (
                Type::Wildcard {
                    upper: Some(upper), ..
                },
                Type::Parameter { .. },
            ) => {
                self.nominal_bounded_observed(upper, bound, span, depth + 1, remaining, observation)
            }
            (_, Type::Parameter { .. }) => Ok(false),
            (
                Type::Classifier {
                    declaration: a,
                    arguments: aa,
                    ..
                },
                Type::Classifier {
                    declaration: b,
                    arguments: ba,
                    ..
                },
            ) => {
                if a == b {
                    if aa.len() != ba.len() {
                        return Ok(false);
                    }
                    for (actual, bound) in aa.iter().zip(ba) {
                        observation.work(1, &span.source)?;
                        if !self.nominal_matching_observed(
                            actual,
                            bound,
                            span,
                            depth + 1,
                            remaining,
                            observation,
                        )? {
                            return Ok(false);
                        }
                    }
                    return Ok(true);
                }
                let ak = self.classifier_observed(a, observation)?;
                let bk = self.classifier_observed(b, observation)?;
                if ak == TargetKind::Class && bk == TargetKind::Class {
                    observation.work(1, &span.source)?;
                    if ba.is_empty()
                        && b == crate::definitions::schema::eobject_reference_top_declaration_ref()
                            .map_err(|message| error(span, message))?
                    {
                        return Ok(true);
                    }
                    let ancestry = self.ancestry_view_observed(actual, span, observation)?;
                    for ancestor in ancestry.as_slice() {
                        observation.work(1, &span.source)?;
                        if let Type::Classifier { declaration, .. } = &ancestor.expression
                            && declaration == b
                            && self.nominal_bounded_observed(
                                &ancestor.expression,
                                bound,
                                span,
                                depth + 1,
                                remaining,
                                observation,
                            )?
                        {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                } else if ak == TargetKind::Enumeration && bk == TargetKind::Enumeration {
                    Ok(false)
                } else if let Some(satisfied) =
                    self.classifier_domain_relation_observed(a, b, false, observation)?
                {
                    if !satisfied || aa.len() != ba.len() {
                        return Ok(false);
                    }
                    for (actual, bound) in aa.iter().zip(ba) {
                        observation.work(1, &span.source)?;
                        if !self.nominal_matching_observed(
                            actual,
                            bound,
                            span,
                            depth + 1,
                            remaining,
                            observation,
                        )? {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                } else {
                    Err(unsupported(
                        span,
                        "Distinct datatype/provider bounds require an explicit assignability contract",
                    ))
                }
            }
            (Type::Wildcard { upper, lower }, Type::Classifier { .. }) => {
                match upper.as_ref().or(lower.as_ref()) {
                    Some(actual) => self.nominal_bounded_observed(
                        actual,
                        bound,
                        span,
                        depth + 1,
                        remaining,
                        observation,
                    ),
                    None => Ok(false),
                }
            }
            (Type::Wildcard { .. }, Type::Wildcard { .. }) => Ok(false),
            (_, Type::Wildcard { upper, lower }) => {
                if let Some(upper) = upper {
                    self.nominal_bounded_observed(
                        actual,
                        upper,
                        span,
                        depth + 1,
                        remaining,
                        observation,
                    )
                } else if let Some(lower) = lower {
                    self.nominal_bounded_observed(
                        lower,
                        actual,
                        span,
                        depth + 1,
                        remaining,
                        observation,
                    )
                } else {
                    Ok(false)
                }
            }
        }
    }
    pub(super) fn invariant_equal(
        &self,
        actual: &Type,
        expected: &Type,
        span: &SourceSpan,
        depth: usize,
        remaining: &mut usize,
    ) -> Result<bool, ImportError> {
        self.invariant_equal_observed(
            actual,
            expected,
            span,
            depth,
            remaining,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    pub(crate) fn invariant_equal_observed(
        &self,
        actual: &Type,
        expected: &Type,
        span: &SourceSpan,
        depth: usize,
        remaining: &mut usize,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        observation.work(1, &span.source)?;
        budget(remaining, depth, span)?;
        if actual == expected {
            return Ok(true);
        }
        match (actual, expected) {
            (
                Type::Classifier {
                    declaration: a,
                    arguments: aa,
                    ..
                },
                Type::Classifier {
                    declaration: b,
                    arguments: ba,
                    ..
                },
            ) => {
                if a != b {
                    match self.classifier_domain_relation_observed(a, b, true, observation)? {
                        Some(true) => {}
                        Some(false) => return Ok(false),
                        None if self.classifier_observed(a, observation)?
                            == TargetKind::Datatype
                            && self.classifier_observed(b, observation)?
                                == TargetKind::Datatype =>
                        {
                            return Err(unsupported(
                                span,
                                "Datatype argument equivalence requires an explicit provider contract",
                            ));
                        }
                        None => return Ok(false),
                    }
                }
                if aa.len() != ba.len() {
                    return Ok(false);
                }
                for (a, b) in aa.iter().zip(ba) {
                    observation.work(1, &span.source)?;
                    if !self.invariant_equal_observed(
                        a,
                        b,
                        span,
                        depth + 1,
                        remaining,
                        observation,
                    )? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (
                Type::Wildcard {
                    upper: au,
                    lower: al,
                },
                Type::Wildcard {
                    upper: bu,
                    lower: bl,
                },
            ) => {
                for (a, b) in [(au, bu), (al, bl)] {
                    observation.work(1, &span.source)?;
                    match (a, b) {
                        (None, None) => {}
                        (Some(a), Some(b))
                            if self.invariant_equal_observed(
                                a,
                                b,
                                span,
                                depth + 1,
                                remaining,
                                observation,
                            )? => {}
                        _ => return Ok(false),
                    }
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }
    fn nominal_matching_observed(
        &self,
        actual: &Type,
        bound: &Type,
        span: &SourceSpan,
        depth: usize,
        remaining: &mut usize,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        observation.work(1, &span.source)?;
        budget(remaining, depth, span)?;
        if actual == bound {
            return Ok(true);
        }
        match bound {
            // Matching concrete nested arguments is invariant, including nested wildcards.
            Type::Classifier { .. } => self.invariant_equal_observed(
                actual,
                bound,
                span,
                depth + 1,
                remaining,
                observation,
            ),
            // Substitution was already applied simultaneously. Distinct
            // remaining parameters do not match merely because bounds agree.
            Type::Parameter { .. } | Type::RigidParameter { .. } => Ok(false),
            Type::Wildcard { upper, lower } => {
                if matches!(actual, Type::Wildcard { .. }) {
                    return Ok(false);
                }
                if let Some(upper) = upper {
                    self.nominal_bounded_observed(
                        actual,
                        upper,
                        span,
                        depth + 1,
                        remaining,
                        observation,
                    )
                }
                // Ecore isMatching uses reversed matching here, not reversed boundedness.
                else if let Some(lower) = lower {
                    self.nominal_matching_observed(
                        lower,
                        actual,
                        span,
                        depth + 1,
                        remaining,
                        observation,
                    )
                } else {
                    Ok(true)
                }
            }
        }
    }
}

#[derive(Debug)]
pub(super) struct ParameterBoundInputs {
    parameter: ResolvedParameter,
    expressions: Vec<TypeUse>,
    owner: DeclarationId,
}
#[derive(Default, Debug)]
pub(super) struct ParameterBoundInputTable {
    parameters: std::collections::BTreeMap<DeclarationId, std::rc::Rc<ParameterBoundInputs>>,
    expressions: BTreeSet<std::rc::Rc<Type>>,
    spans: Vec<std::rc::Rc<SourceSpan>>,
    pub(super) runtime_types: std::rc::Rc<
        std::cell::RefCell<crate::definitions::generic_types::RuntimeExpressionInputIndex>,
    >,
}
impl std::ops::Deref for ParameterBoundInputTable {
    type Target = std::collections::BTreeMap<DeclarationId, std::rc::Rc<ParameterBoundInputs>>;
    fn deref(&self) -> &Self::Target {
        &self.parameters
    }
}
impl std::ops::DerefMut for ParameterBoundInputTable {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.parameters
    }
}
enum ParameterBounds {
    Owned(Vec<TypeUse>),
    Shared(std::rc::Rc<ParameterBoundInputs>),
}
impl ParameterBounds {
    fn as_slice(&self) -> &[TypeUse] {
        match self {
            Self::Owned(values) => values,
            Self::Shared(inputs) => &inputs.expressions,
        }
    }
}
enum RuntimeCandidateInputs<'input> {
    Owned(Vec<Type>),
    View {
        root: &'input Type,
        ancestry: super::inheritance::AncestryView,
    },
}
impl RuntimeCandidateInputs<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Owned(values) => values.len(),
            Self::View { ancestry, .. } => 1 + ancestry.as_slice().len(),
        }
    }
    fn get(&self, index: usize) -> Option<&Type> {
        match self {
            Self::Owned(values) => values.get(index),
            Self::View { root, .. } if index == 0 => Some(root),
            Self::View { ancestry, .. } => ancestry
                .as_slice()
                .get(index - 1)
                .map(|row| &row.expression),
        }
    }
}
// These inputs stay alive in the caller's substitution/ancestry views. The
// recursive borrow cannot escape the current matching attempt.
struct RuntimeBorrowedGuard<'input> {
    actual: &'input Type,
    expected: &'input Type,
    invariant: bool,
    guards: usize,
    parent: Option<&'input RuntimeBorrowedGuard<'input>>,
}
fn runtime_span_input<'input>(
    span: &'input SourceSpan,
    borrow: bool,
    observation: Option<&crate::definitions::ecore::ParserObservation<'_, '_>>,
) -> Result<std::borrow::Cow<'input, SourceSpan>, ImportError> {
    if let Some(observation) = observation {
        observation.work(1, &span.source)?;
        if !borrow {
            observation.serialized(span, &span.source)?;
            observation.reserve(128, &span.source)?;
        }
    }
    Ok(if borrow {
        std::borrow::Cow::Borrowed(span)
    } else {
        std::borrow::Cow::Owned(span.clone())
    })
}
struct RuntimeGuardExit {
    legacy: Option<(Type, Type, bool)>,
    input_depth: usize,
}
struct RuntimeDomainChecks<'a, 'b, 'c, 'source> {
    classifiers: Option<AttemptClassifiers<'source>>,
    borrow_input_types: bool,
    observation: Option<&'a crate::definitions::ecore::ParserObservation<'b, 'c>>,
    remaining: usize,
    environment: Substitution,
    span: std::borrow::Cow<'a, SourceSpan>,
    active: std::collections::BTreeMap<(Type, Type, bool), usize>,
    active_inputs: Vec<((Type, Type, bool), usize)>,
}
// Keep observation temporaries outside the recursive predicate's frame.
impl RuntimeDomainChecks<'_, '_, '_, '_> {
    fn apply<'input>(
        &self,
        expression: &'input Type,
    ) -> Result<std::borrow::Cow<'input, Type>, ImportError> {
        match self.observation {
            Some(observation) if self.borrow_input_types => observation
                .preparation_at(&self.span, |operation| {
                    self.environment.apply_view_observed(expression, operation)
                }),
            Some(observation) => observation
                .preparation_at(&self.span, |operation| {
                    self.environment.apply_observed(expression, operation)
                })
                .map(std::borrow::Cow::Owned),
            None => self
                .environment
                .apply(expression)
                .map(std::borrow::Cow::Owned)
                .map_err(|e| error(&self.span, e)),
        }
    }
    fn pair(
        &self,
        actual: &Type,
        expected: &Type,
        invariant: bool,
    ) -> Result<(Type, Type, bool), ImportError> {
        if let Some(observation) = self.observation {
            observation.serialized(actual, &self.span.source)?;
            observation.serialized(expected, &self.span.source)?;
            observation.reserve(256, &self.span.source)?;
        }
        Ok((actual.clone(), expected.clone(), invariant))
    }
    fn previous_borrowed_guard(
        &self,
        actual: &Type,
        expected: &Type,
        invariant: bool,
        mut frame: Option<&RuntimeBorrowedGuard<'_>>,
    ) -> Result<Option<usize>, ImportError> {
        while let Some(input) = frame {
            if let Some(observation) = self.observation {
                observation.work(1, &self.span.source)?;
                observation.preparation_at(&self.span, |operation| {
                    crate::definitions::generic_types::shape_observed(input.actual, operation)?;
                    crate::definitions::generic_types::shape_observed(input.expected, operation)
                })?;
            }
            if input.invariant == invariant && input.actual == actual && input.expected == expected
            {
                return Ok(Some(input.guards));
            }
            frame = input.parent;
        }
        Ok(None)
    }
    fn previous_pair(&self, pair: &(Type, Type, bool)) -> Result<Option<usize>, ImportError> {
        if self.borrow_input_types {
            for (input, guards) in &self.active_inputs {
                if let Some(observation) = self.observation {
                    observation.work(1, &self.span.source)?;
                }
                if input == pair {
                    return Ok(Some(*guards));
                }
            }
            Ok(None)
        } else {
            Ok(self.active.get(pair).copied())
        }
    }
    fn insert_pair(
        &mut self,
        pair: (Type, Type, bool),
        guards: usize,
    ) -> Result<RuntimeGuardExit, ImportError> {
        if self.borrow_input_types {
            if self.active_inputs.len() == self.active_inputs.capacity() {
                if let Some(observation) = self.observation {
                    observation.reserve(32, &self.span.source)?;
                }
                self.active_inputs
                    .try_reserve_exact(1)
                    .map_err(|_| error(&self.span, "Runtime guard input allocation failed"))?;
            }
            let depth = self.active_inputs.len();
            self.active_inputs.push((pair, guards));
            return Ok(RuntimeGuardExit {
                legacy: None,
                input_depth: depth,
            });
        }
        if let Some(observation) = self.observation {
            observation.serialized(&pair, &self.span.source)?;
            observation.reserve(256 + 128, &self.span.source)?;
        }
        self.active.insert(pair.clone(), guards);
        Ok(RuntimeGuardExit {
            legacy: Some(pair),
            input_depth: 0,
        })
    }
    fn remove_pair(&mut self, exit: RuntimeGuardExit) {
        if let Some(pair) = exit.legacy {
            self.active.remove(&pair);
        } else {
            self.active_inputs.truncate(exit.input_depth);
        }
    }
    fn kind(
        &mut self,
        resolver: &Resolver,
        declaration: &DeclarationId,
    ) -> Result<TargetKind, ImportError> {
        match (self.observation, &mut self.classifiers) {
            (Some(observation), Some(classifiers)) => classifiers.kind(declaration, observation),
            (Some(observation), None) => resolver.classifier_observed(declaration, observation),
            (None, _) => resolver.classifier(declaration),
        }
    }
    fn invariant(
        &mut self,
        resolver: &Resolver,
        actual: &Type,
        expected: &Type,
        depth: usize,
    ) -> Result<bool, ImportError> {
        match self.observation {
            Some(observation) => resolver.invariant_equal_observed(
                actual,
                expected,
                &self.span,
                depth,
                &mut self.remaining,
                observation,
            ),
            None => {
                resolver.invariant_equal(actual, expected, &self.span, depth, &mut self.remaining)
            }
        }
    }
    fn candidates<'input>(
        &self,
        resolver: &Resolver,
        actual: &'input Type,
        inherit: bool,
    ) -> Result<RuntimeCandidateInputs<'input>, ImportError> {
        if self.borrow_input_types
            && let Some(observation) = self.observation
        {
            let ancestry = if inherit {
                resolver.ancestry_view_observed(actual, &self.span, observation)?
            } else {
                super::inheritance::AncestryView::Owned(Vec::new())
            };
            // Shared ancestry stores complete immutable paths/expressions.
            // Recheck each source classifier before using its input here.
            for row in ancestry.as_slice() {
                let Type::Classifier { declaration, .. } = &row.expression else {
                    return Err(error(&self.span, "Expected classifier ancestry input"));
                };
                resolver.classifier_observed(declaration, observation)?;
            }
            return Ok(RuntimeCandidateInputs::View {
                root: actual,
                ancestry,
            });
        }
        if let Some(observation) = self.observation {
            observation.serialized(actual, &self.span.source)?;
            observation.reserve(128 + 32, &self.span.source)?;
        }
        let mut result = vec![actual.clone()];
        if inherit {
            let ancestry = match self.observation {
                Some(observation) => {
                    resolver.ancestry_expression_observed(actual, &self.span, observation)?
                }
                None => resolver.ancestry_expression(actual, &self.span)?,
            };
            if let Some(observation) = self.observation {
                observation.reserve(
                    observation.size(ancestry.len().checked_mul(32), &self.span.source)?,
                    &self.span.source,
                )?;
            }
            result.extend(ancestry.into_iter().map(|a| a.expression));
        }
        Ok(RuntimeCandidateInputs::Owned(result))
    }
    fn bounds(
        &self,
        resolver: &Resolver,
        declaration: &DeclarationId,
    ) -> Result<ParameterBounds, ImportError> {
        match self.observation {
            Some(observation) => resolver.parameter_bound_inputs_observed(declaration, observation),
            None => resolver
                .parameter_bound_types(declaration)
                .map(ParameterBounds::Owned),
        }
    }
    fn relation(
        &self,
        resolver: &Resolver,
        actual: &DeclarationId,
        expected: &DeclarationId,
        invariant: bool,
    ) -> Result<bool, ImportError> {
        match self.observation {
            Some(observation) => resolver.classifier_domain_relation_observed(
                actual,
                expected,
                invariant,
                observation,
            )?,
            None => resolver.classifier_domain_relation(actual, expected, invariant)?,
        }
        .ok_or_else(|| {
            unsupported(
                &self.span,
                "Runtime attribute domain requires an explicit provider relation",
            )
        })
    }
}
fn has_open_parameter(ty: &Type) -> bool {
    match ty {
        Type::Parameter { .. } | Type::RigidParameter { .. } => true,
        Type::Classifier { arguments, .. } => arguments.iter().any(has_open_parameter),
        Type::Wildcard { upper, lower } => [upper, lower]
            .into_iter()
            .flatten()
            .any(|ty| has_open_parameter(ty)),
    }
}
// Only the resolver can construct this proof. Serialized RuntimeMembers and
// caller-supplied domains cannot enter the retained checked path. The immutable
// resolver borrow also fixes source identities and conversion policy for its life.
pub(crate) struct PreparedRuntimeMembers<'a> {
    resolver: &'a Resolver,
    members: super::members::MaterializationMembers<'a>,
}
impl<'a> std::ops::Deref for PreparedRuntimeMembers<'a> {
    type Target = super::members::MaterializationMembers<'a>;
    fn deref(&self) -> &Self::Target {
        &self.members
    }
}
impl PreparedRuntimeMembers<'_> {
    // Ownership transfer after all candidate and class consumers release this
    // checked report. No source projection is reconstructed or omitted.
    pub(crate) fn into_feature_domains_observed(
        self,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<RuntimeFeatureDomain>, ImportError> {
        self.members
            .feature_domains
            .into_owned_observed(observation)
    }
    pub(crate) fn into_shared_feature_domains_observed(
        self,
        inputs: &mut crate::definitions::schema::RuntimeDomainInputs,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<crate::definitions::schema::SharedRuntimeDomain>, ImportError> {
        self.members
            .feature_domains
            .into_shared_observed(inputs, observation)
    }
    pub(crate) fn checked_feature_domain(
        &self,
        index: usize,
    ) -> Option<CheckedRuntimeFeatureDomain<'_>> {
        self.members
            .feature_domains
            .get(index)
            .map(|domain| CheckedRuntimeFeatureDomain {
                resolver: self.resolver,
                domain,
            })
    }
    #[cfg(test)]
    pub(crate) fn checked_feature_domains(
        &self,
    ) -> impl Iterator<Item = CheckedRuntimeFeatureDomain<'_>> {
        self.members
            .feature_domains
            .iter()
            .map(|domain| CheckedRuntimeFeatureDomain {
                resolver: self.resolver,
                domain,
            })
    }
}
// A checked domain borrows the exact immutable resolver and source domain. It
// cannot be forged or reused with another resolver by materialization callers.
pub(crate) struct CheckedRuntimeFeatureDomain<'a> {
    resolver: &'a Resolver,
    domain: super::members::RuntimeDomainRef<'a>,
}
impl CheckedRuntimeFeatureDomain<'_> {
    pub(crate) fn source(&self) -> super::members::RuntimeDomainRef<'_> {
        self.domain
    }
    pub(crate) fn accepts(
        &self,
        actual: &crate::definitions::generic_types::RuntimeType,
    ) -> Result<bool, ImportError> {
        self.resolver
            .runtime_feature_accepts_checked(self.domain, actual, None)
    }
    pub(crate) fn accepts_observed(
        &self,
        actual: &crate::definitions::generic_types::RuntimeType,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        self.resolver
            .runtime_feature_accepts_checked(self.domain, actual, Some(observation))
    }
}
impl Resolver {
    /// Check native retained type membership without inventing bindings for raw formals.
    /// This preserves argument information unavailable to Java erased datatype instances.
    pub fn runtime_feature_accepts(
        &self,
        domain: &RuntimeFeatureDomain,
        actual: &crate::definitions::generic_types::RuntimeType,
    ) -> Result<bool, ImportError> {
        self.check_runtime_feature_domain(domain)?.accepts(actual)
    }

    /// Fresh source-bound membership with caller-supplied cancellation/work/storage.
    /// This route shares the existing matcher; no outcome survives this call.
    pub fn runtime_feature_accepts_observed(
        &self,
        domain: &RuntimeFeatureDomain,
        actual: &crate::definitions::generic_types::RuntimeType,
        operation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<bool, crate::definitions::ecore::ObservedImportError> {
        operation
            .checkpoint()
            .map_err(crate::definitions::ecore::ObservedImportError::Observation)?;
        let observation = crate::definitions::ecore::ParserObservation::new(operation);
        let result = (|| {
            let current = self
                .prepare_runtime_members_observed(&domain.projection.receiver, &observation)?
                .into_feature_domains_observed(&observation)?;
            let mut found = false;
            for candidate in &current {
                observation.work(1, &domain.projection.source.span.source)?;
                if candidate == domain {
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(error(
                    &domain.projection.source.span,
                    "Runtime feature domain differs from checked source",
                ));
            }
            CheckedRuntimeFeatureDomain {
                resolver: self,
                domain: domain.into(),
            }
            .accepts_observed(actual, &observation)
        })();
        observation.finish(result)
    }

    pub(crate) fn prepare_runtime_members_observed<'a>(
        &'a self,
        receiver: &crate::definitions::generic_types::RuntimeType,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<PreparedRuntimeMembers<'a>, ImportError> {
        Ok(PreparedRuntimeMembers {
            resolver: self,
            members: self.runtime_members_for_materialization_observed(receiver, observation)?,
        })
    }

    pub(crate) fn check_runtime_feature_domain<'a>(
        &'a self,
        domain: &'a RuntimeFeatureDomain,
    ) -> Result<CheckedRuntimeFeatureDomain<'a>, ImportError> {
        let current = self.runtime_feature_domains(&domain.projection.receiver)?;
        if !current.contains(domain) {
            return Err(error(
                &domain.projection.source.span,
                "Runtime feature domain differs from checked source",
            ));
        }
        Ok(CheckedRuntimeFeatureDomain {
            resolver: self,
            domain: domain.into(),
        })
    }

    fn runtime_feature_accepts_checked(
        &self,
        domain: super::members::RuntimeDomainRef<'_>,
        actual: &crate::definitions::generic_types::RuntimeType,
        observation: Option<&crate::definitions::ecore::ParserObservation<'_, '_>>,
    ) -> Result<bool, ImportError> {
        let borrow_inputs =
            observation.is_some() && self.prepared_parameter_bound_inputs.borrow().is_some();
        let span = runtime_span_input(domain.projection.source.span, borrow_inputs, observation)?;
        let Type::Classifier { declaration, .. } = actual.expression() else {
            return Err(error(
                &span,
                "Runtime domain candidate requires a classifier",
            ));
        };
        let mut classifiers = (borrow_inputs && self.coalesce_runtime_classifiers.get())
            .then(|| AttemptClassifiers::new(self));
        let kind = match (observation, &mut classifiers) {
            (Some(observation), Some(classifiers)) => classifiers.kind(declaration, observation)?,
            (Some(observation), None) => self.classifier_observed(declaration, observation)?,
            (None, _) => self.classifier(declaration)?,
        };
        if (kind == TargetKind::Class) != domain.reference {
            return Ok(false);
        }
        let mut checks = match observation {
            Some(observation) => self.check_expression_attempt_observed(
                actual.expression(),
                &span,
                observation,
                !borrow_inputs,
                classifiers,
            )?,
            None => self.check_expression(actual.expression(), &span)?,
        };
        let invalid = checks.bounds.iter().any(|check| !check.satisfied);
        let classifiers = checks.classifiers.take();
        drop(checks);
        if invalid {
            return Ok(false);
        }
        if let Some(observation) = observation {
            for binding in domain.projection.bindings {
                observation.serialized(binding, &span.source)?;
                observation.reserve(128 + 32, &span.source)?;
            }
        }
        let bindings = domain.projection.bindings.to_vec();
        let environment = match observation {
            Some(observation) => observation.preparation_at(&span, |operation| {
                Substitution::new_observed(bindings, operation)
            })?,
            None => Substitution::new(bindings).map_err(|e| error(&span, e))?,
        };
        let mut state = RuntimeDomainChecks {
            classifiers,
            observation,
            borrow_input_types: borrow_inputs,
            remaining: 16_384,
            environment,
            span,
            active: std::collections::BTreeMap::new(),
            active_inputs: Vec::new(),
        };
        self.runtime_domain_match(
            actual.expression(),
            domain.expression,
            false,
            &mut state,
            (0, 0),
            None,
        )
    }

    fn runtime_domain_match(
        &self,
        actual: &Type,
        expected: &Type,
        invariant: bool,
        state: &mut RuntimeDomainChecks<'_, '_, '_, '_>,
        progress: (usize, usize),
        prior: Option<&RuntimeBorrowedGuard<'_>>,
    ) -> Result<bool, ImportError> {
        let (depth, guards) = progress;
        let observation = state.observation;
        if let Some(observation) = observation {
            observation.work(1, &state.span.source)?;
        }
        budget(&mut state.remaining, depth, &state.span)?;
        // Lower wildcards reverse compatibility, so the actual side can carry a
        // source parameter. Resolve only established receiver/method bindings.
        let actual = state.apply(actual)?;
        let actual = actual.as_ref();
        let expected_view = state.apply(expected)?;
        let expected = expected_view.as_ref();
        let pair = if state.borrow_input_types {
            None
        } else {
            Some(state.pair(actual, expected, invariant)?)
        };
        let previous = match &pair {
            Some(pair) => state.previous_pair(pair)?,
            None => state.previous_borrowed_guard(actual, expected, invariant, prior)?,
        };
        if let Some(previous) = previous {
            // A recursive constraint must make a classifier-membership step before
            // revisiting the same obligation. Parameter-only cycles are not proof.
            if guards > previous {
                return Ok(true);
            }
            return Err(error(
                &state.span,
                "Unguarded recursive runtime feature constraint",
            ));
        }
        let guard_exit = match pair {
            Some(pair) => Some(state.insert_pair(pair, guards)?),
            None => None,
        };
        let borrowed_guard = RuntimeBorrowedGuard {
            actual,
            expected,
            invariant,
            guards,
            parent: prior,
        };
        let next = if state.borrow_input_types {
            Some(&borrowed_guard)
        } else {
            None
        };
        let result = (|| match expected {
            Type::RigidParameter { .. } => Err(error(
                &state.span,
                "Rigid symbolic parameters cannot be runtime feature domains",
            )),
            Type::Parameter { declaration } => {
                let bounds = state.bounds(self, declaration)?;
                for bound in bounds.as_slice() {
                    if let Some(observation) = observation {
                        observation.work(1, &state.span.source)?;
                    }
                    if !self.runtime_domain_match(
                        actual,
                        &bound.expression,
                        false,
                        state,
                        (depth + 1, guards),
                        next,
                    )? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Type::Wildcard { upper, lower } => {
                if let Some(upper) = upper {
                    self.runtime_domain_match(
                        actual,
                        upper,
                        false,
                        state,
                        (depth + 1, guards),
                        next,
                    )
                } else if let Some(lower) = lower {
                    // Runtime isCompatibleArgument reverses compatibility;
                    // declaration-bound isMatching remains a separate contract.
                    self.runtime_domain_match(
                        lower,
                        actual,
                        false,
                        state,
                        (depth + 1, guards),
                        next,
                    )
                } else {
                    Ok(true)
                }
            }
            Type::Classifier {
                declaration: expected_id,
                arguments: expected_arguments,
                raw: expected_raw,
            } => {
                if invariant && !has_open_parameter(expected) {
                    return state.invariant(self, actual, expected, depth + 1);
                }
                if !invariant && let Type::Parameter { declaration } = actual {
                    // A reversed raw parameter supplies any compatible upper
                    // bound. This does not bind/capture the parameter and is
                    // not a guarded classifier-argument progress step.
                    let mut unresolved = None;
                    let bounds = state.bounds(self, declaration)?;
                    for bound in bounds.as_slice() {
                        if let Some(observation) = observation {
                            observation.work(1, &state.span.source)?;
                        }
                        match self.runtime_domain_match(
                            &bound.expression,
                            expected,
                            false,
                            state,
                            (depth + 1, guards),
                            next,
                        ) {
                            Ok(true) => return Ok(true),
                            Ok(false) => {}
                            Err(error) if matches!(error.kind, ErrorKind::Unsupported) => {
                                if unresolved.is_none() {
                                    unresolved = Some(error);
                                }
                            }
                            Err(error) => return Err(error),
                        }
                    }
                    return match unresolved {
                        Some(error) => Err(error),
                        None => Ok(false),
                    };
                }
                let Type::Classifier {
                    declaration: actual_id,
                    ..
                } = actual
                else {
                    return Ok(false);
                };
                let expected_kind = state.kind(self, expected_id)?;
                let actual_kind = state.kind(self, actual_id)?;
                if expected_kind == TargetKind::Class
                    && actual_kind == TargetKind::Class
                    && *expected_id
                        == crate::definitions::schema::eobject_reference_top_declaration()
                            .map_err(|e| error(&state.span, e))?
                {
                    return Ok(true);
                }
                let candidates = state.candidates(
                    self,
                    actual,
                    !invariant
                        && actual_kind == TargetKind::Class
                        && expected_kind == TargetKind::Class,
                )?;
                for index in 0..candidates.len() {
                    let candidate = candidates
                        .get(index)
                        .ok_or_else(|| error(&state.span, "Missing runtime candidate input"))?;
                    if let Some(observation) = observation {
                        observation.work(1, &state.span.source)?;
                    }
                    let Type::Classifier {
                        declaration,
                        arguments,
                        raw,
                    } = candidate
                    else {
                        continue;
                    };
                    let related = if declaration == expected_id {
                        true
                    } else if actual_kind == TargetKind::Class
                        || expected_kind == TargetKind::Class
                        || actual_kind == TargetKind::Enumeration
                        || expected_kind == TargetKind::Enumeration
                    {
                        false
                    } else {
                        state.relation(self, declaration, expected_id, invariant)?
                    };
                    if !related {
                        continue;
                    }
                    if *expected_raw {
                        return Ok(true);
                    }
                    if *raw && !expected_arguments.is_empty() {
                        continue;
                    }
                    if arguments.len() != expected_arguments.len() {
                        continue;
                    }
                    let mut matches = true;
                    for (argument, bound) in arguments.iter().zip(expected_arguments) {
                        if let Some(observation) = observation {
                            observation.work(1, &state.span.source)?;
                        }
                        if !self.runtime_domain_match(
                            argument,
                            bound,
                            true,
                            state,
                            (depth + 1, guards + 1),
                            next,
                        )? {
                            matches = false;
                            break;
                        }
                    }
                    if matches {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        })();
        if let Some(guard_exit) = guard_exit {
            state.remove_pair(guard_exit);
        }
        result
    }
}

#[cfg(test)]
mod parameter_bound_input_controls {
    use super::*;
    use crate::definitions::{
        bundle::DefinitionSource,
        conversion::{InvocationContext, InvocationObserver, ProviderId},
        ecore::ParserObservation,
    };
    use std::{cell::Cell, rc::Rc};
    struct Meter {
        work: Cell<usize>,
        bytes: Cell<usize>,
        cap: usize,
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("No provider expected".into())
        }
        fn work(&self, n: usize) -> Result<(), String> {
            self.work.set(self.work.get() + n);
            Ok(())
        }
        fn retain_storage(&self, n: usize) -> Result<(), String> {
            let next = self.bytes.get() + n;
            if next > self.cap {
                return Err("Bound input storage denied".into());
            }
            self.bytes.set(next);
            Ok(())
        }
    }
    fn fixture() -> (DefinitionBundle, DeclarationId) {
        let source = DefinitionSource::new(
            "memory:bound-inputs.ecore",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:bound-inputs'><eClassifiers x:type='e:EClass' name='Base'/><eClassifiers x:type='e:EClass' name='Other'/><eClassifiers x:type='e:EClass' name='Holder'><eTypeParameters name='T'><eBounds eClassifier='#//Base'/></eTypeParameters><eStructuralFeatures x:type='e:EReference' name='value'><eGenericType eTypeParameter='#//Holder/T'/></eStructuralFeatures></eClassifiers></e:EPackage>",
        );
        let id = DeclarationId {
            source: source.id.clone(),
            path: vec![2, 0],
        };
        (
            DefinitionBundle {
                version: 1,
                sources: vec![source],
                namespaces: vec![],
            },
            id,
        )
    }
    #[test]
    fn complete_bound_inputs_share_every_field_but_membership_remains_fresh() {
        let (bundle, id) = fixture();
        let resolver = Resolver::new(&bundle).unwrap();
        let operation = InvocationContext::default();
        let observation = ParserObservation::new(&operation);
        resolver
            .enable_parameter_bound_inputs(&observation)
            .unwrap();
        let first = resolver
            .parameter_bound_inputs_observed(&id, &observation)
            .unwrap();
        let second = resolver
            .parameter_bound_inputs_observed(&id, &observation)
            .unwrap();
        let (ParameterBounds::Shared(first), ParameterBounds::Shared(second)) = (first, second)
        else {
            panic!("Expected complete shared bound inputs");
        };
        assert!(Rc::ptr_eq(&first, &second));
        assert_eq!(first.parameter, resolver.resolve_parameter(&id).unwrap());
        assert_eq!(
            first.expressions,
            resolver.parameter_bound_types(&id).unwrap()
        );
        let ty = |index, raw| {
            crate::definitions::generic_types::RuntimeType::new(Type::Classifier {
                declaration: DeclarationId {
                    source: id.source.clone(),
                    path: vec![index],
                },
                arguments: vec![],
                raw,
            })
            .unwrap()
        };
        let owner = ty(2, true);
        let domain = resolver.runtime_feature_domains(&owner).unwrap().remove(0);
        for (actual, expected) in [
            (ty(0, false), true),
            (ty(1, false), false),
            (ty(0, false), true),
        ] {
            let meter = Rc::new(Meter {
                work: Cell::new(0),
                bytes: Cell::new(0),
                cap: usize::MAX,
            });
            let weak = Rc::downgrade(&meter);
            assert_eq!(
                resolver
                    .runtime_feature_accepts_observed(
                        &domain,
                        &actual,
                        &InvocationContext::new(meter.clone())
                    )
                    .unwrap(),
                expected
            );
            assert!(meter.work.get() > 0);
            drop(meter);
            assert!(weak.upgrade().is_none());
        }
    }
    #[test]
    fn complete_application_inputs_share_storage_but_check_reports_and_sources_are_fresh() {
        let (bundle, id) = fixture();
        let mut resolver = Resolver::new(&bundle).unwrap();
        let operation = InvocationContext::default();
        let observation = ParserObservation::new(&operation);
        resolver
            .enable_parameter_bound_inputs(&observation)
            .unwrap();
        let base = Type::Classifier {
            declaration: DeclarationId {
                source: id.source.clone(),
                path: vec![0],
            },
            arguments: vec![],
            raw: false,
        };
        let expression = Type::Classifier {
            declaration: DeclarationId {
                source: id.source.clone(),
                path: vec![2],
            },
            arguments: vec![base],
            raw: false,
        };
        let span = SourceSpan {
            source: id.source.clone(),
            start: 0,
            end: bundle.sources[0].text.len(),
        };
        let cold = Rc::new(Meter {
            work: Cell::new(0),
            bytes: Cell::new(0),
            cap: usize::MAX,
        });
        let cold_operation = InvocationContext::new(cold.clone());
        let first = resolver
            .check_expression_observed(&expression, &span, &ParserObservation::new(&cold_operation))
            .unwrap();
        assert!(first.seen.is_empty());
        assert!(!first.shared_seen.as_ref().unwrap().is_empty());
        assert_eq!(first.bounds.len(), 1);
        assert!(first.bounds[0].satisfied);
        let original_input = first
            .shared_seen
            .as_ref()
            .unwrap()
            .get(&expression)
            .unwrap()
            .clone();
        assert_eq!(
            serde_json::to_value(original_input.as_ref()).unwrap(),
            serde_json::to_value(&expression).unwrap()
        );
        let hot = Rc::new(Meter {
            work: Cell::new(0),
            bytes: Cell::new(0),
            cap: usize::MAX,
        });
        let hot_operation = InvocationContext::new(hot.clone());
        let second = resolver
            .check_expression_observed(&expression, &span, &ParserObservation::new(&hot_operation))
            .unwrap();
        let next_input = second
            .shared_seen
            .as_ref()
            .unwrap()
            .get(&expression)
            .unwrap();
        assert!(Rc::ptr_eq(&original_input, next_input));
        assert_eq!(first.bounds, second.bounds);
        assert_eq!(first.warnings, second.warnings);
        assert_eq!(first.remaining, second.remaining);
        assert!(hot.work.get() > 0);
        assert!(hot.bytes.get() < cold.bytes.get());
        let legacy_resolver = Resolver::new(&bundle).unwrap();
        let legacy = legacy_resolver
            .check_expression(&expression, &span)
            .unwrap();
        assert!(legacy.shared_seen.is_none());
        assert_eq!(legacy.bounds, first.bounds);
        assert_eq!(legacy.warnings, first.warnings);
        assert_eq!(legacy.remaining, first.remaining);
        let changed = DefinitionSource::new(
            id.source.uri.clone(),
            SourceKind::Ecore,
            bundle.sources[0]
                .text
                .replace("name='Base'", "name='Changed'"),
        );
        resolver.resources = ResourceSet::new(&DefinitionBundle {
            version: 1,
            sources: vec![changed],
            namespaces: vec![],
        })
        .unwrap();
        assert!(
            resolver
                .check_expression_observed(&expression, &span, &observation)
                .is_err()
        );
    }
    #[test]
    fn application_input_precharges_publication_and_hit_and_recovers_without_cached_outcome() {
        let (bundle, id) = fixture();
        let resolver = Resolver::new(&bundle).unwrap();
        let operation = InvocationContext::default();
        let observation = ParserObservation::new(&operation);
        resolver
            .enable_parameter_bound_inputs(&observation)
            .unwrap();
        let expression = Type::Classifier {
            declaration: DeclarationId {
                source: id.source.clone(),
                path: vec![0],
            },
            arguments: vec![],
            raw: false,
        };
        let span = SourceSpan {
            source: id.source.clone(),
            start: 0,
            end: bundle.sources[0].text.len(),
        };
        let denied_operation = InvocationContext::new(Rc::new(Meter {
            work: Cell::new(0),
            bytes: Cell::new(0),
            cap: 0,
        }));
        let denied = ParserObservation::new(&denied_operation);
        assert!(
            resolver
                .shared_application_input(&expression, &span, &denied)
                .is_err()
        );
        assert_eq!(
            resolver
                .prepared_parameter_bound_inputs
                .borrow()
                .as_ref()
                .unwrap()
                .borrow()
                .expressions
                .len(),
            0
        );
        let first = resolver
            .shared_application_input(&expression, &span, &observation)
            .unwrap();
        assert!(
            resolver
                .shared_application_input(&expression, &span, &denied)
                .is_err()
        );
        assert_eq!(
            resolver
                .prepared_parameter_bound_inputs
                .borrow()
                .as_ref()
                .unwrap()
                .borrow()
                .expressions
                .len(),
            1
        );
        let second = resolver
            .shared_application_input(&expression, &span, &observation)
            .unwrap();
        assert!(Rc::ptr_eq(&first, &second));
        assert_eq!(first.as_ref(), &expression);
        let table = resolver
            .prepared_parameter_bound_inputs
            .borrow()
            .as_ref()
            .unwrap()
            .clone();
        let mut held = table.borrow_mut();
        assert!(
            resolver
                .shared_application_input(&expression, &span, &observation)
                .is_err()
        );
        held.expressions.clear();
        drop(held);
        assert!(
            resolver
                .shared_application_input(&expression, &span, &observation)
                .is_ok()
        );
    }
    #[test]
    fn warning_source_inputs_preserve_fresh_messages_full_export_and_changed_source_checks() {
        let (bundle, id) = fixture();
        let mut resolver = Resolver::new(&bundle).unwrap();
        let operation = InvocationContext::default();
        let observation = ParserObservation::new(&operation);
        resolver
            .enable_parameter_bound_inputs(&observation)
            .unwrap();
        let expression = Type::Classifier {
            declaration: DeclarationId {
                source: id.source.clone(),
                path: vec![2],
            },
            arguments: vec![],
            raw: true,
        };
        let span = SourceSpan {
            source: id.source.clone(),
            start: 7,
            end: bundle.sources[0].text.len(),
        };
        let first = resolver
            .check_expression_observed(&expression, &span, &observation)
            .unwrap();
        let second = resolver
            .check_expression_observed(&expression, &span, &observation)
            .unwrap();
        let (CheckWarnings::Shared(a), CheckWarnings::Shared(b)) =
            (&first.warnings, &second.warnings)
        else {
            panic!("Shared input warnings expected");
        };
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert!(Rc::ptr_eq(&a[0].span, &b[0].span));
        assert_eq!(a[0].span.as_ref(), &span);
        assert_eq!(a[0].message, b[0].message);
        assert_ne!(a[0].message.as_ptr(), b[0].message.as_ptr());
        assert_eq!(a[0].code, "raw_type");
        let legacy = Resolver::new(&bundle)
            .unwrap()
            .runtime_application_checks(&expression, &span)
            .unwrap()
            .1;
        let exported = first
            .warnings
            .into_owned_observed(&span, &observation)
            .unwrap();
        assert_eq!(
            serde_json::to_value(&exported).unwrap(),
            serde_json::to_value(&legacy).unwrap()
        );
        let (_, second_exported) = resolver
            .runtime_application_checks_observed(&expression, &span, &observation)
            .unwrap();
        assert_eq!(exported, second_exported);
        let changed_span = SourceSpan {
            start: 8,
            ..span.clone()
        };
        let distinct = resolver
            .shared_warning_span(&changed_span, &observation)
            .unwrap();
        assert!(!Rc::ptr_eq(&distinct, &b[0].span));
        assert_eq!(distinct.as_ref(), &changed_span);
        let changed = DefinitionSource::new(
            id.source.uri.clone(),
            SourceKind::Ecore,
            bundle.sources[0]
                .text
                .replace("name='Base'", "name='Changed'"),
        );
        resolver.resources = ResourceSet::new(&DefinitionBundle {
            version: 1,
            sources: vec![changed],
            namespaces: vec![],
        })
        .unwrap();
        assert!(
            resolver
                .check_expression_observed(&expression, &span, &observation)
                .is_err()
        );
        resolver.resources = ResourceSet::new(&bundle).unwrap();
        assert_eq!(
            resolver
                .runtime_application_checks_observed(&expression, &span, &observation)
                .unwrap()
                .1,
            exported
        );
    }
    #[test]
    fn warning_source_insertion_hits_and_full_export_precharge_and_recover() {
        let (bundle, id) = fixture();
        let resolver = Resolver::new(&bundle).unwrap();
        let operation = InvocationContext::default();
        let observation = ParserObservation::new(&operation);
        resolver
            .enable_parameter_bound_inputs(&observation)
            .unwrap();
        let span = SourceSpan {
            source: id.source.clone(),
            start: 0,
            end: bundle.sources[0].text.len(),
        };
        let denied_operation = InvocationContext::new(Rc::new(Meter {
            work: Cell::new(0),
            bytes: Cell::new(0),
            cap: 0,
        }));
        let denied = ParserObservation::new(&denied_operation);
        assert!(resolver.shared_warning_span(&span, &denied).is_err());
        let table = resolver
            .prepared_parameter_bound_inputs
            .borrow()
            .as_ref()
            .unwrap()
            .clone();
        assert!(table.borrow().spans.is_empty());
        let input = resolver.shared_warning_span(&span, &observation).unwrap();
        assert!(resolver.shared_warning_span(&span, &denied).is_err());
        assert_eq!(table.borrow().spans.len(), 1);
        let warnings = || {
            CheckWarnings::Shared(vec![SharedCheckWarning {
                code: "raw_type",
                message: "complete fresh warning".into(),
                span: input.clone(),
            }])
        };
        assert!(warnings().into_owned_observed(&span, &denied).is_err());
        let exported = warnings().into_owned_observed(&span, &observation).unwrap();
        assert_eq!(exported[0].span, span);
        assert_eq!(exported[0].message, "complete fresh warning");
        assert!(Rc::ptr_eq(
            &input,
            &resolver.shared_warning_span(&span, &observation).unwrap()
        ));
    }
    #[test]
    fn warning_source_registry_orders_all_fields_and_interrupts_lookup_without_publication() {
        struct WorkDenied;
        impl InvocationObserver for WorkDenied {
            fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
                Err("No provider expected".into())
            }
            fn retain_storage(&self, _: usize) -> Result<(), String> {
                Ok(())
            }
            fn work(&self, _: usize) -> Result<(), String> {
                Err("Warning input lookup cancelled".into())
            }
        }
        let (bundle, id) = fixture();
        let resolver = Resolver::new(&bundle).unwrap();
        let operation = InvocationContext::default();
        let observation = ParserObservation::new(&operation);
        resolver
            .enable_parameter_bound_inputs(&observation)
            .unwrap();
        let mut inputs = Vec::new();
        for start in [10, 2, 7, 5, 0] {
            let span = SourceSpan {
                source: id.source.clone(),
                start,
                end: 20,
            };
            let retained = resolver.shared_warning_span(&span, &observation).unwrap();
            inputs.push((span, retained));
        }
        for (span, retained) in &inputs {
            assert!(Rc::ptr_eq(
                retained,
                &resolver.shared_warning_span(span, &observation).unwrap()
            ));
        }
        for span in [
            SourceSpan {
                end: 21,
                ..inputs[0].0.clone()
            },
            SourceSpan {
                source: crate::definitions::bundle::SourceId {
                    sha256: "b".repeat(64),
                    ..id.source.clone()
                },
                ..inputs[0].0.clone()
            },
            SourceSpan {
                source: crate::definitions::bundle::SourceId {
                    uri: "memory:other-warning-source.ecore".into(),
                    ..id.source.clone()
                },
                ..inputs[0].0.clone()
            },
        ] {
            let retained = resolver.shared_warning_span(&span, &observation).unwrap();
            assert_eq!(retained.as_ref(), &span);
            assert!(!Rc::ptr_eq(&retained, &inputs[0].1));
        }
        let table = resolver
            .prepared_parameter_bound_inputs
            .borrow()
            .as_ref()
            .unwrap()
            .clone();
        let previous = table.borrow().spans.clone();
        let denied_operation = InvocationContext::new(Rc::new(WorkDenied));
        let denied = ParserObservation::new(&denied_operation);
        assert!(resolver.shared_warning_span(&inputs[0].0, &denied).is_err());
        let new_span = SourceSpan {
            start: 3,
            ..inputs[0].0.clone()
        };
        assert!(resolver.shared_warning_span(&new_span, &denied).is_err());
        assert_eq!(table.borrow().spans, previous);
        assert!(
            resolver
                .shared_warning_span(&new_span, &observation)
                .is_ok()
        );
        assert_eq!(table.borrow().spans.len(), previous.len() + 1);
    }
    #[test]
    fn borrowed_parameter_owner_preserves_full_identity_roles_and_source_checks() {
        let (bundle, id) = fixture();
        let mut resolver = Resolver::new(&bundle).unwrap();
        let cold = Rc::new(Meter {
            work: Cell::new(0),
            bytes: Cell::new(0),
            cap: usize::MAX,
        });
        let cold_operation = InvocationContext::new(cold.clone());
        let owner = resolver
            .parameter_owner_observed(&id, &ParserObservation::new(&cold_operation))
            .unwrap();
        assert_eq!(owner.source, id.source);
        assert_eq!(owner.path, vec![2]);
        let borrowed = Rc::new(Meter {
            work: Cell::new(0),
            bytes: Cell::new(0),
            cap: usize::MAX,
        });
        let borrowed_operation = InvocationContext::new(borrowed.clone());
        let borrowed_observation = ParserObservation::new(&borrowed_operation);
        resolver
            .parameter_owner_input_observed(&id, &owner, &borrowed_observation)
            .unwrap();
        assert!(borrowed.work.get() > 0);
        assert!(borrowed.bytes.get() < cold.bytes.get());
        for bad in [
            DeclarationId {
                path: vec![1],
                ..owner.clone()
            },
            DeclarationId {
                source: crate::definitions::bundle::SourceId {
                    sha256: "b".repeat(64),
                    ..owner.source.clone()
                },
                ..owner.clone()
            },
            DeclarationId {
                source: crate::definitions::bundle::SourceId {
                    uri: "memory:other-owner.ecore".into(),
                    ..owner.source.clone()
                },
                ..owner.clone()
            },
        ] {
            assert!(
                resolver
                    .parameter_owner_input_observed(&id, &bad, &borrowed_observation)
                    .is_err()
            );
        }
        assert!(
            resolver
                .parameter_owner_input_observed(&owner, &owner, &borrowed_observation)
                .is_err()
        );
        let changed = DefinitionSource::new(
            id.source.uri.clone(),
            SourceKind::Ecore,
            bundle.sources[0]
                .text
                .replace("name='Holder'", "name='Changed'"),
        );
        resolver.resources = ResourceSet::new(&DefinitionBundle {
            version: 1,
            sources: vec![changed],
            namespaces: vec![],
        })
        .unwrap();
        assert!(
            resolver
                .parameter_owner_input_observed(&id, &owner, &borrowed_observation)
                .is_err()
        );
        resolver.resources = ResourceSet::new(&bundle).unwrap();
        assert!(
            resolver
                .parameter_owner_input_observed(&id, &owner, &borrowed_observation)
                .is_ok()
        );
        let source = DefinitionSource::new(
            "memory:operation-parameter.ecore",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:op-param'><eClassifiers x:type='e:EClass' name='Owner'><eOperations name='op'><eTypeParameters name='U'/></eOperations></eClassifiers></e:EPackage>",
        );
        let resolver = Resolver::new(&DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        })
        .unwrap();
        let id = DeclarationId {
            source: source.id,
            path: vec![0, 0, 0],
        };
        let owner = resolver
            .parameter_owner_observed(&id, &borrowed_observation)
            .unwrap();
        assert_eq!(owner.path, vec![0, 0]);
        resolver
            .parameter_owner_input_observed(&id, &owner, &borrowed_observation)
            .unwrap();
    }
    #[test]
    fn borrowed_parameter_owner_cancels_each_work_boundary_and_recovers_freshly() {
        struct WorkMeter {
            count: Cell<usize>,
            limit: usize,
        }
        impl InvocationObserver for WorkMeter {
            fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
                Err("No provider expected".into())
            }
            fn retain_storage(&self, _: usize) -> Result<(), String> {
                Ok(())
            }
            fn work(&self, n: usize) -> Result<(), String> {
                let next = self.count.get() + n;
                if next > self.limit {
                    return Err("Parent validation cancelled".into());
                }
                self.count.set(next);
                Ok(())
            }
        }
        let (bundle, id) = fixture();
        let resolver = Resolver::new(&bundle).unwrap();
        let owner = DeclarationId {
            source: id.source.clone(),
            path: vec![2],
        };
        let complete = Rc::new(WorkMeter {
            count: Cell::new(0),
            limit: usize::MAX,
        });
        let complete_operation = InvocationContext::new(complete.clone());
        resolver
            .parameter_owner_input_observed(
                &id,
                &owner,
                &ParserObservation::new(&complete_operation),
            )
            .unwrap();
        assert!(complete.count.get() > 0);
        for limit in 0..complete.count.get() {
            let operation = InvocationContext::new(Rc::new(WorkMeter {
                count: Cell::new(0),
                limit,
            }));
            assert!(
                resolver
                    .parameter_owner_input_observed(
                        &id,
                        &owner,
                        &ParserObservation::new(&operation)
                    )
                    .is_err(),
                "limit {limit}"
            );
        }
        let recovery = InvocationContext::default();
        assert!(
            resolver
                .parameter_owner_input_observed(&id, &owner, &ParserObservation::new(&recovery))
                .is_ok()
        );
    }
    #[test]
    fn small_visits_preserve_all_input_pointers_and_precharge_full_ordered_promotion() {
        let (_, id) = fixture();
        let span = SourceSpan {
            source: id.source.clone(),
            start: 0,
            end: 1,
        };
        let inputs: Vec<_> = (0..18)
            .map(|index| {
                Rc::new(Type::Classifier {
                    declaration: DeclarationId {
                        source: id.source.clone(),
                        path: vec![index],
                    },
                    arguments: vec![],
                    raw: false,
                })
            })
            .collect();
        let meter = Rc::new(Meter {
            work: Cell::new(0),
            bytes: Cell::new(0),
            cap: usize::MAX,
        });
        let operation = InvocationContext::new(meter.clone());
        let observation = ParserObservation::new(&operation);
        let mut visits = SharedApplicationVisits::default();
        for input in &inputs[..16] {
            visits
                .insert_observed(input.clone(), &span, &observation)
                .unwrap();
        }
        assert!(matches!(visits, SharedApplicationVisits::Small(_)));
        assert_eq!(meter.bytes.get(), 16 * 32);
        let denied_operation = InvocationContext::new(Rc::new(Meter {
            work: Cell::new(0),
            bytes: Cell::new(0),
            cap: 0,
        }));
        assert!(
            visits
                .insert_observed(
                    inputs[16].clone(),
                    &span,
                    &ParserObservation::new(&denied_operation)
                )
                .is_err()
        );
        assert!(matches!(visits, SharedApplicationVisits::Small(_)));
        for input in &inputs[..16] {
            assert!(Rc::ptr_eq(input, visits.get(input.as_ref()).unwrap()));
        }
        assert!(visits.get(inputs[16].as_ref()).is_none());
        visits
            .insert_observed(inputs[16].clone(), &span, &observation)
            .unwrap();
        assert!(matches!(visits, SharedApplicationVisits::Large(_)));
        assert_eq!(meter.bytes.get(), 16 * 32 + 17 * 128);
        visits
            .insert_observed(inputs[17].clone(), &span, &observation)
            .unwrap();
        assert_eq!(meter.bytes.get(), 16 * 32 + 18 * 128);
        for input in &inputs {
            assert!(Rc::ptr_eq(input, visits.get(input.as_ref()).unwrap()));
            assert!(
                visits
                    .contains_observed(input.as_ref(), &span, &observation)
                    .unwrap()
            );
        }
        assert!(
            !SharedApplicationVisits::default()
                .contains_observed(inputs[0].as_ref(), &span, &observation)
                .unwrap()
        );
        let mut changed = inputs[0].as_ref().clone();
        let Type::Classifier { declaration, .. } = &mut changed else {
            panic!("Classifier expected");
        };
        declaration.source.sha256 = "b".repeat(64);
        assert!(
            !visits
                .contains_observed(&changed, &span, &observation)
                .unwrap()
        );
    }
    #[test]
    fn small_visits_cancel_lookup_and_promotion_without_partial_movement_and_recover() {
        struct WorkDenied;
        impl InvocationObserver for WorkDenied {
            fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
                Err("No provider expected".into())
            }
            fn retain_storage(&self, _: usize) -> Result<(), String> {
                Ok(())
            }
            fn work(&self, _: usize) -> Result<(), String> {
                Err("Visit work cancelled".into())
            }
        }
        let (_, id) = fixture();
        let span = SourceSpan {
            source: id.source.clone(),
            start: 0,
            end: 1,
        };
        let inputs: Vec<_> = (0..17)
            .map(|index| {
                Rc::new(Type::Classifier {
                    declaration: DeclarationId {
                        source: id.source.clone(),
                        path: vec![index],
                    },
                    arguments: vec![],
                    raw: false,
                })
            })
            .collect();
        let operation = InvocationContext::default();
        let observation = ParserObservation::new(&operation);
        let denied_operation = InvocationContext::new(Rc::new(WorkDenied));
        let denied = ParserObservation::new(&denied_operation);
        let mut visits = SharedApplicationVisits::default();
        for input in &inputs[..16] {
            visits
                .insert_observed(input.clone(), &span, &observation)
                .unwrap();
        }
        assert!(
            visits
                .contains_observed(inputs[15].as_ref(), &span, &denied)
                .is_err()
        );
        assert!(
            visits
                .insert_observed(inputs[16].clone(), &span, &denied)
                .is_err()
        );
        assert!(matches!(visits, SharedApplicationVisits::Small(_)));
        for input in &inputs[..16] {
            assert!(Rc::ptr_eq(input, visits.get(input.as_ref()).unwrap()));
        }
        assert!(visits.get(inputs[16].as_ref()).is_none());
        assert!(
            visits
                .contains_observed(inputs[15].as_ref(), &span, &observation)
                .unwrap()
        );
        visits
            .insert_observed(inputs[16].clone(), &span, &observation)
            .unwrap();
        assert!(
            visits
                .contains_observed(inputs[16].as_ref(), &span, &observation)
                .unwrap()
        );
    }
    #[test]
    fn membership_only_checks_skip_unpublished_warnings_keep_checks_and_reject_export() {
        let (bundle, id) = fixture();
        let resolver = Resolver::new(&bundle).unwrap();
        let setup = InvocationContext::default();
        resolver
            .enable_parameter_bound_inputs(&ParserObservation::new(&setup))
            .unwrap();
        let expression = Type::Classifier {
            declaration: DeclarationId {
                source: id.source.clone(),
                path: vec![2],
            },
            arguments: vec![],
            raw: true,
        };
        let span = SourceSpan {
            source: id.source.clone(),
            start: 0,
            end: bundle.sources[0].text.len(),
        };
        let metered = || {
            Rc::new(Meter {
                work: Cell::new(0),
                bytes: Cell::new(0),
                cap: usize::MAX,
            })
        };
        let report_meter = metered();
        let report_operation = InvocationContext::new(report_meter.clone());
        let report = resolver
            .check_expression_with_warnings_observed(
                &expression,
                &span,
                &ParserObservation::new(&report_operation),
                true,
            )
            .unwrap();
        assert_eq!(report.warnings.len(), 1);
        let predicate_meter = metered();
        let predicate_operation = InvocationContext::new(predicate_meter.clone());
        let predicate_observation = ParserObservation::new(&predicate_operation);
        let predicate = resolver
            .check_expression_with_warnings_observed(
                &expression,
                &span,
                &predicate_observation,
                false,
            )
            .unwrap();
        assert_eq!(predicate.bounds, report.bounds);
        assert_eq!(predicate.remaining, report.remaining);
        assert!(matches!(predicate.warnings, CheckWarnings::Discarded));
        assert!(predicate_meter.work.get() < report_meter.work.get());
        assert!(predicate_meter.bytes.get() < report_meter.bytes.get());
        assert!(
            predicate
                .warnings
                .into_owned_observed(&span, &predicate_observation)
                .is_err()
        );
        let warnings = report
            .warnings
            .into_owned_observed(&span, &ParserObservation::new(&setup))
            .unwrap();
        assert_eq!(
            warnings,
            Resolver::new(&bundle)
                .unwrap()
                .runtime_application_checks(&expression, &span)
                .unwrap()
                .1
        );
        let mut stale = expression.clone();
        if let Type::Classifier { declaration, .. } = &mut stale {
            declaration.source.sha256 = "0".repeat(64);
        }
        let invalid_raw = Type::Classifier {
            declaration: DeclarationId {
                source: id.source.clone(),
                path: vec![0],
            },
            arguments: vec![],
            raw: true,
        };
        for invalid in [&stale, &invalid_raw] {
            let report = resolver
                .check_expression_with_warnings_observed(
                    invalid,
                    &span,
                    &predicate_observation,
                    true,
                )
                .err()
                .unwrap();
            let predicate = resolver
                .check_expression_with_warnings_observed(
                    invalid,
                    &span,
                    &predicate_observation,
                    false,
                )
                .err()
                .unwrap();
            assert_eq!(predicate, report);
        }
        let invalid_bound = Type::Classifier {
            declaration: DeclarationId {
                source: id.source.clone(),
                path: vec![2],
            },
            arguments: vec![Type::Classifier {
                declaration: DeclarationId {
                    source: id.source.clone(),
                    path: vec![1],
                },
                arguments: vec![],
                raw: false,
            }],
            raw: false,
        };
        let reported_bounds = resolver
            .check_expression_with_warnings_observed(
                &invalid_bound,
                &span,
                &predicate_observation,
                true,
            )
            .unwrap();
        let predicate_bounds = resolver
            .check_expression_with_warnings_observed(
                &invalid_bound,
                &span,
                &predicate_observation,
                false,
            )
            .unwrap();
        assert!(predicate_bounds.bounds.iter().any(|check| !check.satisfied));
        assert_eq!(predicate_bounds.bounds, reported_bounds.bounds);
        struct Cancel;
        impl InvocationObserver for Cancel {
            fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
                Err("Unexpected provider".into())
            }
            fn work(&self, _: usize) -> Result<(), String> {
                Err("Predicate cancelled".into())
            }
        }
        let cancelled = InvocationContext::new(Rc::new(Cancel));
        assert!(
            resolver
                .check_expression_with_warnings_observed(
                    &expression,
                    &span,
                    &ParserObservation::new(&cancelled),
                    false
                )
                .is_err()
        );
        assert!(
            resolver
                .check_expression_with_warnings_observed(
                    &expression,
                    &span,
                    &predicate_observation,
                    false
                )
                .is_ok()
        );
    }
    #[test]
    fn borrowed_warning_code_retains_fresh_messages_and_full_owned_export() {
        let (bundle, id) = fixture();
        let resolver = Resolver::new(&bundle).unwrap();
        let operation = InvocationContext::default();
        let observation = ParserObservation::new(&operation);
        resolver
            .enable_parameter_bound_inputs(&observation)
            .unwrap();
        let expression = Type::Classifier {
            declaration: DeclarationId {
                source: id.source.clone(),
                path: vec![2],
            },
            arguments: vec![],
            raw: true,
        };
        let span = SourceSpan {
            source: id.source.clone(),
            start: 0,
            end: bundle.sources[0].text.len(),
        };
        let first = resolver
            .check_expression_observed(&expression, &span, &observation)
            .unwrap();
        let second = resolver
            .check_expression_observed(&expression, &span, &observation)
            .unwrap();
        let (CheckWarnings::Shared(a), CheckWarnings::Shared(b)) =
            (&first.warnings, &second.warnings)
        else {
            panic!("Shared warnings expected");
        };
        assert_eq!(a[0].code, "raw_type");
        assert_eq!(a[0].code.as_ptr(), b[0].code.as_ptr());
        assert_eq!(a[0].message, b[0].message);
        assert_ne!(a[0].message.as_ptr(), b[0].message.as_ptr());
        let owned = first
            .warnings
            .into_owned_observed(&span, &observation)
            .unwrap();
        let legacy = Resolver::new(&bundle)
            .unwrap()
            .runtime_application_checks(&expression, &span)
            .unwrap()
            .1;
        assert_eq!(
            serde_json::to_value(&owned).unwrap(),
            serde_json::to_value(&legacy).unwrap()
        );
        assert_ne!(owned[0].code.as_ptr(), b[0].code.as_ptr());
        assert_eq!(owned[0].span, span);
    }
    #[test]
    fn bound_inputs_precharge_before_insertion_and_recheck_changed_source_context() {
        let (bundle, id) = fixture();
        let mut resolver = Resolver::new(&bundle).unwrap();
        let operation = InvocationContext::default();
        let observation = ParserObservation::new(&operation);
        let denied = InvocationContext::new(Rc::new(Meter {
            work: Cell::new(0),
            bytes: Cell::new(0),
            cap: 0,
        }));
        assert!(
            resolver
                .enable_parameter_bound_inputs(&ParserObservation::new(&denied))
                .is_err()
        );
        assert!(resolver.prepared_parameter_bound_inputs.borrow().is_none());
        resolver
            .enable_parameter_bound_inputs(&observation)
            .unwrap();
        assert!(
            resolver
                .parameter_bound_inputs_observed(&id, &ParserObservation::new(&denied))
                .is_err()
        );
        assert!(
            resolver
                .prepared_parameter_bound_inputs
                .borrow()
                .as_ref()
                .unwrap()
                .borrow()
                .is_empty()
        );
        resolver
            .parameter_bound_inputs_observed(&id, &observation)
            .unwrap();
        let changed = DefinitionSource::new(
            bundle.sources[0].id.uri.clone(),
            SourceKind::Ecore,
            bundle.sources[0]
                .text
                .replace("name='Base'", "name='Changed'"),
        );
        let replacement = DefinitionBundle {
            version: 1,
            sources: vec![changed],
            namespaces: vec![],
        };
        resolver.resources = ResourceSet::new(&replacement).unwrap();
        assert!(
            resolver
                .parameter_bound_inputs_observed(&id, &observation)
                .is_err()
        );
        assert_eq!(
            resolver
                .prepared_parameter_bound_inputs
                .borrow()
                .as_ref()
                .unwrap()
                .borrow()
                .len(),
            1
        );
    }
}

#[cfg(test)]
mod moved_guard_input_controls {
    use super::*;
    use crate::definitions::{
        bundle::SourceId,
        conversion::{InvocationContext, InvocationObserver, ProviderId},
        ecore::ParserObservation,
    };
    struct Denied;
    impl InvocationObserver for Denied {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("No provider expected".into())
        }
        fn retain_storage(&self, _: usize) -> Result<(), String> {
            Err("Guard storage denied".into())
        }
    }
    fn ty(index: usize) -> Type {
        Type::Classifier {
            declaration: DeclarationId {
                source: SourceId {
                    uri: "memory:guard.ecore".into(),
                    sha256: "a".repeat(64),
                },
                path: vec![index],
            },
            arguments: vec![],
            raw: false,
        }
    }
    #[test]
    fn borrowed_guard_keeps_exact_input_identity_nested_scope_and_fresh_recovery() {
        let operation = InvocationContext::default();
        let observation = ParserObservation::new(&operation);
        let state = RuntimeDomainChecks {
            classifiers: None,
            borrow_input_types: true,
            observation: Some(&observation),
            remaining: 16384,
            environment: Substitution::new(vec![]).unwrap(),
            span: std::borrow::Cow::Owned(SourceSpan {
                source: SourceId {
                    uri: "memory:guard.ecore".into(),
                    sha256: "a".repeat(64),
                },
                start: 0,
                end: 1,
            }),
            active: Default::default(),
            active_inputs: vec![],
        };
        let actual = ty(0);
        let expected = ty(1);
        let other = ty(2);
        let outer = RuntimeBorrowedGuard {
            actual: &actual,
            expected: &expected,
            invariant: false,
            guards: 2,
            parent: None,
        };
        assert!(std::ptr::eq(outer.actual, &actual));
        assert!(std::ptr::eq(outer.expected, &expected));
        let copied_actual = actual.clone();
        let copied_expected = expected.clone();
        assert_eq!(
            state
                .previous_borrowed_guard(&copied_actual, &copied_expected, false, Some(&outer))
                .unwrap(),
            Some(2)
        );
        let inner = RuntimeBorrowedGuard {
            actual: &actual,
            expected: &other,
            invariant: true,
            guards: 3,
            parent: Some(&outer),
        };
        assert_eq!(
            state
                .previous_borrowed_guard(&actual, &expected, false, Some(&inner))
                .unwrap(),
            Some(2)
        );
        assert_eq!(
            state
                .previous_borrowed_guard(&actual, &other, true, Some(&inner))
                .unwrap(),
            Some(3)
        );
        assert_eq!(
            state
                .previous_borrowed_guard(&actual, &other, true, Some(&outer))
                .unwrap(),
            None
        );
        assert_eq!(
            state
                .previous_borrowed_guard(&actual, &expected, true, Some(&outer))
                .unwrap(),
            None
        );
        let mut changed = actual.clone();
        let Type::Classifier { declaration, .. } = &mut changed else {
            panic!("Classifier expected");
        };
        declaration.source.sha256 = "b".repeat(64);
        assert_eq!(
            state
                .previous_borrowed_guard(&changed, &expected, false, Some(&outer))
                .unwrap(),
            None
        );
        assert_eq!(
            state
                .previous_borrowed_guard(&actual, &expected, false, None)
                .unwrap(),
            None
        );
        assert!(state.active.is_empty());
        assert!(state.active_inputs.is_empty());
        let denied_operation = InvocationContext::new(std::rc::Rc::new(Denied));
        let denied_observation = ParserObservation::new(&denied_operation);
        let denied = RuntimeDomainChecks {
            classifiers: None,
            observation: Some(&denied_observation),
            ..state
        };
        assert!(
            denied
                .previous_borrowed_guard(&actual, &expected, false, Some(&outer))
                .is_err()
        );
        assert!(denied.active.is_empty());
        assert!(denied.active_inputs.is_empty());
        let recovered = RuntimeDomainChecks {
            classifiers: None,
            observation: Some(&observation),
            ..denied
        };
        assert_eq!(
            recovered
                .previous_borrowed_guard(&actual, &expected, false, Some(&outer))
                .unwrap(),
            Some(2)
        );
    }
    #[test]
    fn domain_span_borrow_preserves_every_field_and_observer_boundaries() {
        let span = SourceSpan {
            source: SourceId {
                uri: "memory:span-α.ecore".into(),
                sha256: "a".repeat(64),
            },
            start: 3,
            end: 987,
        };
        let operation = InvocationContext::default();
        let observation = ParserObservation::new(&operation);
        let borrowed = runtime_span_input(&span, true, Some(&observation)).unwrap();
        assert!(matches!(borrowed, std::borrow::Cow::Borrowed(_)));
        assert!(std::ptr::eq(borrowed.as_ref(), &span));
        assert_eq!(
            serde_json::to_value(borrowed.as_ref()).unwrap(),
            serde_json::to_value(&span).unwrap()
        );
        let owned = runtime_span_input(&span, false, Some(&observation)).unwrap();
        assert!(matches!(owned, std::borrow::Cow::Owned(_)));
        assert_eq!(owned.as_ref(), &span);
        assert_ne!(owned.source.uri.as_ptr(), span.source.uri.as_ptr());
        let denied_operation = InvocationContext::new(std::rc::Rc::new(Denied));
        let denied = ParserObservation::new(&denied_operation);
        assert!(runtime_span_input(&span, false, Some(&denied)).is_err());
        assert!(runtime_span_input(&span, true, Some(&denied)).is_ok());
        struct Cancelled;
        impl InvocationObserver for Cancelled {
            fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
                Err("No provider expected".into())
            }
            fn retain_storage(&self, _: usize) -> Result<(), String> {
                Ok(())
            }
            fn work(&self, _: usize) -> Result<(), String> {
                Err("Span input cancelled".into())
            }
        }
        let cancelled_operation = InvocationContext::new(std::rc::Rc::new(Cancelled));
        assert!(
            runtime_span_input(
                &span,
                true,
                Some(&ParserObservation::new(&cancelled_operation))
            )
            .is_err()
        );
        let recovered = runtime_span_input(&span, true, Some(&observation)).unwrap();
        assert!(std::ptr::eq(recovered.as_ref(), &span));
        assert_eq!(span.start, 3);
        assert_eq!(span.end, 987);
    }
    #[test]
    fn moved_guard_keeps_full_pair_identity_and_unwinds_only_its_own_depth() {
        let mut state = RuntimeDomainChecks {
            classifiers: None,
            borrow_input_types: true,
            observation: None,
            remaining: 16384,
            environment: Substitution::new(vec![]).unwrap(),
            span: std::borrow::Cow::Owned(SourceSpan {
                source: SourceId {
                    uri: "memory:guard.ecore".into(),
                    sha256: "a".repeat(64),
                },
                start: 0,
                end: 1,
            }),
            active: Default::default(),
            active_inputs: vec![],
        };
        let pair = (ty(0), ty(1), false);
        let expected = pair.clone();
        let Type::Classifier { declaration, .. } = &pair.0 else {
            panic!("classifier");
        };
        let allocation = declaration.source.uri.as_ptr();
        let outer = state.insert_pair(pair, 2).unwrap();
        assert_eq!(state.previous_pair(&expected).unwrap(), Some(2));
        assert_eq!(state.active_inputs[0].0, expected);
        let Type::Classifier { declaration, .. } = &state.active_inputs[0].0.0 else {
            panic!("classifier");
        };
        assert_eq!(declaration.source.uri.as_ptr(), allocation);
        let inner = state.insert_pair((ty(1), ty(2), true), 3).unwrap();
        assert_eq!(state.active_inputs.len(), 2);
        state.remove_pair(inner);
        assert_eq!(state.previous_pair(&expected).unwrap(), Some(2));
        state.remove_pair(outer);
        assert!(state.active_inputs.is_empty());
        assert!(state.active.is_empty());
    }
    #[test]
    fn denied_guard_growth_cannot_publish_a_partial_active_obligation() {
        let operation = InvocationContext::new(std::rc::Rc::new(Denied));
        let observation = ParserObservation::new(&operation);
        let mut state = RuntimeDomainChecks {
            classifiers: None,
            borrow_input_types: true,
            observation: Some(&observation),
            remaining: 16384,
            environment: Substitution::new(vec![]).unwrap(),
            span: std::borrow::Cow::Owned(SourceSpan {
                source: SourceId {
                    uri: "memory:guard.ecore".into(),
                    sha256: "a".repeat(64),
                },
                start: 0,
                end: 1,
            }),
            active: Default::default(),
            active_inputs: vec![],
        };
        assert!(state.insert_pair((ty(0), ty(1), false), 1).is_err());
        assert!(state.active_inputs.is_empty());
        assert!(state.active.is_empty());
    }
}

#[cfg(test)]
mod moved_guard_depth_control {
    use super::*;
    use crate::definitions::{conversion::InvocationContext, ecore::ParserObservation};
    #[test]
    #[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
    fn moved_guard_mode_preserves_the_original_parameter_depth_boundary() {
        use crate::definitions::{
            bundle::{DefinitionBundle, DefinitionSource, SourceKind},
            ecore::ErrorKind,
            generic_types::{RuntimeType, Type},
            resources::DeclarationId,
        };
        for count in [126, 127] {
            let parameters = (0..count)
                .rev()
                .map(|index| {
                    let bound = if index + 1 == count {
                        "<eBounds eClassifier='#//Base'/>".to_owned()
                    } else {
                        format!("<eBounds eTypeParameter='#//Owner/T{}'/>", index + 1)
                    };
                    format!("<eTypeParameters name='T{index}'>{bound}</eTypeParameters>")
                })
                .collect::<String>();
            let source = DefinitionSource::new(
                "memory:observed-lower-depth.ecore",
                SourceKind::Ecore,
                format!(
                    "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='limits' nsURI='urn:lower-limits'><eClassifiers x:type='e:EClass' name='Base'/><eClassifiers x:type='e:EClass' name='Other'/><eClassifiers x:type='e:EClass' name='Box'><eTypeParameters name='E'/></eClassifiers><eClassifiers x:type='e:EClass' name='Owner'>{parameters}<eStructuralFeatures x:type='e:EReference' name='value'><eGenericType eClassifier='#//Box'><eTypeArguments><eLowerBound eTypeParameter='#//Owner/T0'/></eTypeArguments></eGenericType></eStructuralFeatures></eClassifiers></e:EPackage>"
                ),
            );
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source.clone()],
                namespaces: vec![],
            };
            let resolver = Resolver::new(&bundle).unwrap();
            let input_operation = InvocationContext::default();
            resolver
                .enable_parameter_bound_inputs(&ParserObservation::new(&input_operation))
                .unwrap();
            let ty = |index, args, raw| {
                RuntimeType::new(Type::Classifier {
                    declaration: DeclarationId {
                        source: source.id.clone(),
                        path: vec![index],
                    },
                    arguments: args,
                    raw,
                })
                .unwrap()
            };
            let owner = ty(3, vec![], true);
            let domain = resolver
                .runtime_feature_domains(&owner)
                .unwrap()
                .into_iter()
                .find(|d| d.projection.source.name == "value")
                .unwrap();
            let actual = ty(2, vec![ty(1, vec![], false).expression().clone()], false);
            let result = resolver.runtime_feature_accepts_observed(
                &domain,
                &actual,
                &InvocationContext::default(),
            );
            if count == 127 {
                let crate::definitions::ecore::ObservedImportError::Import(e) = result.unwrap_err()
                else {
                    panic!("Depth rejection must remain an import error");
                };
                assert_eq!(e.kind, ErrorKind::ResourceLimit);
                assert_eq!(e.message, "Generic conformance budget exceeded");
            } else {
                assert!(!result.unwrap());
            }
        }
    }
}

#[cfg(test)]
mod ancestry_input_view_controls {
    use super::*;
    use crate::definitions::{
        bundle::DefinitionSource,
        conversion::{InvocationContext, InvocationObserver, ProviderId},
        ecore::ParserObservation,
    };
    use std::{cell::Cell, rc::Rc};
    struct Meter {
        bytes: Cell<usize>,
        work: Cell<usize>,
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("No provider expected".into())
        }
        fn retain_storage(&self, n: usize) -> Result<(), String> {
            self.bytes.set(self.bytes.get() + n);
            Ok(())
        }
        fn work(&self, n: usize) -> Result<(), String> {
            self.work.set(self.work.get() + n);
            Ok(())
        }
    }
    #[test]
    fn complete_ancestry_view_borrows_root_and_paths_and_rechecks_changed_sources() {
        let source = DefinitionSource::new(
            "memory:ancestry-view.ecore",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:ancestry-view'><eClassifiers x:type='e:EClass' name='Base'/><eClassifiers x:type='e:EClass' name='Left' eSuperTypes='#//Base'/><eClassifiers x:type='e:EClass' name='Right' eSuperTypes='#//Base'/><eClassifiers x:type='e:EClass' name='Diamond' eSuperTypes='#//Left #//Right'/></e:EPackage>",
        );
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let mut resolver = Resolver::new(&bundle).unwrap();
        let operation = InvocationContext::default();
        let observation = ParserObservation::new(&operation);
        resolver
            .resolve_materialization_inventory_observed(&observation)
            .unwrap();
        let root = Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![3],
            },
            arguments: vec![],
            raw: false,
        };
        let span = SourceSpan {
            source: source.id.clone(),
            start: 0,
            end: source.text.len(),
        };
        let expected = resolver.ancestry_expression(&root, &span).unwrap();
        let cold = Rc::new(Meter {
            bytes: Cell::new(0),
            work: Cell::new(0),
        });
        let cold_operation = InvocationContext::new(cold.clone());
        let cold_observation = ParserObservation::new(&cold_operation);
        let state = RuntimeDomainChecks {
            classifiers: None,
            borrow_input_types: true,
            observation: Some(&cold_observation),
            remaining: 16384,
            environment: Substitution::new(vec![]).unwrap(),
            span: std::borrow::Cow::Owned(span.clone()),
            active: Default::default(),
            active_inputs: vec![],
        };
        let first = state.candidates(&resolver, &root, true).unwrap();
        let RuntimeCandidateInputs::View {
            root: borrowed,
            ancestry,
        } = first
        else {
            panic!("Input view expected");
        };
        assert!(std::ptr::eq(borrowed, &root));
        assert_eq!(ancestry.as_slice(), expected);
        let hot = Rc::new(Meter {
            bytes: Cell::new(0),
            work: Cell::new(0),
        });
        let hot_operation = InvocationContext::new(hot.clone());
        let hot_observation = ParserObservation::new(&hot_operation);
        let mut state = RuntimeDomainChecks {
            classifiers: None,
            observation: Some(&hot_observation),
            ..state
        };
        let second = state.candidates(&resolver, &root, true).unwrap();
        assert_eq!(second.len(), expected.len() + 1);
        for (index, row) in expected.iter().enumerate() {
            assert_eq!(second.get(index + 1), Some(&row.expression));
        }
        assert!(hot.work.get() > 0);
        assert!(hot.bytes.get() < cold.bytes.get());
        let changed = DefinitionSource::new(
            source.id.uri.clone(),
            SourceKind::Ecore,
            source.text.replace("name='Base'", "name='Changed'"),
        );
        resolver.resources = ResourceSet::new(&DefinitionBundle {
            version: 1,
            sources: vec![changed],
            namespaces: vec![],
        })
        .unwrap();
        assert!(state.candidates(&resolver, &root, true).is_err());
        state.active_inputs.clear();
    }
}

#[cfg(test)]
#[path = "generic_attempt_classifier_tests.rs"]
mod attempt_classifier_controls;

#[path = "generic_nominal_rules.rs"]
pub(crate) mod nominal_rules;

#[cfg(test)]
#[path = "generic_operation_input_tests.rs"]
mod operation_input_controls;

// A completed retained report can enter the checked wrapper only through its
// fixed resolver-owning preparation, never through a caller-supplied DTO.
impl<'source, 'input> super::members::RuntimeMembersPreparation<'source, 'input> {
    pub(crate) fn finish_prepared(
        self,
    ) -> Result<
        (
            PreparedRuntimeMembers<'source>,
            std::borrow::Cow<'input, crate::definitions::generic_types::RuntimeType>,
        ),
        ImportError,
    > {
        let resolver = self.resolver();
        let (members, receiver) = self.finish_parts()?;
        Ok((PreparedRuntimeMembers { resolver, members }, receiver))
    }
}
