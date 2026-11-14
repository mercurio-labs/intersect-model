//! Receiver-specific generic ancestry. Bound satisfaction is checked separately.
use super::*;
use crate::definitions::generic_types::{ParameterBinding, Substitution, Type, TypeUse};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InheritedType {
    pub expression: Type,
    /// First declared traversal path, including the receiver's classifier.
    pub path: Vec<DeclarationId>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InheritedTypes {
    pub receiver: TypeUse,
    /// Distinct substituted applications, including raw/applied equivalent paths.
    pub supertypes: Vec<InheritedType>,
}
pub(super) type StructuralAncestryCache =
    std::cell::RefCell<BTreeMap<Type, std::rc::Rc<Vec<InheritedType>>>>;

pub(crate) enum AncestryView {
    Owned(Vec<InheritedType>),
    Shared(std::rc::Rc<Vec<InheritedType>>),
}
impl AncestryView {
    pub(crate) fn as_slice(&self) -> &[InheritedType] {
        match self {
            Self::Owned(rows) => rows,
            Self::Shared(rows) => rows,
        }
    }
}
#[path = "generic_inheritance_preparation.rs"]
mod preparation;
pub(crate) use preparation::{AncestryPreparation, AncestryTraversal};
#[path = "generic_ancestry_view_preparation.rs"]
mod view_preparation;
pub(crate) use view_preparation::AncestryViewPreparation;
#[path = "generic_direct_parent_preparation.rs"]
mod direct_parent_preparation;
pub(crate) use direct_parent_preparation::DirectParentPreparation;

struct Traversal {
    path: Vec<DeclarationId>,
    completed: BTreeMap<Type, Vec<Type>>,
    result: Vec<InheritedType>,
    remaining: usize,
}
impl Resolver {
    pub fn inherited_types(&self, usage: &DeclarationId) -> Result<InheritedTypes, ImportError> {
        let receiver = self.type_expression(usage)?;
        let supertypes = self.ancestry_expression(&receiver.expression, &receiver.span)?;
        Ok(InheritedTypes {
            receiver,
            supertypes,
        })
    }
    pub(super) fn ancestry_expression(
        &self,
        ty: &Type,
        span: &SourceSpan,
    ) -> Result<Vec<InheritedType>, ImportError> {
        self.ancestry_expression_observed(
            ty,
            span,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    pub(crate) fn ancestry_expression_observed(
        &self,
        ty: &Type,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<InheritedType>, ImportError> {
        let mut preparation = AncestryPreparation::begin(self, ty, span);
        while !preparation.advance(observation)? {}
        preparation.finish()
    }
    // Reuse only successful, argument-free EClass ancestry from this immutable
    // preparation. Consumers still perform every application and bound check.
    pub(super) fn ancestry_view_observed(
        &self,
        ty: &Type,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<AncestryView, ImportError> {
        let mut preparation = AncestryViewPreparation::begin(self);
        while !preparation.advance(ty, span, observation)? {}
        preparation.finish(span)
    }
    pub(super) fn direct_parent_types(
        &self,
        ty: &Type,
        span: &SourceSpan,
    ) -> Result<Vec<(Type, SourceSpan)>, ImportError> {
        self.direct_parent_types_observed(
            ty,
            span,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    pub(crate) fn direct_parent_types_observed(
        &self,
        ty: &Type,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<(Type, SourceSpan)>, ImportError> {
        let mut preparation = DirectParentPreparation::begin(self);
        while !preparation.advance(ty, span, observation)? {}
        preparation.finish(span)
    }
    // Borrowed duplicate keys cannot outlive the owned result. This final
    // predicate remains synchronous; retention must preserve both ownership
    // profiles and the original logical receipts without a second identity copy.
    fn check_direct_parent_duplicates_observed(
        &self,
        result: &[(Type, SourceSpan)],
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        // Equivalent ancestors reached through different parents are valid diamonds,
        // but duplicate direct superclass declarations are invalid in Ecore.
        let shared_inputs = self.compiled_feature_inputs_selected(span)?;
        let mut direct = std::collections::BTreeSet::new();
        let mut borrowed_direct = std::collections::BTreeSet::new();
        for (parent, location) in result {
            observation.work(1, &span.source)?;
            if let Type::Classifier { declaration, .. } = parent {
                let inserted = if shared_inputs {
                    // The fresh result owns every identity for this check's lifetime.
                    // Retain only tree entries, without another declaration copy.
                    observation.reserve(128, &span.source)?;
                    borrowed_direct.insert(declaration)
                } else {
                    observation.serialized(declaration, &span.source)?;
                    observation.reserve(128, &span.source)?;
                    direct.insert(declaration.clone())
                };
                if !inserted {
                    return Err(error(location, "Duplicate direct generic superclass"));
                }
            }
        }
        Ok(())
    }
}

fn inheritance_budget(
    traversal: &mut Traversal,
    span: &SourceSpan,
    depth: usize,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    observation.work(1, &span.source)?;
    if depth >= 128 || traversal.remaining == 0 {
        return Err(ImportError::new(
            ErrorKind::ResourceLimit,
            span,
            "Generic inheritance traversal budget exceeded",
        ));
    }
    traversal.remaining -= 1;
    Ok(())
}

/// EClass ancestor-list equivalence only; never use as a bound or assignment relation.
struct AncestorEquivalence<'a, 'b, 'c, 'd> {
    resolver: &'a Resolver,
    context: &'a [Type],
    traversal: &'b mut Traversal,
    span: &'a SourceSpan,
    observation: &'a crate::definitions::ecore::ParserObservation<'c, 'd>,
}
impl<'a, 'b, 'c, 'd> AncestorEquivalence<'a, 'b, 'c, 'd> {
    // Prepared resolution selects an existing immutable expression. The
    // standalone path preserves owned selection and its exact copy receipts.
    fn resolve<'t>(
        &mut self,
        ty: &'t Type,
        depth: usize,
    ) -> Result<std::borrow::Cow<'t, Type>, ImportError>
    where
        'a: 't,
    {
        inheritance_budget(self.traversal, self.span, depth, self.observation)?;
        let Type::Parameter { declaration } = ty else {
            return self.resolved_view(ty);
        };
        let owner = self
            .resolver
            .parameter_owner_observed(declaration, self.observation)?;
        let formals = self
            .resolver
            .formal_info_observed(&owner, self.observation)?;
        let mut index = None;
        for (position, formal) in formals.iter().enumerate() {
            self.observation.work(1, &self.span.source)?;
            if &formal.declaration == declaration {
                index = Some(position);
                break;
            }
        }
        let Some(index) = index else {
            return Err(error(self.span, "Unknown ancestor type parameter"));
        };
        for application in self.context {
            self.observation.work(1, &self.span.source)?;
            if let Type::Classifier {
                declaration,
                arguments,
                ..
            } = application
                && declaration == &owner
                && let Some(actual) = arguments.get(index)
            {
                if actual == ty {
                    return self.resolved_view(ty);
                }
                return self.resolve(actual, depth + 1);
            }
        }
        self.resolved_view(ty)
    }
    fn resolved_view<'t>(&self, ty: &'t Type) -> Result<std::borrow::Cow<'t, Type>, ImportError> {
        if self.resolver.prepared_formals.get().is_some() {
            Ok(std::borrow::Cow::Borrowed(ty))
        } else {
            // Preserve the standalone owned path and its exact copy receipts.
            self.observation.serialized(ty, &self.span.source)?;
            Ok(std::borrow::Cow::Owned(ty.clone()))
        }
    }
    fn equivalent(&mut self, left: &Type, right: &Type, depth: usize) -> Result<bool, ImportError> {
        inheritance_budget(self.traversal, self.span, depth, self.observation)?;
        if left == right {
            return Ok(true);
        }
        let left = self.resolve(left, depth + 1)?;
        let right = self.resolve(right, depth + 1)?;
        match (left.as_ref(), right.as_ref()) {
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
                    match self.resolver.classifier_domain_relation_observed(
                        a,
                        b,
                        true,
                        self.observation,
                    )? {
                        Some(true) => {}
                        Some(false) => return Ok(false),
                        None if self.resolver.classifier_observed(a, self.observation)?
                            == TargetKind::Datatype
                            && self.resolver.classifier_observed(b, self.observation)?
                                == TargetKind::Datatype =>
                        {
                            return Err(ImportError::new(
                                ErrorKind::Unsupported,
                                self.span,
                                "Datatype ancestor equivalence requires an explicit provider contract",
                            ));
                        }
                        None => return Ok(false),
                    }
                }
                if aa.len() != ba.len() {
                    // Preserve existing raw/applied ancestor agreement, but
                    // differing concrete constructor arities cannot agree.
                    return Ok(aa.is_empty() || ba.is_empty());
                }
                for (a, b) in aa.iter().zip(ba) {
                    if !self.equivalent(a, b, depth + 1)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (Type::Parameter { declaration: a }, Type::Parameter { declaration: b }) => Ok(a == b),
            (Type::Wildcard { .. }, Type::Wildcard { .. }) => Ok(true),
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod prepared_parent_tests {
    use super::*;
    use crate::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        conversion::{InvocationContext, InvocationObserver, ProviderId},
        ecore::{ObservedImportError, ParserObservation},
    };
    use std::{cell::Cell, rc::Rc};

    struct Storage {
        bytes: Cell<usize>,
        limit: usize,
    }
    impl InvocationObserver for Storage {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            panic!("parent projection must not invoke providers")
        }
        fn retain_storage(&self, bytes: usize) -> Result<(), String> {
            let next = self.bytes.get() + bytes;
            if next > self.limit {
                return Err("parent storage denied".into());
            }
            self.bytes.set(next);
            Ok(())
        }
    }

    fn structural_fixture() -> DefinitionBundle {
        DefinitionBundle {
            version: 1,
            sources: vec![DefinitionSource::new(
                "memory:structural-ancestry.ecore",
                SourceKind::Ecore,
                "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:structural-ancestry'><eClassifiers x:type='e:EClass' name='Root'><eTypeParameters name='T'/></eClassifiers><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root'/><eClassifiers x:type='e:EClass' name='Base'><eTypeParameters name='U'/></eClassifiers></e:EPackage>",
            )],
            namespaces: vec![],
        }
    }
    fn structural_resolver(bundle: &DefinitionBundle) -> Resolver {
        let mut resolver = Resolver::new(bundle).unwrap();
        let invocation = InvocationContext::default();
        resolver
            .resolve_materialization_inventory_observed(&ParserObservation::new(&invocation))
            .unwrap();
        resolver
    }
    fn structural_child(bundle: &DefinitionBundle) -> Type {
        Type::Classifier {
            declaration: DeclarationId {
                source: bundle.sources[0].id.clone(),
                path: vec![1],
            },
            arguments: vec![],
            raw: false,
        }
    }
    struct CacheMeter {
        bytes: Cell<usize>,
        work: Cell<usize>,
        limit: usize,
        deny_work: Option<usize>,
    }
    impl InvocationObserver for CacheMeter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            panic!("structural ancestry cannot invoke providers")
        }
        fn retain_storage(&self, bytes: usize) -> Result<(), String> {
            let next = self
                .bytes
                .get()
                .checked_add(bytes)
                .ok_or("ancestry storage overflow")?;
            if next > self.limit {
                return Err("ancestry storage denied".into());
            }
            self.bytes.set(next);
            Ok(())
        }
        fn work(&self, steps: usize) -> Result<(), String> {
            let next = self
                .work
                .get()
                .checked_add(steps)
                .ok_or("ancestry work overflow")?;
            if self.deny_work.is_some_and(|limit| next >= limit) {
                return Err("ancestry work denied".into());
            }
            self.work.set(next);
            Ok(())
        }
    }
    #[test]
    fn structural_ancestry_shares_complete_reports_and_admits_cache_before_publication() {
        let bundle = structural_fixture();
        let child = structural_child(&bundle);
        let run = |limit| {
            let resolver = structural_resolver(&bundle);
            let span = &resolver
                .node(&DeclarationId {
                    source: bundle.sources[0].id.clone(),
                    path: vec![1],
                })
                .unwrap()
                .span;
            let receipt = Rc::new(Storage {
                bytes: Cell::new(0),
                limit,
            });
            let invocation = InvocationContext::new(receipt.clone());
            let observation = ParserObservation::new(&invocation);
            let result = observation
                .finish(resolver.ancestry_view_observed(&child, span, &observation))
                .map(|view| view.as_slice().to_vec());
            let entries = resolver.prepared_ancestry.get().unwrap().borrow().len();
            (result, receipt.bytes.get(), entries)
        };
        let (expected, bytes, entries) = run(usize::MAX);
        assert_eq!(entries, 1);
        assert!(bytes > 0);
        assert_eq!(run(bytes).0, expected);
        let denied = run(bytes - 1);
        assert_eq!(
            denied.0.unwrap_err(),
            ObservedImportError::Observation("parent storage denied".into())
        );
        assert_eq!(denied.2, 0);
        assert_eq!(run(bytes).0, expected);
        let resolver = structural_resolver(&bundle);
        let span = &resolver
            .node(&DeclarationId {
                source: bundle.sources[0].id.clone(),
                path: vec![1],
            })
            .unwrap()
            .span;
        let invocation = InvocationContext::default();
        let observation = ParserObservation::new(&invocation);
        let AncestryView::Shared(first) = resolver
            .ancestry_view_observed(&child, span, &observation)
            .unwrap()
        else {
            panic!("shared ancestry required")
        };
        let zero = Rc::new(Storage {
            bytes: Cell::new(0),
            limit: 0,
        });
        let invocation = InvocationContext::new(zero.clone());
        let observation = ParserObservation::new(&invocation);
        let AncestryView::Shared(second) = resolver
            .ancestry_view_observed(&child, span, &observation)
            .unwrap()
        else {
            panic!("shared ancestry required")
        };
        assert!(Rc::ptr_eq(&first, &second));
        assert_eq!(zero.bytes.get(), 0);
        let fresh = Resolver::new(&bundle).unwrap();
        assert_eq!(
            first.as_ref(),
            &fresh.ancestry_expression(&child, span).unwrap()
        );
    }
    #[test]
    fn structural_ancestry_cancels_every_preparation_checkpoint_and_cached_hit() {
        let bundle = structural_fixture();
        let source = bundle.sources[0].clone();
        let child = structural_child(&bundle);
        let run = |deny_work| {
            let resolver = structural_resolver(&bundle);
            let span = &resolver
                .node(&DeclarationId {
                    source: source.id.clone(),
                    path: vec![1],
                })
                .unwrap()
                .span;
            let meter = Rc::new(CacheMeter {
                bytes: Cell::new(0),
                work: Cell::new(0),
                limit: usize::MAX,
                deny_work,
            });
            let invocation = InvocationContext::new(meter.clone());
            let observation = ParserObservation::new(&invocation);
            let result = observation
                .finish(resolver.ancestry_view_observed(&child, span, &observation))
                .map(|view| view.as_slice().to_vec());
            let entries = resolver.prepared_ancestry.get().unwrap().borrow().len();
            (result, meter.work.get(), entries)
        };
        let (expected, steps, _) = run(None);
        assert!(steps > 0);
        for step in 1..=steps {
            let denied = run(Some(step));
            assert_eq!(
                denied.0.unwrap_err(),
                ObservedImportError::Observation("ancestry work denied".into())
            );
            assert_eq!(denied.2, 0);
        }
        assert_eq!(run(None).0, expected);
        let resolver = structural_resolver(&bundle);
        let span = &resolver
            .node(&DeclarationId {
                source: source.id.clone(),
                path: vec![1],
            })
            .unwrap()
            .span;
        let warm = InvocationContext::default();
        resolver
            .ancestry_view_observed(&child, span, &ParserObservation::new(&warm))
            .unwrap();
        let meter = Rc::new(CacheMeter {
            bytes: Cell::new(0),
            work: Cell::new(0),
            limit: 0,
            deny_work: Some(1),
        });
        let invocation = InvocationContext::new(meter);
        let observation = ParserObservation::new(&invocation);
        assert!(
            matches!(observation.finish(resolver.ancestry_view_observed(&child, span, &observation)), Err(ObservedImportError::Observation(message)) if message == "ancestry work denied")
        );
        assert_eq!(
            resolver
                .ancestry_view_observed(&child, span, &ParserObservation::new(&warm))
                .unwrap()
                .as_slice(),
            expected.unwrap()
        );
        assert_eq!(source, bundle.sources[0]);
    }
    #[test]
    fn structural_ancestry_invalidates_changed_contexts_and_keeps_applied_types_owned() {
        let bundle = structural_fixture();
        let resolver = structural_resolver(&bundle);
        let child = structural_child(&bundle);
        let span = &resolver
            .node(&DeclarationId {
                source: bundle.sources[0].id.clone(),
                path: vec![1],
            })
            .unwrap()
            .span;
        let invocation = InvocationContext::default();
        let observation = ParserObservation::new(&invocation);
        resolver
            .ancestry_view_observed(&child, span, &observation)
            .unwrap();
        for changed in [
            resolver
                .clone()
                .with_conversions(&crate::definitions::conversion::ConversionRegistry::default())
                .unwrap(),
            resolver
                .clone()
                .with_constructors(&crate::definitions::conversion::ConstructorRegistry::default())
                .unwrap(),
            resolver.clone().with_rigid_parameters(vec![]).unwrap(),
        ] {
            assert!(changed.prepared_ancestry.get().is_none());
            assert!(matches!(
                changed
                    .ancestry_view_observed(&child, span, &observation)
                    .unwrap(),
                AncestryView::Owned(_)
            ));
        }
        let applied = Type::Classifier {
            declaration: DeclarationId {
                source: bundle.sources[0].id.clone(),
                path: vec![2],
            },
            arguments: vec![child],
            raw: false,
        };
        assert!(matches!(
            resolver
                .ancestry_view_observed(&applied, span, &observation)
                .unwrap(),
            AncestryView::Owned(_)
        ));
        assert_eq!(resolver.prepared_ancestry.get().unwrap().borrow().len(), 1);
    }
    #[test]
    fn structural_ancestry_reuse_does_not_cache_application_warnings_or_spans() {
        let bundle = structural_fixture();
        let resolver = structural_resolver(&bundle);
        let child = structural_child(&bundle);
        let span = resolver
            .node(&DeclarationId {
                source: bundle.sources[0].id.clone(),
                path: vec![1],
            })
            .unwrap()
            .span
            .clone();
        let invocation = InvocationContext::default();
        let observation = ParserObservation::new(&invocation);
        let first = resolver
            .runtime_application_checks_observed(&child, &span, &observation)
            .unwrap();
        assert_eq!(first.1.len(), 1);
        assert_eq!(first.1[0].code, "raw_type");
        assert_eq!(first.1[0].span, span);
        let mut later = span.clone();
        later.start += 1;
        let repeated = resolver
            .runtime_application_checks_observed(&child, &later, &observation)
            .unwrap();
        let fresh = Resolver::new(&bundle)
            .unwrap()
            .runtime_application_checks_observed(&child, &later, &observation)
            .unwrap();
        assert_eq!(repeated, fresh);
        assert_eq!(repeated.1[0].span, later);
        assert_ne!(repeated.1[0].span, first.1[0].span);
        assert_eq!(resolver.prepared_ancestry.get().unwrap().borrow().len(), 2);
    }

    #[test]
    fn ancestor_resolution_borrows_exact_input_and_context_type_graphs() {
        let source = DefinitionSource::new(
            "memory:ancestor-borrows.ecore",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:ancestor-borrows'><eClassifiers x:type='e:EClass' name='Root'/><eClassifiers x:type='e:EClass' name='Base'><eTypeParameters name='T'/></eClassifiers></e:EPackage>",
        );
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let mut resolver = Resolver::new(&bundle).unwrap();
        let preparation = InvocationContext::default();
        resolver
            .resolve_materialization_inventory_observed(&ParserObservation::new(&preparation))
            .unwrap();
        let root = Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![0],
            },
            arguments: vec![],
            raw: false,
        };
        let context = vec![Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![1],
            },
            arguments: vec![root.clone()],
            raw: false,
        }];
        let parameter = Type::Parameter {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![1, 0],
            },
        };
        let span = &resolver
            .node(&DeclarationId {
                source: source.id.clone(),
                path: vec![1],
            })
            .unwrap()
            .span;
        let receipt = Rc::new(Storage {
            bytes: Cell::new(0),
            limit: usize::MAX,
        });
        let invocation = InvocationContext::new(receipt.clone());
        let observation = ParserObservation::new(&invocation);
        let mut traversal = Traversal {
            path: vec![],
            completed: BTreeMap::new(),
            result: vec![],
            remaining: 16_384,
        };
        let mut equivalence = AncestorEquivalence {
            resolver: &resolver,
            context: &context,
            traversal: &mut traversal,
            span,
            observation: &observation,
        };
        let selected = equivalence.resolve(&parameter, 0).unwrap();
        let Type::Classifier { arguments, .. } = &context[0] else {
            panic!("classifier context required")
        };
        assert!(std::ptr::eq(selected.as_ref(), &arguments[0]));
        assert_eq!(selected.as_ref(), &root);
        let before = receipt.bytes.get();
        assert!(std::ptr::eq(
            equivalence.resolve(&root, 0).unwrap().as_ref(),
            &root
        ));
        assert_eq!(receipt.bytes.get(), before);
        assert!(equivalence.equivalent(&parameter, &root, 0).unwrap());
        assert_eq!(source, bundle.sources[0]);
    }

    #[test]
    fn prepared_ancestry_keeps_raw_reverse_order_paths_errors_and_storage_recovery() {
        for (parents, valid) in [
            ("#//Raw #//Left #//Right", true),
            ("#//Left #//Raw #//Right", true),
            ("#//Left #//Right #//Raw", false),
        ] {
            let text = format!(
                "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:prepared-raw-order'><eClassifiers x:type='e:EClass' name='Root'/><eClassifiers x:type='e:EClass' name='Other'/><eClassifiers x:type='e:EClass' name='Base'><eTypeParameters name='T'/></eClassifiers><eClassifiers x:type='e:EClass' name='Raw' eSuperTypes='#//Base'/><eClassifiers x:type='e:EClass' name='Left'><eGenericSuperTypes eClassifier='#//Base'><eTypeArguments eClassifier='#//Root'/></eGenericSuperTypes></eClassifiers><eClassifiers x:type='e:EClass' name='Right'><eGenericSuperTypes eClassifier='#//Base'><eTypeArguments eClassifier='#//Other'/></eGenericSuperTypes></eClassifiers><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='{parents}'/></e:EPackage>"
            );
            let source =
                DefinitionSource::new("memory:prepared-raw-order.ecore", SourceKind::Ecore, text);
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source.clone()],
                namespaces: vec![],
            };
            let fresh = Resolver::new(&bundle).unwrap();
            let mut prepared = Resolver::new(&bundle).unwrap();
            let preparation = InvocationContext::default();
            prepared
                .resolve_materialization_inventory_observed(&ParserObservation::new(&preparation))
                .unwrap();
            let child = DeclarationId {
                source: source.id.clone(),
                path: vec![6],
            };
            let receiver = Type::Classifier {
                declaration: child.clone(),
                arguments: vec![],
                raw: false,
            };
            let span = &fresh.node(&child).unwrap().span;
            let run = |resolver: &Resolver, limit| {
                let receipt = Rc::new(Storage {
                    bytes: Cell::new(0),
                    limit,
                });
                let invocation = InvocationContext::new(receipt.clone());
                let observation = ParserObservation::new(&invocation);
                let result = observation.finish(resolver.ancestry_expression_observed(
                    &receiver,
                    span,
                    &observation,
                ));
                (result, receipt.bytes.get())
            };
            let (ordinary, ordinary_bytes) = run(&fresh, usize::MAX);
            let (borrowed, borrowed_bytes) = run(&prepared, usize::MAX);
            assert_eq!(borrowed, ordinary);
            assert_eq!(borrowed.is_ok(), valid);
            assert!(borrowed_bytes > 0 && borrowed_bytes < ordinary_bytes);
            if valid {
                assert_eq!(run(&prepared, borrowed_bytes).0, borrowed);
                assert_eq!(
                    run(&prepared, borrowed_bytes - 1).0.unwrap_err(),
                    ObservedImportError::Observation("parent storage denied".into())
                );
                assert_eq!(run(&prepared, borrowed_bytes).0, borrowed);
            }
            assert_eq!(source, bundle.sources[0]);
        }
    }

    #[test]
    fn concrete_parent_bindings_borrow_checked_formals_and_admit_exact_owned_identities() {
        let source = DefinitionSource::new(
            "memory:prepared-parents.ecore",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:parents'><eClassifiers x:type='e:EClass' name='Root'/><eClassifiers x:type='e:EClass' name='Base'><eTypeParameters name='T'/></eClassifiers><eClassifiers x:type='e:EClass' name='Child'><eTypeParameters name='U'/><eGenericSuperTypes eClassifier='#//Base'><eTypeArguments eTypeParameter='#//Child/U'/></eGenericSuperTypes></eClassifiers></e:EPackage>",
        );
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let fresh = Resolver::new(&bundle).unwrap();
        let mut prepared = Resolver::new(&bundle).unwrap();
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        prepared
            .resolve_materialization_inventory_observed(&observation)
            .unwrap();
        let root = Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![0],
            },
            arguments: vec![],
            raw: false,
        };
        let child = Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![2],
            },
            arguments: vec![root.clone()],
            raw: false,
        };
        let span = &fresh
            .node(&DeclarationId {
                source: source.id.clone(),
                path: vec![2],
            })
            .unwrap()
            .span;
        let run = |resolver: &Resolver, limit| {
            let receipt = Rc::new(Storage {
                bytes: Cell::new(0),
                limit,
            });
            let context = InvocationContext::new(receipt.clone());
            let observation = ParserObservation::new(&context);
            let result = observation.finish(resolver.direct_parent_types_observed(
                &child,
                span,
                &observation,
            ));
            (result, receipt.bytes.get())
        };
        let (ordinary, ordinary_bytes) = run(&fresh, usize::MAX);
        let (borrowed, borrowed_bytes) = run(&prepared, usize::MAX);
        assert_eq!(borrowed, ordinary);
        let parents = borrowed.unwrap();
        assert_eq!(parents.len(), 1);
        assert_eq!(
            parents[0].0,
            Type::Classifier {
                declaration: DeclarationId {
                    source: source.id.clone(),
                    path: vec![1]
                },
                arguments: vec![root],
                raw: false
            }
        );
        assert!(borrowed_bytes > 0);
        assert!(borrowed_bytes < ordinary_bytes);
        assert_eq!(run(&prepared, borrowed_bytes).0.unwrap(), parents);
        assert_eq!(
            run(&prepared, borrowed_bytes - 1).0.unwrap_err(),
            ObservedImportError::Observation("parent storage denied".into())
        );
        assert_eq!(run(&prepared, borrowed_bytes).0.unwrap(), parents);
        assert_eq!(source, bundle.sources[0]);
    }
}

#[cfg(test)]
#[path = "generic_inheritance_preparation_tests.rs"]
mod preparation_tests;

#[cfg(test)]
#[path = "generic_direct_parent_preparation_tests.rs"]
mod direct_parent_tests;
