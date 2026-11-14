//! Transactional overload selection reusing the ordinary expression checker.
use super::*;
use std::fmt::Write;

pub(crate) fn snapshot_charge(
    remaining: &mut usize,
    value: &impl std::fmt::Debug,
) -> Result<(), String> {
    struct Counter<'a>(&'a mut usize);
    impl std::fmt::Write for Counter<'_> {
        fn write_str(&mut self, value: &str) -> std::fmt::Result {
            if value.len() > *self.0 {
                *self.0 = 0;
                return Err(std::fmt::Error);
            }
            *self.0 -= value.len();
            Ok(())
        }
    }
    write!(Counter(remaining), "{value:?}")
        .map_err(|_| "Overload snapshot byte limit exceeded".into())
}
pub(super) fn fatal(message: &str) -> bool {
    // Existing adapters preserve typed import errors as these anchored prefixes.
    // Never inspect arbitrary user names embedded in mismatch diagnostics.
    if message.starts_with("ResourceLimit: ") || message.starts_with("Unsupported: ") {
        return true;
    }
    let message = message
        .strip_prefix("InvalidDefinition: ")
        .unwrap_or(message);
    matches!(
        message,
        "Generic inference work limit exceeded"
            | "Overload snapshot byte limit exceeded"
            | "Overload resolution work limit exceeded"
            | "Overload trial nesting limit exceeded"
            | "Overload candidate limit exceeded (256)"
            | "Pattern generic bound work limit exceeded"
            | "Symbolic application work limit exceeded"
            | "Symbolic application nesting exceeds 64 levels"
            | "Nominal generic bound work limit exceeded"
            | "Nominal generic bound nesting exceeds 64 levels"
            | "Symbolic member type nesting exceeds 64 levels"
            | "Rcore model application closure exceeds 1024 types"
            | "Type nesting exceeds 64 levels"
            | "Generic expression size/depth budget exceeded"
            | "Generic binding count budget exceeded"
            | "Generic substitution size/depth budget exceeded"
            | "Generic inheritance traversal budget exceeded"
            | "Generic conformance budget exceeded"
            | "Raw type derivation depth exceeds 128"
            | "Classifier relation provider budget exceeded (128)"
            | "Raw feature bound expansion budget exceeded"
            | "Generic member projection budget exceeded"
            | "Rigid context parameter budget exceeded"
            | "Symbolic nominal constraint budget exceeded"
            | "Symbolic value conformance budget exceeded"
            | "Symbolic ancestor projection budget exceeded"
            | "Materialization type budget exceeded"
            | "Runtime feature candidate comparison budget exceeded"
            | "Equality for functions or collections is unsupported, including wrapped values"
            | "Symbolic member retains an unsupported open Ecore type"
            | "Unsupported nominal classifier kind"
            | "Rcore feature domain contains unresolved runtime candidates; complete static read coverage is unavailable"
            | "Rcore constrained attribute domain requires an explicit static domain type; checked source projection is available"
            | "Rcore constrained reference domain has no verified erased read type"
            | "Feature datatype requires E05/E06"
            | "Explicit applied conversion registry required"
            | "Explicit conversion registry required"
            | "No conversion provider for this exact datatype revision"
            | "No provider for this exact datatype application"
            | "Conversion provider changed its registered contract"
            | "Applied provider changed its registered contract"
            | "Applied provider implementation differs from its contract"
            | "Unsupported scalar source literal"
            | "Scalar payload nesting limit exceeded"
            | "Scalar value byte limit exceeded"
            | "Scalar digit limit exceeded"
            | "Literal byte limit exceeded"
            | "Integer digit limit exceeded"
    )
}

