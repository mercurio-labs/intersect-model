//! Borrow stored values directly from the mutable arena during inspection.
use super::{FrozenStore, Header, ObjId, Slot, StorageResult, Value};
use crate::core::{
    frozen::ClassInfo,
    model::AssignmentState,
    storage::{ReferenceView, TypeDescriptor},
};
use intersect_model::definitions::initialization::InitializationReceipt;

#[derive(Clone, Copy)]
pub(in crate::core) struct FrozenObject<'a> {
    store: &'a FrozenStore,
    header: &'a Header,
    row: ObjId,
    assignment: Option<&'a AssignmentState>,
}

impl<'a> FrozenObject<'a> {
    pub(super) fn new(store: &'a FrozenStore, row: ObjId) -> StorageResult<Self> {
        let header = store.builder.data.header(row).ok_or("Missing object")?;
        Ok(Self {
            store,
            header,
            row,
            assignment: None,
        })
    }

    pub(super) fn class_info(self) -> Option<&'a ClassInfo> {
        self.store.builder.metamodel.class_info(self.header.class)
    }

    pub(crate) fn id(self) -> &'a str {
        self.store.builder.data.strings.resolve(self.header.id)
    }

    pub(crate) fn class(self) -> &'a str {
        self.store.builder.metamodel.class_key(self.header.class)
    }

    pub(crate) fn descriptor(self) -> Option<&'a TypeDescriptor> {
        self.store.schema_descriptor().ok()?.class(self.class())
    }

    pub(crate) fn type_identity(self) -> &'a str {
        self.class_info()
            .map_or("", |class| class.identity.as_ref())
    }

    pub(crate) fn resource(self) -> &'a str {
        let data = &self.store.builder.data;
        data.resources
            .get(self.header.resource as usize)
            .map_or("", |uri| data.strings.resolve(*uri))
    }

    pub(crate) fn start(self) -> usize {
        self.header.start
    }

    pub(crate) fn end(self) -> usize {
        self.header.end
    }

    pub(crate) fn implicit(self) -> bool {
        self.store.builder.data.implicit.contains(&self.row)
    }

    pub(crate) fn initialization(self) -> Option<&'a InitializationReceipt> {
        self.store
            .builder
            .data
            .carried
            .get(&self.row)?
            .initialization
            .as_ref()
    }

    pub(crate) fn assignment_state(self) -> Option<&'a AssignmentState> {
        if self.assignment.is_some() {
            return self.assignment;
        }
        self.store
            .builder
            .data
            .carried
            .get(&self.row)?
            .assignment_state
            .as_ref()
    }

    pub(super) fn archive_assignment(self) -> StorageResult<Option<AssignmentState>> {
        if self.assignment_state().is_some()
            || self.class_info().is_none_or(|class| class.has_unsettable)
        {
            return Ok(None);
        }
        super::super::interop::derived_assignment(
            &self.store.builder.metamodel,
            &self.store.builder.data,
            self.header.class,
            self.header.slots as usize,
        )
    }

    pub(super) fn with_assignment(mut self, assignment: Option<&'a AssignmentState>) -> Self {
        self.assignment = assignment;
        self
    }

    pub(crate) fn value(self, name: &str) -> Option<crate::core::storage::ValueView<'a>> {
        let mm = &self.store.builder.metamodel;
        let feature = mm.feature(self.header.class, name)?;
        self.value_at(mm.slot(self.header.class, feature)?, feature)
    }

    pub(crate) fn next_field(
        self,
        index: &mut usize,
    ) -> Option<(&'a str, crate::core::storage::ValueView<'a>)> {
        let fields = &self.class_info()?.stored;
        while let Some(&feature) = fields.get(*index) {
            let position = *index;
            *index += 1;
            if let Some(value) = self.value_at(position, feature) {
                return Some((self.store.builder.metamodel.feature_name(feature), value));
            }
        }
        None
    }

    fn value_at(
        self,
        index: usize,
        feature: crate::core::frozen::FeatureId,
    ) -> Option<crate::core::storage::ValueView<'a>> {
        use crate::core::storage::ValueView;
        let slot = self
            .store
            .builder
            .data
            .slots
            .get(self.header.slots as usize + index)?;
        match slot {
            Slot::Absent => None,
            Slot::Default => Some(
                self.store
                    .builder
                    .metamodel
                    .definition(feature)
                    .map_or_else(ValueView::null, |field| ValueView::json(&field.default)),
            ),
            slot => Some(ValueView::frozen(FrozenValue {
                store: self.store,
                slot,
            })),
        }
    }
}

#[derive(Clone, Copy)]
pub(in crate::core) struct FrozenValue<'a> {
    store: &'a FrozenStore,
    slot: &'a Slot,
}

pub(in crate::core) enum FrozenAtom<'a> {
    Null,
    Bool(bool),
    Int(i64),
    Text(&'a str),
    Identity(&'a str),
    Pending(&'a str),
    Json(&'a Value),
    Many(FrozenItems<'a>),
}

impl<'a> FrozenValue<'a> {
    pub(crate) fn atom(self) -> FrozenAtom<'a> {
        let data = &self.store.builder.data;
        match *self.slot {
            Slot::Absent | Slot::Default | Slot::Null => FrozenAtom::Null,
            Slot::Bool(value) => FrozenAtom::Bool(value),
            Slot::Int(value) => FrozenAtom::Int(value),
            Slot::Str(symbol) => FrozenAtom::Text(data.strings.resolve(symbol)),
            Slot::Json(index) => data
                .json
                .get(index as usize)
                .map_or(FrozenAtom::Null, FrozenAtom::Json),
            Slot::Ref(row) => data.header(row).map_or(FrozenAtom::Null, |header| {
                FrozenAtom::Identity(data.strings.resolve(header.id))
            }),
            Slot::Proxy(index) => {
                data.proxies
                    .get(index as usize)
                    .map_or(FrozenAtom::Null, |proxy| {
                        let spelling = data.strings.resolve(proxy.spelling);
                        if self.store.identity_proxies.contains(&index) {
                            FrozenAtom::Identity(spelling)
                        } else {
                            FrozenAtom::Pending(spelling)
                        }
                    })
            }
            Slot::Many(index) => FrozenAtom::Many(FrozenItems {
                store: self.store,
                items: data.lists.get(index as usize).map_or(&[], Vec::as_slice),
            }),
        }
    }

    pub(crate) fn reference(self) -> Option<ReferenceView<'a>> {
        match self.atom() {
            FrozenAtom::Identity(identity) => Some(self.store.handle(identity).map_or(
                ReferenceView::Unavailable { identity },
                ReferenceView::Resolved,
            )),
            FrozenAtom::Pending(spelling) => Some(ReferenceView::Unresolved { spelling }),
            _ => None,
        }
    }

    pub(crate) fn to_json(self) -> Value {
        self.store.decode(*self.slot)
    }
}

#[derive(Clone, Copy)]
pub(in crate::core) struct FrozenItems<'a> {
    store: &'a FrozenStore,
    items: &'a [Slot],
}

impl<'a> FrozenItems<'a> {
    pub(crate) fn len(self) -> usize {
        self.items.len()
    }

    pub(crate) fn get(self, index: usize) -> Option<FrozenValue<'a>> {
        Some(FrozenValue {
            store: self.store,
            slot: self.items.get(index)?,
        })
    }
}
