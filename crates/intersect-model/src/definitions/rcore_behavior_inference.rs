//! Bounded Rcore call constraints. Model relations remain in ModelTypes.
use super::{
    model_types::ModelTypes,
    signatures::FunctionSignature,
    types::{CollectionKind, Type, TypeRef, TypeTable},
};
use std::collections::{BTreeMap, BTreeSet};
pub(super) type Substitution = BTreeMap<TypeRef, TypeRef>;
const LIMIT: usize = 16384;
fn spend(budget: &mut usize) -> Result<(), String> {
    *budget = budget
        .checked_sub(1)
        .ok_or("Generic inference work limit exceeded")?;
    Ok(())
}
pub(super) fn children(ty: &Type) -> Vec<TypeRef> {
    match ty {
        Type::Collection { element, .. } | Type::Many { element, .. } | Type::Option(element) => {
            vec![*element]
        }
        Type::SymbolicModelApplication { arguments, .. }
        | Type::SymbolicDatatypeApplication { arguments, .. } => arguments.clone(),
        Type::Tuple(v) => v.clone(),
        Type::Record(v) => v.values().copied().collect(),
        Type::Result { ok, error } => vec![*ok, *error],
        Type::Function { parameters, result } => {
            parameters.iter().copied().chain([*result]).collect()
        }
        _ => vec![],
    }
}
pub(super) fn contains(
    table: &TypeTable,
    ty: TypeRef,
    vars: &BTreeSet<TypeRef>,
) -> Result<bool, String> {
    let mut work = vec![ty];
    let mut seen = BTreeSet::new();
    let mut budget = LIMIT;
    while let Some(ty) = work.pop() {
        spend(&mut budget)?;
        if vars.contains(&ty) {
            return Ok(true);
        }
        if seen.insert(ty) {
            work.extend(children(table.get(ty)?));
        }
    }
    Ok(false)
}
pub(super) fn substitute(
    table: &mut TypeTable,
    models: &mut ModelTypes<'_>,
    ty: TypeRef,
    values: &Substitution,
) -> Result<TypeRef, String> {
    let mut work = vec![(ty, false)];
    let mut done = BTreeMap::new();
    let mut budget = LIMIT;
    while let Some((id, exit)) = work.pop() {
        spend(&mut budget)?;
        if done.contains_key(&id) {
            continue;
        }
        if let Some(value) = values.get(&id) {
            done.insert(id, *value);
            continue;
        }
        let source = table.get(id)?.clone();
        if !exit {
            work.push((id, true));
            work.extend(children(&source).into_iter().map(|child| (child, false)));
            continue;
        }
        let get = |child: &TypeRef| {
            done.get(child)
                .copied()
                .ok_or_else(|| "Missing substituted child".to_owned())
        };
        let target = match source {
            Type::SymbolicModelApplication {
                declaration,
                arguments,
            } => models.application_type(
                table,
                declaration,
                arguments.iter().map(get).collect::<Result<_, _>>()?,
                false,
            )?,
            Type::SymbolicDatatypeApplication {
                declaration,
                arguments,
            } => models.application_type(
                table,
                declaration,
                arguments.iter().map(get).collect::<Result<_, _>>()?,
                true,
            )?,
            Type::Collection { kind, element } => Type::Collection {
                kind,
                element: get(&element)?,
            },
            Type::Tuple(items) => Type::Tuple(items.iter().map(get).collect::<Result<_, _>>()?),
            Type::Record(fields) => Type::Record(
                fields
                    .into_iter()
                    .map(|(name, ty)| Ok((name, get(&ty)?)))
                    .collect::<Result<_, String>>()?,
            ),
            Type::Option(inner) => Type::Option(get(&inner)?),
            Type::Result { ok, error } => Type::Result {
                ok: get(&ok)?,
                error: get(&error)?,
            },
            Type::Function { parameters, result } => Type::Function {
                parameters: parameters.iter().map(get).collect::<Result<_, _>>()?,
                result: get(&result)?,
            },
            Type::Many {
                element,
                lower,
                upper,
                ordered,
                unique,
            } => Type::Many {
                element: get(&element)?,
                lower,
                upper,
                ordered,
                unique,
            },
            leaf => leaf,
        };
        done.insert(id, table.intern(target)?);
    }
    done.get(&ty)
        .copied()
        .ok_or_else(|| "Missing substituted type".into())
}
fn collection_kind(ordered: bool, unique: bool) -> CollectionKind {
    match (ordered, unique) {
        (true, false) => CollectionKind::Sequence,
        (false, true) => CollectionKind::Set,
        (true, true) => CollectionKind::OrderedSet,
        (false, false) => CollectionKind::Bag,
    }
}
/// A contextual upper solution cannot safely fix a contravariant occurrence.
pub(super) fn covariant_hint(
    table: &TypeTable,
    ty: TypeRef,
    flexible: &BTreeSet<TypeRef>,
) -> Result<bool, String> {
    let mut work = vec![(ty, true)];
    let mut budget = LIMIT;
    while let Some((ty, positive)) = work.pop() {
        spend(&mut budget)?;
        if !positive && flexible.contains(&ty) {
            return Ok(false);
        }
        match table.get(ty)? {
            Type::SymbolicModelApplication { arguments, .. }
            | Type::SymbolicDatatypeApplication { arguments, .. } => {
                for argument in arguments {
                    if contains(table, *argument, flexible)? {
                        return Ok(false);
                    }
                }
            }
            Type::Function { parameters, result } => {
                work.push((*result, positive));
                work.extend(parameters.iter().map(|ty| (*ty, !positive)));
            }
            other => work.extend(children(other).into_iter().map(|ty| (ty, positive))),
        }
    }
    Ok(true)
}
// Structural decomposition only; leaf identity and all Ecore relations delegate.
fn pairs(a: &Type, b: &Type) -> Option<Vec<(TypeRef, TypeRef)>> {
    match (a, b) {
        (
            Type::Collection {
                kind: a,
                element: x,
            },
            Type::Collection {
                kind: b,
                element: y,
            },
        ) if a == b => Some(vec![(*x, *y)]),
        (
            Type::Many {
                element: a,
                ordered,
                unique,
                ..
            },
            Type::Collection { kind, element: b },
        ) if collection_kind(*ordered, *unique) == *kind => Some(vec![(*a, *b)]),
        (
            Type::Collection { kind, element: a },
            Type::Many {
                element: b,
                lower: 0,
                upper: -1,
                ordered,
                unique,
            },
        ) if collection_kind(*ordered, *unique) == *kind => Some(vec![(*a, *b)]),
        (
            Type::Many {
                element: a,
                lower: al,
                upper: au,
                ordered: ao,
                unique: aq,
            },
            Type::Many {
                element: b,
                lower: bl,
                upper: bu,
                ordered: bo,
                unique: bq,
            },
        ) if al >= bl && (*bu == -1 || (*au != -1 && au <= bu)) && ao == bo && aq == bq => {
            Some(vec![(*a, *b)])
        }
        (Type::Tuple(a), Type::Tuple(b)) if a.len() == b.len() => {
            Some(a.iter().copied().zip(b.iter().copied()).collect())
        }
        (Type::Record(a), Type::Record(b)) if a.keys().eq(b.keys()) => {
            Some(a.values().copied().zip(b.values().copied()).collect())
        }
        (Type::Option(a), Type::Option(b)) => Some(vec![(*a, *b)]),
        (Type::Result { ok: a, error: ae }, Type::Result { ok: b, error: be }) => {
            Some(vec![(*a, *b), (*ae, *be)])
        }
        (
            Type::Function {
                parameters: a,
                result: ar,
            },
            Type::Function {
                parameters: b,
                result: br,
            },
        ) if a.len() == b.len() => Some(
            std::iter::once((*ar, *br))
                .chain(b.iter().copied().zip(a.iter().copied()))
                .collect(),
        ),
        _ => None,
    }
}
pub(super) fn assignable(
    models: &mut ModelTypes<'_>,
    table: &TypeTable,
    a: TypeRef,
    b: TypeRef,
    bounds: &Substitution,
) -> Result<bool, String> {
    let mut work = vec![(a, b)];
    let mut seen = BTreeSet::new();
    let mut budget = LIMIT;
    while let Some((a, b)) = work.pop() {
        spend(&mut budget)?;
        if a == b {
            continue;
        }
        if !seen.insert((a, b)) {
            continue;
        }
        if let Some(bound) = bounds.get(&a) {
            work.push((*bound, b));
            continue;
        }
        if let Some(children) = pairs(table.get(a)?, table.get(b)?) {
            work.extend(children);
        } else if !models.assignable(table, a, b)? {
            return Ok(false);
        }
    }
    Ok(true)
}
enum Constraint {
    Assignable(TypeRef, TypeRef, bool),
    NominalInvariant(TypeRef, TypeRef, bool),
}
#[derive(Clone)]
pub(super) struct Inference {
    pub fixed: Substitution,
    pub relations: Vec<(TypeRef, TypeRef, bool)>,
    remaining: usize,
    shared: Option<std::rc::Rc<std::cell::Cell<usize>>>,
}
impl Default for Inference {
    fn default() -> Self {
        Self {
            fixed: BTreeMap::new(),
            relations: vec![],
            remaining: LIMIT,
            shared: None,
        }
    }
}
struct SharedSolveBudget {
    remaining: usize,
    initial: usize,
    shared: Option<std::rc::Rc<std::cell::Cell<usize>>>,
}
impl std::ops::Deref for SharedSolveBudget {
    type Target = usize;
    fn deref(&self) -> &usize {
        &self.remaining
    }
}
impl std::ops::DerefMut for SharedSolveBudget {
    fn deref_mut(&mut self) -> &mut usize {
        &mut self.remaining
    }
}
impl Drop for SharedSolveBudget {
    fn drop(&mut self) {
        if let Some(shared) = &self.shared {
            shared.set(
                shared
                    .get()
                    .saturating_sub(self.initial.saturating_sub(self.remaining)),
            );
        }
    }
}
impl Inference {
    pub fn with_shared_budget(shared: std::rc::Rc<std::cell::Cell<usize>>) -> Self {
        Self {
            shared: Some(shared),
            ..Self::default()
        }
    }