impl Checker<'_, '_> {
    pub(super) fn target(&self, id: ExprId) -> Option<&Target> {
        self.selected_targets
            .get(&id.0)
            .or_else(|| self.bindings.expressions.get(&id.0))
    }
    fn ungroup(&self, mut id: ExprId) -> ExprId {
        while let Expr::Group(inner) = self.doc.expressions[id.0].value {
            id = inner;
        }
        id
    }
    pub(super) fn has_overloads(&self, id: ExprId) -> bool {
        matches!(self.target(self.ungroup(id)), Some(Target::Overload(_)))
    }
    pub(super) fn charge_work(&self, amount: usize) -> Result<(), String> {
        let remaining = self
            .shared_work
            .get()
            .checked_sub(amount)
            .ok_or("Overload resolution work limit exceeded")?;
        self.shared_work.set(remaining);
        Ok(())
    }
    pub(super) fn charge_snapshot(&self, value: &impl std::fmt::Debug) -> Result<(), String> {
        let mut remaining = self.snapshot_bytes.get();
        let result = snapshot_charge(&mut remaining, value);
        self.snapshot_bytes.set(remaining);
        result
    }
    pub(super) fn resolve_overload(
        &mut self,
        root: ExprId,
        callee: ExprId,
        expected: Option<TypeRef>,
    ) -> Result<(), String> {
        if self.trial_depth >= 32 {
            return Err("Overload trial nesting limit exceeded".into());
        }
        let base = self.ungroup(callee);
        let Some(Target::Overload(candidates)) = self.target(base) else {
            return Err("Missing overload candidate set".into());
        };
        if candidates.len() > 256 {
            return Err("Overload candidate limit exceeded (256)".into());
        }
        self.charge_work(candidates.len())?;
        self.charge_snapshot(candidates)?;
        let candidates = candidates.clone();
        let call = match &self.doc.expressions[root.0].value {
            Expr::Call {
                callee: invoked,
                arguments,
                types,
            } if self.ungroup(*invoked) == base => Some((arguments.len(), types.len())),
            _ => None,
        };
        let mut winner = None;
        let mut successes = 0usize;
        for symbol in candidates {
            let function_type = *self
                .functions
                .get(&symbol)
                .ok_or("Overload candidate has no indexed signature")?;
            if let Some((arity, explicit)) = call {
                let Type::Function { parameters, .. } = self.table.get(function_type)? else {
                    return Err("Overload candidate is not callable".into());
                };
                if parameters.len() != arity {
                    continue;
                }
                let generics = self
                    .generic_functions
                    .get(&symbol)
                    .map_or(0, |s| s.generics.len());
                if explicit != 0 && generics != explicit {
                    continue;
                }
            }
            self.charge_work(1)?;
            // Counters are shared authority outside all candidate snapshots.
            let mut remaining = self.snapshot_bytes.get();
            let snapshots = (|| {
                snapshot_charge(&mut remaining, self.table)?;
                snapshot_charge(&mut remaining, &self.report)?;
                snapshot_charge(&mut remaining, &self.locals)?;
                snapshot_charge(&mut remaining, &self.comprehension_hints)?;
                snapshot_charge(&mut remaining, &self.rigid_bounds)?;
                snapshot_charge(&mut remaining, &self.selected_targets)?;
                // The new override is not part of the baseline map yet.
                snapshot_charge(&mut remaining, &symbol)?;
                let models = self.models.trial_clone(&mut remaining)?;
                Ok::<_, String>((self.table.clone(), models))
            })();
            self.snapshot_bytes.set(remaining);
            let (mut table, mut models) = snapshots?;
            let mut selected_targets = self.selected_targets.clone();
            selected_targets.insert(base.0, Target::Symbol(symbol.clone()));
            let mut trial = Checker {
                doc: self.doc,
                bindings: self.bindings,
                annotations: self.annotations,
                functions: self.functions,
                generic_functions: self.generic_functions,
                rigid_bounds: self.rigid_bounds.clone(),
                table: &mut table,
                models: &mut models,
                locals: self.locals.clone(),
                comprehension_hints: self.comprehension_hints.clone(),
                report: self.report.clone(),
                errors: vec![],
                selected_targets,
                shared_work: self.shared_work.clone(),
                snapshot_bytes: self.snapshot_bytes.clone(),
                trial_depth: self.trial_depth + 1,
            };
            // A contextual revisit must not inherit an earlier callable choice.
            // Nested argument selections remain candidate-local checked evidence.
            let mut expression = callee;
            loop {
                trial.report.function_instantiations.remove(&expression.0);
                match self.doc.expressions[expression.0].value {
                    Expr::Group(inner) => expression = inner,
                    _ => break,
                }
            }
            trial.walk(root, expected);
            if let Some(error) = trial.errors.iter().find(|e| fatal(&e.message)) {
                return Err(error.message.clone());
            }
            if !trial.errors.is_empty() || !trial.report.expressions.contains_key(&root.0) {
                continue;
            }
            let selected_type = trial.value(callee)?;
            let selection = if self.generic_functions.contains_key(&symbol) {
                let selection = trial
                    .report
                    .function_instantiations
                    .get(&callee.0)
                    .or_else(|| trial.report.function_instantiations.get(&base.0))
                    .ok_or("Missing fresh generic overload selection")?;
                if selection.symbol != symbol || selection.function_type != selected_type {
                    return Err("Fresh overload selection differs from current candidate".into());
                }
                self.charge_snapshot(selection)?;
                selection.clone()
            } else {
                self.charge_snapshot(&symbol)?;
                FunctionInstantiation {
                    symbol: symbol.clone(),
                    arguments: vec![],
                    function_type: selected_type,
                }
            };
            // Retain exact selection at every grouped callee/value expression.
            let mut expression = callee;
            loop {
                self.charge_snapshot(&selection)?;
                trial.report.expressions.insert(expression.0, selected_type);
                trial
                    .report
                    .function_instantiations
                    .insert(expression.0, selection.clone());
                match self.doc.expressions[expression.0].value {
                    Expr::Group(inner) => expression = inner,
                    _ => break,
                }
            }
            let report = trial.report;
            let locals = trial.locals;
            let comprehension_hints = trial.comprehension_hints;
            let rigid_bounds = trial.rigid_bounds;
            successes += 1;
            if winner.is_none() {
                winner = Some((
                    table,
                    models,
                    report,
                    locals,
                    rigid_bounds,
                    comprehension_hints,
                ));
            }
            // Keep checking: a later resource/unsupported failure must not hide
            // behind an earlier success or ambiguity.
        }
        if successes != 1 {
            return Err(if successes == 0 {
                "No applicable overload"
            } else {
                "Ambiguous overload"
            }
            .into());
        }
        let Some((table, models, report, locals, rigid_bounds, comprehension_hints)) = winner
        else {
            return Err("Missing unique overload selection".into());
        };
        *self.table = table;
        *self.models = models;
        self.report = report;
        self.locals = locals;
        self.comprehension_hints = comprehension_hints;
        self.rigid_bounds = rigid_bounds;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unavailable_model_domains_and_provider_dependencies_are_fatal() {
        for message in [
            "Rcore feature domain contains unresolved runtime candidates; complete static read coverage is unavailable",
            "Rcore constrained attribute domain requires an explicit static domain type; checked source projection is available",
            "Rcore constrained reference domain has no verified erased read type",
            "Feature datatype requires E05/E06",
            "Explicit applied conversion registry required",
            "Explicit conversion registry required",
            "No conversion provider for this exact datatype revision",
            "No provider for this exact datatype application",
            "Conversion provider changed its registered contract",
            "Applied provider changed its registered contract",
            "Applied provider implementation differs from its contract",
        ] {
            assert!(fatal(message), "{message}");
            assert!(!fatal(&format!(
                "Type mismatch: Record field {message:?} differs"
            )));
        }
    }
    #[test]
    fn contextual_revisit_replaces_every_grouped_selection_with_current_symbol() {
        let source = crate::definitions::bundle::DefinitionSource::new(
            "memory:revisit-behavior.rcore",
            crate::definitions::bundle::SourceKind::RcoreBehavior,
            "rcore 1;package revisit version \"1\";fn pick(x:Int8)->Int8=x;fn pick(x:Bool)->Bool=x;fn hold()->(Int8)->Int8=((pick));",
        );
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let mut signatures = signatures::index(&bundle, &source.id).unwrap();
        let mut models = ModelTypes::new(&bundle).unwrap();
        models.configure_symbolic(&signatures).unwrap();
        let functions: BTreeMap<_, _> = signatures
            .functions
            .iter()
            .map(|f| (f.symbol.clone(), f.function_type))
            .collect();
        let generic_functions = BTreeMap::new();
        let doc = &signatures.bound.index.modules[0].document;
        let Declaration::Function { body, .. } = doc.declarations[2].value else {
            panic!("function")
        };
        let symbol = |declaration| SymbolId {
            source: source.id.clone(),
            declaration,
        };
        let first_type = functions[&symbol(0)];
        let second_type = functions[&symbol(1)];
        let mut checker = Checker {
            doc,
            bindings: &signatures.bound.bindings[0],
            annotations: &signatures.modules[0].annotations,
            functions: &functions,
            generic_functions: &generic_functions,
            rigid_bounds: BTreeMap::new(),
            table: &mut signatures.types,
            models: &mut models,
            locals: BTreeMap::new(),
            comprehension_hints: BTreeMap::new(),
            report: ModuleReport {
                source: source.id.clone(),
                expressions: BTreeMap::new(),
                function_instantiations: BTreeMap::new(),
                members: BTreeMap::new(),
                symbolic_members: BTreeMap::new(),
                providers: BTreeMap::new(),
                constructions: BTreeMap::new(),
            },
            errors: vec![],
            selected_targets: BTreeMap::new(),
            shared_work: std::rc::Rc::new(std::cell::Cell::new(1_048_576)),
            snapshot_bytes: std::rc::Rc::new(std::cell::Cell::new(67_108_864)),
            trial_depth: 0,
        };
        // Exercise the same already-bound expression under two independent
        // contextual checks; this is a transaction regression, not a changed
        // source function's return annotation or an execution certificate.
        for (expected, declaration) in [(first_type, 0), (second_type, 1)] {
            checker.walk(body, Some(expected));
            assert!(checker.errors.is_empty(), "{:?}", checker.errors);
            assert_eq!(checker.report.expressions[&body.0], expected);
            let mut expression = body;
            loop {
                let selection = &checker.report.function_instantiations[&expression.0];
                assert_eq!(selection.symbol, symbol(declaration));
                assert_eq!(selection.function_type, expected);
                assert!(selection.arguments.is_empty());
                match doc.expressions[expression.0].value {
                    Expr::Group(inner) => expression = inner,
                    _ => break,
                }
            }
        }
    }
}
