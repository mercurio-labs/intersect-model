//! Source-derived body checking. No type report is an executable program.
use super::{
    SyntaxDiagnostic,
    ast::{self, Declaration, Expr, ExprId, Literal},
    bindings::{ModuleBindings, Target},
    inference::{self, Inference, Substitution},
    model_types::{FeatureType, ModelTypes},
    modules::{ImportBinding, SymbolId},
    signatures::{self, FunctionSignature, Signatures},
    types::{CollectionKind, Scalar, Type, TypeRef, TypeTable},
};
use crate::definitions::bundle::{DefinitionBundle, SourceId};
use serde::Serialize;
use std::collections::BTreeMap;
#[path = "rcore_behavior_overloads.rs"]
mod overloads;
pub(crate) use overloads::snapshot_charge;

/// Fresh checker evidence for a selected named function value. Nongeneric
/// overload selections retain an empty ordered actual-argument list. Type references
/// belong to the accompanying TypeReport; caller-rigid arguments are not yet
/// executable concrete instances. Serialization is inspection, never a proof input.
#[derive(Clone, Debug, Serialize)]
pub struct FunctionInstantiation {
    pub symbol: SymbolId,
    /// Actuals in source formal order, including formals absent from the signature.
    pub arguments: Vec<TypeRef>,
    pub function_type: TypeRef,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModuleReport {
    pub source: SourceId,
    pub expressions: BTreeMap<usize, TypeRef>,
    /// Specialized callee expression (including a grouped direct callee), or
    /// contextual Name/Member expression. Local closures retain dynamic identity.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub function_instantiations: BTreeMap<usize, FunctionInstantiation>,
    pub members: BTreeMap<usize, FeatureType>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub symbolic_members: BTreeMap<usize, super::model_types::SymbolicFeatureType>,
    pub providers: BTreeMap<usize, ProviderType>,
    /// `new` expressions: the constructed class and each set feature.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub constructions: BTreeMap<usize, Construction>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Construction {
    pub class: crate::definitions::resources::DeclarationId,
    pub features: Vec<(String, FeatureType)>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ProviderType {
    pub receiver: TypeRef,
    pub feature: Option<FeatureType>,
}
#[derive(Clone, Debug, Serialize)]
pub struct TypeReport {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub model_domains: Vec<super::model_types::ModelDomainContext>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub symbolic_applications: Vec<super::model_types::SymbolicApplicationCheck>,
    pub signatures: Signatures,
    pub modules: Vec<ModuleReport>,
    /// Checked shared Ecore projections, not executable Rcore bodies.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub generic_models: Vec<crate::definitions::generic_resolution::RuntimeMembers>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub generic_datatypes: Vec<crate::definitions::generic_resolution::CheckedDatatypeApplication>,
}

/// Rebuild names and signatures from exact sources, then check bodies. Unsupported
/// forms reject; success never grants execution or validates provider availability.
pub fn check(
    bundle: &DefinitionBundle,
    entry: &SourceId,
) -> Result<TypeReport, Vec<SyntaxDiagnostic>> {
    check_with_source_limits(bundle, entry, None, Default::default())
}

/// Check nominal custom datatypes through explicitly selected native converters.
/// This validates definition defaults, not Rcore execution or provider bodies.
pub fn check_with_conversions(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    conversions: &crate::definitions::conversion::ConversionRegistry<'_>,
) -> Result<TypeReport, Vec<SyntaxDiagnostic>> {
    check_with_source_limits(bundle, entry, Some(conversions), Default::default())
}
pub(crate) fn check_with_source_limits(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<TypeReport, Vec<SyntaxDiagnostic>> {
    check_with_resources(bundle, entry, conversions, limits).map(|(report, _)| report)
}

/// Internal source-derived completion. The resource set carries parsed inputs,
/// never authorization to skip fresh body or image verification.
pub(crate) fn check_with_resources(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<(TypeReport, crate::definitions::resources::ResourceSet), Vec<SyntaxDiagnostic>> {
    check_with_store(bundle, entry, conversions, limits, None)
}

/// `check_with_resources` whose model types share completed runtime-schema
/// preparations through `store`. The report is the same with or without it.
pub(crate) fn check_with_store(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    limits: crate::definitions::ecore::ParseLimits,
    store: Option<&super::model_types::RuntimeSchemaStore>,
) -> Result<(TypeReport, crate::definitions::resources::ResourceSet), Vec<SyntaxDiagnostic>> {
    let (signatures, resources) = signatures::index_with_resources(bundle, entry, limits)?;
    check_prepared(bundle, conversions, limits, signatures, resources, store)
}

pub(super) fn check_prepared(
    bundle: &DefinitionBundle,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    limits: crate::definitions::ecore::ParseLimits,
    mut signatures: Signatures,
    resources: crate::definitions::resources::ResourceSet,
    store: Option<&super::model_types::RuntimeSchemaStore>,
) -> Result<(TypeReport, crate::definitions::resources::ResourceSet), Vec<SyntaxDiagnostic>> {
    let mut models = ModelTypes::with_source_resources(bundle, conversions, limits, resources);
    models.attach_schema_store(store);
    models.configure_symbolic(&signatures).map_err(|error| {
        vec![SyntaxDiagnostic {
            span: error.span,
            message: error.message,
        }]
    })?;
    let functions: BTreeMap<_, _> = signatures
        .functions
        .iter()
        .map(|f| (f.symbol.clone(), f.function_type))
        .collect();
    let generic_functions: BTreeMap<_, _> = signatures
        .functions
        .iter()
        .filter(|f| !f.generics.is_empty())
        .map(|f| (f.symbol.clone(), f.clone()))
        .collect();
    let mut errors = Vec::new();
    let mut modules = Vec::new();
    let shared_work = std::rc::Rc::new(std::cell::Cell::new(1_048_576usize));
    let snapshot_bytes = std::rc::Rc::new(std::cell::Cell::new(67_108_864usize));
    for ((module, bindings), annotations) in signatures
        .bound
        .index
        .modules
        .iter()
        .zip(&signatures.bound.bindings)
        .zip(&signatures.modules)
    {
        let doc = &module.document;
        for (ordinal, ty) in &annotations.annotations {
            if matches!(
                signatures.types.get(*ty),
                Ok(Type::SymbolicModelApplication { .. }
                    | Type::SymbolicDatatypeApplication { .. })
            ) && let Err(message) = super::symbolic::expression(&signatures.types, *ty)
                .and_then(|expression| models.check_symbolic(&expression))
            {
                errors.push(SyntaxDiagnostic {
                    span: doc.types[*ordinal].span.clone(),
                    message,
                });
            }
            if let Ok(Type::Enumeration(id)) = signatures.types.get(*ty)
                && let Err(message) = models.enumeration(id)
            {
                errors.push(SyntaxDiagnostic {
                    span: doc.types[*ordinal].span.clone(),
                    message,
                });
            }
            if let Ok(Type::Datatype(id)) = signatures.types.get(*ty)
                && let Err(message) = models.datatype(id)
            {
                errors.push(SyntaxDiagnostic {
                    span: doc.types[*ordinal].span.clone(),
                    message,
                });
            }
            if let Ok(Type::DatatypeApplication(expression)) = signatures.types.get(*ty)
                && let Err(message) = models.applied_datatype(expression)
            {
                errors.push(SyntaxDiagnostic {
                    span: doc.types[*ordinal].span.clone(),
                    message,
                });
            }
            if let Ok(Type::ModelApplication(expression)) = signatures.types.get(*ty)
                && let Err(message) = models.conforms_application(expression, expression)
            {
                errors.push(SyntaxDiagnostic {
                    span: doc.types[*ordinal].span.clone(),
                    message,
                });
            }
            if let Ok(Type::Model(class)) = signatures.types.get(*ty)
                && let Err(message) = models.conforms(class, class)
            {
                errors.push(SyntaxDiagnostic {
                    span: doc.types[*ordinal].span.clone(),
                    message,
                });
            }
        }
        let mut checker = Checker {
            doc,
            bindings,
            annotations: &annotations.annotations,
            functions: &functions,
            generic_functions: &generic_functions,
            table: &mut signatures.types,
            models: &mut models,
            locals: BTreeMap::new(),
            comprehension_hints: BTreeMap::new(),
            rigid_bounds: BTreeMap::new(),
            report: ModuleReport {
                source: doc.source.id.clone(),
                expressions: BTreeMap::new(),
                function_instantiations: BTreeMap::new(),
                members: BTreeMap::new(),
                symbolic_members: BTreeMap::new(),
                providers: BTreeMap::new(),
                constructions: BTreeMap::new(),
            },
            errors: Vec::new(),
            selected_targets: BTreeMap::new(),
            shared_work: shared_work.clone(),
            snapshot_bytes: snapshot_bytes.clone(),
            trial_depth: 0,
        };
        for (ordinal, declaration) in doc.declarations.iter().enumerate() {
            match &declaration.value {
                Declaration::Function {
                    external,
                    parameters,
                    result,
                    body,
                    ..
                } => {
                    checker.locals.clear();
                    checker.rigid_bounds.clear();
                    if let Some(signature) = generic_functions.get(&SymbolId {
                        source: doc.source.id.clone(),
                        declaration: ordinal,
                    }) {
                        checker.rigid_bounds.extend(
                            signature
                                .generics
                                .iter()
                                .filter_map(|g| g.bound.map(|bound| (g.ty, bound))),
                        );
                    }
                    for parameter in parameters {
                        if let Some(ty) = checker.annotations.get(&parameter.ty.0) {
                            checker.locals.insert(parameter.name.span.start, *ty);
                        }
                    }
                    if !external {
                        checker.walk(*body, checker.annotations.get(&result.0).copied());
                    }
                }
                Declaration::Derive { .. } | Declaration::Validate { .. } => {
                    checker.rigid_bounds.clear();
                    if let Err(message) =
                        checker.provider(ordinal, &declaration.value, &module.imports)
                    {
                        checker.errors.push(SyntaxDiagnostic {
                            span: declaration.span.clone(),
                            message,
                        });
                    }
                }
                _ => {}
            }
        }
        errors.extend(checker.errors);
        modules.push(checker.report);
    }
    errors.sort_by(|a, b| {
        (&a.span.source, a.span.start, a.span.end, &a.message).cmp(&(
            &b.span.source,
            b.span.start,
            b.span.end,
            &b.message,
        ))
    });
    if errors.is_empty() {
        let generic_models = models.generic_checks();
        let generic_datatypes = models.datatype_checks();
        let model_domains = models.domain_contexts();
        let symbolic_applications = models.symbolic_checks();
        let resources = models.into_source_resources();
        Ok((
            TypeReport {
                model_domains,
                symbolic_applications,
                signatures,
                modules,
                generic_models,
                generic_datatypes,
            },
            resources,
        ))
    } else {
        Err(errors)
    }
}

struct GenericCall {
    signature: FunctionSignature,
    inference: Inference,
    arguments: Vec<ExprId>,
    callee: ExprId,
    next: usize,
    order: Vec<usize>,
    seeded: std::collections::BTreeSet<usize>,
}
#[path = "rcore_behavior_collection_check.rs"]
mod collection_check;
use super::ir_v2::{AlgebraOp, CollectionOp, MembershipOp, TextOp};
enum Work {
    ContextualBranch(ExprId, ExprId),
    CollectionRightOperand(ExprId, Option<TypeRef>, bool),
    CollectionCallback(ExprId, Option<TypeRef>, CollectionOp),
    GenericSeed(ExprId, Option<TypeRef>, Box<GenericCall>, ExprId, TypeRef),
    GenericNext(ExprId, Option<TypeRef>, Box<GenericCall>),
    GenericCollect(ExprId, Option<TypeRef>, Box<GenericCall>),
    CollectionConvert(
        ExprId,
        Option<TypeRef>,
        CollectionKind,
        Option<TypeRef>,
        ExprId,
    ),
    MatchArms(ExprId, Option<TypeRef>),
    Constructor(ExprId, Option<TypeRef>, Option<TypeRef>, Vec<ExprId>),
    Visit(ExprId, Option<TypeRef>),
    Finish(ExprId, Option<TypeRef>),
    BinaryRight(ExprId, Option<TypeRef>),
    Arguments(ExprId, Option<TypeRef>),
    Local(usize, ExprId, Option<TypeRef>),
}
struct Checker<'a, 'b> {
    doc: &'a ast::Document,
    bindings: &'a ModuleBindings,
    annotations: &'a BTreeMap<usize, TypeRef>,
    functions: &'a BTreeMap<SymbolId, TypeRef>,
    generic_functions: &'a BTreeMap<SymbolId, FunctionSignature>,
    rigid_bounds: Substitution,
    table: &'b mut TypeTable,
    models: &'b mut ModelTypes<'a>,
    locals: BTreeMap<usize, TypeRef>,
    comprehension_hints: BTreeMap<usize, TypeRef>,
    report: ModuleReport,
    errors: Vec<SyntaxDiagnostic>,
    selected_targets: BTreeMap<usize, Target>,
    shared_work: std::rc::Rc<std::cell::Cell<usize>>,
    snapshot_bytes: std::rc::Rc<std::cell::Cell<usize>>,
    trial_depth: usize,
}
impl Checker<'_, '_> {
    fn rigid_view(&self, mut ty: TypeRef) -> Result<TypeRef, String> {
        let mut seen = std::collections::BTreeSet::new();
        while let Some(bound) = self.rigid_bounds.get(&ty) {
            if !seen.insert(ty) {
                return Err("Cyclic generic bound proof".into());
            }
            ty = *bound;
        }
        Ok(ty)
    }
    fn record_instantiation(
        &mut self,
        expression: ExprId,
        signature: &FunctionSignature,
        values: &inference::Substitution,
        function_type: TypeRef,
    ) -> Result<(), String> {
        let arguments = signature
            .generics
            .iter()
            .map(|generic| {
                values.get(&generic.ty).copied().ok_or_else(|| {
                    format!(
                        "Missing checked instantiation for generic parameter {}",
                        generic.name
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if self.trial_depth != 0 {
            self.charge_snapshot(&signature.symbol)?;
            self.charge_snapshot(&arguments)?;
        }
        self.report.function_instantiations.insert(
            expression.0,
            FunctionInstantiation {
                symbol: signature.symbol.clone(),
                arguments,
                function_type,
            },
        );
        Ok(())
    }
    fn generic_signature(&self, mut id: ExprId) -> Option<&FunctionSignature> {
        while let Expr::Group(inner) = self.doc.expressions[id.0].value {
            id = inner;
        }
        match self.target(id) {
            Some(Target::Symbol(symbol)) => self.generic_functions.get(symbol),
            _ => None,
        }
    }
    fn generic_call(
        &mut self,
        id: ExprId,
        expected: Option<TypeRef>,
        signature: FunctionSignature,
        work: &mut Vec<Work>,
    ) -> Result<(), String> {
        let Expr::Call {
            callee,
            types,
            arguments,
        } = &self.doc.expressions[id.0].value
        else {
            return Err("Expected generic call".into());
        };
        if arguments.len() != signature.parameters.len() {
            return Err(format!(
                "Call requires {} arguments, got {}",
                signature.parameters.len(),
                arguments.len()
            ));
        }
        let mut state = Inference::with_shared_budget(self.shared_work.clone());
        if !types.is_empty() {
            if types.len() != signature.generics.len() {
                return Err(format!(
                    "Generic call requires {} type arguments, got {}",
                    signature.generics.len(),
                    types.len()
                ));
            }
            for (generic, argument) in signature.generics.iter().zip(types) {
                state.fixed.insert(
                    generic.ty,
                    *self
                        .annotations
                        .get(&argument.0)
                        .ok_or("Missing generic type argument")?,
                );
            }
        }
        if let Some(expected) = expected {
            state.relations.push((signature.result, expected, true));
        }
        work.push(Work::GenericNext(
            id,
            expected,
            Box::new(GenericCall {
                signature,
                inference: state,
                arguments: arguments.clone(),
                callee: *callee,
                next: 0,
                order: (0..arguments.len()).collect(),
                seeded: std::collections::BTreeSet::new(),
            }),
        ));
        Ok(())
    }
    fn generic_leaves(
        &self,
        root: ExprId,
        template: TypeRef,
    ) -> Result<Vec<(ExprId, TypeRef)>, String> {
        let mut work = vec![(root, template)];
        let mut leaves = Vec::new();
        let mut budget = 16384usize;
        while let Some((id, template)) = work.pop() {
            budget = budget
                .checked_sub(1)
                .ok_or("Generic inference work limit exceeded")?;
            match (&self.doc.expressions[id.0].value, self.table.get(template)?) {
                (Expr::Group(inner), _) => work.push((*inner, template)),
                (
                    Expr::If {
                        then_branch,
                        else_branch,
                        ..
                    },
                    _,
                ) => work.extend([(*else_branch, template), (*then_branch, template)]),
                (
                    Expr::Call {
                        callee,
                        types,
                        arguments,
                    },
                    ty,
                ) if types.is_empty() && arguments.len() == 1 => {
                    let payload = match (self.target(*callee), ty) {
                        (Some(Target::Builtin { name, .. }), Type::Option(inner))
                            if name == "Some" =>
                        {
                            Some(*inner)
                        }
                        (Some(Target::Builtin { name, .. }), Type::Result { ok, .. })
                            if name == "Ok" =>
                        {
                            Some(*ok)
                        }
                        (Some(Target::Builtin { name, .. }), Type::Result { error, .. })
                            if name == "Err" =>
                        {
                            Some(*error)
                        }
                        _ => None,
                    };
                    if let Some(payload) = payload {
                        work.push((arguments[0], payload));
                    } else {
                        leaves.push((id, template));
                    }
                }
                (Expr::Tuple(items), Type::Tuple(types)) if items.len() == types.len() => {
                    work.extend(items.iter().copied().zip(types.iter().copied()).rev())
                }
                (Expr::Record(items), Type::Record(types)) => {
                    for (name, id) in items.iter().rev() {
                        if let Some(ty) = types.get(&name.value) {
                            work.push((*id, *ty));
                        }
                    }
                }
                (
                    Expr::List(items),
                    Type::Collection {
                        kind: CollectionKind::Sequence,
                        element,
                    },
                ) => work.extend(items.iter().rev().map(|id| (*id, *element))),
                _ => leaves.push((id, template)),
            }
        }
        Ok(leaves)
    }
    fn needs_context(&self, root: ExprId) -> Result<bool, String> {
        let mut work = vec![root];
        let mut budget = 16384usize;
        while let Some(id) = work.pop() {
            budget = budget
                .checked_sub(1)
                .ok_or("Generic inference work limit exceeded")?;
            match &self.doc.expressions[id.0].value {
                Expr::Literal(Literal::None) => return Ok(true),
                Expr::List(items) if items.is_empty() => return Ok(true),
                Expr::List(items) | Expr::Tuple(items) => work.extend(items),
                Expr::Record(fields) => work.extend(fields.iter().map(|(_, id)| *id)),
                Expr::Group(inner) => work.push(*inner),
                Expr::Lambda { parameters, body } => {
                    if parameters
                        .iter()
                        .any(|p| matches!(self.doc.types[p.ty.0].value, ast::Type::Inferred))
                    {
                        return Ok(true);
                    }
                    work.push(*body);
                }
                Expr::Block { result, .. } => work.push(*result),
                Expr::Match { arms, .. } => work.extend(arms.iter().map(|(_, body)| *body)),
                Expr::If {
                    then_branch,
                    else_branch,
                    ..
                } => work.extend([*then_branch, *else_branch]),
                Expr::Call {
                    callee,
                    types,
                    arguments,
                } if types.is_empty() => match self.target(*callee) {
                    _ if matches!(
                        self.doc.expressions[callee.0].value,
                        Expr::ComprehensionIntrinsic { .. }
                    ) =>
                    {
                        // Generated parameter types come from the source. Only
                        // source/yield expressions can need outside call evidence.
                        if let [source, callback] = arguments.as_slice() {
                            work.push(*source);
                            if let Expr::Lambda { body, .. } =
                                self.doc.expressions[callback.0].value
                            {
                                if matches!(
                                    self.doc.expressions[callee.0].value,
                                    Expr::ComprehensionIntrinsic { flat: true }
                                ) && let Expr::If { then_branch, .. } =
                                    self.doc.expressions[body.0].value
                                    && let Expr::List(items) =
                                        &self.doc.expressions[then_branch.0].value
                                {
                                    work.extend(items);
                                } else {
                                    work.push(body);
                                }
                            }
                        }
                    }
                    Some(Target::Builtin { name, .. })
                        if matches!(name.as_str(), "None" | "Ok" | "Err") =>
                    {
                        return Ok(true);
                    }
                    Some(Target::Builtin { name, .. }) if name == "Some" => work.extend(arguments),
                    Some(Target::Builtin { name, .. })
                        if Self::collection_kind(name).is_some() && arguments.is_empty() =>
                    {
                        return Ok(true);
                    }
                    _ if arguments.is_empty() && self.generic_signature(*callee).is_some() => {
                        return Ok(true);
                    }
                    _ => {}
                },
                Expr::Name(_) | Expr::Member { .. }
                    if self.generic_signature(id).is_some()
                        || self.has_overloads(id)
                        || matches!(self.target(id),Some(Target::Builtin{name,..}) if name=="None") =>
                {
                    return Ok(true);
                }
                _ => {}
            }
        }
        Ok(false)
    }
    fn generic_next(
        &mut self,
        id: ExprId,
        expected: Option<TypeRef>,
        mut call: GenericCall,
        work: &mut Vec<Work>,
    ) -> Result<(), String> {
        let complete = call.next == call.arguments.len();
        let values = call.inference.solve(
            &call.signature,
            self.table,
            self.models,
            &self.rigid_bounds,
            complete,
        )?;
        if complete {
            let function = inference::substitute(
                self.table,
                self.models,
                call.signature.function_type,
                &values,
            )?;
            self.report.expressions.insert(call.callee.0, function);
            let result =
                inference::substitute(self.table, self.models, call.signature.result, &values)?;
            self.finish(id, result, expected)?;
            return self.record_instantiation(call.callee, &call.signature, &values, function);
        }
        // Context-dependent arguments wait for independent sibling evidence.
        // Ordinary values must not be constrained to a provisional lower bound.
        let mut chosen = None;
        let mut candidates = (call.next..call.order.len())
            .map(|position| {
                let ordinal = call.order[position];
                Ok((
                    self.needs_context(call.arguments[ordinal])?,
                    position,
                    ordinal,
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        // Finish independent evidence before specializing a contextual callback.
        // This ordering is static checking only, not evaluation order.
        candidates.sort_by_key(|(context, position, _)| (*context, *position));
        if candidates.iter().all(|(context, _, _)| *context) {
            // Seed all independently checkable structural leaves before choosing
            // a complete context. A Left leaf must not prematurely fix T while
            // an unchecked Right leaf can establish their shared Base join.
            let unfixed = call
                .signature
                .generics
                .iter()
                .filter(|g| !call.inference.fixed.contains_key(&g.ty))
                .map(|g| g.ty)
                .collect();
            let mut context = call.inference.clone();
            context.relations.retain(|(_, _, outward)| *outward);
            let hints = context.solve(
                &call.signature,
                self.table,
                self.models,
                &self.rigid_bounds,
                false,
            )?;
            let missing = call
                .signature
                .generics
                .iter()
                .filter(|g| !hints.contains_key(&g.ty))
                .map(|g| g.ty)
                .collect();
            for (_, _, ordinal) in &candidates {
                for (leaf, template) in self.generic_leaves(
                    call.arguments[*ordinal],
                    call.signature.parameters[*ordinal],
                )? {
                    if call.seeded.contains(&leaf.0)
                        || self.needs_context(leaf)?
                        || !inference::contains(self.table, template, &unfixed)?
                    {
                        continue;
                    }
                    let ty = inference::substitute(self.table, self.models, template, &hints)?;
                    let hint = if inference::contains(self.table, ty, &missing)?
                        || !inference::covariant_hint(self.table, template, &unfixed)?
                    {
                        None
                    } else {
                        Some(ty)
                    };
                    call.seeded.insert(leaf.0);
                    work.push(Work::GenericSeed(
                        id,
                        expected,
                        Box::new(call),
                        leaf,
                        template,
                    ));
                    work.push(Work::Visit(leaf, hint));
                    return Ok(());
                }
            }
        }
        for (requires_context, position, ordinal) in candidates {
            let mut context = call.inference.clone();
            if !requires_context {
                context.relations.retain(|(_, _, outward)| *outward);
            }
            let hints = context.solve(
                &call.signature,
                self.table,
                self.models,
                &self.rigid_bounds,
                false,
            )?;
            let missing = call
                .signature
                .generics
                .iter()
                .filter(|g| !hints.contains_key(&g.ty))
                .map(|g| g.ty)
                .collect();
            let mut evidence = call.inference.clone();
            evidence.relations.retain(|(_, _, outward)| !*outward);
            let observed = if requires_context {
                evidence.solve(
                    &call.signature,
                    self.table,
                    self.models,
                    &self.rigid_bounds,
                    false,
                )?
            } else {
                BTreeMap::new()
            };
            let flexible = call
                .signature
                .generics
                .iter()
                .filter(|g| !context.fixed.contains_key(&g.ty) && !observed.contains_key(&g.ty))
                .map(|g| g.ty)
                .collect();
            let template = call.signature.parameters[ordinal];
            let parameter = inference::substitute(self.table, self.models, template, &hints)?;
            // A contextual generic callback is instantiated against this complete
            // monomorphic candidate and its own bounds are checked by visit.
            // It then contributes its concrete function type to the final global
            // variance constraints. Ordinary callbacks retain the polarity guard.
            let hint = if inference::contains(self.table, parameter, &missing)?
                || !inference::covariant_hint(self.table, template, &flexible)?
            {
                None
            } else {
                Some(parameter)
            };
            if !requires_context || hint.is_some() {
                chosen = Some((position, ordinal, hint));
                break;
            }
        }
        if chosen.is_none() {
            // A polymorphic callback can reveal its input type from a known
            // output (or another known structural component). Use only outward
            // context here; ordinary sibling evidence is processed above.
            let mut context = call.inference.clone();
            context.relations.retain(|(_, _, outward)| *outward);
            let hints = context.solve(
                &call.signature,
                self.table,
                self.models,
                &self.rigid_bounds,
                false,
            )?;
            let missing = call
                .signature
                .generics
                .iter()
                .filter(|g| !hints.contains_key(&g.ty))
                .map(|g| g.ty)
                .collect();
            for position in call.next..call.order.len() {
                let ordinal = call.order[position];
                let Some(signature) = self.generic_signature(call.arguments[ordinal]).cloned()
                else {
                    continue;
                };
                let parameter = inference::substitute(
                    self.table,
                    self.models,
                    call.signature.parameters[ordinal],
                    &hints,
                )?;
                let mut callback = Inference::with_shared_budget(self.shared_work.clone());
                callback.known_context(self.table, signature.function_type, parameter, &missing)?;
                let values = callback.solve(
                    &signature,
                    self.table,
                    self.models,
                    &self.rigid_bounds,
                    false,
                )?;
                if signature
                    .generics
                    .iter()
                    .any(|g| !values.contains_key(&g.ty))
                {
                    continue;
                }
                let values = callback.solve(
                    &signature,
                    self.table,
                    self.models,
                    &self.rigid_bounds,
                    true,
                )?;
                let function = inference::substitute(
                    self.table,
                    self.models,
                    signature.function_type,
                    &values,
                )?;
                chosen = Some((position, ordinal, Some(function)));
                break;
            }
        }
        let Some((position, ordinal, hint)) = chosen else {
            return Err("Generic arguments require additional type context".into());
        };
        call.order.swap(call.next, position);
        let argument = call.arguments[ordinal];
        call.next += 1;
        work.push(Work::GenericCollect(id, expected, Box::new(call)));
        work.push(Work::Visit(argument, hint));
        Ok(())
    }
    fn provider(
        &mut self,
        ordinal: usize,
        declaration: &Declaration,
        imports: &BTreeMap<String, ImportBinding>,
    ) -> Result<(), String> {
        let (target, receiver) = match declaration {
            Declaration::Derive {
                target, receiver, ..
            }
            | Declaration::Validate {
                target, receiver, ..
            } => (target, receiver),
            _ => return Err("Expected provider declaration".into()),
        };
        if target.len() != 3 {
            return Err("Provider target requires alias.Class.feature or alias.Class.rule".into());
        }
        let Some(ImportBinding::Model(package)) = imports.get(&target[0].value) else {
            return Err("Provider receiver requires a model import".into());
        };
        let class = self.models.classifier(package, &target[1].value)?;
        let receiver_type = self.table.intern(Type::Model(class.clone()))?;
        self.locals.clear();
        self.locals.insert(receiver.span.start, receiver_type);
        let feature = match declaration {
            Declaration::Derive { result, body, .. } => {
                let feature = self.models.feature(self.table, &class, &target[2].value)?;
                if !feature.derived {
                    return Err("Derivation requires a derived feature".into());
                }
                if !feature.assignment_exact {
                    return Err("Rcore derivation requires an exact result domain; an erased read type cannot certify a constrained feature value".into());
                }
                let result = self
                    .annotations
                    .get(&result.0)
                    .copied()
                    .ok_or("Missing derivation result type")?;
                if result != feature.value {
                    return Err(format!(
                        "Derivation result must match feature type and multiplicity: expected {:?}, got {:?}",
                        self.table.get(feature.value)?,
                        self.table.get(result)?
                    ));
                }
                self.walk(*body, Some(result));
                Some(feature)
            }
            Declaration::Validate { requirements, .. } => {
                let boolean = self.scalar(Scalar::Bool)?;
                let string = self.scalar(Scalar::String)?;
                for requirement in requirements {
                    self.walk(requirement.value.condition, Some(boolean));
                    self.walk(requirement.value.message, Some(string));
                }
                None
            }
            _ => return Err("Expected provider declaration".into()),
        };
        self.report.providers.insert(
            ordinal,
            ProviderType {
                receiver: receiver_type,
                feature,
            },
        );
        Ok(())
    }
    fn error(&mut self, id: ExprId, message: impl Into<String>) {
        self.errors.push(SyntaxDiagnostic {
            span: self.doc.expressions[id.0].span.clone(),
            message: message.into(),
        });
    }
    fn scalar(&mut self, scalar: Scalar) -> Result<TypeRef, String> {
        self.table.intern(Type::Scalar(scalar))
    }
    fn value(&self, id: ExprId) -> Result<TypeRef, String> {
        self.report
            .expressions
            .get(&id.0)
            .copied()
            .ok_or_else(|| "Child expression has no checked type".into())
    }
    /// The model class a `new` expression constructs.
    fn new_class(
        &self,
        id: ExprId,
    ) -> Result<crate::definitions::resources::DeclarationId, String> {
        match self.target(id) {
            Some(Target::Classifier(class)) => Ok(class.clone()),
            _ => Err("`new` requires a model class".into()),
        }
    }
    fn target_type(&self, id: ExprId) -> Option<TypeRef> {
        match self.target(id) {
            Some(Target::Local { declaration, .. }) => self.locals.get(&declaration.start).copied(),
            Some(Target::Symbol(symbol)) => self.functions.get(symbol).copied(),
            _ => None,
        }
    }
    fn numeric_hint(&self, expected: Option<TypeRef>) -> Option<TypeRef> {
        expected
            .and_then(|ty| self.rigid_view(ty).ok())
            .filter(|id| matches!(self.table.get(*id), Ok(Type::Scalar(s)) if s.numeric()))
    }
    fn finish(&mut self, id: ExprId, ty: TypeRef, expected: Option<TypeRef>) -> Result<(), String> {
        if let Some(expected) = expected
            && !inference::assignable(self.models, self.table, ty, expected, &self.rigid_bounds)?
        {
            return Err(format!(
                "Type mismatch: {:?} is not assignable to {:?}",
                self.table.get(ty)?,
                self.table.get(expected)?
            ));
        }
        self.report.expressions.insert(id.0, ty);
        Ok(())
    }
    fn walk(&mut self, root: ExprId, expected: Option<TypeRef>) {
        let mut work = vec![Work::Visit(root, expected)];
        while let Some(task) = work.pop() {
            if let Err(message) = self.charge_work(1) {
                self.error(root, message);
                return;
            }
            let (id, result) = match task {
                Work::ContextualBranch(branch, evidence) => {
                    let result = self
                        .value(evidence)
                        .map(|ty| work.push(Work::Visit(branch, Some(ty))));
                    (branch, result)
                }
                Work::GenericSeed(id, expected, mut call, leaf, template) => {
                    let result = (|| {
                        let actual = self.value(leaf)?;
                        call.inference.relations.push((template, actual, false));
                        work.push(Work::GenericNext(id, expected, call));
                        Ok(())
                    })();
                    (id, result)
                }
                Work::GenericNext(id, expected, call) => {
                    (id, self.generic_next(id, expected, *call, &mut work))
                }
                Work::GenericCollect(id, expected, mut call) => {
                    let result = (|| {
                        let ordinal = call.order[call.next - 1];
                        let actual = self.value(call.arguments[ordinal])?;
                        call.inference.relations.push((
                            call.signature.parameters[ordinal],
                            actual,
                            false,
                        ));
                        work.push(Work::GenericNext(id, expected, call));
                        Ok(())
                    })();
                    (id, result)
                }
                Work::CollectionRightOperand(id, expected, collection) => (
                    id,
                    self.collection_right_operand(id, expected, collection, &mut work),
                ),
                Work::CollectionCallback(id, expected, operation) => (
                    id,
                    self.collection_callback(id, expected, operation, &mut work),
                ),
                Work::CollectionConvert(id, expected, kind, element, argument) => {
                    let result = self
                        .collection_conversion(kind, element, argument)
                        .and_then(|ty| self.finish(id, ty, expected));
                    (id, result)
                }
                Work::MatchArms(id, expected) => (id, self.match_arms(id, expected, &mut work)),
                Work::Constructor(id, expected, result, arguments) => {
                    let result = (|| {
                        for argument in &arguments {
                            self.value(*argument)?;
                        }
                        let ty = match result {
                            Some(ty) => ty,
                            None => self.table.intern(Type::Option(self.value(
                                *arguments.first().ok_or("Missing constructor payload")?,
                            )?))?,
                        };
                        self.finish(id, ty, expected)
                    })();
                    (id, result)
                }
                Work::Visit(id, expected) => (id, self.visit(id, expected, &mut work)),
                Work::Finish(id, expected) => (
                    id,
                    self.result(id, expected)
                        .and_then(|ty| self.finish(id, ty, expected)),
                ),
                Work::BinaryRight(id, expected) => (id, self.binary_right(id, expected, &mut work)),
                Work::Arguments(id, expected) => (id, self.arguments(id, expected, &mut work)),
                Work::Local(start, value, annotation) => {
                    if let Ok(ty) = self.value(value) {
                        self.locals.insert(start, annotation.unwrap_or(ty));
                    }
                    continue;
                }
            };
            if let Err(message) = result {
                // A failed child already has a source-located diagnostic.
                if message != "Child expression has no checked type" {
                    let fatal = overloads::fatal(&message);
                    self.error(id, message);
                    if fatal {
                        return;
                    }
                }
            }
        }
    }
    fn visit(
        &mut self,
        id: ExprId,
        expected: Option<TypeRef>,
        work: &mut Vec<Work>,
    ) -> Result<(), String> {
        let expr = self.doc.expressions[id.0].value.clone();
        let candidate = match &expr {
            Expr::Call { callee, .. } => Some(*callee),
            Expr::Name(_) | Expr::Member { .. } | Expr::Group(_) => Some(id),
            _ => None,
        };
        if let Some(candidate) = candidate
            && self.has_overloads(candidate)
        {
            return self.resolve_overload(id, candidate, expected);
        }
        // Resolve a deferred callback against its enclosing constraints. Each
        // candidate must type-check the entire call, including empty siblings;
        // selecting just the callback would discard those constraints.
        if let Expr::Call {
            callee, arguments, ..
        } = &expr
        {
            let collection = matches!(self.target(*callee),
                Some(Target::Builtin { name, .. }) if CollectionOp::named(name).is_some());
            let candidate = if collection {
                arguments
                    .last()
                    .copied()
                    .filter(|id| self.has_overloads(*id))
            } else if self.generic_signature(*callee).is_some() {
                arguments.iter().copied().find(|id| self.has_overloads(*id))
            } else {
                None
            };
            if let Some(callback) = candidate {
                return self.resolve_overload(id, callback, expected);
            }
        }
        if let Some(Target::EnumLiteral {
            enumeration,
            literal,
        }) = self.target(id).cloned()
        {
            self.models
                .enumeration(&enumeration)?
                .by_id(&literal)
                .ok_or("Unknown enum literal identity")?;
            let ty = self.table.intern(Type::Enumeration(enumeration.clone()))?;
            return self.finish(id, ty, expected);
        }
        if matches!(&expr, Expr::Name(_) | Expr::Member { .. })
            && let Some(signature) = self.generic_signature(id)
        {
            if self.trial_depth != 0 {
                self.charge_snapshot(signature)?;
            }
            let signature = signature.clone();
            let expected = expected
                .ok_or("Generic function value requires a monomorphic contextual function type")?;
            if !matches!(self.table.get(expected)?, Type::Function { .. }) {
                return Err(
                    "Generic function value requires a monomorphic contextual function type".into(),
                );
            }
            let mut state = Inference::with_shared_budget(self.shared_work.clone());
            state
                .relations
                .push((signature.function_type, expected, true));
            let values = state.solve(
                &signature,
                self.table,
                self.models,
                &self.rigid_bounds,
                true,
            )?;
            let ty =
                inference::substitute(self.table, self.models, signature.function_type, &values)?;
            self.finish(id, ty, Some(expected))?;
            return self.record_instantiation(id, &signature, &values, ty);
        }
        match expr {
            Expr::Literal(literal) => {
                let ty = self.literal(&literal, expected)?;
                self.finish(id, ty, expected)
            }
            Expr::Tuple(items) if items.is_empty() => {
                let ty = self.scalar(Scalar::Unit)?;
                self.finish(id, ty, expected)
            }
            Expr::Name(_) | Expr::ComprehensionIntrinsic { .. } => {
                if matches!(self.target(id), Some(Target::Builtin { name, .. }) if name == "None") {
                    let ty = self.literal(&Literal::None, expected)?;
                    return self.finish(id, ty, expected);
                }
                let ty = self
                    .target_type(id)
                    .ok_or("Name does not identify a typed runtime value")?;
                self.finish(id, ty, expected)
            }
            Expr::Group(child) => {
                work.extend([Work::Finish(id, expected), Work::Visit(child, expected)]);
                Ok(())
            }
            Expr::Unary { operator, operand } => {
                if matches!(operator.value.as_str(), "+" | "-") {
                    let mut literal_operand = operand;
                    while let Expr::Group(inner) = &self.doc.expressions[literal_operand.0].value {
                        literal_operand = *inner;
                    }
                    if let Expr::Literal(Literal::Number(number)) =
                        &self.doc.expressions[literal_operand.0].value
                    {
                        let text = if operator.value == "-" {
                            format!("-{number}")
                        } else {
                            number.clone()
                        };
                        let ty = self.literal(&Literal::Number(text), expected)?;
                        return self.finish(id, ty, expected);
                    }
                }
                let hint = if operator.value == "!" {
                    Some(self.scalar(Scalar::Bool)?)
                } else {
                    self.numeric_hint(expected)
                };
                work.extend([Work::Finish(id, expected), Work::Visit(operand, hint)]);
                Ok(())
            }
            Expr::Binary {
                operator,
                left,
                right,
            } => {
                let hint = if matches!(operator.value.as_str(), "&&" | "||") {
                    Some(self.scalar(Scalar::Bool)?)
                } else if operator.value == "??" {
                    None
                } else {
                    self.numeric_hint(expected)
                        .or_else(|| self.numeric_hint(self.target_type(right)))
                };
                work.extend([Work::BinaryRight(id, expected), Work::Visit(left, hint)]);
                Ok(())
            }
            Expr::Call { callee, types, .. } => {
                if let Some(Target::Builtin { name, .. }) = self.target(callee) {
                    let name = name.clone();
                    return self.constructor(id, expected, &name, work);
                }
                if let Some(signature) = self.generic_signature(callee) {
                    if self.trial_depth != 0 {
                        self.charge_snapshot(signature)?;
                    }
                    let signature = signature.clone();
                    return self.generic_call(id, expected, signature, work);
                }
                if !types.is_empty() {
                    return Err(
                        "Explicit generic calls require a supported constructor or S04/E07".into(),
                    );
                }
                work.extend([Work::Arguments(id, expected), Work::Visit(callee, None)]);
                Ok(())
            }
            Expr::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let bool_ty = self.scalar(Scalar::Bool)?;
                let then_context = self.needs_context(then_branch)?;
                let else_context = self.needs_context(else_branch)?;
                if expected.is_none() && then_context != else_context {
                    let (contextual, evidence) = if then_context {
                        (then_branch, else_branch)
                    } else {
                        (else_branch, then_branch)
                    };
                    work.extend([
                        Work::Finish(id, expected),
                        Work::ContextualBranch(contextual, evidence),
                        Work::Visit(evidence, None),
                        Work::Visit(condition, Some(bool_ty)),
                    ]);
                } else {
                    work.extend([
                        Work::Finish(id, expected),
                        Work::Visit(else_branch, expected),
                        Work::Visit(then_branch, expected),
                        Work::Visit(condition, Some(bool_ty)),
                    ]);
                }
                Ok(())
            }
            Expr::Block { bindings, result } => {
                work.extend([Work::Finish(id, expected), Work::Visit(result, expected)]);
                for binding in bindings.iter().rev() {
                    let binding = &binding.value;
                    let annotation = binding
                        .ty
                        .and_then(|id| self.annotations.get(&id.0).copied());
                    work.extend([
                        Work::Local(binding.name.span.start, binding.value, annotation),
                        Work::Visit(binding.value, annotation),
                    ]);
                }
                Ok(())
            }
            Expr::Lambda { parameters, body } => {
                let contextual = expected.and_then(|id| match self.table.get(id) {
                    Ok(Type::Function { parameters, .. }) => Some(parameters.clone()),
                    _ => None,
                });
                if contextual
                    .as_ref()
                    .is_some_and(|types| types.len() != parameters.len())
                {
                    return Err("Lambda parameter arity differs from context".into());
                }
                for (ordinal, parameter) in parameters.iter().enumerate() {
                    let ty = if matches!(self.doc.types[parameter.ty.0].value, ast::Type::Inferred)
                    {
                        contextual
                            .as_ref()
                            .and_then(|types| types.get(ordinal))
                            .copied()
                            .or_else(|| self.locals.get(&parameter.name.span.start).copied())
                            .ok_or("Lambda parameter needs a type annotation or checked context")?
                    } else {
                        *self
                            .annotations
                            .get(&parameter.ty.0)
                            .ok_or("Missing lambda parameter type")?
                    };
                    self.locals.insert(parameter.name.span.start, ty);
                }
                let hint = expected.and_then(|id| match self.table.get(id) {
                    Ok(Type::Function { result, .. }) => Some(*result),
                    _ => None,
                });
                work.extend([Work::Finish(id, expected), Work::Visit(body, hint)]);
                Ok(())
            }
            Expr::Member { receiver, .. } => {
                if let Some(ty) = self.target_type(id) {
                    return self.finish(id, ty, expected);
                }
                work.extend([Work::Finish(id, expected), Work::Visit(receiver, None)]);
                Ok(())
            }
            Expr::Match { value, .. } => {
                work.extend([Work::MatchArms(id, expected), Work::Visit(value, None)]);
                Ok(())
            }
            Expr::List(items) => {
                let hint = match expected.map(|ty| self.table.get(ty)).transpose()? {
                    Some(Type::Collection {
                        kind: CollectionKind::Sequence,
                        element,
                    }) => Some(*element),
                    _ => None,
                };
                work.push(Work::Finish(id, expected));
                work.extend(items.into_iter().rev().map(|item| Work::Visit(item, hint)));
                Ok(())
            }
            Expr::Tuple(items) => {
                let hints = match expected.map(|ty| self.table.get(ty)).transpose()? {
                    Some(Type::Tuple(types)) if types.len() == items.len() => types.clone(),
                    _ => vec![],
                };
                work.push(Work::Finish(id, expected));
                work.extend(
                    items
                        .into_iter()
                        .enumerate()
                        .rev()
                        .map(|(index, item)| Work::Visit(item, hints.get(index).copied())),
                );
                Ok(())
            }
            Expr::New { fields, .. } => {
                let class = self.new_class(id)?;
                let mut names = std::collections::BTreeSet::new();
                if fields
                    .iter()
                    .any(|(name, _)| !names.insert(name.value.clone()))
                {
                    return Err("Duplicate feature in `new`".into());
                }
                let mut hints = Vec::with_capacity(fields.len());
                for (name, _) in &fields {
                    // An optional feature takes a plain value or an Option, so it
                    // gives no expected type; collections keep theirs.
                    let hint = match self.models.feature(self.table, &class, &name.value) {
                        Ok(feature) => match self.table.get(feature.value)? {
                            Type::Option(_) => None,
                            Type::Many { element, .. } => {
                                let element = *element;
                                Some(self.table.intern(Type::Collection {
                                    kind: CollectionKind::Sequence,
                                    element,
                                })?)
                            }
                            _ => Some(feature.value),
                        },
                        Err(_) => None,
                    };
                    hints.push(hint);
                }
                work.push(Work::Finish(id, expected));
                work.extend(
                    fields
                        .into_iter()
                        .zip(hints)
                        .rev()
                        .map(|((_, item), hint)| Work::Visit(item, hint)),
                );
                Ok(())
            }
            Expr::Record(fields) => {
                let hints = match expected.map(|ty| self.table.get(ty)).transpose()? {
                    Some(Type::Record(types)) => types.clone(),
                    _ => BTreeMap::new(),
                };
                let mut names = std::collections::BTreeSet::new();
                if fields
                    .iter()
                    .any(|(name, _)| !names.insert(name.value.clone()))
                {
                    return Err("Duplicate record field".into());
                }
                work.push(Work::Finish(id, expected));
                work.extend(
                    fields
                        .into_iter()
                        .rev()
                        .map(|(name, item)| Work::Visit(item, hints.get(&name.value).copied())),
                );
                Ok(())
            }
            Expr::Index { receiver, index } => {
                let integer = self.scalar(Scalar::Int64)?;
                work.extend([
                    Work::Finish(id, expected),
                    Work::Visit(index, Some(integer)),
                    Work::Visit(receiver, None),
                ]);
                Ok(())
            }
            Expr::Error => Err("Malformed expression cannot be typed".into()),
        }
    }
    fn match_arms(
        &mut self,
        id: ExprId,
        expected: Option<TypeRef>,
        work: &mut Vec<Work>,
    ) -> Result<(), String> {
        let Expr::Match { value, arms } = self.doc.expressions[id.0].value.clone() else {
            return Err("Expected match".into());
        };
        let rigid = self.value(value)?;
        let mut coverage =
            super::patterns::Patterns::new(rigid, self.table, self.models, &self.rigid_bounds)?;
        for (pattern, _) in &arms {
            match coverage.arm(self.doc, self.bindings, self.table, self.models, *pattern) {
                Ok(mut locals) => {
                    if let ast::Pattern::Binding(name) = &self.doc.patterns[pattern.0].value {
                        for (start, ty) in &mut locals {
                            if *start == name.span.start {
                                *ty = rigid;
                            }
                        }
                    }
                    self.locals.extend(locals);
                }
                Err(message) => self.errors.push(SyntaxDiagnostic {
                    span: self.doc.patterns[pattern.0].span.clone(),
                    message,
                }),
            }
        }
        if !coverage.exhaustive() {
            self.error(
                id,
                "Non-exhaustive match: uncovered values require another arm",
            );
        }
        work.push(Work::Finish(id, expected));
        work.extend(
            arms.into_iter()
                .rev()
                .map(|(_, body)| Work::Visit(body, expected)),
        );
        Ok(())
    }
    fn constructor(
        &mut self,
        id: ExprId,
        expected: Option<TypeRef>,
        name: &str,
        work: &mut Vec<Work>,
    ) -> Result<(), String> {
        if super::ir_v2::OwnershipOp::named(name).is_some() {
            let Expr::Call {
                types, arguments, ..
            } = &self.doc.expressions[id.0].value
            else {
                return Err("Expected ownership call".into());
            };
            if types.len() != 1 || arguments.len() != 1 {
                return Err(format!("{name} requires one type and one object"));
            }
            work.extend([Work::Finish(id, expected), Work::Visit(arguments[0], None)]);
            return Ok(());
        }
        if name == "incoming" {
            let Expr::Call {
                types, arguments, ..
            } = self.doc.expressions[id.0].value.clone()
            else {
                return Err("Expected incoming call".into());
            };
            if types.len() != 1 || arguments.len() != 2 {
                return Err("incoming requires one source type and two arguments".into());
            }
            if !matches!(
                self.doc.expressions[arguments[1].0].value,
                Expr::Literal(Literal::String(_))
            ) {
                return Err("incoming feature selector must be a string literal".into());
            }
            work.push(Work::Finish(id, expected));
            let string = self.scalar(Scalar::String)?;
            work.push(Work::Visit(arguments[1], Some(string)));
            work.push(Work::Visit(arguments[0], None));
            return Ok(());
        }
        if let Some(operation) = AlgebraOp::named(name) {
            let Expr::Call {
                types, arguments, ..
            } = self.doc.expressions[id.0].value.clone()
            else {
                return Err("Expected collection algebra call".into());
            };
            let arity = if operation == AlgebraOp::Distinct {
                1
            } else {
                2
            };
            if !types.is_empty() || arguments.len() != arity {
                return Err(format!(
                    "{name} requires {arity} arguments and no explicit type arguments"
                ));
            }
            if arity == 1 {
                work.push(Work::Finish(id, expected));
            } else {
                work.push(Work::CollectionRightOperand(id, expected, true));
            }
            work.push(Work::Visit(arguments[0], None));
            return Ok(());
        }
        if let Some(operation) = MembershipOp::named(name) {
            let Expr::Call {
                types, arguments, ..
            } = self.doc.expressions[id.0].value.clone()
            else {
                return Err("Expected membership call".into());
            };
            if !types.is_empty() || arguments.len() != 2 {
                return Err(format!(
                    "{name} requires two arguments and no explicit type arguments"
                ));
            }
            work.push(Work::CollectionRightOperand(
                id,
                expected,
                operation.multiple(),
            ));
            work.push(Work::Visit(arguments[0], None));
            return Ok(());
        }
        if let Some(operation) = CollectionOp::named(name) {
            let Expr::Call {
                types, arguments, ..
            } = self.doc.expressions[id.0].value.clone()
            else {
                return Err("Expected collection operation".into());
            };
            let arity = if operation == CollectionOp::Fold {
                3
            } else {
                2
            };
            if !types.is_empty() || arguments.len() != arity {
                return Err(format!(
                    "{name} requires {arity} values and no explicit type arguments"
                ));
            }
            let input_hint = self.collection_input_hint(
                arguments[0],
                arguments[arity - 1],
                operation,
                expected,
            )?;
            work.push(Work::CollectionCallback(id, expected, operation));
            if operation == CollectionOp::Fold {
                work.push(Work::Visit(arguments[1], expected));
            }
            work.push(Work::Visit(arguments[0], input_hint));
            return Ok(());
        }
        if matches!(name, "flatten" | "first" | "last" | "at") {
            let Expr::Call {
                types, arguments, ..
            } = self.doc.expressions[id.0].value.clone()
            else {
                return Err("Expected collection operation".into());
            };
            let arity = if name == "at" { 2 } else { 1 };
            if !types.is_empty() || arguments.len() != arity {
                return Err(format!(
                    "{name} requires {arity} values and no explicit type arguments"
                ));
            }
            work.push(Work::Finish(id, expected));
            if name == "at" {
                let integer = self.scalar(Scalar::Int64)?;
                work.push(Work::Visit(arguments[1], Some(integer)));
            }
            work.push(Work::Visit(arguments[0], None));
            return Ok(());
        }
        if let Some(op) = TextOp::named(name) {
            let Expr::Call {
                types, arguments, ..
            } = self.doc.expressions[id.0].value.clone()
            else {
                return Err("Expected text primitive call".into());
            };
            if !types.is_empty() || arguments.len() != 1 {
                return Err(format!("{name} requires one value and no type arguments"));
            }
            let expected_argument = if op.reads_model() {
                None
            } else {
                Some(self.scalar(Scalar::String)?)
            };
            work.push(Work::Finish(id, expected));
            work.push(Work::Visit(arguments[0], expected_argument));
            return Ok(());
        }
        if name == "split" {
            let Expr::Call {
                types, arguments, ..
            } = self.doc.expressions[id.0].value.clone()
            else {
                return Err("Expected split call".into());
            };
            if !types.is_empty() || arguments.len() != 2 {
                return Err("split requires a String and a String separator".into());
            }
            let string = self.scalar(Scalar::String)?;
            work.push(Work::Finish(id, expected));
            work.push(Work::Visit(arguments[1], Some(string)));
            work.push(Work::Visit(arguments[0], Some(string)));
            return Ok(());
        }
        if matches!(name, "size" | "isEmpty") {
            let Expr::Call {
                types, arguments, ..
            } = &self.doc.expressions[id.0].value
            else {
                return Err("Expected collection measure call".into());
            };
            if !types.is_empty() || arguments.len() != 1 {
                return Err(format!(
                    "{name} requires one collection and no type arguments"
                ));
            }
            work.extend([Work::Finish(id, expected), Work::Visit(arguments[0], None)]);
            return Ok(());
        }
        if let Some(kind) = Self::collection_kind(name) {
            return self.collection_constructor(id, expected, kind, work);
        }
        if !matches!(name, "Some" | "None" | "Ok" | "Err") {
            return Err(format!(
                "Builtin {name} requires later query/collection typing"
            ));
        }
        let Expr::Call {
            types, arguments, ..
        } = self.doc.expressions[id.0].value.clone()
        else {
            return Err("Expected constructor call".into());
        };
        let expected_arity = usize::from(name != "None");
        if arguments.len() != expected_arity {
            return Err(format!("{name} requires {expected_arity} arguments"));
        }
        let result = if types.is_empty() {
            expected
        } else {
            let required = if matches!(name, "Some" | "None") {
                1
            } else {
                2
            };
            if types.len() != required {
                return Err(format!(
                    "{name} requires {required} explicit type arguments"
                ));
            }
            let types = types
                .iter()
                .map(|id| {
                    self.annotations
                        .get(&id.0)
                        .copied()
                        .ok_or_else(|| "Missing constructor type argument".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?;
            Some(self.table.intern(if required == 1 {
                Type::Option(types[0])
            } else {
                Type::Result {
                    ok: types[0],
                    error: types[1],
                }
            })?)
        };
        let payload = match (name, result.map(|id| self.table.get(id)).transpose()?) {
            ("Some" | "None", Some(Type::Option(inner))) => Some(*inner),
            ("Ok", Some(Type::Result { ok, .. })) => Some(*ok),
            ("Err", Some(Type::Result { error, .. })) => Some(*error),
            ("Some", None) => None,
            (_, None) => {
                return Err(format!(
                    "{name} requires an expected type or explicit type arguments"
                ));
            }
            _ => return Err(format!("{name} has an incompatible expected type")),
        };
        work.push(Work::Constructor(id, expected, result, arguments.clone()));
        work.extend(
            arguments
                .into_iter()
                .rev()
                .map(|id| Work::Visit(id, payload)),
        );
        Ok(())
    }
    fn collection_kind(name: &str) -> Option<CollectionKind> {
        Some(match name {
            "Sequence" => CollectionKind::Sequence,
            "Set" => CollectionKind::Set,
            "OrderedSet" => CollectionKind::OrderedSet,
            "Bag" => CollectionKind::Bag,
            _ => return None,
        })
    }
    fn collection_constructor(
        &mut self,
        id: ExprId,
        expected: Option<TypeRef>,
        kind: CollectionKind,
        work: &mut Vec<Work>,
    ) -> Result<(), String> {
        let Expr::Call {
            types, arguments, ..
        } = self.doc.expressions[id.0].value.clone()
        else {
            return Err("Expected collection conversion".into());
        };
        if arguments.len() != 1 || types.len() > 1 {
            return Err(
                "Collection conversion requires one value and at most one element type argument"
                    .into(),
            );
        }
        let element = if let Some(ty) = types.first() {
            Some(
                *self
                    .annotations
                    .get(&ty.0)
                    .ok_or("Missing collection element type")?,
            )
        } else {
            match expected.map(|ty| self.table.get(ty)).transpose()? {
                Some(Type::Collection {
                    kind: target,
                    element,
                }) if *target == kind => Some(*element),
                _ => None,
            }
        };
        let argument = arguments[0];
        let mut inner = argument;
        while let Expr::Group(next) = self.doc.expressions[inner.0].value {
            inner = next;
        }
        let hint = if matches!(self.doc.expressions[inner.0].value, Expr::List(_)) {
            element
                .map(|element| {
                    self.table.intern(Type::Collection {
                        kind: CollectionKind::Sequence,
                        element,
                    })
                })
                .transpose()?
        } else {
            None
        };
        work.extend([
            Work::CollectionConvert(id, expected, kind, element, argument),
            Work::Visit(argument, hint),
        ]);
        Ok(())
    }
    fn collection_parts(&self, ty: TypeRef) -> Result<(CollectionKind, TypeRef), String> {
        let ty = self.rigid_view(ty)?;
        match self.table.get(ty)? {
            Type::Collection { kind, element } => Ok((kind.clone(), *element)),
            Type::Many {
                element,
                ordered,
                unique,
                ..
            } => Ok((
                match (*ordered, *unique) {
                    (true, false) => CollectionKind::Sequence,
                    (true, true) => CollectionKind::OrderedSet,
                    (false, false) => CollectionKind::Bag,
                    (false, true) => CollectionKind::Set,
                },
                *element,
            )),
            _ => Err("Collection conversion or indexing requires a collection".into()),
        }
    }
    fn collection_conversion(
        &mut self,
        kind: CollectionKind,
        element: Option<TypeRef>,
        argument: ExprId,
    ) -> Result<TypeRef, String> {
        let (source, actual) = self.collection_parts(self.value(argument)?)?;
        if matches!(source, CollectionKind::Set | CollectionKind::Bag)
            && matches!(kind, CollectionKind::Sequence | CollectionKind::OrderedSet)
        {
            return Err("Unordered collection cannot supply positional order; use an explicit ordering operation".into());
        }
        let element = element.unwrap_or(actual);
        if !inference::assignable(self.models, self.table, actual, element, &self.rigid_bounds)? {
            return Err("Collection conversion has incompatible element types".into());
        }
        self.table.intern(Type::Collection { kind, element })
    }
    fn index_type(&mut self, receiver: ExprId, index: ExprId) -> Result<TypeRef, String> {
        self.value(index)?;
        let receiver = self.rigid_view(self.value(receiver)?)?;
        if let Type::Tuple(items) = self.table.get(receiver)? {
            let mut inner = index;
            while let Expr::Group(next) = self.doc.expressions[inner.0].value {
                inner = next;
            }
            let Expr::Literal(Literal::Number(text)) = &self.doc.expressions[inner.0].value else {
                return Err("Tuple index must be a nonnegative integer literal".into());
            };
            let ordinal = text
                .parse::<usize>()
                .map_err(|_| "Tuple index must be a nonnegative integer literal")?;
            return items
                .get(ordinal)
                .copied()
                .ok_or_else(|| "Tuple index is outside its arity".into());
        }
        let (kind, element) = self.collection_parts(receiver)?;
        if !matches!(kind, CollectionKind::Sequence | CollectionKind::OrderedSet) {
            return Err("Positional indexing requires an ordered collection".into());
        }
        self.table.intern(Type::Option(element))
    }
    fn literal(&mut self, literal: &Literal, expected: Option<TypeRef>) -> Result<TypeRef, String> {
        let scalar = match literal {
            Literal::Number(text) => match self.numeric_hint(expected) {
                Some(id) => {
                    if let Type::Scalar(s) = self.table.get(id)? {
                        *s
                    } else {
                        return Err("Expected numeric scalar".into());
                    }
                }
                None => {
                    if text.contains(['.', 'e', 'E']) {
                        Scalar::Decimal
                    } else {
                        Scalar::Int64
                    }
                }
            },
            Literal::Boolean(_) => Scalar::Bool,
            Literal::String(_) => Scalar::String,
            Literal::None => {
                return expected
                    .filter(|id| matches!(self.table.get(*id), Ok(Type::Option(_))))
                    .ok_or_else(|| "None requires an expected Option type".into());
            }
        };
        scalar.check_literal(literal)?;
        self.scalar(scalar)
    }
    fn binary_right(
        &mut self,
        id: ExprId,
        expected: Option<TypeRef>,
        work: &mut Vec<Work>,
    ) -> Result<(), String> {
        let Expr::Binary {
            operator,
            left,
            right,
        } = &self.doc.expressions[id.0].value
        else {
            return Err("Expected binary expression".into());
        };
        let left_ty = self.rigid_view(self.value(*left)?)?;
        let hint = if operator.value == "??" {
            match self.table.get(left_ty)? {
                Type::Option(inner) => Some(*inner),
                _ => return Err("Coalesce requires an Option receiver".into()),
            }
        } else if matches!(self.table.get(left_ty)?, Type::Scalar(_)) {
            Some(left_ty)
        } else {
            None
        };
        work.extend([Work::Finish(id, expected), Work::Visit(*right, hint)]);
        Ok(())
    }
    fn arguments(
        &mut self,
        id: ExprId,
        expected: Option<TypeRef>,
        work: &mut Vec<Work>,
    ) -> Result<(), String> {
        let Expr::Call {
            callee, arguments, ..
        } = &self.doc.expressions[id.0].value
        else {
            return Err("Expected call".into());
        };
        let Type::Function { parameters, .. } =
            self.table.get(self.rigid_view(self.value(*callee)?)?)?
        else {
            return Err("Callee is not a function".into());
        };
        if arguments.len() != parameters.len() {
            return Err(format!(
                "Call requires {} arguments, got {}",
                parameters.len(),
                arguments.len()
            ));
        }
        work.push(Work::Finish(id, expected));
        work.extend(
            arguments
                .iter()
                .zip(parameters)
                .rev()
                .map(|(argument, ty)| Work::Visit(*argument, Some(*ty))),
        );
        Ok(())
    }
    fn result(&mut self, id: ExprId, expected: Option<TypeRef>) -> Result<TypeRef, String> {
        match self.doc.expressions[id.0].value.clone() {
            Expr::Match { value, arms } => {
                self.value(value)?;
                let mut joined = None;
                for (_, body) in arms {
                    let ty = self.value(body)?;
                    joined = Some(match (expected, joined) {
                        (Some(expected), _) => expected,
                        (None, Some(previous)) => self.join(previous, ty)?,
                        (None, None) => ty,
                    });
                }
                joined
                    .or(expected)
                    .ok_or_else(|| "Match has no result arms or expected result type".into())
            }
            Expr::List(items) => {
                let mut element = match expected.map(|ty| self.table.get(ty)).transpose()? {
                    Some(Type::Collection {
                        kind: CollectionKind::Sequence,
                        element,
                    }) => Some(*element),
                    _ => None,
                };
                for item in items {
                    let value = self.value(item)?;
                    element = Some(match element {
                        Some(previous) => self.join(previous, value)?,
                        None => value,
                    });
                }
                let element = element.ok_or("Empty list requires a contextual element type")?;
                self.table.intern(Type::Collection {
                    kind: CollectionKind::Sequence,
                    element,
                })
            }
            Expr::Tuple(items) => {
                let items = items
                    .into_iter()
                    .map(|item| self.value(item))
                    .collect::<Result<Vec<_>, _>>()?;
                self.table.intern(Type::Tuple(items))
            }
            Expr::New { fields, .. } => {
                let class = self.new_class(id)?;
                let mut features = Vec::with_capacity(fields.len());
                for (name, item) in fields {
                    let feature = self.models.feature(self.table, &class, &name.value)?;
                    if feature.derived || !feature.changeable {
                        return Err(format!(
                            "`new` sets only stored, changeable features; {} is not",
                            name.value
                        ));
                    }
                    let value = self.value(item)?;
                    // An optional feature also takes a plain value; a many-valued
                    // feature takes a Sequence of its element type.
                    let (target, optional) = match self.table.get(feature.value)?.clone() {
                        Type::Option(inner) => (feature.value, Some(inner)),
                        Type::Many { element, .. } => (
                            self.table.intern(Type::Collection {
                                kind: CollectionKind::Sequence,
                                element,
                            })?,
                            None,
                        ),
                        _ => (feature.value, None),
                    };
                    let fits = inference::assignable(
                        self.models,
                        self.table,
                        value,
                        target,
                        &self.rigid_bounds,
                    )? || match optional {
                        Some(inner) => inference::assignable(
                            self.models,
                            self.table,
                            value,
                            inner,
                            &self.rigid_bounds,
                        )?,
                        None => false,
                    };
                    if !fits {
                        return Err(format!("`new` value does not match feature {}", name.value));
                    }
                    features.push((name.value, feature));
                }
                self.report.constructions.insert(
                    id.0,
                    Construction {
                        class: class.clone(),
                        features,
                    },
                );
                self.table.intern(Type::Model(class))
            }
            Expr::Record(fields) => {
                let fields = fields
                    .into_iter()
                    .map(|(name, item)| Ok((name.value, self.value(item)?)))
                    .collect::<Result<BTreeMap<_, _>, String>>()?;
                self.table.intern(Type::Record(fields))
            }
            Expr::Index { receiver, index } => self.index_type(receiver, index),
            Expr::Group(inner) => self.value(inner),
            Expr::Block { result, .. } => self.value(result),
            Expr::Unary { operator, operand } => {
                let ty = self.rigid_view(self.value(operand)?)?;
                let Type::Scalar(scalar) = self.table.get(ty)? else {
                    return Err("Unary operator requires a scalar".into());
                };
                if (operator.value == "!" && *scalar == Scalar::Bool)
                    || (matches!(operator.value.as_str(), "+" | "-") && scalar.numeric())
                {
                    Ok(ty)
                } else {
                    Err("Unary operator has an incompatible operand".into())
                }
            }
            Expr::Binary {
                operator,
                left,
                right,
            } => self.binary(&operator.value, left, right),
            Expr::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.value(condition)?;
                let left = self.value(then_branch)?;
                let right = self.value(else_branch)?;
                if let Some(expected) = expected {
                    Ok(expected)
                } else {
                    self.join(left, right)
                }
            }
            Expr::Call {
                callee,
                arguments,
                types,
            } => {
                if let Some(Target::Builtin { name, .. }) = self.target(callee)
                    && let Some(operation) = super::ir_v2::OwnershipOp::named(name)
                {
                    let selected = *self
                        .annotations
                        .get(&types[0].0)
                        .ok_or("Missing ownership type")?;
                    let receiver = self.value(arguments[0])?;
                    for ty in [selected, receiver] {
                        if !matches!(
                            self.table.get(self.rigid_view(ty)?)?,
                            Type::Model(_)
                                | Type::ModelApplication(_)
                                | Type::SymbolicModelApplication { .. }
                        ) {
                            return Err("Ownership navigation requires model-bounded types".into());
                        }
                    }
                    return self.table.intern(if operation.singular() {
                        Type::Option(selected)
                    } else {
                        Type::Collection {
                            kind: CollectionKind::OrderedSet,
                            element: selected,
                        }
                    });
                }
                if let Some(Target::Builtin { name, .. }) = self.target(callee)
                    && name == "incoming"
                {
                    let source = *self
                        .annotations
                        .get(&types[0].0)
                        .ok_or("Missing incoming source type")?;
                    let target = self.value(arguments[0])?;
                    let Expr::Literal(Literal::String(field)) =
                        &self.doc.expressions[arguments[1].0].value
                    else {
                        return Err("incoming feature selector must be a string literal".into());
                    };
                    let source_bound = self.rigid_view(source)?;
                    let target_bound = self.rigid_view(target)?;
                    if matches!(
                        self.table.get(source_bound)?,
                        Type::SymbolicModelApplication { .. }
                    ) {
                        let feature =
                            self.models
                                .symbolic_feature(self.table, source_bound, field)?;
                        if feature.derived {
                            return Err(
                                "Derived model navigation requires an executable provider".into()
                            );
                        }
                        if !feature.reference {
                            return Err("incoming requires a stored reference member".into());
                        }
                        let element = match self.table.get(feature.value)? {
                            Type::Option(inner) | Type::Many { element: inner, .. } => *inner,
                            _ => feature.value,
                        };
                        if !inference::assignable(
                            self.models,
                            self.table,
                            target,
                            element,
                            &self.rigid_bounds,
                        )? {
                            return Err(
                                "incoming target does not conform to the reference domain".into()
                            );
                        }
                    } else {
                        self.models.incoming_feature(
                            self.table,
                            source_bound,
                            target_bound,
                            field,
                        )?;
                    }
                    return self.table.intern(Type::Collection {
                        kind: CollectionKind::OrderedSet,
                        element: source,
                    });
                }
                if let Some(Target::Builtin { name, .. }) = self.target(callee)
                    && let Some(operation) = AlgebraOp::named(name)
                {
                    return self.algebra_result(operation, &arguments);
                }
                if let Some(Target::Builtin { name, .. }) = self.target(callee)
                    && let Some(operation) = MembershipOp::named(name)
                {
                    let (_, element) = self.collection_parts(self.value(arguments[0])?)?;
                    let other = self.value(arguments[1])?;
                    let other = if operation.multiple() {
                        self.collection_parts(other)?.1
                    } else {
                        other
                    };
                    self.join(element, other)?;
                    return self.scalar(if operation == MembershipOp::Count {
                        Scalar::Int64
                    } else {
                        Scalar::Bool
                    });
                }
                if let Some(Target::Builtin { name, .. }) = self.target(callee)
                    && let Some(operation) = CollectionOp::named(name)
                {
                    return self.collection_result(operation, &arguments);
                }
                if let Some(Target::Builtin { name, .. }) = self.target(callee)
                    && let Some(op) = TextOp::named(name)
                {
                    let actual = self.value(*arguments.first().ok_or("Missing operand")?)?;
                    let shape = self.table.get(self.rigid_view(actual)?)?;
                    let valid = if op.reads_model() {
                        matches!(shape, Type::Model(_) | Type::ModelApplication(_))
                    } else {
                        matches!(shape, Type::Scalar(Scalar::String))
                    };
                    if !valid {
                        return Err(if op.reads_model() {
                            format!("{name} requires a model object")
                        } else {
                            format!("{name} requires a String")
                        });
                    }
                    return self.scalar(Scalar::String);
                }
                if let Some(Target::Builtin { name, .. }) = self.target(callee)
                    && name == "split"
                {
                    let string = self.scalar(Scalar::String)?;
                    for argument in arguments.iter() {
                        let actual = self.value(*argument)?;
                        if !matches!(
                            self.table.get(self.rigid_view(actual)?)?,
                            Type::Scalar(Scalar::String)
                        ) {
                            return Err("split requires String arguments".into());
                        }
                    }
                    return self.table.intern(Type::Collection {
                        kind: CollectionKind::Sequence,
                        element: string,
                    });
                }
                if let Some(Target::Builtin { name, .. }) = self.target(callee)
                    && matches!(name.as_str(), "size" | "isEmpty")
                {
                    let scalar = if name == "size" {
                        Scalar::Int64
                    } else {
                        Scalar::Bool
                    };
                    let argument = *arguments.first().ok_or("Missing collection argument")?;
                    self.collection_parts(self.value(argument)?)?;
                    return self.scalar(scalar);
                }
                if let Some(Target::Builtin { name, .. }) = self.target(callee)
                    && matches!(name.as_str(), "flatten" | "first" | "last" | "at")
                {
                    let (kind, element) = self.collection_parts(self.value(arguments[0])?)?;
                    let ordered =
                        matches!(kind, CollectionKind::Sequence | CollectionKind::OrderedSet);
                    if name == "flatten" {
                        let (inner, element) = self.collection_parts(element)?;
                        let kind = if ordered
                            && matches!(
                                inner,
                                CollectionKind::Sequence | CollectionKind::OrderedSet
                            ) {
                            CollectionKind::Sequence
                        } else {
                            CollectionKind::Bag
                        };
                        return self.table.intern(Type::Collection { kind, element });
                    }
                    if !ordered {
                        return Err("Positional selection requires an ordered collection".into());
                    }
                    if name == "at" {
                        self.value(arguments[1])?;
                    }
                    return self.table.intern(Type::Option(element));
                }
                for argument in arguments {
                    self.value(argument)?;
                }
                match self.table.get(self.rigid_view(self.value(callee)?)?)? {
                    Type::Function { result, .. } => Ok(*result),
                    _ => Err("Callee is not a function".into()),
                }
            }
            Expr::Lambda { parameters, body } => {
                let parameters = parameters
                    .iter()
                    .map(|p| {
                        self.locals
                            .get(&p.name.span.start)
                            .copied()
                            .ok_or_else(|| "Missing checked lambda parameter type".to_owned())
                    })
                    .collect::<Result<_, _>>()?;
                let result = self.value(body)?;
                self.table.intern(Type::Function { parameters, result })
            }
            Expr::Member {
                receiver,
                name,
                safe,
            } => {
                let receiver = self.rigid_view(self.value(receiver)?)?;
                let receiver = if safe {
                    match self.table.get(receiver)? {
                        Type::Option(inner) => *inner,
                        _ => return Err("Safe access requires an optional model receiver".into()),
                    }
                } else {
                    receiver
                };
                let receiver = self.rigid_view(receiver)?;
                let receiver_type = self.table.get(receiver)?.clone();
                if let Type::Record(fields) = &receiver_type {
                    let value = *fields.get(&name.value).ok_or("Unknown record field")?;
                    return if safe && !matches!(self.table.get(value)?, Type::Option(_)) {
                        self.table.intern(Type::Option(value))
                    } else {
                        Ok(value)
                    };
                }
                if matches!(receiver_type, Type::SymbolicModelApplication { .. }) {
                    let feature =
                        self.models
                            .symbolic_feature(self.table, receiver, &name.value)?;
                    let mut value = feature.value;
                    if safe && !matches!(self.table.get(value)?, Type::Option(_)) {
                        value = self.table.intern(Type::Option(value))?;
                    }
                    self.report.symbolic_members.insert(id.0, feature);
                    return Ok(value);
                }
                let feature = match receiver_type {
                    Type::Model(class) => self.models.feature(self.table, &class, &name.value)?,
                    Type::ModelApplication(expression) => {
                        self.models
                            .feature_application(self.table, &expression, &name.value)?
                    }
                    _ => return Err(
                        "Member access requires a model receiver; optional values need safe access"
                            .into(),
                    ),
                };
                let mut value = feature.value;
                if safe && !matches!(self.table.get(value)?, Type::Option(_)) {
                    value = self.table.intern(Type::Option(value))?;
                }
                self.report.members.insert(id.0, feature);
                Ok(value)
            }
            _ => Err("Expression has no implemented type rule".into()),
        }
    }
    fn join(&mut self, left: TypeRef, right: TypeRef) -> Result<TypeRef, String> {
        if inference::assignable(self.models, self.table, left, right, &self.rigid_bounds)? {
            return Ok(right);
        }
        if inference::assignable(self.models, self.table, right, left, &self.rigid_bounds)? {
            return Ok(left);
        }
        self.models.join(self.table, left, right)
    }
    fn binary(&mut self, operator: &str, left: ExprId, right: ExprId) -> Result<TypeRef, String> {
        let left = self.value(left)?;
        let right = self.value(right)?;
        if operator == "??" {
            if let Type::Option(inner) = self.table.get(self.rigid_view(left)?)? {
                return Ok(*inner);
            }
            return Err("Coalesce requires Option".into());
        }
        if matches!(operator, "==" | "!=") {
            self.join(left, right)?;
            let mut pending = vec![left, right];
            let mut visited = std::collections::BTreeSet::new();
            while let Some(ty) = pending.pop() {
                if !visited.insert(ty) {
                    continue;
                }
                match self.table.get(ty)? {
                    Type::Function { .. } | Type::Many { .. } | Type::Collection { .. } => return Err("Equality for functions or collections is unsupported, including wrapped values".into()),
                    Type::Parameter { .. } => {
                        let bound=self.rigid_bounds.get(&ty).copied().ok_or("Equality requires a universally supported generic bound")?;
                        pending.push(bound);
                    }
                    Type::Tuple(items) => pending.extend(items),
                    Type::Record(fields) => pending.extend(fields.values()),
                    Type::Option(inner) => pending.push(*inner),
                    Type::Result { ok, error } => pending.extend([*ok, *error]),
                    _ => {}
                }
            }
            return self.scalar(Scalar::Bool);
        }
        let left = self.rigid_view(left)?;
        let right = self.rigid_view(right)?;
        if left != right {
            return Err("Operator operands must have matching scalar types".into());
        }
        let Type::Scalar(scalar) = self.table.get(left)? else {
            return Err("Operator requires scalar operands".into());
        };
        if matches!(operator, "&&" | "||") && *scalar == Scalar::Bool {
            return Ok(left);
        }
        if operator == "+" && *scalar == Scalar::String {
            return Ok(left);
        }
        if scalar.numeric() {
            if matches!(operator, "<" | "<=" | ">" | ">=") {
                return self.scalar(Scalar::Bool);
            }
            if matches!(operator, "+" | "-" | "*" | "/" | "%") {
                return Ok(left);
            }
        }
        Err("Operator has incompatible scalar operands".into())
    }
}
