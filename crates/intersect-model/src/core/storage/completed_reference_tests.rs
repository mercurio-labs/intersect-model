//! Full-audit reuse preserves graph failures and never lends cross-audit proof.
use super::*;
use crate::definitions::schema::ReferenceConformancePreparation;
use serde_json::json;

fn schema() -> StorageResult<Schema> {
    crate::definitions::compiler::inspect_ecore_schema(
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="reuse" nsURI="urn:reuse">
<eClassifiers xsi:type="e:EClass" name="Base">
<eStructuralFeatures xsi:type="e:EReference" name="r" eType="#//Target" upperBound="-1"/>
<eStructuralFeatures xsi:type="e:EReference" name="other" eType="#//Other"/>
<eStructuralFeatures xsi:type="e:EReference" name="children" eType="#//Base" upperBound="-1" containment="true"/>
<eStructuralFeatures xsi:type="e:EReference" name="peer" eType="#//Base" eOpposite="#//Base/back"/>
<eStructuralFeatures xsi:type="e:EReference" name="back" eType="#//Base" eOpposite="#//Base/peer"/>
</eClassifiers>
<eClassifiers xsi:type="e:EClass" name="Receiver" eSuperTypes="#//Base"/>
<eClassifiers xsi:type="e:EClass" name="Target"/>
<eClassifiers xsi:type="e:EClass" name="Good" eSuperTypes="#//Target"/>
<eClassifiers xsi:type="e:EClass" name="Other"/>
</e:EPackage>"##,
    ).map_err(|error| error.to_string())
}

fn graph(schema: &Schema) -> StorageResult<Model> {
    let mut model = Model::default();
    for (id, class) in [("g0", "Good"), ("g1", "Good"), ("bad", "Other")] {
        model.create(schema, id, class, "memory:reuse", 0, 0)?;
    }
    for i in 0..8 {
        let index = model.create(schema, &format!("r{i}"), "Receiver", "memory:reuse", 0, 0)?;
        model.elements[index]
            .values
            .insert("r".into(), json!(["g0", "g1"]));
    }
    Ok(model)
}

fn audit(model: &Model, schema: &Schema, reuse: bool) -> StorageResult<()> {
    let view = ModelView::new(model)?;
    let mut pending = StructuralPreparation::new(true, false);
    pending.synchronous = true;
    let mut completed = CompletedReferences::default();
    while !pending.advance_cached(&view, schema, None, None, reuse.then_some(&mut completed))? {}
    pending.finish().map(|_| ())
}

#[test]
fn completed_cursors_preserve_live_targets_member_keys_and_graph_checks() -> StorageResult<()> {
    let schema = schema()?;
    let model = graph(&schema)?;
    assert_eq!(audit(&model, &schema, true), Ok(()));
    for (feature, value, message) in [
        ("r", json!(["g0", "missing"]), "Missing reference target"),
        ("r", json!(["g0", "bad"]), "Reference type mismatch"),
        ("r", json!(["g0", "g0"]), "Duplicate"),
        ("other", json!("g0"), "Reference type mismatch"),
        ("peer", json!("r0"), "Missing reciprocal opposite"),
        ("children", json!(["r7"]), "Containment cycle"),
    ] {
        let mut invalid = model.clone();
        invalid
            .elements
            .iter_mut()
            .find(|e| e.id == "r7")
            .ok_or("Missing receiver")?
            .values
            .insert(feature.into(), value);
        let expected = audit(&invalid, &schema, false);
        assert_eq!(audit(&invalid, &schema, true), expected);
        assert!(
            expected
                .err()
                .ok_or("Invalid graph accepted")?
                .contains(message)
        );
    }
    // A later schema/call must recompute nominal answers, even for the same keys.
    let mut changed = schema.clone();
    changed
        .classes
        .get_mut("Good")
        .ok_or("Missing Good")?
        .parents
        .clear();
    assert_eq!(
        audit(&model, &changed, true),
        audit(&model, &changed, false)
    );
    assert!(audit(&model, &changed, true).is_err());
    Ok(())
}

#[test]
fn completed_cursor_reuse_moves_original_storage_and_refuses_incomplete_or_negative_proof()
-> StorageResult<()> {
    let schema = schema()?;
    let mut cache = CompletedReferences::default();
    assert!(
        cache
            .retain(ReferenceConformancePreparation::new(
                "Receiver", "r", "Good"
            ))
            .is_err()
    );
    for actual in ["Good", "Other"] {
        let mut cursor = ReferenceConformancePreparation::new("Receiver", "r", actual);
        while !cursor.advance(&schema)? {}
        if actual == "Other" {
            assert!(cache.retain(cursor).is_err());
            continue;
        }
        let before = cursor.selection().2.as_ptr();
        cache.retain(cursor)?;
        assert!(cache.take("Base", "r", actual).is_none());
        assert!(cache.take("Receiver", "other", actual).is_none());
        let mut reused = cache
            .take("Receiver", "r", actual)
            .ok_or("Missing completed cursor")?;
        assert_eq!(reused.selection().2.as_ptr(), before);
        assert!(reused.advance(&schema)?);
        assert!(reused.finish()?);
    }
    Ok(())
}

#[test]
fn completed_cursor_retention_is_bounded_and_denial_keeps_canonical_answers() -> StorageResult<()> {
    let mut source = String::from(
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="cap" nsURI="urn:cap">
<eClassifiers xsi:type="e:EClass" name="Receiver"><eStructuralFeatures xsi:type="e:EReference" name="r" eType="#//Target"/></eClassifiers>
<eClassifiers xsi:type="e:EClass" name="Target"/>"##,
    );
    for i in 0..300 {
        source.push_str(&format!(
            r##"<eClassifiers xsi:type="e:EClass" name="G{i}" eSuperTypes="#//Target"/>"##
        ));
    }
    source.push_str("</e:EPackage>");
    let schema =
        crate::definitions::compiler::inspect_ecore_schema(&source).map_err(|e| e.to_string())?;
    let mut cache = CompletedReferences::default();
    for i in 0..300 {
        let mut cursor = ReferenceConformancePreparation::new("Receiver", "r", &format!("G{i}"));
        while !cursor.advance(&schema)? {}
        assert!(cursor.completed_result()?);
        cache.retain(cursor)?;
    }
    assert!(cache.take("Receiver", "r", "G0").is_some());
    assert!(cache.take("Receiver", "r", "G255").is_some());
    assert!(cache.take("Receiver", "r", "G256").is_none());
    assert!(cache.take("Receiver", "r", "G299").is_none());
    Ok(())
}
