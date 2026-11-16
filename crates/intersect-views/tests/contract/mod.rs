use intersect_views::{PreparedEdit, Scene, SceneLimits, SceneResponse, ToolCapability};
use serde_json::{Value, json};
type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
fn fixture() -> Result<Value> {
    Ok(serde_json::from_str(include_str!(
        "../fixtures/scene-v1.json"
    ))?)
}

#[test]
fn scene_v1_round_trips_without_losing_occurrence_context_or_source_bindings() -> Result {
    let value = fixture()?;
    let scene: Scene = serde_json::from_value(value.clone())?;
    scene.validate()?;
    assert_eq!(serde_json::to_value(&scene)?, value);
    assert_eq!(scene.nodes[0].binding.key, scene.nodes[1].binding.key);
    assert_ne!(scene.nodes[0].id, scene.nodes[1].id);
    assert_ne!(
        scene.nodes[0].binding.context,
        scene.nodes[1].binding.context
    );
    assert!(
        scene
            .validate_with_limits(SceneLimits {
                nodes: 1,
                ..Default::default()
            })
            .is_err()
    );
    Ok(())
}
#[test]
fn rust_rejects_the_shared_invalid_wire_vectors() -> Result {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../fixtures/scene-v1-rejections.json"))?;
    for case in cases {
        let mut value = fixture()?;
        let pointer = case["pointer"].as_str().ok_or("Missing fixture pointer")?;
        let (parent, key) = pointer.rsplit_once('/').ok_or("Bad fixture pointer")?;
        let object = value
            .pointer_mut(parent)
            .and_then(Value::as_object_mut)
            .ok_or("Missing fixture object")?;
        object.insert(key.into(), case["value"].clone());
        let accepted =
            serde_json::from_value::<Scene>(value).is_ok_and(|scene| scene.validate().is_ok());
        assert!(!accepted, "accepted invalid vector: {}", case["name"]);
    }
    Ok(())
}
#[test]
fn every_tool_needs_one_capability_and_disabled_tools_need_a_reason() -> Result {
    let mut response = SceneResponse {
        scene: serde_json::from_value(fixture()?)?,
        capabilities: vec![ToolCapability {
            tool_id: "select".into(),
            enabled: true,
            reason: String::new(),
        }],
    };
    response.validate()?;
    response.capabilities[0].enabled = false;
    assert!(response.validate().is_err());
    response.capabilities[0].reason = "Unavailable for this revision".into();
    response.validate()?;
    response.capabilities.clear();
    assert!(response.validate().is_err());
    Ok(())
}
#[test]
fn prepared_source_edits_have_portable_revisions_ranges_and_buffer_hashes() -> Result {
    let mut edit: PreparedEdit = serde_json::from_value(
        json!({"schemaVersion":1,"token":"edit:1","baseRevision":1,"nextRevision":2,"created":"element:new","patches":[{
            "uri":"file:///example.model","range":{"start":{"line":0,"character":2},"end":{"line":0,"character":2}},
            "original":"","replacement":"new","beforeHash":"0".repeat(64),"afterHash":"a".repeat(64)
        }]}),
    )?;
    edit.validate()?;
    edit.next_revision = edit.base_revision;
    assert!(edit.validate().is_err());
    edit.next_revision += 1;
    edit.patches[0].before_hash = "bad".into();
    assert!(edit.validate().is_err());
    Ok(())
}
