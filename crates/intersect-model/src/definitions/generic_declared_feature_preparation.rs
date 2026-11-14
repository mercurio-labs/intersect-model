//! Ordered provider-declaration selection over one immutable resolver.
use super::*;
use crate::definitions::{ecore::ParserObservation, generic_types::Type};

pub(crate) struct DeclaredFeaturePreparation<'a> {
    resolver: &'a Resolver,
    receiver: &'a Type,
    name: &'a str,
    span: &'a SourceSpan,
    ancestry: Option<inheritance::AncestryPreparation<'a>>,
    ancestors: Vec<InheritedType>,
    owner: usize,
    child: Option<usize>,
    selected: Option<DeclarationId>,
    complete: bool,
    failed: bool,
}
impl<'a> DeclaredFeaturePreparation<'a> {
    pub(crate) fn begin(
        resolver: &'a Resolver,
        receiver: &'a Type,
        name: &'a str,
        span: &'a SourceSpan,
    ) -> Self {
        Self {
            resolver,
            receiver,
            name,
            span,
            ancestry: Some(inheritance::AncestryPreparation::begin(
                resolver, receiver, span,
            )),
            ancestors: vec![],
            owner: 0,
            child: None,
            selected: None,
            complete: false,
            failed: false,
        }
    }
    pub(crate) fn advance(
        &mut self,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        if self.failed {
            return Err(error(self.span, "Declared feature preparation has failed"));
        }
        if self.complete {
            return Ok(true);
        }
        match self.advance_inner(observation) {
            Ok(ready) => Ok(ready),
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }
    fn advance_inner(
        &mut self,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        if let Some(ancestry) = self.ancestry.as_mut() {
            if ancestry.advance(observation)? {
                self.ancestors = self
                    .ancestry
                    .take()
                    .ok_or_else(|| error(self.span, "Missing feature ancestry"))?
                    .finish()?;
            }
            return Ok(false);
        }
        if self.owner > self.ancestors.len() {
            if self.selected.is_none() {
                return Err(error(self.span, "Unknown provider feature"));
            }
            self.complete = true;
            return Ok(true);
        }
        let owner = if self.owner == 0 {
            self.receiver
        } else {
            &self.ancestors[self.owner - 1].expression
        };
        let Type::Classifier { declaration, .. } = owner else {
            return Err(error(
                self.span,
                "Provider feature owner requires a classifier",
            ));
        };
        // Preserve the original source-selection and kind/error ordering. These
        // metadata helpers remain synchronous; the owner and child scan retain
        // their position. The old ordinary entry had no feature-scan observer.
        let class = self.resolver.node(declaration)?;
        if self.child.is_none() {
            if kind(class)? != "EClass" {
                return Err(error(
                    self.span,
                    "Provider feature owner requires an EClass",
                ));
            }
            self.child = Some(0);
            return Ok(false);
        }
        let index = self
            .child
            .ok_or_else(|| error(self.span, "Missing feature child position"))?;
        let Some(node) = class.children.get(index) else {
            self.owner += 1;
            self.child = None;
            return Ok(false);
        };
        self.child = Some(index + 1);
        if !named(node, "eStructuralFeatures") || attr(node, "name") != Some(self.name) {
            return Ok(false);
        }
        let mut identity = declaration.clone();
        identity.path.push(index);
        if self
            .selected
            .as_ref()
            .is_some_and(|prior| prior != &identity)
        {
            return Err(error(self.span, "Ambiguous inherited provider feature"));
        }
        self.selected = Some(identity);
        Ok(false)
    }
    pub(crate) fn finish(self) -> Result<DeclarationId, ImportError> {
        if self.failed {
            return Err(error(self.span, "Declared feature preparation has failed"));
        }
        if !self.complete {
            return Err(error(
                self.span,
                "Declared feature preparation is incomplete",
            ));
        }
        self.selected
            .ok_or_else(|| error(self.span, "Missing complete provider feature"))
    }
}