    /// Retain only fully known parts of a contextual function type. Unknown
    /// enclosing formals are holes, not rigid callback arguments. This seeds the
    /// ordinary solver; full call checking still validates the resulting type.
    pub fn known_context(
        &mut self,
        table: &TypeTable,
        template: TypeRef,
        context: TypeRef,
        missing: &BTreeSet<TypeRef>,
    ) -> Result<(), String> {
        let mut work = vec![(template, context, true)];
        while let Some((template, context, outward)) = work.pop() {
            spend(&mut self.remaining)?;
            if let Some(shared) = &self.shared {
                let next = shared
                    .get()
                    .checked_sub(1)
                    .ok_or("Generic inference work limit exceeded")?;
                shared.set(next);
            }
            if !contains(table, context, missing)? {
                self.relations.push((template, context, outward));
                continue;
            }
            match (table.get(template)?, table.get(context)?) {
                (
                    Type::Function {
                        parameters: a,
                        result: ar,
                    },
                    Type::Function {
                        parameters: b,
                        result: br,
                    },
                ) if a.len() == b.len() => {
                    work.push((*ar, *br, outward));
                    work.extend(a.iter().zip(b).map(|(a, b)| (*a, *b, !outward)));
                }
                (a, b) => {
                    let parts = if outward {
                        pairs(a, b)
                    } else {
                        pairs(b, a).map(|parts| {
                            parts
                                .into_iter()
                                .map(|(context, template)| (template, context))
                                .collect()
                        })
                    };
                    if let Some(parts) = parts {
                        work.extend(parts.into_iter().map(|(a, b)| (a, b, outward)));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn solve(
        &mut self,
        signature: &FunctionSignature,
        table: &mut TypeTable,
        models: &mut ModelTypes<'_>,
        bounds: &Substitution,
        complete: bool,
    ) -> Result<Substitution, String> {
        let vars: BTreeSet<_> = signature.generics.iter().map(|g| g.ty).collect();
        let mut lower: BTreeMap<TypeRef, Vec<TypeRef>> = BTreeMap::new();
        let mut upper: BTreeMap<TypeRef, Vec<TypeRef>> = BTreeMap::new();
        let mut nominal: BTreeMap<TypeRef, Vec<TypeRef>> = BTreeMap::new();
        let mut invariants = Vec::new();
        let mut work: Vec<_> = self
            .relations
            .iter()
            .map(|(a, b, outward)| Constraint::Assignable(*a, *b, *outward))
            .collect();
        let initial = self
            .shared
            .as_ref()
            .map_or(self.remaining, |b| self.remaining.min(b.get()));
        if initial == 0 {
            return Err("Generic inference work limit exceeded".into());
        }
        let mut budget = SharedSolveBudget {
            remaining: initial,
            initial,
            shared: self.shared.clone(),
        };
        let mut values;
        let mut propagated = BTreeSet::new();
        loop {
            while let Some(constraint) = work.pop() {
                spend(&mut budget)?;
                let (template, actual, outward, invariant) = match constraint {
                    Constraint::Assignable(a, b, outward) => (a, b, outward, false),
                    Constraint::NominalInvariant(a, b, outward) => {
                        invariants.push((a, b));
                        (a, b, outward, true)
                    }
                };
                if vars.contains(&template) && !self.fixed.contains_key(&template) {
                    if invariant {
                        nominal.entry(template).or_default().push(actual);
                    } else if outward {
                        upper.entry(template).or_default().push(actual);
                    } else {
                        lower.entry(template).or_default().push(actual);
                    }
                    continue;
                }
                // Only an actual source value can expose its declared bound.
                // Direct formal inference above retains the actual rigid identity;
                // an expected rigid type is never widened to its bound.
                if !invariant
                    && !outward
                    && let Some(bound) = bounds.get(&actual)
                {
                    work.push(Constraint::Assignable(template, *bound, outward));
                    continue;
                }
                if let Some(relations) =
                    models.symbolic_inference_pairs(table, template, actual, outward)?
                {
                    work.extend(
                        relations
                            .into_iter()
                            .map(|(a, b)| Constraint::NominalInvariant(a, b, outward)),
                    );
                    continue;
                }
                if invariant {
                    continue;
                }
                match (table.get(template)?, table.get(actual)?) {
                    (
                        Type::Function {
                            parameters: a,
                            result: ar,
                        },
                        Type::Function {
                            parameters: b,
                            result: br,
                        },
                    ) if a.len() == b.len() => {
                        work.push(Constraint::Assignable(*ar, *br, outward));
                        work.extend(
                            a.iter()
                                .copied()
                                .zip(b.iter().copied())
                                .map(|(a, b)| Constraint::Assignable(a, b, !outward)),
                        );
                    }
                    (a, b) => {
                        let parts = if outward {
                            pairs(a, b)
                        } else {
                            pairs(b, a).map(|parts| {
                                parts
                                    .into_iter()
                                    .map(|(actual, template)| (template, actual))
                                    .collect()
                            })
                        };
                        if let Some(parts) = parts {
                            work.extend(
                                parts
                                    .into_iter()
                                    .map(|(a, b)| Constraint::Assignable(a, b, outward)),
                            );
                        }
                    }
                }
            }
            values = self.fixed.clone();
            for generic in &signature.generics {
                spend(&mut budget)?;
                if values.contains_key(&generic.ty) {
                    continue;
                }
                let mut candidate = None;
                for actual in lower.get(&generic.ty).into_iter().flatten() {
                    spend(&mut budget)?;
                    candidate = Some(match candidate {
                        None => *actual,
                        Some(previous) => {
                            if assignable(models, table, *actual, previous, bounds)? {
                                previous
                            } else if assignable(models, table, previous, *actual, bounds)? {
                                *actual
                            } else {
                                models.join(table, previous, *actual)?
                            }
                        }
                    });
                }
                if candidate.is_none()
                    && let Some(observations) = nominal.get(&generic.ty)
                {
                    // Value positions retain exact nominal tags. Provider domain
                    // equivalence proves bounds, never an implicit value conversion.
                    let choices: BTreeSet<_> = observations.iter().copied().collect();
                    if choices.len() > 1 {
                        if complete {
                            return Err("Ambiguous nominal generic argument inference; supply an explicit exact type".into());
                        }
                    } else {
                        candidate = choices.into_iter().next();
                    }
                }
                if candidate.is_none() {
                    for target in upper.get(&generic.ty).into_iter().flatten() {
                        spend(&mut budget)?;
                        candidate = Some(match candidate {
                            None => *target,
                            Some(previous) => {
                                if assignable(models, table, previous, *target, bounds)? {
                                    previous
                                } else if assignable(models, table, *target, previous, bounds)? {
                                    *target
                                } else {
                                    return Err("Ambiguous generic inference constraints".into());
                                }
                            }
                        });
                    }
                }
                if let Some(candidate) = candidate {
                    values.insert(generic.ty, candidate);
                }
            }
            for generic in &signature.generics {
                if let (Some(bound), Some(actual)) =
                    (generic.bound, values.get(&generic.ty).copied())
                {
                    // The source bound is the template; the inferred actual remains rigid.
                    if propagated.insert((bound, actual)) {
                        work.push(Constraint::Assignable(bound, actual, false));
                    }
                }
            }
            if work.is_empty() {
                break;
            }
        }
        if complete {
            for generic in &signature.generics {
                if !values.contains_key(&generic.ty) {
                    return Err(format!("Cannot infer generic parameter {}", generic.name));
                }
            }
            for (template, actual) in invariants {
                spend(&mut budget)?;
                let template = substitute(table, models, template, &values)?;
                if !models.symbolic_value_argument_equal(table, template, actual)? {
                    return Err(
                        "Nominal generic value arguments require exact invariant identity".into(),
                    );
                }
            }
            for (template, actual, outward) in &self.relations {
                spend(&mut budget)?;
                let template = substitute(table, models, *template, &values)?;
                let (a, b) = if *outward {
                    (template, *actual)
                } else {
                    (*actual, template)
                };
                if !assignable(models, table, a, b, bounds)? {
                    return Err("Generic inference constraints are incompatible".into());
                }
            }
            for generic in &signature.generics {
                if let Some(bound) = generic.bound {
                    let bound = substitute(table, models, bound, &values)?;
                    let actual = *values.get(&generic.ty).ok_or("Missing generic argument")?;
                    if !assignable(models, table, actual, bound, bounds)? {
                        return Err(format!(
                            "Generic argument {} does not satisfy its declared bound",
                            generic.name
                        ));
                    }
                }
            }
        }
        self.remaining = *budget;
        Ok(values)
    }
}
