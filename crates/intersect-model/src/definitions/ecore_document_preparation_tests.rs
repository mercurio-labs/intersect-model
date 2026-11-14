use super::*;
use crate::definitions::conversion::{InvocationContext, InvocationObserver, ProviderId};
use std::{cell::Cell, rc::Rc};
struct Meter {
    calls: Cell<usize>,
    bytes: Cell<usize>,
    work: Cell<usize>,
    stop: usize,
    byte_limit: usize,
    work_limit: usize,
}
impl Meter {
    fn new(stop: usize, byte_limit: usize, work_limit: usize) -> Rc<Self> {
        Rc::new(Self {
            calls: Cell::new(0),
            bytes: Cell::new(0),
            work: Cell::new(0),
            stop,
            byte_limit,
            work_limit,
        })
    }
    fn tick(&self) -> Result<(), String> {
        let n = self.calls.get();
        self.calls.set(n + 1);
        if n == self.stop {
            Err("document cancelled".into())
        } else {
            Ok(())
        }
    }
}
impl InvocationObserver for Meter {
    fn checkpoint(&self) -> Result<(), String> {
        self.tick()
    }
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected provider".into())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.tick()?;
        let value = self.bytes.get() + n;
        if value > self.byte_limit {
            return Err("document storage denied".into());
        }
        self.bytes.set(value);
        Ok(())
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.tick()?;
        let value = self.work.get() + n;
        if value > self.work_limit {
            return Err("document work denied".into());
        }
        self.work.set(value);
        Ok(())
    }
}
fn fixture() -> DefinitionSource {
    let mut source = DefinitionSource::new(
        "memory:events.ecore",
        SourceKind::Ecore,
        include_str!("../../assets/fixtures/ecore-generics/document-events.ecore"),
    );
    source
        .dependencies
        .push(crate::definitions::bundle::SourceId {
            uri: "memory:retained-dependency.ecore".into(),
            sha256: "f".repeat(64),
        });
    source
}
fn run(
    source: &DefinitionSource,
    meter: Rc<Meter>,
) -> Result<(Document, usize), ObservedImportError> {
    let mut preparation = OwnedDocumentPreparation::begin(
        source,
        ParseLimits::default(),
        &InvocationContext::new(meter.clone()),
    )?;
    let mut advances = 0;
    loop {
        advances += 1;
        if preparation.advance(&InvocationContext::new(meter.clone()))? {
            break;
        }
    }
    Ok((preparation.finish()?, advances))
}
#[test]
fn retained_xml_events_keep_literal_names_payloads_utf8_spans_and_complete_source() {
    let mut source = fixture();
    let original = source.clone();
    let context = InvocationContext::default();
    let mut preparation =
        OwnedDocumentPreparation::begin(&source, ParseLimits::default(), &context).unwrap();
    source.text.clear(); // The admitted source is owned independently of the caller.
    source.dependencies.clear();
    let mut advances = 0;
    loop {
        advances += 1;
        if preparation.advance(&context).unwrap() {
            break;
        }
    }
    assert_eq!(advances, 16); // Declaration, whitespace, comment, six tags, five body payloads, PI and EOF.
    assert!(preparation.advance(&context).unwrap());
    let actual = preparation.finish().unwrap();
    assert_eq!(actual.source, original);
    let expected = Document::parse(original.clone()).unwrap();
    assert_eq!(format!("{:?}", actual), format!("{:?}", expected));
    assert_eq!(actual.root.name.namespace.as_deref(), Some(ECORE));
    assert_eq!(actual.root.name.local, "EPackage");
    assert_eq!(
        actual
            .root
            .attributes
            .iter()
            .find(|a| a.lexical_name == "name")
            .unwrap()
            .value,
        "AB"
    );
    let child = &actual.root.children[0];
    assert_eq!(child.name.local, "eClassifiers");
    assert!(child.name.namespace.is_none());
    let kind = child
        .attributes
        .iter()
        .find(|a| a.lexical_name == "x:type")
        .unwrap()
        .type_name
        .as_ref()
        .unwrap();
    assert_eq!(kind.namespace.as_deref(), Some(ECORE));
    assert_eq!(kind.local, "EClass");
    let name = child
        .attributes
        .iter()
        .find(|a| a.lexical_name == "name")
        .unwrap();
    assert_eq!(name.value, "Nα");
    assert_eq!(
        &original.text[name.span.start..name.span.end],
        "name=\"Nα\""
    );
    let payloads = &child.children[0].payloads;
    let expected = [
        (PayloadKind::Text, "a"),
        (PayloadKind::EntityReference, "&amp;"),
        (PayloadKind::CData, "<![CDATA[b<c]]>"),
        (PayloadKind::ProcessingInstruction, "<?go yes?>"),
        (PayloadKind::Comment, "<!--inner-->"),
    ];
    assert_eq!(payloads.len(), expected.len());
    for (actual, (kind, text)) in payloads.iter().zip(expected) {
        assert_eq!(actual.kind, kind);
        assert_eq!(actual.span.source, original.id);
        assert_eq!(&original.text[actual.span.start..actual.span.end], text);
    }
    assert_eq!(
        actual.misc.iter().map(|p| p.kind).collect::<Vec<_>>(),
        [
            PayloadKind::Declaration,
            PayloadKind::Text,
            PayloadKind::Comment,
            PayloadKind::ProcessingInstruction
        ]
    );
    let start = original.text.find("<e:EPackage").unwrap();
    let end = original.text.find("<?after").unwrap();
    assert_eq!(
        actual.root.span,
        SourceSpan {
            source: original.id,
            start,
            end
        }
    );
}
#[test]
fn retained_xml_has_explicit_holder_receipt_and_observes_every_interruption_and_limit() {
    let source = fixture();
    let full = Meter::new(usize::MAX, usize::MAX, usize::MAX);
    let (expected, _) = run(&source, full.clone()).unwrap();
    let legacy = Meter::new(usize::MAX, usize::MAX, usize::MAX);
    Document::parse_observed(&source, &InvocationContext::new(legacy.clone())).unwrap();
    assert_eq!(full.bytes.get(), legacy.bytes.get() + 2048);
    assert_eq!(full.work.get(), legacy.work.get());
    for stop in 0..full.calls.get() {
        assert!(
            matches!(run(&source,Meter::new(stop,usize::MAX,usize::MAX)),Err(ObservedImportError::Observation(message)) if message=="document cancelled")
        );
    }
    for (bytes, work, success) in [
        (full.bytes.get() - 1, full.work.get(), false),
        (full.bytes.get(), full.work.get() - 1, false),
        (full.bytes.get(), full.work.get(), true),
    ] {
        assert_eq!(
            run(&source, Meter::new(usize::MAX, bytes, work)).is_ok(),
            success
        );
    }
    assert_eq!(
        format!(
            "{:?}",
            run(&source, Meter::new(usize::MAX, usize::MAX, usize::MAX))
                .unwrap()
                .0
        ),
        format!("{:?}", expected)
    );
}
#[test]
fn retained_xml_cancellation_is_terminal_at_every_event_boundary_including_completion() {
    let source = fixture();
    let context = InvocationContext::default();
    assert!(
        OwnedDocumentPreparation::begin(&source, ParseLimits::default(), &context)
            .unwrap()
            .finish()
            .is_err()
    );
    for boundary in 0..=16 {
        let mut preparation =
            OwnedDocumentPreparation::begin(&source, ParseLimits::default(), &context).unwrap();
        for _ in 0..boundary {
            preparation.advance(&context).unwrap();
        }
        let cancelled = preparation
            .advance(&InvocationContext::new(Meter::new(
                0,
                usize::MAX,
                usize::MAX,
            )))
            .unwrap_err();
        assert_eq!(
            cancelled,
            ObservedImportError::Observation("document cancelled".into())
        );
        assert_eq!(preparation.advance(&context).unwrap_err(), cancelled);
        assert_eq!(preparation.finish().unwrap_err(), cancelled);
    }
}
#[test]
fn retained_xml_preserves_exact_existing_error_categories_order_and_spans() {
    let context = InvocationContext::default();
    let mut cases = Vec::new();
    for text in [
        "",
        "<a>",
        "<a/><b/>",
        "<!DOCTYPE a><a/>",
        "<a x='1' x='2'/>",
        "<x:a/>",
        "<a>&unknown;</a>",
        "<a><b></a>",
        "<![CDATA[outside]]>",
        "<a>\0</a>",
    ] {
        cases.push((
            DefinitionSource::new("memory:invalid.ecore", SourceKind::Ecore, text),
            ParseLimits::default(),
        ));
    }
    let mut bad = fixture();
    bad.id.sha256 = "0".repeat(64);
    cases.push((bad, ParseLimits::default()));
    let mut bad = fixture();
    bad.id.uri = " ".into();
    cases.push((bad, ParseLimits::default()));
    let mut bad = fixture();
    bad.kind = SourceKind::RcoreBehavior;
    cases.push((bad, ParseLimits::default()));
    cases.push((fixture(), ParseLimits { source_bytes: 1 }));
    for (source, limits) in cases {
        let expected = Document::parse_with_limits(source.clone(), limits).unwrap_err();
        for retained_input in [false, true] {
            let actual = (|| {
                let mut p = if retained_input {
                    OwnedDocumentPreparation::begin_with_retained_input(&source, limits, &context)?
                } else {
                    OwnedDocumentPreparation::begin(&source, limits, &context)?
                };
                while !p.advance(&context)? {}
                p.finish()
            })();
            assert_eq!(
                actual.unwrap_err(),
                ObservedImportError::Import(expected.clone())
            );
        }
    }
}

