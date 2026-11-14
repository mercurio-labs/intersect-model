//! Checked symbolic Ecore projections. Never a constructible runtime identity.
use super::*;
use crate::definitions::generic_types::{RigidParameterId, Type};
use std::collections::BTreeMap;
#[cfg(test)]
#[path = "generic_symbolic_kind_tests.rs"]
mod kind_tests;

#[derive(Clone, Debug)]
pub(crate) struct SymbolicMembers {
    pub receiver: Type,
    pub parents: Vec<Type>,
    pub members: Vec<SpecializedMember>,
    pub bounds: Vec<BoundCheck>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::bundle::DefinitionSource;
    use crate::definitions::generic_types::{ClosedType, RuntimeType};

    trait TestContext {
        fn test_rigid_parameters(
            self,
            entries: Vec<(RigidParameterId, Vec<Type>)>,
        ) -> Result<Resolver, ImportError>;
    }
    impl TestContext for Resolver {
        fn test_rigid_parameters(
            self,
            entries: Vec<(RigidParameterId, Vec<Type>)>,
        ) -> Result<Resolver, ImportError> {
            self.with_rigid_parameters(
                entries
                    .into_iter()
                    .map(|(id, bounds)| {
                        let span = SourceSpan {
                            source: id.source.clone(),
                            start: 0,
                            end: 1,
                        };
                        (id, bounds, span)
                    })
                    .collect(),
            )
        }
    }

