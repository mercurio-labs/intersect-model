//! Dense, source-local structural ancestry and shared nominal predicate code.
//! Candidate positions follow the existing canonical report order. No result
//! bitmap or schema certificate is accepted from a serialized artifact.
use super::*;
use crate::definitions::generic_resolution::{CheckedRuntimeFeatureDomain, NominalRule};
use std::{cell::RefCell, rc::Rc};

pub(super) struct NominalReferenceIndex<'source> {
    resolver: &'source Resolver,
    len: usize,
    masks: BTreeMap<DeclarationId, Vec<u64>>,
    rules: RefCell<BTreeMap<Type, Rc<NominalRule>>>,
    // Current private continuation, never a reusable result bitmap.
    active_scan: RefCell<Option<NominalMaskScan>>,
}
const INDEX_BYTES: usize = 256;
const _: () = assert!(std::mem::size_of::<NominalReferenceIndex<'static>>() <= INDEX_BYTES);
impl<'source> NominalReferenceIndex<'source> {
    pub(super) fn from_reports(
        reports: &[(String, Rc<PreparedRuntimeMembers<'source>>)],
        at: &Xml,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Option<Box<Self>>, ImportError> {
        let Some((_, first)) = reports.first() else {
            return Ok(None);
        };
        // A retention policy, with complete old-checker fallback above capacity.
        if reports.len() > 4096 {
            return Ok(None);
        }
        let resolver = first.source_resolver();
        let mut ancestry = Vec::new();
        for (_, report) in reports {
            observation.work(1, &at.opening.source)?;
            if !std::ptr::eq(resolver, report.source_resolver()) {
                return Ok(None);
            }
            let Some(rows) = report.nominal_ancestry_observed(observation)? else {
                return Ok(None);
            };
            observation.reserve(32, &at.opening.source)?;
            ancestry
                .try_reserve_exact(1)
                .map_err(|_| at.error("Nominal ancestry input allocation failed"))?;
            ancestry.push(rows);
        }
        let len = reports.len();
        let words = len.div_ceil(64);
        let mut masks = BTreeMap::<DeclarationId, Vec<u64>>::new();
        for (position, ((_, report), ancestors)) in reports.iter().zip(&ancestry).enumerate() {
            for expression in std::iter::once(report.receiver.expression())
                .chain(ancestors.as_slice().iter().map(|row| &row.expression))
            {
                let Type::Classifier { declaration, .. } = expression else {
                    return Err(at.error("Missing nominal classifier input"));
                };
                observe_key(declaration, observation)?;
                if !masks.contains_key(declaration) {
                    observation.serialized(declaration, &at.opening.source)?;
                    observation.reserve(128 + 24 + words * 8, &at.opening.source)?;
                    let mut bits = Vec::new();
                    bits.try_reserve_exact(words)
                        .map_err(|_| at.error("Nominal bitmap allocation failed"))?;
                    observation.work(words, &at.opening.source)?;
                    bits.resize(words, 0);
                    masks.insert(declaration.clone(), bits);
                }
                observation.work(1, &at.opening.source)?;
                let bits = masks
                    .get_mut(declaration)
                    .ok_or_else(|| at.error("Missing nominal ancestry bitmap"))?;
                bits[position / 64] |= 1u64 << (position % 64);
            }
        }
        observation.reserve(INDEX_BYTES, &at.opening.source)?;
        Ok(Some(Box::new(Self {
            resolver,
            len,
            masks,
            rules: RefCell::new(BTreeMap::new()),
            active_scan: RefCell::new(None),
        })))
    }
    pub(super) fn start_selection(
        &self,
        admission: &CheckedRuntimeFeatureDomain<'_>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        let selection = self.select(admission, observation)?;
        let selected = selection.is_some();
        *self.active_scan.try_borrow_mut().map_err(|_| {
            error_for(
                admission.source().projection.source.span,
                "Nominal continuation is already borrowed",
            )
        })? = selection;
        Ok(selected)
    }
    pub(super) fn advance_selection<A: crate::definitions::schema::RuntimeAlternative>(
        &self,
        scan: &mut CandidateScan<A>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        field: &Xml,
    ) -> Result<Option<bool>, ImportError> {
        let mut active = self
            .active_scan
            .try_borrow_mut()
            .map_err(|_| field.error("Nominal continuation is already borrowed"))?;
        match active.as_mut() {
            Some(bitmap) => bitmap.advance(scan, observation, field).map(Some),
            None => Ok(None),
        }
    }
    pub(super) fn select(
        &self,
        admission: &CheckedRuntimeFeatureDomain<'_>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Option<NominalMaskScan>, ImportError> {
        let domain = admission.source();
        if !admission.nominal_context_matches(self.resolver)
            || !domain.reference
            || !domain.projection.bindings.is_empty()
        {
            return Ok(None);
        }
        let span = domain.projection.source.span;
        observation.work(1, &span.source)?;
        let cached = self
            .rules
            .try_borrow()
            .map_err(|_| error_for(span, "Nominal rule code is already borrowed"))?
            .get(domain.expression)
            .cloned();
        let rule = if let Some(rule) = cached {
            rule
        } else {
            let Some(rule) = admission.compile_nominal_rule_observed(observation)? else {
                return Ok(None);
            };
            let retain = self
                .rules
                .try_borrow()
                .map_err(|_| error_for(span, "Nominal rule code is already borrowed"))?
                .len()
                < 4096;
            observation.reserve(128 + 32, &span.source)?;
            if retain {
                observation.serialized(domain.expression, &span.source)?;
                observation.reserve(128, &span.source)?;
            }
            let rule = Rc::new(rule);
            if retain {
                self.rules
                    .try_borrow_mut()
                    .map_err(|_| error_for(span, "Nominal rule code is already borrowed"))?
                    .insert(domain.expression.clone(), Rc::clone(&rule));
            }
            rule
        };
        let words = self.len.div_ceil(64);
        observation.reserve(128 + words * 8, &span.source)?;
        let mut bits = Vec::new();
        bits.try_reserve_exact(words)
            .map_err(|_| error_for(span, "Nominal selection allocation failed"))?;
        observation.work(words, &span.source)?;
        bits.resize(words, u64::MAX);
        if !self.len.is_multiple_of(64)
            && let Some(last) = bits.last_mut()
        {
            *last &= (1u64 << (self.len % 64)) - 1;
        }
        for clause in rule.clauses() {
            observe_key(clause, observation)?;
            let allowed = self.masks.get(clause);
            for (index, word) in bits.iter_mut().enumerate() {
                observation.work(1, &span.source)?;
                *word &= allowed.map_or(0, |mask| mask[index]);
            }
        }
        Ok(Some(NominalMaskScan {
            bits,
            len: self.len,
            word: 0,
        }))
    }
}
fn error_for(span: &SourceSpan, message: &str) -> ImportError {
    ImportError::new(ErrorKind::ResourceLimit, span, message)
}
fn observe_key(
    id: &DeclarationId,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    observation.work(
        1 + id.source.uri.len().div_ceil(256)
            + id.source.sha256.len().div_ceil(256)
            + id.path.len(),
        &id.source,
    )
}
pub(super) struct NominalMaskScan {
    bits: Vec<u64>,
    len: usize,
    word: usize,
}
impl NominalMaskScan {
    pub(super) fn advance<A: crate::definitions::schema::RuntimeAlternative>(
        &mut self,
        scan: &mut CandidateScan<A>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        field: &Xml,
    ) -> Result<bool, ImportError> {
        if scan.failed || scan.published {
            return Err(field.error("Nominal candidate scan has failed"));
        }
        let result = self.advance_inner(scan, observation, field);
        if result.is_err() {
            scan.failed = true;
        }
        result
    }
    fn advance_inner<A: crate::definitions::schema::RuntimeAlternative>(
        &mut self,
        scan: &mut CandidateScan<A>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        field: &Xml,
    ) -> Result<bool, ImportError> {
        let Some(bits) = self.bits.get(self.word).copied() else {
            return Ok(true);
        };
        observation.work(1, &field.opening.source)?;
        let additional = bits.count_ones() as usize;
        let needed = scan
            .reference_selection
            .len()
            .checked_add(additional)
            .ok_or_else(|| field.error("Nominal selection size overflow"))?;
        if needed > scan.reference_selection.capacity() {
            observation.reserve(
                (needed - scan.reference_selection.capacity()) * 8,
                &field.opening.source,
            )?;
            scan.reference_selection
                .try_reserve_exact(needed - scan.reference_selection.len())
                .map_err(|_| field.error("Nominal selected-index allocation failed"))?;
        }
        let mut remaining = bits;
        while remaining != 0 {
            observation.work(1, &field.opening.source)?;
            let position = self.word * 64 + remaining.trailing_zeros() as usize;
            if position >= self.len {
                return Err(field.error("Nominal bitmap exceeds its candidate universe"));
            }
            scan.reference_selection.push(position);
            remaining &= remaining - 1;
        }
        self.word += 1;
        scan.next_candidate = (self.word * 64).min(self.len);
        Ok(self.word == self.bits.len())
    }
}

#[cfg(test)]
#[path = "nominal_bitmap_tests.rs"]
mod tests;