fn input_fixture() -> DefinitionSource {
    DefinitionSource::new(
        format!("memory:{}", "α".repeat(150)),
        SourceKind::Ecore,
        format!("<a>{}</a>", "α".repeat(300)),
    )
}

fn run_input(
    source: &DefinitionSource,
    limits: ParseLimits,
    meter: Rc<Meter>,
) -> Result<(Document, usize), ObservedImportError> {
    let mut preparation = OwnedDocumentPreparation::begin_with_retained_input(
        source,
        limits,
        &InvocationContext::new(meter.clone()),
    )?;
    let mut advances = 0;
    loop {
        advances += 1;
        if preparation.advance(&InvocationContext::new(meter.clone()))? {
            return Ok((preparation.finish()?, advances));
        }
    }
}

#[test]
fn retained_xml_input_yields_before_events_and_preserves_source_tree_and_receipts() {
    let source = input_fixture();
    let meter = Meter::new(usize::MAX, usize::MAX, usize::MAX);
    let (actual, advances) = run_input(&source, ParseLimits::default(), meter.clone()).unwrap();
    let legacy = Meter::new(usize::MAX, usize::MAX, usize::MAX);
    let (expected, events) = run(&source, legacy.clone()).unwrap();
    assert_eq!(events, 4);
    assert_eq!(advances, 12); // Two URI, three digest, three XML-character chunks, four events.
    assert_eq!(meter.work.get(), legacy.work.get());
    assert_eq!(meter.bytes.get(), legacy.bytes.get());
    assert_eq!(format!("{actual:?}"), format!("{expected:?}"));

    let context = InvocationContext::default();
    let mut pending = OwnedDocumentPreparation::begin_with_retained_input(
        &source,
        ParseLimits::default(),
        &context,
    )
    .unwrap();
    for _ in 0..8 {
        let cursor = pending.holder.borrow_dependent();
        assert!(cursor.input.is_some());
        assert!(cursor.root.is_none());
        assert!(cursor.stack.is_empty());
        assert!(!pending.advance(&context).unwrap());
    }
    assert!(pending.holder.borrow_dependent().input.is_none());
    assert!(pending.finish().is_err());

    let observer = Meter::new(usize::MAX, usize::MAX, usize::MAX);
    let weak = Rc::downgrade(&observer);
    let mut pending = OwnedDocumentPreparation::begin_with_retained_input(
        &source,
        ParseLimits::default(),
        &InvocationContext::new(observer),
    )
    .unwrap();
    assert!(weak.upgrade().is_none());
    assert!(!pending.advance(&context).unwrap());
}

