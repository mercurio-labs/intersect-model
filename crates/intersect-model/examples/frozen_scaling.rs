//! Handwritten measurement host: frozen engine vs the checked BasicModel path
//! on the native-scaling DOE chain fixture (names + link chain resolved by name).
use intersect_model::{
    core::{
        api::{BasicModel, ModelEdit},
        frozen::{ByName, Metamodel, ModelBuilder},
        storage::{self, ModelRead},
    },
    definitions::schema::Schema,
};
use serde_json::json;
use std::{sync::Arc, time::Instant};

fn source(metaclasses: usize) -> String {
    let body = "attr name: String [1,1] default \"\"; ref links: Node [0,*]; ref children: Node [0,*] containment; ref out: Node [0,*] opposite Node.incoming; ref incoming: Node [0,*] opposite Node.out;";
    let mut s = format!(
        "rcore 1; package doe version \"1\"; metamodel \"urn:intersect:doe\" prefix d {{ class Node {{ {body} }}"
    );
    for k in 1..metaclasses {
        s += &format!(
            "class Other{k} {{ {} }}",
            body.replace("Node", &format!("Other{k}"))
        );
    }
    s + "}"
}
fn node_key(schema: &Schema) -> Result<String, String> {
    schema
        .declarations
        .iter()
        .find(|(k, d)| d.name == "Node" && schema.classes.contains_key(*k))
        .map(|(k, _)| k.clone())
        .ok_or_else(|| "Missing Node".into())
}
fn ms(clock: Instant) -> f64 {
    clock.elapsed().as_secs_f64() * 1e3
}

fn existing(schema: &Schema, node: &str, n: usize) -> Result<f64, String> {
    let ids: Vec<String> = (0..n).map(|i| format!("n{i:06}")).collect();
    let clock = Instant::now();
    let mut model = BasicModel::new(schema.clone())?;
    for id in &ids {
        model.create(id, node, "memory:/doe/r0")?;
    }
    let names: Vec<_> = ids
        .iter()
        .map(|id| (id.as_str(), "name", json!(id)))
        .collect();
    model.set_many(&names)?;
    let mut links = Vec::new();
    for i in 0..n.saturating_sub(1) {
        let found = model.query(storage::Query::Value("name", &json!(&ids[i + 1])))?;
        links.push((ids[i].as_str(), "links", json!(found)));
    }
    model.set_many(&links)?;
    model.validate()?;
    Ok(ms(clock))
}

fn frozen(
    mm: &Arc<Metamodel>,
    node: &str,
    n: usize,
    oracle: Option<&Schema>,
) -> Result<[f64; 4], String> {
    let class = mm.class(node).ok_or("Missing Node")?;
    let name = mm.feature(class, "name").ok_or("Missing name")?;
    let links = mm.feature(class, "links").ok_or("Missing links")?;
    let ids: Vec<String> = (0..n).map(|i| format!("n{i:06}")).collect();
    let clock = Instant::now();
    let mut b = ModelBuilder::with_capacity(mm.clone(), n);
    let objs = ids
        .iter()
        .map(|id| b.create(id, class, "memory:/doe/r0"))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for (obj, id) in objs.iter().zip(&ids) {
        b.set_str(*obj, name, id).map_err(|e| e.to_string())?;
    }
    for i in 0..n.saturating_sub(1) {
        b.add_proxy(objs[i], links, &ids[i + 1])
            .map_err(|e| e.to_string())?;
    }
    let build = ms(clock);
    let clock = Instant::now();
    let linked = b.link(&ByName::new(name)).map_err(|e| e.to_string())?;
    let link = ms(clock);
    let clock = Instant::now();
    let model = linked.validate().map_err(|e| e.to_string())?;
    let validate = ms(clock);
    let clock = Instant::now();
    let exported = model.to_model().map_err(|e| e.to_string())?;
    let export = ms(clock);
    if let Some(schema) = oracle {
        exported.validate(schema)?;
    }
    Ok([build, link, validate, export])
}

fn main() -> Result<(), String> {
    println!(
        "M,N,freeze_ms,frozen_build_ms,frozen_link_ms,frozen_validate_ms,frozen_total_ms,frozen_export_ms,existing_ms,speedup"
    );
    for m in [1usize, 10, 100] {
        let schema = intersect_model::compile_rcore_at("memory:/doe.rcore", &source(m))
            .map_err(|e| e.to_string())?
            .schema()
            .clone();
        let node = node_key(&schema)?;
        let clock = Instant::now();
        let mm = Arc::new(Metamodel::freeze(&schema).map_err(|e| e.to_string())?);
        let freeze = ms(clock);
        for n in [100usize, 1_000, 10_000, 100_000] {
            let oracle = (n <= 1_000).then_some(&schema);
            let [build, link, validate, export] = frozen(&mm, &node, n, oracle)?;
            let total = build + link + validate;
            let existing = if n <= 1_000 && m * n <= 10_000 {
                Some(existing(&schema, &node, n)?)
            } else {
                None
            };
            println!(
                "{m},{n},{freeze:.3},{build:.3},{link:.3},{validate:.3},{total:.3},{export:.3},{},{}",
                existing.map_or("-".into(), |e| format!("{e:.1}")),
                existing.map_or("-".into(), |e| format!("{:.0}x", e / total)),
            );
        }
    }
    Ok(())
}
