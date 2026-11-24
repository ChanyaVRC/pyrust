#[test]
fn value_remains_one_nanboxed_word() {
    assert_eq!(std::mem::size_of::<Value>(), 8);
    assert_eq!(std::mem::size_of::<Value>(), std::mem::size_of::<u64>());
    assert_eq!(std::mem::align_of::<Value>(), std::mem::align_of::<u64>());
}

#[test]
fn nanbox_heap_pointer_payload_checks_release_boundaries() {
    assert_eq!(try_nanbox_heap_pointer_payload(0), None);
    assert_eq!(try_nanbox_heap_pointer_payload(1), None);
    assert_eq!(
        try_nanbox_heap_pointer_payload(NANBOX_HEAP_POINTER_ALIGNMENT),
        Some(NANBOX_HEAP_POINTER_ALIGNMENT as u64)
    );

    let largest_aligned_payload = (PAYLOAD_MASK as usize) & !(NANBOX_HEAP_POINTER_ALIGNMENT - 1);
    assert_eq!(
        try_nanbox_heap_pointer_payload(largest_aligned_payload),
        Some(largest_aligned_payload as u64)
    );
    assert_eq!(
        try_nanbox_heap_pointer_payload((PAYLOAD_MASK as usize).saturating_add(1)),
        None
    );
    assert_eq!(try_nanbox_heap_pointer_payload(usize::MAX), None);
}

#[test]
fn nanbox_heap_pointer_encoder_preserves_every_address_bit() {
    let address = 0x0000_1234_5678_9ab8usize;
    // The helper does not dereference its input; this synthetic pointer
    // tests the exact bit-level round trip without allocating at a fixed
    // virtual address.
    let pointer = std::ptr::without_provenance::<u8>(address);
    let bits = encode_nanbox_heap_pointer(TAG_LIST_BITS, pointer);
    assert_eq!(bits & PAYLOAD_MASK, address as u64);
    assert_eq!(bits & !PAYLOAD_MASK, TAG_LIST_BITS);
}

#[test]
fn nanbox_owned_string_layout_checks_header_boundaries() {
    assert_eq!(
        try_nanbox_owned_string_layout(0).map(|layout| layout.size()),
        Some(STR_OWNED_HEADER_SIZE)
    );
    if usize::BITS > 32 {
        assert_eq!(
            try_nanbox_owned_string_layout(STR_MAX_BYTE_LEN).map(|layout| layout.size()),
            Some(STR_OWNED_HEADER_SIZE + STR_MAX_BYTE_LEN)
        );
    }
    assert!(try_nanbox_owned_string_layout(STR_MAX_BYTE_LEN.saturating_add(1)).is_none());
    assert!(try_nanbox_owned_string_layout(usize::MAX).is_none());
}

#[test]
fn not_implemented_round_trips_through_kind() {
    let v = Value::not_implemented();
    assert!(v.is_not_implemented());
    assert!(matches!(v.kind(), ValueKind::NotImplemented));
    assert!(!v.is_unset());
}

#[test]
fn not_implemented_is_not_classified_as_float() {
    // The NaN-box bit pattern shares the float top16 range; `kind()`
    // must intercept it before the float arm.  Regression guard for the
    // top16-vs-exact-bits caveat noted on `top16`.
    let v = Value::not_implemented();
    assert!(!matches!(v.kind(), ValueKind::Float(_)));
    assert!(!v.is_float());
}

#[test]
fn not_implemented_repr_is_canonical() {
    assert_eq!(Value::not_implemented().repr_raw(), "NotImplemented");
}

#[test]
fn unset_and_not_implemented_are_distinct_patterns() {
    // Both use the positive-NaN sentinel family; they must not collide.
    let unset = Value::unset();
    let nimpl = Value::not_implemented();
    assert!(unset.is_unset());
    assert!(!unset.is_not_implemented());
    assert!(nimpl.is_not_implemented());
    assert!(!nimpl.is_unset());
}

#[test]
fn unset_is_unset_returns_true() {
    // Basic sanity: the sentinel round-trips through is_unset().
    let v = Value::unset();
    assert!(v.is_unset());
    assert!(!v.is_none());
    assert!(!v.is_float());
    assert!(!v.is_not_implemented());
}

#[test]
fn minted_nans_carry_distinct_object_identities() {
    // #2911: each boxed NaN gets its own payload so that "same object" is
    // exactly "same bits" — the test `is_identical_nan` performs.
    let a = Value::float(f64::NAN);
    let b = Value::float(f64::NAN);

    for v in [&a, &b] {
        assert!(v.is_float(), "a minted NaN must still classify as float");
        assert!(matches!(v.kind(), ValueKind::Float(f) if f.is_nan()));
    }

    assert!(a.is_identical_nan(&a), "a NaN must be identical to itself");
    assert!(
        !a.is_identical_nan(&b),
        "two independently boxed NaNs must be distinct objects"
    );
    assert!(
        a.clone().is_identical_nan(&a),
        "cloning must preserve NaN identity"
    );
    // Bare equality is untouched: NaN never equals anything.
    assert!(a != b);
    assert!(a != a);
}