#[test]
fn retained_xml_input_preserves_digest_kind_size_and_character_error_priority() {
    let context = InvocationContext::default();
    let source = DefinitionSource::new(
        "memory:invalid-input.ecore",
        SourceKind::Ecore,
        format!("<a>{}\0</a>", "α".repeat(257)),
    );
    let mut cases = vec![(
        source.clone(),
        ParseLimits::default(),
        ErrorKind::MalformedXml,
        517,
        518,
    )];
    cases.push((
        source.clone(),
        ParseLimits { source_bytes: 1 },
        ErrorKind::ResourceLimit,
        0,
        0,
    ));
    let mut bad_kind = source.clone();
    bad_kind.kind = SourceKind::RcoreBehavior;
    cases.push((
        bad_kind.clone(),
        ParseLimits { source_bytes: 1 },
        ErrorKind::InvalidDefinition,
        0,
        0,
    ));
    bad_kind.id.sha256 = "0".repeat(64);
    cases.push((
        bad_kind,
        ParseLimits { source_bytes: 1 },
        ErrorKind::InvalidDefinition,
        0,
        0,
    ));
    for (source, limits, kind, start, end) in cases {
        let expected = Document::parse_with_limits(source.clone(), limits).unwrap_err();
        assert_eq!(
            (expected.kind, expected.span.start, expected.span.end),
            (kind, start, end)
        );
        let mut pending =
            OwnedDocumentPreparation::begin_with_retained_input(&source, limits, &context).unwrap();
        assert!(pending.holder.borrow_dependent().root.is_none());
        let actual = loop {
            match pending.advance(&context) {
                Ok(false) => {}
                Ok(true) => panic!("Invalid input published"),
                Err(error) => break error,
            }
        };
        assert_eq!(actual, ObservedImportError::Import(expected));
        assert_eq!(pending.advance(&context).unwrap_err(), actual);
        assert_eq!(pending.finish().unwrap_err(), actual);
    }
}

