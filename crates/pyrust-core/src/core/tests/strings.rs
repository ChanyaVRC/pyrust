#[test]
fn unicode_string_metadata_transitions_from_length_to_offsets() {
    let text = Value::string("日本語🙂");
    let initial = unsafe { *text.str_unicode_state_slot() };
    assert_eq!(initial, 0);

    assert_eq!(text.str_codepoint_len(), 4);
    let length_state = unsafe { *text.str_unicode_state_slot() };
    assert_eq!(
        length_state,
        (4 << STR_CODEPOINT_LEN_SHIFT) | STR_CODEPOINT_LEN_TAG
    );

    assert_eq!(text.str_codepoint_len_for_index(), 4);
    assert_eq!(text.str_codepoint_byte_range(2), (6, 9));
    assert_eq!(text.str_codepoint_len_for_index(), 4);
    let offset_state = unsafe { *text.str_unicode_state_slot() };
    assert_ne!(offset_state, 0);
    assert_eq!(offset_state & STR_CODEPOINT_LEN_TAG, 0);
    assert_eq!(text.str_codepoint_len(), 4);
}

#[test]
fn inline_unicode_uses_payload_without_header_access() {
    for text in ["é", "界", "🙂"] {
        let value = Value::string(text);
        assert!(str_is_inline_bits(value.0));
        assert_eq!(value.str_codepoint_len(), 1);
        assert_eq!(value.str_codepoint_len_for_index(), 1);
        assert_eq!(value.str_codepoint_byte_range(0), (0, text.len()));
    }
}

#[test]
fn unicode_codepoint_counter_handles_utf8_and_cesu8() {
    for text in ["", "ascii", "日本語🙂", "aé界🙂z", "αβγδεζηθ"] {
        assert_eq!(
            utf8_codepoint_count(text.as_bytes()),
            text.chars().count(),
            "{text:?}"
        );
    }

    // U+D800 encoded as pyrust's three-byte CESU-8 sequence, followed by
    // ASCII and U+DFFF. Rust rejects these as scalar values, but pyrust
    // intentionally stores them in its str backing.
    let cesu8 = [0xED, 0xA0, 0x80, b'a', 0xED, 0xBF, 0xBF];
    assert_eq!(utf8_codepoint_count(&cesu8), 3);
    for (index, expected) in [0, 3, 4, 7].into_iter().enumerate() {
        assert_eq!(uncached_utf8_byte_offset(&cesu8, index, 3), expected);
    }

    for text in [
        "日本語🙂abcdefαβγδεζηθ",
        "aaaaaaa🙂bbbbbbbb界ccccccccé",
        "🙂🙂🙂🙂🙂🙂🙂🙂🙂",
    ] {
        let offsets: Vec<usize> = text
            .char_indices()
            .map(|(offset, _)| offset)
            .chain(std::iter::once(text.len()))
            .collect();
        for (index, expected) in offsets.into_iter().enumerate() {
            assert_eq!(
                uncached_utf8_byte_offset(text.as_bytes(), index, text.chars().count()),
                expected,
                "{text:?} at {index}"
            );
        }
    }
}

#[test]
fn unicode_slice_cache_survives_root_drop() {
    let root = Value::string("αβγδεζηθ");
    assert_eq!(root.str_codepoint_len_for_index(), 8);
    assert_eq!(root.str_codepoint_len_for_index(), 8);
    assert_eq!(root.str_codepoint_byte_offset(7), 14);
    let view = root.string_slice(2, 14);
    assert_eq!(unsafe { *view.str_unicode_state_slot() }, 0);
    drop(root);

    assert_eq!(view.str_codepoint_len(), 6);
    assert_eq!(view.str_codepoint_byte_range(4), (8, 10));
}

#[test]
fn unicode_append_invalidates_cached_metadata() {
    let mut text = Value::string("αβγδεζηθ");
    assert_eq!(text.str_codepoint_len(), 8);
    assert_ne!(unsafe { *text.str_unicode_state_slot() }, 0);

    assert!(text.str_append_in_place("界🙂"));
    assert_eq!(unsafe { *text.str_unicode_state_slot() }, 0);
    assert_eq!(text.str_codepoint_len(), 10);
    assert_eq!(text.str_codepoint_byte_range(9), (19, 23));
}

#[test]
fn public_string_raw_storage_helpers_reject_invalid_safe_inputs() {
    let scalar = Value::int(1);
    assert!(std::panic::catch_unwind(|| scalar.string_slice(0, 0)).is_err());
    assert!(std::panic::catch_unwind(|| scalar.str_codepoint_byte_offset(0)).is_err());
    assert!(std::panic::catch_unwind(|| scalar.str_codepoint_byte_range(0)).is_err());

    let text = Value::string("éclair");
    assert!(
        std::panic::catch_unwind(|| text.string_slice(0, text.as_str().unwrap().len() + 1))
            .is_err()
    );
    assert!(std::panic::catch_unwind(|| text.string_slice(1, 2)).is_err());

    let mut scalar = Value::int(1);
    assert!(!scalar.str_append_in_place("unsafe"));
    assert_eq!(scalar.as_int(), Some(1));
}

#[test]
fn saturated_string_refcount_preserves_layout_flags() {
    let text = Value::string("long enough for heap storage");
    let header = unsafe { text.str_hdr() as *mut u32 };
    let flags = unsafe { *header } & 0b111;
    unsafe { *header = (STR_RC_MAX << 3) | flags };

    let alias = text.clone();
    assert_eq!(unsafe { *header } & 0b111, flags);
    assert_eq!(unsafe { *header } >> 3, STR_RC_MAX);

    // Restore the exact number of live handles so the test does not leak
    // the allocation whose saturation behaviour it inspected.
    unsafe { *header = (2 * STR_RC_ONE) | flags };
    drop(alias);
    drop(text);
}
