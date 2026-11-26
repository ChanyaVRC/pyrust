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

/// Build a pyrust string from codepoints, writing lone surrogates as CESU-8.
fn surrogate_text(codepoints: &[u32]) -> String {
    let mut out = String::new();
    for &cp in codepoints {
        push_codepoint(&mut out, cp);
    }
    out
}

#[test]
fn surrogate_detection_skips_valid_ed_lead_bytes() {
    // Hangul syllables U+D000..=U+D7FF share the 0xED lead byte with the
    // surrogate block but are valid UTF-8; long inputs exercise the
    // word-at-a-time scan.
    for text in ["", "ascii", "한글", &"한글텍스트".repeat(9), "日本語🙂"] {
        assert!(!str_has_surrogates(text), "{text:?}");
    }
    for prefix_len in [0usize, 1, 7, 8, 9, 15, 16, 31] {
        let mut cps: Vec<u32> = "한".repeat(prefix_len).chars().map(u32::from).collect();
        cps.push(0xDCE9);
        let text = surrogate_text(&cps);
        assert!(str_has_surrogates(&text), "prefix {prefix_len}");
    }
    assert!(str_has_surrogates(&surrogate_text(&[0xD800])));
    assert!(str_has_surrogates(&surrogate_text(&[0xDFFF, 0x61])));
}

#[test]
fn surrogate_segments_split_text_runs_from_lone_surrogates() {
    let plain = "héllo 한글";
    assert_eq!(
        str_segments(plain).collect::<Vec<_>>(),
        vec![StrSegment::Text(plain)]
    );
    assert_eq!(str_segments("").count(), 0);

    let text = surrogate_text(&[0xDC80, 0x61, 0xD55C, 0xD83D, 0xDE00, 0x62]);
    let segments: Vec<_> = str_segments(&text).collect();
    assert_eq!(segments.len(), 5);
    assert!(matches!(segments[0], StrSegment::Surrogate { cp: 0xDC80, raw } if raw.len() == 3));
    assert_eq!(segments[1], StrSegment::Text("a한"));
    assert!(matches!(
        segments[2],
        StrSegment::Surrogate { cp: 0xD83D, .. }
    ));
    assert!(matches!(
        segments[3],
        StrSegment::Surrogate { cp: 0xDE00, .. }
    ));
    assert_eq!(segments[4], StrSegment::Text("b"));

    // Copying every segment through reproduces the input byte-for-byte.
    let upper = map_str_segments(&text, |run, out| out.push_str(&run.to_uppercase()));
    assert_eq!(
        upper.as_bytes(),
        surrogate_text(&[0xDC80, 0x41, 0xD55C, 0xD83D, 0xDE00, 0x42]).as_bytes()
    );
}

#[test]
fn surrogate_safe_cursors_and_offsets_round_trip() {
    let cps = [0x61, 0xDCE9, 0xE9, 0x1F642, 0xD800, 0x7A];
    let text = surrogate_text(&cps);
    assert_eq!(cesu8_codepoints(&text).collect::<Vec<_>>(), cps);
    assert_eq!(str_codepoint_count(&text), cps.len());

    // Walking backwards visits the same codepoints in reverse order.
    let mut end = text.len();
    let mut backwards = Vec::new();
    while let Some((cp, start)) = cesu8_prev_codepoint(&text, end) {
        backwards.push(cp);
        end = start;
    }
    assert_eq!(end, 0);
    backwards.reverse();
    assert_eq!(backwards, cps);

    let starts = [0usize, 1, 4, 6, 10, 13, 14];
    for (n, &expected) in starts.iter().enumerate() {
        assert_eq!(str_codepoint_offset(&text, n), expected, "offset {n}");
        assert_eq!(str_codepoint_prefix(&text, n).len(), expected);
    }
    assert_eq!(str_codepoint_offset(&text, 99), text.len());
}

#[test]
fn push_codepoint_writes_scalars_and_lone_surrogates() {
    let mut out = String::new();
    for cp in [0x41, 0xE9, 0x10FFFF, 0xD800, 0xDFFF] {
        push_codepoint(&mut out, cp);
    }
    assert_eq!(
        out.as_bytes(),
        b"A\xC3\xA9\xF4\x8F\xBF\xBF\xED\xA0\x80\xED\xBF\xBF"
    );
}

#[test]
fn surrogate_scan_and_offsets_cover_every_window_alignment() {
    // A surrogate at each byte offset exercises every lane of the 7-byte-step
    // window scan, after both ASCII and Hangul (0xED-led, non-surrogate) text.
    for filler in ['a', '\u{D55C}', '\u{E9}'] {
        for count in 0..24 {
            let prefix: String = std::iter::repeat_n(filler, count).collect();
            assert!(!str_has_surrogates(&prefix), "{filler:?} x {count}");
            let mut text = prefix.clone();
            push_codepoint(&mut text, 0xDC80);
            text.push_str(&prefix);
            assert!(str_has_surrogates(&text), "{filler:?} x {count}");
            let first = str_segments(&text).find_map(|seg| match seg {
                StrSegment::Surrogate { cp, .. } => Some(cp),
                StrSegment::Text(_) => None,
            });
            assert_eq!(first, Some(0xDC80));
        }
    }

    // Word-at-a-time codepoint offsets agree with `char_indices` on long
    // mixed-width text at every index, including the end boundary.
    let text = "ab\u{E9}\u{65E5}\u{1F642}cd\u{D55C}".repeat(7);
    let expected: Vec<usize> = text
        .char_indices()
        .map(|(offset, _)| offset)
        .chain(std::iter::once(text.len()))
        .collect();
    for (n, &offset) in expected.iter().enumerate() {
        assert_eq!(str_codepoint_offset(&text, n), offset, "index {n}");
    }
}