#[test]
fn scalar_bits_equal_is_safe_for_richcompare_fast_paths() {
    let one = Value::int(1);
    let one_float = Value::float(1.0);
    assert!(one.scalar_bits_equal(&one.clone()));
    assert!(!one.scalar_bits_equal(&one_float));
    assert_eq!(one, one_float);

    let nan = Value::float(f64::NAN);
    let other_nan = Value::float(f64::NAN);
    assert!(nan.scalar_bits_equal(&nan.clone()));
    assert!(!nan.scalar_bits_equal(&other_nan));

    let list = Value::list(vec![one]);
    assert!(!list.scalar_bits_equal(&list.clone()));
}

#[test]
fn minted_nans_never_alias_the_reserved_sentinels() {
    // The sentinels live at payloads 0xBAD0/0xBAD2/0xBAD4 in the same
    // positive-NaN family.  A collision would be silent and severe: a
    // 0xBAD0-payload NaN makes `is_unset()` true, and release builds only
    // `debug_assert!` against reading an unset slot.  `NAN_IDENTITY_BIT`
    // keeps every payload above that region — pin it over many mintings.
    for _ in 0..100_000 {
        let v = Value::float(f64::NAN);
        assert!(!v.is_unset(), "minted NaN aliased UNSET_BITS");
        assert!(
            !v.is_not_implemented(),
            "minted NaN aliased NOT_IMPLEMENTED_BITS"
        );
        assert!(
            !matches!(v.kind(), ValueKind::Ellipsis | ValueKind::NotImplemented),
            "minted NaN decoded as a singleton sentinel"
        );
        assert!(v.is_float());
    }
}

#[test]
fn float_from_bits_preserves_nan_identity_and_normalises_the_rest() {
    // Container iteration rebuilds a key through this constructor; it must
    // hand back the *same* NaN object the container stores (#2911).
    let n = Value::float(f64::NAN);
    let ValueKind::Float(raw) = n.kind() else {
        unreachable!("NaN must be a float");
    };
    let restored = Value::float_from_bits(raw.to_bits());
    assert!(
        n.is_identical_nan(&restored),
        "restoring a key's bits must preserve NaN identity"
    );

    // Ordinary floats round-trip by value, unchanged.
    for f in [0.0f64, -0.0, 1.0, -1.5, f64::INFINITY, f64::NEG_INFINITY] {
        let v = Value::float_from_bits(f.to_bits());
        assert_eq!(v, Value::float(f));
    }

    // A negative NaN never minted by `float()` must be normalised rather
    // than stored verbatim — its top 16 bits overlap the pointer tags.
    let hostile = Value::float_from_bits(0xFFFF_0000_0000_0000);
    assert!(hostile.is_float());
    assert!(matches!(hostile.kind(), ValueKind::Float(f) if f.is_nan()));
    assert!(!hostile.is_unset() && !hostile.is_not_implemented());
}

#[test]
fn unset_as_some_returns_none() {
    // as_some() is the safe way to probe an unset slot.
    let v = Value::unset();
    assert!(v.as_some().is_none());
}

// In debug builds, calling kind() on an unset Value must panic with a
// diagnostic message so missed CheckLocal emissions surface immediately
// rather than silently propagating a NaN through the program.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "uninitialised register slot")]
fn unset_kind_panics_in_debug() {
    let v = Value::unset();
    let _ = v.kind();
}

// In debug builds, truthy() routes through kind() and must also panic.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "uninitialised register slot")]
fn unset_truthy_panics_in_debug() {
    let v = Value::unset();
    let _ = v.truthy_raw();
}

// The direct NaN-box accessors bypass kind(), so they each need their own
// tripwire.  The following tests confirm that each one panics (rather than
// silently returning None / a garbage bit pattern) when called on an unset
// Value in a debug build.

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "uninitialised register slot")]
fn unset_as_int_panics_in_debug() {
    let v = Value::unset();
    let _ = v.as_int();
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "uninitialised register slot")]
fn unset_as_str_panics_in_debug() {
    let v = Value::unset();
    let _ = v.as_str();
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "uninitialised register slot")]
fn unset_as_bool_panics_in_debug() {
    let v = Value::unset();
    let _ = v.as_bool();
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "uninitialised register slot")]
fn unset_as_int_raw_panics_in_debug() {
    let v = Value::unset();
    let _ = v.as_int_raw();
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "uninitialised register slot")]
fn unset_as_float_raw_panics_in_debug() {
    let v = Value::unset();
    let _ = v.as_float_raw();
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "uninitialised register slot")]
fn unset_as_list_panics_in_debug() {
    let v = Value::unset();
    let _ = v.as_list();
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "uninitialised register slot")]
fn unset_as_tuple_panics_in_debug() {
    let v = Value::unset();
    let _ = v.as_tuple();
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "uninitialised register slot")]
fn unset_as_opaque_panics_in_debug() {
    let v = Value::unset();
    let _ = v.as_opaque();
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "uninitialised register slot")]
fn unset_as_dict_panics_in_debug() {
    let v = Value::unset();
    let _ = v.as_dict();
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "uninitialised register slot")]
fn unset_as_set_panics_in_debug() {
    let v = Value::unset();
    let _ = v.as_set();
}

// Regression: non-unset values must not be affected by the new guards.

#[test]
fn as_int_on_int_value_still_works() {
    assert_eq!(Value::int(42).as_int(), Some(42));
    assert_eq!(Value::none().as_int(), None);
}

#[test]
fn as_str_on_str_value_still_works() {
    assert_eq!(Value::string("hello").as_str(), Some("hello"));
    assert_eq!(Value::none().as_str(), None);
}
