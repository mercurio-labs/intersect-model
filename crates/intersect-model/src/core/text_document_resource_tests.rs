use super::*;
fn limits() -> DocumentLimits {
    DocumentLimits {
        source_bytes: 1024,
        nodes: 32,
        logical_storage_bytes: 16_384,
        operations: 1024,
    }
}
fn row(uri: &str, text: &str, resource: &str) -> Result<DocumentResourceAssociation, String> {
    let source = SourceId::for_text(uri, text);
    DocumentResourceAssociation::prepare(
        (&source.uri, &source.sha256),
        resource,
        4096,
        &mut QueryBudget::new(1000),
    )
}
fn document(row: &DocumentResourceAssociation, text: &str) -> Result<TextDocument, String> {
    let mut builder = DocumentBuilder::from_associated_source(row, text, 0, limits())?;
    let root = builder.begin(0, NodeData::default())?;
    builder.leaf(
        TextRange {
            start: 0,
            end: text.encode_utf16().count(),
        },
        NodeData::default(),
    )?;
    builder.close_from_children(root, 0)?;
    builder.finish()
}

#[test]
fn explicit_resource_keeps_original_source_and_ordinary_uri_checks() -> Result<(), String> {
    let raw = "library raw/Kernel Semantic/Base.kerml";
    let text = "model café";
    let logical = "library%20raw/Kernel%20Semantic/Base.kerml";
    assert!(ResourceRecord::new(raw).is_err());
    assert!(DocumentBuilder::from_source(raw, text, 0, limits()).is_err());
    assert!(DocumentBuilder::new(raw.into(), text.into(), 0, limits()).is_err());
    let association = row(raw, text, logical)?;
    let doc = document(&association, text)?;
    assert_eq!(doc.source(), &SourceId::for_text(raw, text));
    assert_eq!(doc.text(), text);
    assert_eq!(
        doc.utf8_range(doc.nodes()[0].range)?,
        Some(TextRange {
            start: 0,
            end: text.len()
        })
    );
    let mut resource = ResourceRecord::new(logical)?;
    resource.source = Some(doc.source_correspondence());
    resource.content_revision = 7;
    let doc = doc.bind_associated_resource(&association, &resource)?;
    doc.check_associated_resource(&association, &resource)?;
    assert!(doc.check_resource(&resource).is_err());
    let handle = doc.handle(NodeId(0))?;
    assert_eq!(handle.source, SourceId::for_text(raw, text));
    assert_eq!(handle.resource_revision, 7);
    doc.node(&handle)?;
    Ok(())
}

#[test]
fn explicit_resource_rejects_foreign_source_changed_text_and_reload_revision() -> Result<(), String>
{
    let text = "model p";
    let association = row("raw source", text, "raw%20source")?;
    let doc = document(&association, text)?;
    let mut record = ResourceRecord::new("raw%20source")?;
    record.source = Some(doc.source_correspondence());
    record.content_revision = 7;
    let doc = doc.bind_associated_resource(&association, &record)?;
    let handle = doc.handle(NodeId(0))?;
    let foreign = row("foreign source", text, "raw%20source")?;
    assert!(doc.check_associated_resource(&foreign, &record).is_err());
    assert!(document(&association, "model q").is_err());
    let other_resource = row("raw source", text, "other:resource")?;
    assert!(
        doc.check_associated_resource(&other_resource, &record)
            .is_err()
    );
    record.touch()?;
    assert!(
        doc.check_associated_resource(&association, &record)
            .is_err()
    );
    record.source = Some(doc.source_correspondence());
    assert!(
        doc.check_associated_resource(&association, &record)
            .is_err()
    );
    let reloaded = document(&association, text)?.bind_associated_resource(&association, &record)?;
    reloaded.check_associated_resource(&association, &record)?;
    assert!(reloaded.node(&handle).is_err());
    assert_eq!(reloaded.source(), doc.source());
    assert_eq!(reloaded.nodes(), doc.nodes());
    Ok(())
}
