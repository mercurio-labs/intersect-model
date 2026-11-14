//! String interning and Fx-hashed lookup maps for dense IDs.
//!
//! The maps here are lookup-only: nothing iterates them, so their order can
//! never reach a semantic result.
use std::sync::Arc;

pub use rustc_hash::FxHashMap;
pub type FxBuild = rustc_hash::FxBuildHasher;

/// Interned string. Only meaningful with the string table that issued it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sym(pub(super) u32);

#[derive(Default, Clone)]
pub struct Interner {
    map: FxHashMap<Arc<str>, Sym>,
    strings: Vec<Arc<str>>,
}
impl Interner {
    pub fn intern(&mut self, text: &str) -> Result<Sym, super::FrozenError> {
        if let Some(sym) = self.map.get(text) {
            return Ok(*sym);
        }
        let sym = Sym(u32::try_from(self.strings.len())
            .map_err(|_| super::FrozenError::Capacity("interned strings"))?);
        let text: Arc<str> = Arc::from(text);
        self.strings.push(text.clone());
        self.map.insert(text, sym);
        Ok(sym)
    }
    pub fn get(&self, text: &str) -> Option<Sym> {
        self.map.get(text).copied()
    }
    /// Empty string for a foreign symbol; symbols are never forged publicly.
    pub fn resolve(&self, sym: Sym) -> &str {
        self.strings.get(sym.0 as usize).map_or("", |s| s)
    }
    /// Number of interned strings; every `Sym` below it is valid.
    pub(super) fn len(&self) -> usize {
        self.strings.len()
    }
    /// Drop the symbol suffix created after a mutable-store checkpoint.
    pub(super) fn truncate(&mut self, len: usize) {
        let len = len.min(self.strings.len());
        for text in self.strings.drain(len..) {
            self.map.remove(text.as_ref());
        }
    }
    /// The interned strings in symbol order (`Sym(0)` first).
    pub(super) fn iter(&self) -> impl Iterator<Item = &str> {
        self.strings.iter().map(|s| &**s)
    }
}
