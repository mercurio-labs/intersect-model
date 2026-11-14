#[path = "common/runtime_lower_bounds.rs"]
mod common;
use intersect_model::definitions::generic_resolution::Resolver;
#[test]
fn reversed_runtime_lower_bounds_preserve_raw_substitution_and_invariant_nesting() {
    let f = common::Fixture::new();
    let resolver = Resolver::new(&f.bundle).unwrap();
    let base = f.atom(0);
    let sub = f.atom(1);
    let other = f.atom(2);
    let marker = f.atom(6);
    let both = f.atom(7);
    let check = |owner, actual, expected| {
        let domains = resolver.runtime_feature_domains(&owner).unwrap();
        let domain = domains
            .iter()
            .find(|d| d.projection.source.name == "value")
            .unwrap();
        assert_eq!(
            resolver.runtime_feature_accepts(domain, &actual).unwrap(),
            expected,
            "{owner:?} <- {actual:?}"
        );
    };
    let raw = f.ty(4, vec![], true);
    let closed = f.ty(4, vec![sub.expression().clone()], false);
    check(raw.clone(), f.pair(&base, &sub), true);
    check(raw.clone(), f.pair(&sub, &sub), false);
    check(raw.clone(), f.pair(&base, &other), false);
    check(raw.clone(), f.pair(&other, &sub), false);
    check(closed.clone(), f.pair(&base, &sub), true);
    check(closed.clone(), f.pair(&sub, &sub), true);
    check(closed, f.pair(&base, &base), false);
    check(f.ty(5, vec![], true), f.pair(&base, &sub), true);
    check(f.ty(8, vec![], true), f.pair(&marker, &both), true);
    check(f.ty(8, vec![], true), f.pair(&base, &both), true);
    check(f.ty(8, vec![], true), f.pair(&both, &both), false);
    check(f.ty(8, vec![], true), f.pair(&marker, &sub), false);
    check(f.ty(9, vec![], true), f.pair(&base, &sub), true);
    check(
        f.ty(
            9,
            vec![sub.expression().clone(), sub.expression().clone()],
            false,
        ),
        f.pair(&sub, &sub),
        true,
    );
    let nested = f.pair(&f.pair(&sub, &base), &sub);
    check(f.ty(10, vec![], true), nested.clone(), false);
    check(
        f.ty(10, vec![sub.expression().clone()], false),
        nested,
        true,
    );
    check(
        f.ty(10, vec![sub.expression().clone()], false),
        f.pair(&f.pair(&base, &base), &sub),
        false,
    );
    check(f.ty(11, vec![], true), f.pair(&base, &sub), false);
    let mut forged = resolver
        .runtime_feature_domains(&raw)
        .unwrap()
        .into_iter()
        .find(|d| d.projection.source.name == "value")
        .unwrap();
    forged.projection.source.declaration.source.sha256 = "0".repeat(64);
    assert!(
        resolver
            .runtime_feature_accepts(&forged, &f.pair(&base, &sub))
            .is_err()
    );
}
