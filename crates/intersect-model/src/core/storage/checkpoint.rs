//! Opaque transaction tokens shared by storage adapters without global state.
use super::StorageResult;
use std::sync::Arc;

/// A checkpoint issued by one adapter instance. Clones identify the same
/// transaction; completing it invalidates every clone. Tokens are not durable
/// identities and have no public numeric or serialized representation.
#[derive(Clone, Debug)]
pub struct Checkpoint {
    scope: Arc<()>,
    sequence: u64,
}

impl PartialEq for Checkpoint {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.scope, &other.scope) && self.sequence == other.sequence
    }
}
impl Eq for Checkpoint {}

/// One issuer belongs to one storage adapter instance. Adapters retain the
/// issued token in their transaction frame and compare it with the supplied
/// token before commit or rollback. Equality checks both issuer and sequence;
/// the adapter's frame stack enforces LIFO order and single completion.
///
/// The issuer is deliberately not cloneable. Moving an adapter preserves its
/// scope; a separate adapter must own a separate issuer.
#[derive(Debug, Default)]
pub struct CheckpointIssuer {
    scope: Arc<()>,
    sequence: u64,
}

impl CheckpointIssuer {
    pub fn issue(&mut self) -> StorageResult<Checkpoint> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or("Checkpoint counter exhausted")?;
        Ok(Checkpoint {
            scope: self.scope.clone(),
            sequence: self.sequence,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issuer_scope_sequence_and_exhaustion_are_distinct() {
        let mut first = CheckpointIssuer::default();
        let mut second = CheckpointIssuer::default();
        let token = first.issue().unwrap();
        assert_eq!(token, token.clone());
        assert_ne!(token, second.issue().unwrap());
        assert_ne!(token, first.issue().unwrap());
        first.sequence = u64::MAX;
        assert!(first.issue().is_err());
        assert!(first.issue().is_err());
    }
}