#[test]
fn retained_xml_input_cancels_terminally_at_each_chunk_and_observer_boundary() {
    let source = input_fixture();
    let meter = Meter::new(usize::MAX, usize::MAX, usize::MAX);
    let (_, advances) = run_input(&source, ParseLimits::default(), meter.clone()).unwrap();
    for stop in 0..meter.calls.get() {
        assert!(matches!(
            run_input(&source, ParseLimits::default(), Meter::new(stop, usize::MAX, usize::MAX)),
            Err(ObservedImportError::Observation(message)) if message == "document cancelled"
        ));
    }
    let context = InvocationContext::default();
    for boundary in 0..=advances {
        let mut pending = OwnedDocumentPreparation::begin_with_retained_input(
            &source,
            ParseLimits::default(),
            &context,
        )
        .unwrap();
        for _ in 0..boundary {
            pending.advance(&context).unwrap();
        }
        let error = pending
            .advance(&InvocationContext::new(Meter::new(
                0,
                usize::MAX,
                usize::MAX,
            )))
            .unwrap_err();
        assert_eq!(
            error,
            ObservedImportError::Observation("document cancelled".into())
        );
        assert_eq!(pending.advance(&context).unwrap_err(), error);
        assert_eq!(pending.finish().unwrap_err(), error);
    }
    for (bytes, work, success) in [
        (meter.bytes.get() - 1, meter.work.get(), false),
        (meter.bytes.get(), meter.work.get() - 1, false),
        (meter.bytes.get(), meter.work.get(), true),
    ] {
        assert_eq!(
            run_input(
                &source,
                ParseLimits::default(),
                Meter::new(usize::MAX, bytes, work)
            )
            .is_ok(),
            success
        );
    }
    assert!(
        run_input(
            &source,
            ParseLimits::default(),
            Meter::new(usize::MAX, usize::MAX, usize::MAX)
        )
        .is_ok()
    );
}
