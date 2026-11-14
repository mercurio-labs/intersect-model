//! Handwritten measurement host: Rcore `bind scope` linking on the frozen engine.
//! N properties, each owned by one of 10 namespaces holding 8 named types; each
//! property's targetType is resolved by the source-authored scope.
use intersect_model::{
    core::frozen::{Metamodel, ModelBuilder},
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        compiler::compile_structure_bundle,
        ecore::ParseLimits,
        rcore_behavior::ir_v2::Limits,
        rcore_scope::{compile_scope, source_scope_bindings},
    },
    frozen_scopes::{Execution, RcoreScopes},
};
use std::{sync::Arc, time::Instant};

const SOURCE: &str = r#"rcore 1;
package example.scopeBench version "1";
metamodel "urn:scope-bench" prefix m {
  class Namespace { ref types: Type [0,*] containment; }
  class Type { attr name: String [1,1]; }
  class Property {
    ref owner: Namespace [0,1];
    ref targetType: Type [0,1];
  }
}
behavior {
  import model "urn:scope-bench" as m;
  bind scope m.Property.targetType = visible_types;
  fn visible_types(owner:m.Property, spelling:String)->Sequence<m.Type> =
    match owner.owner {
      Some(ns) => Sequence(filter(ns.types, |t| t.name == spelling)),
      None => []
    };
}
"#;
const NAMESPACES: usize = 10;
const TYPES: usize = 8;

fn main() -> Result<(), String> {
    let source = DefinitionSource::new("memory:scope-bench.rcore", SourceKind::Rcore, SOURCE);
    let entry = source.id.clone();
    let bindings =
        source_scope_bindings(&source, ParseLimits::default()).map_err(|e| format!("{e:?}"))?;
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let scopes = bindings
        .iter()
        .map(|b| compile_scope(&bundle, &entry, b.declaration, &Limits::default()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("{e:?}"))?;
    let schema = compile_structure_bundle(&bundle, &entry, "urn:scope-bench")
        .map_err(|e| format!("{e:?}"))?;
    let key = |name: &str| {
        schema
            .declarations
            .iter()
            .find(|(k, d)| d.name == name && schema.classes.contains_key(*k))
            .map(|(k, _)| k.clone())
            .ok_or_else(|| format!("Missing {name}"))
    };
    let mm = Arc::new(Metamodel::freeze(&schema).map_err(|e| e.to_string())?);
    let class = |name: &str| -> Result<_, String> { mm.class(&key(name)?).ok_or("class".into()) };
    let (namespace, ty, property) = (class("Namespace")?, class("Type")?, class("Property")?);
    let feature = |c, n: &str| mm.feature(c, n).ok_or(format!("feature {n}"));
    let (types, name) = (feature(namespace, "types")?, feature(ty, "name")?);
    let (owner, target) = (
        feature(property, "owner")?,
        feature(property, "targetType")?,
    );
    let err = |e: intersect_model::core::frozen::FrozenError| e.to_string();
    println!("executor,properties,link_ms,us_per_reference");
    for (label, execution) in [("vm", Execution::Vm), ("fast", Execution::Fast)] {
        for n in [10usize, 100, 1_000, 10_000, 100_000] {
            if execution == Execution::Vm && n > 10_000 {
                continue;
            }
            let mut b = ModelBuilder::new(mm.clone());
            let mut spaces = Vec::new();
            for s in 0..NAMESPACES {
                let ns = b
                    .create(&format!("ns{s}"), namespace, "memory:r")
                    .map_err(err)?;
                for t in 0..TYPES {
                    let id = b
                        .create(&format!("t{s}_{t}"), ty, "memory:r")
                        .map_err(err)?;
                    b.set_str(id, name, &format!("T{t}")).map_err(err)?;
                    b.add_ref(ns, types, id).map_err(err)?;
                }
                spaces.push(ns);
            }
            for i in 0..n {
                let p = b
                    .create(&format!("p{i}"), property, "memory:r")
                    .map_err(err)?;
                b.add_ref(p, owner, spaces[i % NAMESPACES]).map_err(err)?;
                b.add_proxy(p, target, &format!("T{}", i % TYPES))
                    .map_err(err)?;
            }
            let resolver = RcoreScopes::new(&mm, &bundle, &scopes)
                .map_err(err)?
                .with_execution(execution);
            let clock = Instant::now();
            b.link(&resolver).map_err(|e| e.to_string())?;
            let ms = clock.elapsed().as_secs_f64() * 1e3;
            println!("{label},{n},{ms:.2},{:.2}", ms * 1e3 / n as f64);
            let stats = resolver.stats();
            if execution == Execution::Fast && (stats.fast != n || stats.deferred != 0) {
                return Err(format!(
                    "direct executor did not run every scope: {stats:?}"
                ));
            }
        }
    }
    Ok(())
}
