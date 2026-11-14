use super::Cell;

#[test]
fn repeated_exception_edits_and_linking_reuse_the_reference_buffer() {
    let mut tokens = Vec::with_capacity(32_768);
    tokens.extend(1..=16_384_u64);
    let original_pointer = tokens.as_ptr();
    let original_capacity = tokens.capacity();
    let mut collection = Cell::References(tokens);
    for round in 0..128 {
        let index = round * 97 % 16_384;
        let token = index as u64 + 1;
        let old = collection
            .replace_item(index, Cell::Unresolved("edit".into()))
            .unwrap();
        assert!(matches!(old, Cell::Reference(found) if found == token));
        let Cell::MixedReferences(mixed) = &collection else {
            panic!("exceptional reference collection");
        };
        assert_eq!(mixed.tokens.as_ptr(), original_pointer);
        assert_eq!(mixed.tokens.capacity(), original_capacity);
        assert_eq!(mixed.tokens[index], 0);
        assert_eq!(mixed.exceptional.len(), 1);

        let old = collection
            .replace_item(index, Cell::Reference(token))
            .unwrap();
        assert!(matches!(old, Cell::Unresolved(spelling) if spelling.as_ref() == "edit"));
        let Cell::References(tokens) = &collection else {
            panic!("fully linked reference collection");
        };
        assert_eq!(tokens.as_ptr(), original_pointer);
        assert_eq!(tokens.capacity(), original_capacity);
        let appended = tokens.len();
        collection.push(Cell::Unresolved("append".into())).unwrap();
        let Cell::MixedReferences(mixed) = &collection else {
            panic!("appended exception");
        };
        assert_eq!(mixed.tokens.as_ptr(), original_pointer);
        assert_eq!(mixed.exceptional.len(), 1);
        assert!(mixed.exceptional.contains_key(&appended));
        collection
            .replace_item(appended, Cell::Reference(appended as u64 + 1))
            .unwrap();
        let Cell::References(tokens) = &collection else {
            panic!("resolved appended exception");
        };
        assert_eq!(tokens.as_ptr(), original_pointer);
        assert_eq!(tokens.capacity(), original_capacity);
    }
    let Cell::References(tokens) = collection else {
        panic!("final compact reference collection");
    };
    assert!(tokens.iter().copied().eq(1..=16_512));
}

#[test]
fn splitting_sparse_exceptions_preserves_slot_order_and_rebases_tail_slots() {
    let mut collection = Cell::References(vec![1, 2, 3, 4, 5, 6]);
    collection.replace_item(1, Cell::Null).unwrap();
    collection
        .replace_item(3, Cell::Unresolved("tail".into()))
        .unwrap();
    collection
        .replace_item(5, Cell::raw(serde_json::json!(["raw", null])))
        .unwrap();
    let tail = collection.split_off(3).unwrap();
    let Cell::MixedReferences(prefix) = &collection else {
        panic!("prefix exception");
    };
    assert_eq!(prefix.tokens, vec![1, 0, 3]);
    assert_eq!(
        prefix.exceptional.keys().copied().collect::<Vec<_>>(),
        vec![1]
    );
    let Cell::MixedReferences(suffix) = &tail else {
        panic!("tail exceptions");
    };
    assert_eq!(suffix.tokens, vec![0, 5, 0]);
    assert_eq!(
        suffix.exceptional.keys().copied().collect::<Vec<_>>(),
        vec![0, 2]
    );
    let mut items = tail.into_cells().unwrap();
    assert!(matches!(items.next(), Some(Cell::Unresolved(value)) if value.as_ref() == "tail"));
    assert!(matches!(items.next(), Some(Cell::Reference(5))));
    assert!(
        matches!(items.next(), Some(Cell::List(values)) if matches!(values.as_slice(), [Cell::Text(value), Cell::Null] if value.as_ref() == "raw"))
    );
    assert!(items.next().is_none());
    collection.replace_item(1, Cell::Reference(2)).unwrap();
    assert!(matches!(collection, Cell::References(values) if values == [1, 2, 3]));
}
