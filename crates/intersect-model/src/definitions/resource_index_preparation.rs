//! Retained traversal of the shared package and XMI identity predicates.
//! Documents remain immutable; borrowed nodes/names never confer type authority.
use super::*;
#[derive(Clone, Copy)]
enum Phase {
    Roots,
    Packages,
    Ids,
    Complete,
    Failed,
}
struct PackageFrame<'doc> {
    node: &'doc Node,
    namespace: &'doc str,
    names: BTreeSet<(&'doc str, &'doc str)>,
    next: usize,
    depth: usize,
}
struct IdFrame<'doc> {
    node: &'doc Node,
    next: usize,
    visited: bool,
}
struct ResourceIndexCursor<'doc> {
    document: &'doc Document,
    mode: RootMode,
    phase: Phase,
    root_index: usize,
    roots: Vec<DeclarationId>,
    packages: BTreeMap<String, DeclarationId>,
    ids: BTreeMap<String, DeclarationId>,
    path: Vec<usize>,
    package_frames: Vec<PackageFrame<'doc>>,
    id_frames: Vec<IdFrame<'doc>>,
}
self_cell::self_cell!(
    struct IndexedDocument {
        owner: Document,
        #[covariant]
        dependent: ResourceIndexCursor,
    }
);
pub(super) struct OwnedResourceIndexPreparation {
    holder: IndexedDocument,
}
const STATE_BYTES: usize = 4096;
const _: () = assert!(
    std::mem::size_of::<Document>() + std::mem::size_of::<ResourceIndexCursor<'static>>() + 128
        <= STATE_BYTES
);
impl OwnedResourceIndexPreparation {
    pub(super) fn new(
        document: Document,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        let mode = resource_root_mode(&document.root)?;
        observation.reserve(STATE_BYTES, &document.source.id)?;
        let holder = IndexedDocument::new(document, |document| ResourceIndexCursor {
            document,
            mode,
            phase: Phase::Roots,
            root_index: 0,
            roots: Vec::new(),
            packages: BTreeMap::new(),
            ids: BTreeMap::new(),
            path: Vec::new(),
            package_frames: Vec::new(),
            id_frames: Vec::new(),
        });
        Ok(Self { holder })
    }
    pub(super) fn advance(
        &mut self,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        self.holder
            .with_dependent_mut(|_, cursor| cursor.advance(observation))
    }
    pub(super) fn finish(self) -> Result<Resource, ImportError> {
        let mut holder = self.holder;
        let (roots, packages, ids) = holder.with_dependent_mut(|_, cursor| {
            if !matches!(cursor.phase, Phase::Complete) {
                return Err(invalid(
                    &cursor.document.root,
                    None,
                    "",
                    "Resource indexing is not complete",
                ));
            }
            Ok((
                std::mem::take(&mut cursor.roots),
                std::mem::take(&mut cursor.packages),
                std::mem::take(&mut cursor.ids),
            ))
        })?;
        Ok(Resource {
            document: holder.into_owner(),
            roots,
            packages,
            ids,
        })
    }
}
// Portable logical slots: traversal positions use32 bytes; borrowed frame
// records use256 bytes. Native pointer widths never alter admission receipts.
const _: () = assert!(
    std::mem::size_of::<PackageFrame<'static>>() <= 128
        && std::mem::size_of::<IdFrame<'static>>() <= 128
);
fn slot<T>(
    values: &mut Vec<T>,
    bytes: usize,
    source: &SourceId,
    observation: &ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    if values.len() == values.capacity() {
        observation.reserve(bytes, source)?;
        values
            .try_reserve_exact(1)
            .map_err(|_| resolution_error(source, "Cannot retain resource cursor".into()))?;
    }
    Ok(())
}
impl<'doc> ResourceIndexCursor<'doc> {
    fn advance(&mut self, observation: &ParserObservation<'_, '_>) -> Result<bool, ImportError> {
        if matches!(self.phase, Phase::Failed) {
            return Err(invalid(
                &self.document.root,
                None,
                "",
                "Resource indexing has failed",
            ));
        }
        let result = self.advance_inner(observation);
        if result.is_err() {
            self.phase = Phase::Failed;
        }
        result
    }
    fn enter_package(
        &mut self,
        node: &'doc Node,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        let source = &self.document.source.id;
        let namespace =
            index_package_header(node, source, &self.path, &mut self.packages, observation)?;
        slot(&mut self.package_frames, 256, source, observation)?;
        self.package_frames.push(PackageFrame {
            node,
            namespace,
            names: BTreeSet::new(),
            next: 0,
            depth: self.path.len(),
        });
        Ok(())
    }
    fn advance_inner(
        &mut self,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        let source = &self.document.source.id;
        observation.work(0, source)?;
        if matches!(self.phase, Phase::Complete) {
            return Ok(true);
        }
        observation.work(1, source)?;
        match self.phase {
            Phase::Roots => match self.mode {
                RootMode::Single => {
                    self.roots
                        .push(retain_resource_root(source, &[], observation)?);
                    observation.reserve(128, source)?;
                    self.root_index = 0;
                    self.phase = Phase::Packages;
                }
                RootMode::Xmi => {
                    if let Some(node) = self.document.root.children.get(self.root_index) {
                        observation.work(1, source)?;
                        check_xmi_package_root(node)?;
                        self.roots.push(retain_resource_root(
                            source,
                            &[self.root_index],
                            observation,
                        )?);
                        self.root_index += 1;
                    } else {
                        observation.reserve(128, source)?;
                        self.root_index = 0;
                        self.phase = Phase::Packages;
                    }
                }
            },
            Phase::Packages => {
                if let Some(frame) = self.package_frames.last_mut() {
                    if let Some(child) = frame.node.children.get(frame.next) {
                        let index = frame.next;
                        let nested = check_package_member(
                            child,
                            frame.namespace,
                            &mut frame.names,
                            source,
                            observation,
                        )?;
                        frame.next += 1;
                        if nested {
                            slot(&mut self.path, 32, source, observation)?;
                            self.path.push(index);
                            self.enter_package(child, observation)?;
                        }
                    } else {
                        let depth = frame.depth;
                        self.package_frames.pop();
                        self.path.truncate(depth.saturating_sub(1));
                        if self.package_frames.is_empty() {
                            self.root_index += 1;
                        }
                    }
                } else if let Some(root) = self.roots.get(self.root_index) {
                    observation.work(root.path.len().max(1), source)?;
                    self.path.clear();
                    for index in &root.path {
                        slot(&mut self.path, 32, source, observation)?;
                        self.path.push(*index);
                    }
                    let node = node_at(&self.document.root, &self.path)
                        .map_err(|e| invalid(&self.document.root, None, "", e))?;
                    self.enter_package(node, observation)?;
                } else {
                    self.path.clear();
                    self.phase = Phase::Ids;
                }
            }
            Phase::Ids => {
                if let Some(frame) = self.id_frames.last_mut() {
                    if !frame.visited {
                        index_id_at_node(
                            frame.node,
                            source,
                            &self.path,
                            &mut self.ids,
                            observation,
                        )?;
                        frame.visited = true;
                    } else if let Some(child) = frame.node.children.get(frame.next) {
                        let index = frame.next;
                        frame.next += 1;
                        observation.work(1, source)?;
                        slot(&mut self.path, 32, source, observation)?;
                        slot(&mut self.id_frames, 256, source, observation)?;
                        self.path.push(index);
                        self.id_frames.push(IdFrame {
                            node: child,
                            next: 0,
                            visited: false,
                        });
                    } else {
                        self.id_frames.pop();
                        self.path.pop();
                        if self.id_frames.is_empty() {
                            self.phase = Phase::Complete;
                        }
                    }
                } else {
                    slot(&mut self.id_frames, 256, source, observation)?;
                    self.id_frames.push(IdFrame {
                        node: &self.document.root,
                        next: 0,
                        visited: false,
                    });
                }
            }
            Phase::Complete | Phase::Failed => {}
        }
        Ok(matches!(self.phase, Phase::Complete))
    }
}
