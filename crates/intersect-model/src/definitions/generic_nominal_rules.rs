//! Compiled nominal reference predicates over one immutable checked source world.
//! Rules are code/input metadata, never cached membership or provider outcomes.
use super::*;

pub(crate) struct NominalRule {
    clauses: Vec<DeclarationId>,
}
impl NominalRule {
    pub(crate) fn clauses(&self) -> &[DeclarationId] {
        &self.clauses
    }
}
fn supported_policy(resolver: &Resolver) -> bool {
    resolver.constructor_contracts.is_empty()
        && resolver.constructor_relations.is_empty()
        && resolver.provider_relations.is_empty()
        && resolver.rigid_parameters.is_empty()
}
impl<'source> PreparedRuntimeMembers<'source> {
    pub(crate) fn source_resolver(&self) -> &'source Resolver {
        self.resolver
    }
    pub(crate) fn nominal_ancestry_observed(
        &self,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Option<super::super::inheritance::AncestryView>, ImportError> {
        if !supported_policy(self.resolver) {
            return Ok(None);
        }
        if !matches!(self.receiver.expression(), Type::Classifier { arguments, .. } if arguments.is_empty())
        {
            return Ok(None);
        }
        let ancestry = self.resolver.ancestry_view_observed(
            self.receiver.expression(),
            &self.declaration_span,
            observation,
        )?;
        for expression in std::iter::once(self.receiver.expression())
            .chain(ancestry.as_slice().iter().map(|row| &row.expression))
        {
            observation.work(1, &self.declaration_span.source)?;
            let Type::Classifier {
                declaration,
                arguments,
                ..
            } = expression
            else {
                return Ok(None);
            };
            if !arguments.is_empty() {
                return Ok(None);
            }
            if self
                .resolver
                .classifier_observed(declaration, observation)?
                != TargetKind::Class
            {
                return Ok(None);
            }
        }
        Ok(Some(ancestry))
    }
}
impl CheckedRuntimeFeatureDomain<'_> {
    pub(crate) fn nominal_context_matches(&self, resolver: &Resolver) -> bool {
        std::ptr::eq(self.resolver, resolver)
    }
    pub(crate) fn compile_nominal_rule_observed(
        &self,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Option<NominalRule>, ImportError> {
        if !self.domain.reference
            || !self.domain.projection.bindings.is_empty()
            || !supported_policy(self.resolver)
        {
            return Ok(None);
        }
        let mut active = Vec::new();
        let mut clauses = Vec::new();
        let mut remaining = 16_384;
        if compile_expression(
            self.resolver,
            self.domain.expression,
            self.domain.projection.source.span,
            observation,
            (0, &mut remaining),
            &mut active,
            &mut clauses,
        )? {
            Ok(Some(NominalRule { clauses }))
        } else {
            Ok(None)
        }
    }
}
fn retain_id(
    values: &mut Vec<DeclarationId>,
    id: &DeclarationId,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    observation.serialized(id, &id.source)?;
    observation.reserve(128 + 32, &id.source)?;
    values
        .try_reserve_exact(1)
        .map_err(|_| error(&fallback(id), "Nominal rule input allocation failed"))?;
    values.push(id.clone());
    Ok(())
}
fn compile_expression(
    resolver: &Resolver,
    expression: &Type,
    span: &SourceSpan,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    progress: (usize, &mut usize),
    active: &mut Vec<DeclarationId>,
    clauses: &mut Vec<DeclarationId>,
) -> Result<bool, ImportError> {
    observation.work(1, &span.source)?;
    let (depth, remaining) = progress;
    if depth >= 128 || *remaining == 0 {
        return Ok(false);
    }
    *remaining -= 1;
    match expression {
        Type::Classifier {
            declaration,
            arguments,
            ..
        } => {
            if !arguments.is_empty()
                || resolver.classifier_observed(declaration, observation)? != TargetKind::Class
            {
                return Ok(false);
            }
            // Exact standard EObject identity is the reference-only top domain.
            if declaration
                != crate::definitions::schema::eobject_reference_top_declaration_ref()
                    .map_err(|message| error(span, message))?
            {
                retain_id(clauses, declaration, observation)?;
            }
            Ok(true)
        }
        Type::Parameter { declaration } => {
            for prior in active.iter() {
                observation.work(1, &span.source)?;
                if prior == declaration {
                    return Ok(false);
                }
            }
            retain_id(active, declaration, observation)?;
            let result = (|| {
                let bounds = resolver.parameter_bound_inputs_observed(declaration, observation)?;
                for bound in bounds.as_slice() {
                    if !compile_expression(
                        resolver,
                        &bound.expression,
                        span,
                        observation,
                        (depth + 1, remaining),
                        active,
                        clauses,
                    )? {
                        return Ok(false);
                    }
                }
                Ok(true)
            })();
            active.pop();
            result
        }
        Type::Wildcard { upper, lower } if lower.is_none() => match upper {
            Some(upper) => compile_expression(
                resolver,
                upper,
                span,
                observation,
                (depth + 1, remaining),
                active,
                clauses,
            ),
            None => Ok(true),
        },
        _ => Ok(false),
    }
}

#[cfg(test)]
mod policy_controls {
    use super::*;
    #[test]
    fn selected_provider_and_constructor_relations_disable_the_nominal_fragment() {
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![],
            namespaces: vec![],
        };
        let mut resolver = Resolver::new(&bundle).unwrap();
        assert!(supported_policy(&resolver));
        let id = crate::definitions::schema::eobject_reference_top_declaration().unwrap();
        resolver.provider_relations.insert(
            (id.clone(), id.clone()),
            crate::definitions::conversion::ClassifierRelation::Equivalent,
        );
        assert!(!supported_policy(&resolver));
        resolver.provider_relations.clear();
        resolver.constructor_relations.insert(
            (id.clone(), id),
            crate::definitions::conversion::ClassifierRelation::Equivalent,
        );
        assert!(!supported_policy(&resolver));
    }
}
