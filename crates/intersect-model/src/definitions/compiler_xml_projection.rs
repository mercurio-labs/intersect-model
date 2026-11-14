//! Shared retained tree projection. Header field work remains synchronous.
use super::*;
use crate::definitions::{
    bundle::SourceSpan,
    ecore::{Node, ParserObservation},
};
struct Frame<'a> {
    node: &'a Node,
    path: Option<Vec<usize>>,
    xml: Option<Xml>,
    child: usize,
}
pub(super) struct XmlProjectionPreparation<'a> {
    source: &'a str,
    frames: Vec<Frame<'a>>,
    output: Option<Xml>,
    failed: bool,
    span: &'a SourceSpan,
}
impl<'a> XmlProjectionPreparation<'a> {
    pub(super) fn begin(node: &'a Node, source: &'a str, path: Vec<usize>) -> Self {
        Self {
            source,
            frames: vec![Frame {
                node,
                path: Some(path),
                xml: None,
                child: 0,
            }],
            output: None,
            failed: false,
            span: &node.opening,
        }
    }
    pub(super) fn advance(
        &mut self,
        observation: &ParserObservation<'_, '_>,
        visit: &impl Fn(&Xml) -> Result<(), ImportError>,
    ) -> Result<bool, ImportError> {
        if self.failed {
            return Err(ImportError::new(
                ErrorKind::InvalidDefinition,
                self.span,
                "XML projection has failed",
            ));
        }
        if self.output.is_some() {
            return Ok(true);
        }
        let result = self.advance_inner(observation, visit);
        if result.is_err() {
            self.failed = true;
        }
        result.map(|()| self.output.is_some())
    }
    fn advance_inner(
        &mut self,
        observation: &ParserObservation<'_, '_>,
        visit: &impl Fn(&Xml) -> Result<(), ImportError>,
    ) -> Result<(), ImportError> {
        let invalid = |message| ImportError::new(ErrorKind::InvalidDefinition, self.span, message);
        let mut frame = self
            .frames
            .pop()
            .ok_or_else(|| invalid("Missing XML projection frame"))?;
        if frame.xml.is_none() {
            let path = frame
                .path
                .take()
                .ok_or_else(|| invalid("Missing XML projection path"))?;
            frame.xml = Some(Xml::header_from_node_at_observed(
                frame.node,
                self.source,
                path,
                observation,
            )?);
            self.frames.push(frame);
            return Ok(());
        }
        let xml = frame
            .xml
            .as_mut()
            .ok_or_else(|| invalid("Missing projected XML header"))?;
        if let Some(child) = frame.node.children.get(frame.child) {
            observation.work(1, &frame.node.opening.source)?;
            observation.serialized(&xml.declaration.path, &frame.node.opening.source)?;
            observation.reserve(32 + 32, &frame.node.opening.source)?;
            let mut path = xml.declaration.path.clone();
            path.push(frame.child);
            frame.child += 1;
            self.frames.push(frame);
            self.frames.push(Frame {
                node: child,
                path: Some(path),
                xml: None,
                child: 0,
            });
            return Ok(());
        }
        visit(xml)?;
        let xml = frame
            .xml
            .take()
            .ok_or_else(|| invalid("Missing completed XML projection"))?;
        if let Some(parent) = self.frames.last_mut() {
            parent
                .xml
                .as_mut()
                .ok_or_else(|| invalid("Missing XML projection parent"))?
                .children
                .push(xml);
        } else {
            self.output = Some(xml);
        }
        Ok(())
    }
    pub(super) fn finish(self) -> Result<Xml, ImportError> {
        if self.failed {
            return Err(ImportError::new(
                ErrorKind::InvalidDefinition,
                self.span,
                "XML projection has failed",
            ));
        }
        self.output.ok_or_else(|| {
            ImportError::new(
                ErrorKind::InvalidDefinition,
                self.span,
                "XML projection is incomplete",
            )
        })
    }
}