    fn fixture() -> (DefinitionBundle, DefinitionSource, RigidParameterId) {
        let model = DefinitionSource::new(
            "memory:symbolic.ecore",
            SourceKind::Ecore,
            r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="p" nsURI="urn:symbolic"><eClassifiers x:type="e:EClass" name="Base"/><eClassifiers x:type="e:EClass" name="Other"/><eClassifiers x:type="e:EClass" name="Parent"><eTypeParameters name="T"><eBounds eClassifier="#//Base"/></eTypeParameters><eStructuralFeatures x:type="e:EReference" name="value" lowerBound="1"><eGenericType eTypeParameter="#//Parent/T"/></eStructuralFeatures></eClassifiers><eClassifiers x:type="e:EClass" name="Child"><eTypeParameters name="U"><eBounds eClassifier="#//Base"/></eTypeParameters><eGenericSuperTypes eClassifier="#//Parent"><eTypeArguments eTypeParameter="#//Child/U"/></eGenericSuperTypes></eClassifiers></e:EPackage>"##,
        );
        let semantic = DefinitionSource::new(
            "memory:symbolic-behavior.rcore",
            SourceKind::RcoreBehavior,
            "rcore 1; package symbolic version \"1\";",
        );
        let identity = RigidParameterId {
            source: semantic.id.clone(),
            declaration: 0,
            parameter: 0,
        };
        (
            DefinitionBundle {
                version: 1,
                sources: vec![model.clone(), semantic],
                namespaces: vec![],
            },
            model,
            identity,
        )
    }
    fn classifier(source: &DefinitionSource, index: usize, arguments: Vec<Type>) -> Type {
        Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![index],
            },
            arguments,
            raw: false,
        }
    }

    #[test]
    fn symbolic_source_projection_preserves_rigid_identity_and_inherited_owner() {
        let (bundle, source, identity) = fixture();
        let rigid = Type::RigidParameter {
            identity: identity.clone(),
        };
        let resolver = Resolver::new(&bundle)
            .unwrap()
            .test_rigid_parameters(vec![(identity, vec![classifier(&source, 0, vec![])])])
            .unwrap();
        let parent = classifier(&source, 2, vec![rigid.clone()]);
        let child = classifier(&source, 3, vec![rigid.clone()]);
        assert!(
            resolver
                .symbolic_application(&child)
                .unwrap()
                .iter()
                .all(|check| check.satisfied)
        );
        assert!(resolver.symbolic_value_conforms(&child, &parent).unwrap());
        assert_eq!(
            resolver
                .symbolic_project_as(
                    &child,
                    &DeclarationId {
                        source: source.id.clone(),
                        path: vec![2]
                    }
                )
                .unwrap(),
            Some(parent.clone())
        );
        let feature = resolver.symbolic_feature(&child, "value").unwrap();
        assert_eq!(feature.member.owner, parent);
        assert_eq!(
            (feature.lower, feature.upper, feature.reference),
            (1, 1, true)
        );
        assert!(
            feature
                .member
                .signatures
                .iter()
                .any(|signature| signature.feature == "eType"
                    && signature.specialized == vec![rigid.clone()])
        );
        assert!(
            !resolver
                .symbolic_value_conforms(
                    &child,
                    &classifier(&source, 2, vec![classifier(&source, 0, vec![])])
                )
                .unwrap()
        );
        let top = Type::Classifier {
            declaration: crate::definitions::schema::eobject_reference_top_declaration().unwrap(),
            arguments: vec![],
            raw: false,
        };
        assert!(resolver.symbolic_value_conforms(&rigid, &top).unwrap());
    }

    #[test]
    fn symbolic_context_and_runtime_boundaries_cannot_be_forged() {
        let (bundle, source, identity) = fixture();
        let rigid = Type::RigidParameter {
            identity: identity.clone(),
        };
        let application = classifier(&source, 2, vec![rigid.clone()]);
        let resolver = Resolver::new(&bundle).unwrap();
        assert!(resolver.symbolic_value_conforms(&rigid, &rigid).is_err());
        assert!(
            resolver
                .symbolic_value_conforms(&application, &application)
                .is_err()
        );
        for expression in [rigid.clone(), application.clone()] {
            assert!(RuntimeType::new(expression.clone()).is_err());
            assert!(ClosedType::new(expression.clone()).is_err());
            let encoded = serde_json::to_string(&expression).unwrap();
            assert!(serde_json::from_str::<RuntimeType>(&encoded).is_err());
            assert!(serde_json::from_str::<ClosedType>(&encoded).is_err());
        }
        let wrong = Resolver::new(&bundle)
            .unwrap()
            .test_rigid_parameters(vec![(
                identity.clone(),
                vec![classifier(&source, 1, vec![])],
            )])
            .unwrap();
        assert!(wrong.symbolic_application(&application).is_err());
        let mut stale = identity.clone();
        stale.source.sha256 = "0".repeat(64);
        assert!(
            Resolver::new(&bundle)
                .unwrap()
                .test_rigid_parameters(vec![(stale, vec![classifier(&source, 0, vec![])])])
                .is_err()
        );
        assert!(
            Resolver::new(&bundle)
                .unwrap()
                .test_rigid_parameters(vec![(identity.clone(), vec![])])
                .is_err()
        );
        assert!(
            Resolver::new(&bundle)
                .unwrap()
                .test_rigid_parameters(vec![(identity, vec![rigid])])
                .is_err()
        );
    }

    #[test]
    fn equal_nominal_bounds_never_merge_rigid_parameters_and_primitive_bounds_reject() {
        let (bundle, source, first) = fixture();
        let mut second = first.clone();
        second.declaration += 1;
        let bound = classifier(&source, 0, vec![]);
        let context = vec![
            (first.clone(), vec![bound.clone()]),
            (second.clone(), vec![bound]),
        ];
        let resolver = Resolver::new(&bundle)
            .unwrap()
            .test_rigid_parameters(context.clone())
            .unwrap();
        let a = Type::RigidParameter {
            identity: first.clone(),
        };
        let b = Type::RigidParameter { identity: second };
        assert!(!resolver.symbolic_value_conforms(&a, &b).unwrap());
        let parent_a = classifier(&source, 2, vec![a]);
        let parent_b = classifier(&source, 2, vec![b]);
        assert!(
            !resolver
                .symbolic_value_conforms(&parent_a, &parent_b)
                .unwrap()
        );
        assert!(
            !resolver
                .symbolic_value_conforms(&parent_b, &parent_a)
                .unwrap()
        );
        let mut reversed = bundle.clone();
        reversed.sources.reverse();
        let replay = Resolver::new(&reversed)
            .unwrap()
            .test_rigid_parameters(context)
            .unwrap();
        assert_eq!(
            resolver.symbolic_application(&parent_a).unwrap(),
            replay.symbolic_application(&parent_a).unwrap()
        );
        let primitive = Resolver::new(&bundle)
            .unwrap()
            .pinned_operation_builtin("EInt")
            .unwrap();
        let primitive = Type::Classifier {
            declaration: primitive,
            arguments: vec![],
            raw: false,
        };
        let location = SourceSpan {
            source: first.source.clone(),
            start: 10,
            end: 15,
        };
        let invalid = Resolver::new(&bundle).unwrap().with_rigid_parameters(vec![(
            first,
            vec![primitive],
            location.clone(),
        )]);
        let error = match invalid {
            Ok(_) => panic!("Primitive argument bound accepted"),
            Err(error) => error,
        };
        assert_eq!(error.span, location);
    }

    #[test]
    fn primitive_feature_types_are_not_mistaken_for_primitive_generic_arguments() {
        let (mut bundle, source, identity) = fixture();
        let changed = DefinitionSource::new(source.id.uri, SourceKind::Ecore,
            source.text.replacen("</eStructuralFeatures>", r#"</eStructuralFeatures><eStructuralFeatures x:type="e:EAttribute" name="count" lowerBound="1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EInt"/>"#, 1));
        bundle.sources[0] = changed.clone();
        let resolver = Resolver::new(&bundle)
            .unwrap()
            .test_rigid_parameters(vec![(
                identity.clone(),
                vec![classifier(&changed, 0, vec![])],
            )])
            .unwrap();
        let child = classifier(&changed, 3, vec![Type::RigidParameter { identity }]);
        let feature = resolver.symbolic_feature(&child, "count").unwrap();
        assert!(!feature.reference);
        assert_eq!((feature.lower, feature.upper), (1, 1));
        let int = resolver.pinned_operation_builtin("EInt").unwrap();
        assert!(
            feature
                .member
                .signatures
                .iter()
                .any(|signature| signature.specialized
                    == vec![Type::Classifier {
                        declaration: int.clone(),
                        arguments: vec![],
                        raw: false
                    }])
        );
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SymbolicFeature {
    pub receiver: Type,
    pub parents: Vec<Type>,
    pub bounds: Vec<BoundCheck>,
    pub member: SpecializedMember,
    pub lower: usize,
    pub upper: i32,
    pub ordered: bool,
    pub unique: bool,
    pub derived: bool,
    pub changeable: bool,
    pub transient: bool,
    pub volatile: bool,
    pub unsettable: bool,
    pub reference: bool,
}

impl Resolver {
    /// Only the freshly checked source frontend may install this non-wire context.
    /// It establishes parameter scope; this resolver checks every nominal bound.
    pub(crate) fn with_rigid_parameters(
        mut self,
        entries: Vec<(RigidParameterId, Vec<Type>, SourceSpan)>,
    ) -> Result<Self, ImportError> {
        let mut parameters = BTreeMap::new();
        if entries.len() > 16_384 {
            return Err(ImportError::new(
                ErrorKind::ResourceLimit,
                &SourceSpan {
                    source: self.standard_id.clone(),
                    start: 0,
                    end: 0,
                },
                "Rigid context parameter budget exceeded",
            ));
        }
        let mut locations = BTreeMap::new();
        for (identity, bounds, span) in entries {
            if span.source != identity.source {
                return Err(error(
                    &span,
                    "Rigid parameter bound span belongs to a different source",
                ));
            }
            if !self.semantic_sources.contains(&identity.source) {
                return Err(error(&span, "Rigid parameter source is absent or stale"));
            }
            if bounds.is_empty() {
                return Err(error(
                    &span,
                    "Symbolic Ecore argument requires an explicit nominal constraint",
                ));
            }
            locations.insert(identity.clone(), span.clone());
            if parameters.insert(identity, bounds).is_some() {
                return Err(error(&span, "Duplicate rigid parameter identity"));
            }
        }
        self.prepared_ancestry.take();
        self.prepared_erased_signatures.take();
        self.rigid_parameters = parameters;
        for (identity, bounds) in &self.rigid_parameters {
            let span = locations.get(identity).ok_or_else(|| {
                error(
                    &SourceSpan {
                        source: identity.source.clone(),
                        start: 0,
                        end: 0,
                    },
                    "Missing rigid parameter source span",
                )
            })?;
            self.symbolic_kind(
                &Type::RigidParameter {
                    identity: identity.clone(),
                },
                span,
                0,
            )?;
            for bound in bounds {
                self.check_argument_kind(bound, span)?;
                let checks = self.check_expression_bounds(bound, span)?;
                if checks.iter().any(|check| !check.satisfied) {
                    return Err(error(
                        span,
                        "Rigid parameter nominal constraint violates constructor bounds",
                    ));
                }
            }
        }
        Ok(self)
    }

    pub(super) fn rigid_parameter_bounds(
        &self,
        identity: &RigidParameterId,
        span: &SourceSpan,
    ) -> Result<&[Type], ImportError> {
        self.rigid_parameters
            .get(identity)
            .map(Vec::as_slice)
            .ok_or_else(|| {
                error(
                    span,
                    "Rigid parameter has no checked nominal source context",
                )
            })
    }

    fn symbolic_kind(
        &self,
        ty: &Type,
        span: &SourceSpan,
        depth: usize,
    ) -> Result<TargetKind, ImportError> {
        self.symbolic_kind_checked(ty, span, depth, &mut 16_384)
    }

    fn symbolic_kind_checked(
        &self,
        ty: &Type,
        span: &SourceSpan,
        depth: usize,
        remaining: &mut usize,
    ) -> Result<TargetKind, ImportError> {
        if depth >= 128 || *remaining == 0 {
            return Err(ImportError::new(
                ErrorKind::ResourceLimit,
                span,
                "Symbolic nominal constraint budget exceeded",
            ));
        }
        *remaining -= 1;
        match ty {
            Type::Classifier { declaration, .. } => self.classifier(declaration),
            Type::RigidParameter { identity } => {
                let mut kind = None;
                for bound in self.rigid_parameter_bounds(identity, span)? {
                    let next = self.symbolic_kind_checked(bound, span, depth + 1, remaining)?;
                    if kind.as_ref().is_some_and(|kind| *kind != next) {
                        return Err(error(
                            span,
                            "Rigid nominal constraints disagree on classifier kind",
                        ));
                    }
                    kind = Some(next);
                }
                kind.ok_or_else(|| error(span, "Rigid parameter requires a nominal bound"))
            }
            _ => Err(error(
                span,
                "Nominal constraint must name a classifier or checked rigid parameter",
            )),
        }
    }

    pub(crate) fn symbolic_application(
        &self,
        expression: &Type,
    ) -> Result<Vec<BoundCheck>, ImportError> {
        let Type::Classifier { declaration, .. } = expression else {
            return Err(error(
                &self.symbolic_span(expression),
                "Symbolic application requires a classifier",
            ));
        };
        let span = self.node(declaration)?.span.clone();
        let checks = self.check_expression_bounds(expression, &span)?;
        if checks.iter().any(|check| !check.satisfied) {
            return Err(error(&span, "Symbolic application violates generic bounds"));
        }
        if self.classifier(declaration)? == TargetKind::Class {
            // Check all feature kinds even if the body never reads them. Call
            // source projection directly to avoid a members/application cycle.
            for member in self.project_member_types(expression, &span, true)? {
                let node = self.node(&member.declaration)?;
                if named(node, "eStructuralFeatures") {
                    self.symbolic_feature_kind(&member, kind(node)? == "EReference")?;
                }
            }
        }
        Ok(checks)
    }

    fn symbolic_feature_kind(
        &self,
        member: &SpecializedMember,
        reference: bool,
    ) -> Result<(), ImportError> {
        for signature in &member.signatures {
            if signature.feature != "eType" {
                continue;
            }
            for target in &signature.specialized {
                if (self.symbolic_kind(target, &member.span, 0)? == TargetKind::Class) != reference
                {
                    return Err(error(
                        &member.span,
                        "Symbolic target disagrees with attribute/reference kind",
                    ));
                }
            }
        }
        Ok(())
    }

    fn symbolic_span(&self, expression: &Type) -> SourceSpan {
        let source = match expression {
            Type::Classifier { declaration, .. } | Type::Parameter { declaration } => {
                declaration.source.clone()
            }
            Type::RigidParameter { identity } => identity.source.clone(),
            Type::Wildcard { .. } => self.standard_id.clone(),
        };
        SourceSpan {
            source,
            start: 0,
            end: 0,
        }
    }

    /// Value views preserve exact nominal argument identity. Constructor bound
    /// equivalence is deliberately not a value conversion or a retagging rule.
    pub(crate) fn symbolic_value_conforms(
        &self,
        actual: &Type,
        expected: &Type,
    ) -> Result<bool, ImportError> {
        let span = self.symbolic_span(actual);
        for expression in [actual, expected] {
            self.check_symbolic_value_term(expression, &span)?;
        }
        let mut pending = vec![actual.clone()];
        let mut seen = BTreeSet::new();
        let mut remaining = 16_384usize;
        while let Some(candidate) = pending.pop() {
            remaining = remaining.checked_sub(1).ok_or_else(|| {
                ImportError::new(
                    ErrorKind::ResourceLimit,
                    &span,
                    "Symbolic value conformance budget exceeded",
                )
            })?;
            if !seen.insert(candidate.clone()) {
                continue;
            }
            if candidate == *expected {
                return Ok(true);
            }
            if let Type::RigidParameter { identity } = &candidate {
                pending.extend(
                    self.rigid_parameter_bounds(identity, &span)?
                        .iter()
                        .cloned(),
                );
                continue;
            }
            let (
                Type::Classifier {
                    declaration: actual_id,
                    ..
                },
                Type::Classifier {
                    declaration: expected_id,
                    arguments,
                    raw,
                },
            ) = (&candidate, expected)
            else {
                continue;
            };
            if self.classifier(actual_id)? != TargetKind::Class
                || self.classifier(expected_id)? != TargetKind::Class
            {
                continue;
            }
            if arguments.is_empty()
                && *expected_id
                    == crate::definitions::schema::eobject_reference_top_declaration()
                        .map_err(|message| error(&span, message))?
            {
                return Ok(true);
            }
            if *raw && actual_id == expected_id {
                return Ok(true);
            }
            pending.extend(
                self.ancestry_expression(&candidate, &span)?
                    .into_iter()
                    .map(|ancestor| ancestor.expression),
            );
        }
        Ok(false)
    }

    fn check_symbolic_value_term(
        &self,
        expression: &Type,
        span: &SourceSpan,
    ) -> Result<(), ImportError> {
        match expression {
            Type::Classifier { raw: true, .. } => {
                let runtime =
                    crate::definitions::generic_types::RuntimeType::new(expression.clone())
                        .map_err(|message| error(span, message))?;
                self.runtime_members(&runtime)?;
            }
            Type::Classifier { .. } => {
                self.symbolic_application(expression)?;
            }
            _ => {
                self.symbolic_kind(expression, span, 0)?;
            }
        }
        Ok(())
    }

    /// Source-checked exact argument identity for invariant value positions.
    pub(crate) fn symbolic_value_argument_equal(
        &self,
        actual: &Type,
        expected: &Type,
    ) -> Result<bool, ImportError> {
        let span = self.symbolic_span(actual);
        for expression in [actual, expected] {
            self.check_argument_kind(expression, &span)?;
            self.check_symbolic_value_term(expression, &span)?;
        }
        Ok(actual == expected)
    }

    pub(crate) fn symbolic_members(&self, receiver: &Type) -> Result<SymbolicMembers, ImportError> {
        let bounds = self.symbolic_application(receiver)?;
        let span = self.symbolic_span(receiver);
        if self.symbolic_kind(receiver, &span, 0)? != TargetKind::Class {
            return Err(error(&span, "Symbolic member receiver requires an EClass"));
        }
        let members = self.project_member_types(receiver, &span, true)?;
        let parents = self
            .direct_parent_types(receiver, &span)?
            .into_iter()
            .map(|(ty, _)| ty)
            .collect();
        Ok(SymbolicMembers {
            receiver: receiver.clone(),
            parents,
            members,
            bounds,
        })
    }

    /// Project an exact nominal ancestor application; no frontend graph walk.
    pub(crate) fn symbolic_project_as(
        &self,
        actual: &Type,
        declaration: &DeclarationId,
    ) -> Result<Option<Type>, ImportError> {
        self.classifier(declaration)?;
        let span = self.symbolic_span(actual);
        let mut pending = vec![actual.clone()];
        let mut seen = BTreeSet::new();
        let mut selected = None;
        let mut remaining = 16_384usize;
        while let Some(candidate) = pending.pop() {
            remaining = remaining.checked_sub(1).ok_or_else(|| {
                ImportError::new(
                    ErrorKind::ResourceLimit,
                    &span,
                    "Symbolic ancestor projection budget exceeded",
                )
            })?;
            if !seen.insert(candidate.clone()) {
                continue;
            }
            if let Type::RigidParameter { identity } = &candidate {
                pending.extend(
                    self.rigid_parameter_bounds(identity, &span)?
                        .iter()
                        .cloned(),
                );
                continue;
            }
            self.symbolic_application(&candidate)?;
            let Type::Classifier {
                declaration: owner, ..
            } = &candidate
            else {
                return Err(error(
                    &span,
                    "Symbolic ancestor requires nominal classifier",
                ));
            };
            if owner == declaration {
                if selected.as_ref().is_some_and(|prior| prior != &candidate) {
                    return Err(error(&span, "Ambiguous symbolic ancestor application"));
                }
                selected = Some(candidate);
            } else if self.classifier(owner)? == TargetKind::Class {
                pending.extend(
                    self.ancestry_expression(&candidate, &span)?
                        .into_iter()
                        .map(|ancestor| ancestor.expression),
                );
            }
        }
        Ok(selected)
    }

    pub(crate) fn symbolic_feature(
        &self,
        receiver: &Type,
        name: &str,
    ) -> Result<SymbolicFeature, ImportError> {
        let report = self.symbolic_members(receiver)?;
        let mut selected: Option<SpecializedMember> = None;
        for member in report.members {
            if member.name != name || !named(self.node(&member.declaration)?, "eStructuralFeatures")
            {
                continue;
            }
            if let Some(prior) = &selected {
                if prior.declaration != member.declaration || prior.owner != member.owner {
                    return Err(error(&member.span, "Ambiguous inherited symbolic feature"));
                }
            } else {
                selected = Some(member);
            }
        }
        let member = selected
            .ok_or_else(|| error(&self.symbolic_span(receiver), "Unknown symbolic feature"))?;
        let node = self.node(&member.declaration)?;
        let reference = kind(node)? == "EReference";
        self.symbolic_feature_kind(&member, reference)?;
        let document = self
            .resources(&member.declaration)
            .document(&member.declaration.source)
            .map_err(|message| error(&member.span, message))?;
        let shape = crate::definitions::compiler::symbolic_feature_shape(
            node,
            &document.source.text,
            member.declaration.path.clone(),
        )?;
        Ok(SymbolicFeature {
            receiver: report.receiver,
            parents: report.parents,
            bounds: report.bounds,
            member,
            lower: shape.lower,
            upper: shape.upper,
            ordered: shape.ordered,
            unique: shape.unique,
            derived: shape.derived,
            changeable: shape.changeable,
            transient: shape.transient,
            volatile: shape.volatile,
            unsettable: shape.unsettable,
            reference,
        })
    }
}
