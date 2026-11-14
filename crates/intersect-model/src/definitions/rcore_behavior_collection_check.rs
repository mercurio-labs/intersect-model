//! Collection callbacks use the existing source type and generic inference authorities.
use super::*;
fn ordered(kind: &CollectionKind) -> bool {
    matches!(kind, CollectionKind::Sequence | CollectionKind::OrderedSet)
}
impl Checker<'_, '_> {
    fn grouping_key_hint(&self, expected: Option<TypeRef>) -> Option<TypeRef> {
        let (_, pair) = self.collection_parts(expected?).ok()?;
        let Type::Tuple(fields) = self.table.get(self.rigid_view(pair).ok()?).ok()? else {
            return None;
        };
        if fields.len() == 2 {
            Some(fields[0])
        } else {
            None
        }
    }
    fn collection_literal_hint_kind(
        &self,
        mut id: ExprId,
    ) -> Result<Option<CollectionKind>, String> {
        while let Expr::Group(inner) = self.doc.expressions[id.0].value {
            id = inner;
        }
        let (kind, mut literal) = match &self.doc.expressions[id.0].value {
            Expr::List(_) => (CollectionKind::Sequence, id),
            Expr::Call {
                callee,
                types,
                arguments,
            } if types.is_empty() && arguments.len() == 1 => {
                let Some(Target::Builtin { name, .. }) = self.target(*callee) else {
                    return Ok(None);
                };
                let Some(kind) = Self::collection_kind(name) else {
                    return Ok(None);
                };
                (kind, arguments[0])
            }
            _ => return Ok(None),
        };
        while let Expr::Group(inner) = self.doc.expressions[literal.0].value {
            literal = inner;
        }
        let Expr::List(items) = &self.doc.expressions[literal.0].value else {
            return Ok(None);
        };
        if self.needs_context(literal)?
            || items
                .iter()
                .all(|id| matches!(self.doc.expressions[id.0].value, Expr::Literal(_)))
        {
            Ok(Some(kind))
        } else {
            Ok(None)
        }
    }
    pub(super) fn algebra_result(
        &mut self,
        operation: AlgebraOp,
        arguments: &[ExprId],
    ) -> Result<TypeRef, String> {
        let (left, element) = self.collection_parts(self.value(arguments[0])?)?;
        if operation == AlgebraOp::Distinct {
            let kind = if ordered(&left) {
                CollectionKind::OrderedSet
            } else {
                CollectionKind::Set
            };
            return self.table.intern(Type::Collection { kind, element });
        }
        let (right, other) = self.collection_parts(self.value(arguments[1])?)?;
        let kind = if operation == AlgebraOp::Concat {
            if !ordered(&left) || !ordered(&right) {
                return Err("concat requires two ordered collections".into());
            }
            CollectionKind::Sequence
        } else {
            if left != right {
                return Err("Collection algebra requires matching collection kinds".into());
            }
            left
        };
        let element = self.join(element, other)?;
        self.table.intern(Type::Collection { kind, element })
    }
    pub(super) fn collection_right_operand(
        &mut self,
        id: ExprId,
        expected: Option<TypeRef>,
        collection: bool,
        work: &mut Vec<Work>,
    ) -> Result<(), String> {
        let Expr::Call { arguments, .. } = self.doc.expressions[id.0].value.clone() else {
            return Err("Expected binary collection call".into());
        };
        let (_, element) = self.collection_parts(self.value(arguments[0])?)?;
        let other = arguments[1];
        let mut inner = other;
        while let Expr::Group(next) = self.doc.expressions[inner.0].value {
            inner = next;
        }
        let hint = if collection {
            // Only contextual literals inherit the left element hint. Explicit
            // constructors and already-typed values must contribute their own
            // domain to the join (for example Child combined with Base).
            let kind = self.collection_literal_hint_kind(inner)?;
            kind.map(|kind| self.table.intern(Type::Collection { kind, element }))
                .transpose()?
        } else if self.needs_context(other)?
            || matches!(self.doc.expressions[inner.0].value, Expr::Literal(_))
        {
            Some(element)
        } else {
            None
        };
        work.extend([Work::Finish(id, expected), Work::Visit(other, hint)]);
        Ok(())
    }
    // Supply literal context from an actual callback contract, never from the
    // mapped output element alone: map may change its element type entirely.
    pub(super) fn collection_input_hint(
        &mut self,
        mut input: ExprId,
        mut callback: ExprId,
        operation: CollectionOp,
        expected: Option<TypeRef>,
    ) -> Result<Option<TypeRef>, String> {
        while let Expr::Group(inner) = self.doc.expressions[input.0].value {
            input = inner;
        }
        let kind = match &self.doc.expressions[input.0].value {
            Expr::List(_) => CollectionKind::Sequence,
            Expr::Call { callee, .. } => match self.target(*callee) {
                Some(Target::Builtin { name, .. }) => match Self::collection_kind(name) {
                    Some(kind) => kind,
                    None => return Ok(None),
                },
                _ => return Ok(None),
            },
            _ => return Ok(None),
        };
        while let Expr::Group(inner) = self.doc.expressions[callback.0].value {
            callback = inner;
        }
        let ordinal = usize::from(operation == CollectionOp::Fold);
        let element = if let Expr::Lambda { parameters, .. } =
            &self.doc.expressions[callback.0].value
        {
            parameters
                .get(ordinal)
                .and_then(|p| self.annotations.get(&p.ty.0))
                .copied()
        } else if self.has_overloads(callback) {
            None // The ordinary overload authority must select the full contract.
        } else if let Some(signature) = self.generic_signature(callback).cloned() {
            let result = if operation == CollectionOp::SortedWith {
                Some(self.scalar(Scalar::Int64)?)
            } else if operation.predicate() {
                Some(self.scalar(Scalar::Bool)?)
            } else if operation == CollectionOp::GroupBy {
                self.grouping_key_hint(expected)
            } else if operation == CollectionOp::Map {
                expected.and_then(|ty| self.collection_parts(ty).ok().map(|(_, element)| element))
            } else if operation == CollectionOp::Fold {
                expected
            } else {
                None
            };
            let mut inference = Inference::with_shared_budget(self.shared_work.clone());
            if let Some(result) = result {
                inference.relations.push((signature.result, result, true));
            }
            let values = inference.solve(
                &signature,
                self.table,
                self.models,
                &self.rigid_bounds,
                false,
            )?;
            let missing = signature
                .generics
                .iter()
                .filter(|g| !values.contains_key(&g.ty))
                .map(|g| g.ty)
                .collect();
            if let Some(parameter) = signature.parameters.get(ordinal) {
                let parameter =
                    inference::substitute(self.table, self.models, *parameter, &values)?;
                if inference::contains(self.table, parameter, &missing)? {
                    None
                } else {
                    Some(parameter)
                }
            } else {
                None
            }
        } else if let Some(ty) = self.target_type(callback) {
            match self.table.get(self.rigid_view(ty)?)? {
                Type::Function { parameters, .. } => parameters.get(ordinal).copied(),
                _ => None,
            }
        } else {
            None
        };
        element
            .map(|element| self.table.intern(Type::Collection { kind, element }))
            .transpose()
    }
    pub(super) fn collection_callback(
        &mut self,
        id: ExprId,
        expected: Option<TypeRef>,
        operation: CollectionOp,
        work: &mut Vec<Work>,
    ) -> Result<(), String> {
        let Expr::Call {
            callee, arguments, ..
        } = self.doc.expressions[id.0].value.clone()
        else {
            return Err("Expected collection call".into());
        };
        let (_, element) = self.collection_parts(self.value(arguments[0])?)?;
        let callback = *arguments.last().ok_or("Missing collection callback")?;
        let mut parameters = vec![element];
        let inherited_element = self.comprehension_hints.remove(&id.0);
        let output_element = expected
            .and_then(|ty| self.collection_parts(ty).ok().map(|(_, element)| element))
            .or(inherited_element);
        let mut hint = if operation == CollectionOp::SortedWith {
            parameters.push(element);
            Some(self.scalar(Scalar::Int64)?)
        } else if operation == CollectionOp::Fold {
            let accumulator = self.value(arguments[1])?;
            parameters.insert(0, accumulator);
            Some(accumulator)
        } else if operation.predicate() {
            Some(self.scalar(Scalar::Bool)?)
        } else if operation == CollectionOp::GroupBy {
            self.grouping_key_hint(expected)
        } else if operation == CollectionOp::Map {
            output_element
        } else {
            None
        };
        // A callback's input domain is known even when its output is inferred.
        // Seed only omitted lambda annotations; explicit types keep normal checks.
        let mut lambda = callback;
        while let Expr::Group(inner) = self.doc.expressions[lambda.0].value {
            lambda = inner;
        }
        if operation == CollectionOp::Closure
            && let Expr::Lambda { body, .. } = self.doc.expressions[lambda.0].value
            && let Some(kind) = self.collection_literal_hint_kind(body)?
        {
            hint = Some(self.table.intern(Type::Collection { kind, element })?);
        }
        if matches!(
            self.doc.expressions[callee.0].value,
            Expr::ComprehensionIntrinsic { flat: true }
        ) && let Some(element) = output_element
            && let Expr::Lambda { body, .. } = self.doc.expressions[lambda.0].value
        {
            match self.doc.expressions[body.0].value {
                Expr::Call { callee, .. }
                    if matches!(
                        self.doc.expressions[callee.0].value,
                        Expr::ComprehensionIntrinsic { .. }
                    ) =>
                {
                    // Propagate only the yielded element. The nested generator
                    // independently determines whether its result is Bag/Sequence.
                    self.comprehension_hints.insert(body.0, element);
                }
                Expr::If { .. } => {
                    // The predicate emits a singleton or empty Sequence.
                    hint = Some(self.table.intern(Type::Collection {
                        kind: CollectionKind::Sequence,
                        element,
                    })?);
                }
                _ => return Err("Invalid generated comprehension body".into()),
            }
        }
        if let Expr::Lambda {
            parameters: declared,
            ..
        } = &self.doc.expressions[lambda.0].value
        {
            if declared.len() != parameters.len() {
                return Err("Collection callback arity mismatch".into());
            }
            for (parameter, ty) in declared.iter().zip(&parameters) {
                if matches!(self.doc.types[parameter.ty.0].value, ast::Type::Inferred) {
                    self.locals.insert(parameter.name.span.start, *ty);
                }
            }
        }
        work.push(Work::Finish(id, expected));
        if let Some(signature) = self.generic_signature(callback).cloned() {
            if signature.parameters.len() != parameters.len() {
                return Err("Collection callback arity mismatch".into());
            }
            let mut inference = Inference::with_shared_budget(self.shared_work.clone());
            for (formal, actual) in signature.parameters.iter().zip(&parameters) {
                inference.relations.push((*formal, *actual, false));
            }
            if let Some(result) = hint {
                inference.relations.push((signature.result, result, true));
            }
            let values = inference.solve(
                &signature,
                self.table,
                self.models,
                &self.rigid_bounds,
                true,
            )?;
            let function =
                inference::substitute(self.table, self.models, signature.function_type, &values)?;
            self.report.expressions.insert(callback.0, function);
            self.record_instantiation(callback, &signature, &values, function)?;
        } else {
            let expected = hint
                .map(|result| self.table.intern(Type::Function { parameters, result }))
                .transpose()?;
            work.push(Work::Visit(callback, expected));
        }
        Ok(())
    }
    pub(super) fn collection_result(
        &mut self,
        operation: CollectionOp,
        arguments: &[ExprId],
    ) -> Result<TypeRef, String> {
        let (kind, element) = self.collection_parts(self.value(arguments[0])?)?;
        let callback = *arguments.last().ok_or("Missing collection callback")?;
        let Type::Function { parameters, result } = self
            .table
            .get(self.rigid_view(self.value(callback)?)?)?
            .clone()
        else {
            return Err("Collection callback must be a function".into());
        };
        let mut actuals = vec![element];
        if operation == CollectionOp::SortedWith {
            actuals.push(element);
        }
        if operation == CollectionOp::Fold {
            actuals.insert(0, self.value(arguments[1])?);
        }
        if parameters.len() != actuals.len() {
            return Err("Collection callback arity mismatch".into());
        }
        for (actual, parameter) in actuals.iter().zip(&parameters) {
            if !inference::assignable(
                self.models,
                self.table,
                *actual,
                *parameter,
                &self.rigid_bounds,
            )? {
                return Err(
                    "Collection element or accumulator is incompatible with callback parameter"
                        .into(),
                );
            }
        }
        if operation.predicate()
            && self.table.get(self.rigid_view(result)?)? != &Type::Scalar(Scalar::Bool)
        {
            return Err("Collection predicate must return Bool".into());
        }
        match operation {
            CollectionOp::Closure => {
                if !matches!(
                    self.table.get(self.rigid_view(element)?)?,
                    Type::Model(_)
                        | Type::ModelApplication(_)
                        | Type::SymbolicModelApplication { .. }
                ) {
                    return Err("closure requires model-reference seeds".into());
                }
                let (_, successor) = self.collection_parts(result)?;
                if !inference::assignable(
                    self.models,
                    self.table,
                    successor,
                    element,
                    &self.rigid_bounds,
                )? {
                    return Err("closure successor must preserve the seed model domain".into());
                }
                self.table.intern(Type::Collection {
                    kind: CollectionKind::OrderedSet,
                    element,
                })
            }

            CollectionOp::SortedBy => {
                if !matches!(self.table.get(self.rigid_view(result)?)?, Type::Scalar(_)) {
                    return Err("Sorting key must be a scalar".into());
                }
                self.table.intern(Type::Collection {
                    kind: CollectionKind::Sequence,
                    element,
                })
            }
            CollectionOp::SortedWith => {
                if self.table.get(self.rigid_view(result)?)? != &Type::Scalar(Scalar::Int64) {
                    return Err("Sorting comparator must return Int".into());
                }
                self.table.intern(Type::Collection {
                    kind: CollectionKind::Sequence,
                    element,
                })
            }
            CollectionOp::GroupBy => {
                let outer = if ordered(&kind) {
                    CollectionKind::Sequence
                } else {
                    CollectionKind::Bag
                };
                let members = self.table.intern(Type::Collection { kind, element })?;
                let element = self.table.intern(Type::Tuple(vec![result, members]))?;
                self.table.intern(Type::Collection {
                    kind: outer,
                    element,
                })
            }
            CollectionOp::Fold => {
                if !inference::assignable(
                    self.models,
                    self.table,
                    result,
                    actuals[0],
                    &self.rigid_bounds,
                )? {
                    return Err("Fold callback must preserve accumulator type".into());
                }
                Ok(actuals[0])
            }
            CollectionOp::Exists | CollectionOp::ForAll | CollectionOp::One => {
                self.scalar(Scalar::Bool)
            }
            CollectionOp::Find => {
                if !ordered(&kind) {
                    return Err("find requires an ordered collection".into());
                }
                self.table.intern(Type::Option(element))
            }
            CollectionOp::Filter => self.table.intern(Type::Collection { kind, element }),
            CollectionOp::Map => self.table.intern(Type::Collection {
                kind: if ordered(&kind) {
                    CollectionKind::Sequence
                } else {
                    CollectionKind::Bag
                },
                element: result,
            }),
            CollectionOp::FlatMap => {
                let (inner, element) = self.collection_parts(result)?;
                self.table.intern(Type::Collection {
                    kind: if ordered(&kind) && ordered(&inner) {
                        CollectionKind::Sequence
                    } else {
                        CollectionKind::Bag
                    },
                    element,
                })
            }
        }
    }
}
