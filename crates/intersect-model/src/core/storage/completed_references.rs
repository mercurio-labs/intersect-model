//! Completed canonical cursors owned by one fixed-schema, unobserved audit.
//! No graph/target identity or public prepared authority is cached here.
use crate::definitions::schema::ReferenceConformancePreparation;
use std::collections::BTreeMap;

const MAX_ENTRIES: usize = 256;
type Targets = BTreeMap<String, Option<ReferenceConformancePreparation>>;

#[derive(Default)]
pub(super) struct CompletedReferences {
    rows: BTreeMap<String, BTreeMap<String, Targets>>,
    entries: usize,
}

impl CompletedReferences {
    pub(super) fn take(
        &mut self,
        receiver: &str,
        member: &str,
        actual: &str,
    ) -> Option<ReferenceConformancePreparation> {
        self.rows
            .get_mut(receiver)?
            .get_mut(member)?
            .get_mut(actual)?
            .take()
    }

    pub(super) fn retain(&mut self, cursor: ReferenceConformancePreparation) -> Result<(), String> {
        if !cursor.completed_result()? {
            return Err("Only successful reference conformance can be retained".into());
        }
        let (receiver, member, actual) = cursor.selection();
        if let Some(slot) = self
            .rows
            .get_mut(receiver)
            .and_then(|members| members.get_mut(member))
            .and_then(|targets| targets.get_mut(actual))
        {
            *slot = Some(cursor);
        } else if self.entries < MAX_ENTRIES {
            let (receiver, member, actual) =
                (receiver.to_owned(), member.to_owned(), actual.to_owned());
            self.rows
                .entry(receiver)
                .or_default()
                .entry(member)
                .or_default()
                .insert(actual, Some(cursor));
            self.entries += 1;
        }
        Ok(())
    }
}
