use super::*;
use serde_json::json;

fn fixture() -> Result<(crate::tests::Fixture, Value), String> {
    let fixture = crate::tests::Fixture::new()?;
    for name in ["compiler", "loader", "views"] {
        fixture.write(
            &format!("{name}/Cargo.toml"),
            &format!("[package]\nname='{name}'\nversion='1.0.0'\n"),
        )?;
        fixture.write(&format!("{name}/src/lib.rs"), "pub const VALUE: u32 = 7;\n")?;
    }
    let root = canonical(&fixture.0)?;
    let packages: Vec<_> = ["compiler", "loader", "views"]
        .iter()
        .map(|name| {
            json!({
                "id":name,"name":name,"version":"1.0.0","source":null,
                "manifest_path":root.join(name).join("Cargo.toml").to_string_lossy()
            })
        })
        .collect();
    let dependency = |id| json!({"name":id,"pkg":id,"dep_kinds":[{"kind":null,"target":null}]});
    let context = json!({"metadata":{"packages":packages,"resolve":{"nodes":[
        {"id":"compiler","features":[],"deps":[dependency("loader")]},
        {"id":"loader","features":[],"deps":[]},
        {"id":"views","features":[],"deps":[]}
    ]}},"rustc":"test rustc","rustflags":"","checksums":{}});
    Ok((fixture, context))
}
fn get(fixture: &crate::tests::Fixture, context: &Value) -> Result<String, String> {
    identity(
        context,
        &canonical(&fixture.0.join("compiler/Cargo.toml"))?,
        &[],
    )
}

#[test]
fn view_source_and_unrelated_revision_are_excluded() -> Result<(), String> {
    let (fixture, mut context) = fixture()?;
    let original = get(&fixture, &context)?;
    fixture.write("views/src/lib.rs", "pub const VALUE: u32 = 99;\n")?;
    context["metadata"]["packages"][2]["source"] =
        json!(format!("git+file:///views#{}", "a".repeat(40)));
    assert_eq!(get(&fixture, &context)?, original);
    fixture.write("loader/src/lib.rs", "pub const VALUE: u32 = 99;\n")?;
    assert_ne!(get(&fixture, &context)?, original);
    Ok(())
}

#[test]
fn relevant_features_versions_and_build_dependencies_invalidate() -> Result<(), String> {
    let (fixture, context) = fixture()?;
    let original = get(&fixture, &context)?;
    let mut changed = context.clone();
    changed["metadata"]["resolve"]["nodes"][1]["features"] = json!(["semantic-option"]);
    assert_ne!(get(&fixture, &changed)?, original);
    changed = context.clone();
    changed["metadata"]["packages"][1]["version"] = json!("2.0.0");
    assert_ne!(get(&fixture, &changed)?, original);
    changed = context.clone();
    changed["metadata"]["resolve"]["nodes"][0]["deps"][0]["dep_kinds"][0]["kind"] = json!("build");
    assert_ne!(get(&fixture, &changed)?, original);
    changed["metadata"]["resolve"]["nodes"][0]["deps"][0]["dep_kinds"][0]["kind"] = json!("dev");
    let dev_key = get(&fixture, &changed)?;
    fixture.write("loader/src/lib.rs", "pub const VALUE: u32 = 99;\n")?;
    assert_eq!(get(&fixture, &changed)?, dev_key);
    Ok(())
}

#[test]
fn context_order_and_checkout_location_do_not_change_key() -> Result<(), String> {
    let (first, a) = fixture()?;
    let (second, mut b) = fixture()?;
    b["metadata"]["packages"]
        .as_array_mut()
        .ok_or("packages")?
        .reverse();
    b["metadata"]["resolve"]["nodes"]
        .as_array_mut()
        .ok_or("nodes")?
        .reverse();
    assert_eq!(get(&first, &a)?, get(&second, &b)?);
    Ok(())
}

#[test]
fn incomplete_ambiguous_and_wrong_feature_contexts_refuse() -> Result<(), String> {
    let (fixture, context) = fixture()?;
    let mut broken = context.clone();
    broken["metadata"]["resolve"]["nodes"]
        .as_array_mut()
        .ok_or("nodes")?
        .remove(1);
    assert!(get(&fixture, &broken).is_err());
    broken = context.clone();
    let duplicate = broken["metadata"]["packages"][0].clone();
    broken["metadata"]["packages"]
        .as_array_mut()
        .ok_or("packages")?
        .push(duplicate);
    assert!(get(&fixture, &broken).is_err());
    broken = context;
    broken["metadata"]["resolve"]["nodes"][0]["features"] = json!(["other"]);
    assert!(get(&fixture, &broken).is_err());
    Ok(())
}

#[test]
fn embedded_inputs_and_registry_checksums_are_tracked() -> Result<(), String> {
    let (fixture, mut context) = fixture()?;
    fixture.write(
        "loader/src/lib.rs",
        "pub const TEXT: &str = include_str!(\"../data.txt\");\n",
    )?;
    fixture.write("loader/data.txt", "first")?;
    let original = get(&fixture, &context)?;
    fixture.write("loader/data.txt", "second")?;
    assert_ne!(get(&fixture, &context)?, original);
    context["metadata"]["packages"][1]["source"] = json!("registry+https://example.invalid/index");
    assert!(get(&fixture, &context).is_err());
    let name = "loader@1.0.0:registry+https://example.invalid/index";
    context["checksums"][name] = json!("a".repeat(64));
    let old = get(&fixture, &context)?;
    context["checksums"][name] = json!("b".repeat(64));
    assert_ne!(get(&fixture, &context)?, old);
    Ok(())
}

#[test]
fn freshness_guards_fail_on_changed_or_missing_inputs() -> Result<(), String> {
    let (fixture, mut context) = fixture()?;
    fixture.write("Cargo.toml", "[workspace]\n")?;
    fixture.write("Cargo.lock", "version = 4\n")?;
    let root = canonical(&fixture.0)?;
    context["metadata"]["workspace_root"] = json!(root.to_string_lossy());
    let paths: Vec<_> = [
        "Cargo.toml",
        "Cargo.lock",
        "compiler/Cargo.toml",
        "loader/Cargo.toml",
        "views/Cargo.toml",
    ]
    .iter()
    .map(|p| root.join(p))
    .collect();
    context["guards"] = json!(paths.iter().map(|p| {
        let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
        Ok(json!({"path":p.to_string_lossy(), "sha256":format!("{:x}", Sha256::digest(bytes))}))
    }).collect::<Result<Vec<Value>, String>>()?);
    validate_guards(&context)?;
    let saved = context.clone();
    context["guards"].as_array_mut().ok_or("guards")?.pop();
    assert!(validate_guards(&context).is_err());
    fixture.write("Cargo.lock", "version = 3\n")?;
    assert!(validate_guards(&saved).is_err());
    Ok(())
}
