// ── float_bits_as_exact_i64 boundary tests ───────────────────────────────

#[test]
fn float_bits_exact_i64_integer_values() {
    // Ordinary integer-valued floats within i64 range.
    assert_eq!(float_bits_as_exact_i64(1.0f64.to_bits()), Some(1));
    assert_eq!(float_bits_as_exact_i64((-1.0f64).to_bits()), Some(-1));
    assert_eq!(float_bits_as_exact_i64(0.0f64.to_bits()), Some(0));
    assert_eq!(float_bits_as_exact_i64(42.0f64.to_bits()), Some(42));
    assert_eq!(
        float_bits_as_exact_i64(1_000_000_000_000_000.0f64.to_bits()),
        Some(1_000_000_000_000_000)
    );
}

#[test]
fn float_bits_exact_i64_fractional_returns_none() {
    assert_eq!(float_bits_as_exact_i64(0.5f64.to_bits()), None);
    assert_eq!(float_bits_as_exact_i64(1.5f64.to_bits()), None);
    assert_eq!(float_bits_as_exact_i64((-0.1f64).to_bits()), None);
}

#[test]
fn float_bits_exact_i64_non_finite_returns_none() {
    assert_eq!(float_bits_as_exact_i64(f64::INFINITY.to_bits()), None);
    assert_eq!(float_bits_as_exact_i64(f64::NEG_INFINITY.to_bits()), None);
    assert_eq!(float_bits_as_exact_i64(f64::NAN.to_bits()), None);
}

#[test]
fn float_bits_exact_i64_i64_min_is_exact() {
    // i64::MIN as f64 is exactly representable (-2^63); must return Some.
    let min_f = i64::MIN as f64;
    assert_eq!(float_bits_as_exact_i64(min_f.to_bits()), Some(i64::MIN));
}

#[test]
fn float_bits_exact_i64_i64_max_rounds_up() {
    // i64::MAX = 2^63-1 is not exactly representable as f64; the nearest f64
    // is 2^63, which is out of range.  Must return None.
    let max_f = i64::MAX as f64; // rounds up to 2^63
    assert_eq!(float_bits_as_exact_i64(max_f.to_bits()), None);
}

#[test]
fn float_bits_exact_i64_out_of_range_large() {
    // 2^63 is exactly representable but exceeds i64::MAX (= 2^63 - 1).
    let too_big = 9_223_372_036_854_775_808.0f64; // 2^63
    assert_eq!(float_bits_as_exact_i64(too_big.to_bits()), None);
    // 2^64 — clearly out of range.
    let much_bigger = 1.844_674_407_370_955_2e19_f64; // 2^64
    assert_eq!(float_bits_as_exact_i64(much_bigger.to_bits()), None);
    // Negative out-of-range: the f64 immediately below i64::MIN as f64.
    // i64::MIN as f64 is exactly -2^63 (in range); the next f64 towards
    // -inf has integer value -2^63 - 2^10, which is out of i64 range.
    let min_f = i64::MIN as f64;
    // Construct the next representable f64 below min_f by decrementing bits.
    let next_below_bits = min_f.to_bits() + 1; // negative floats: +1 bits → more negative
    let too_small = f64::from_bits(next_below_bits);
    assert!(
        too_small < min_f,
        "sanity: next_below must be more negative than i64::MIN as f64"
    );
    assert_eq!(float_bits_as_exact_i64(next_below_bits), None);
}

#[test]
fn pykey_float_int_cross_type_eq() {
    // Core contract: Float(1.0) == Int(1) and they hash equal.
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    fn key_hash(k: &PyKey) -> u64 {
        let mut h = DefaultHasher::new();
        k.hash(&mut h);
        h.finish()
    }

    let f1 = PyKey::Float(1.0f64.to_bits());
    let i1 = PyKey::Int(1);
    assert_eq!(f1, i1, "Float(1.0) must equal Int(1)");
    assert_eq!(
        key_hash(&f1),
        key_hash(&i1),
        "hash(Float(1.0)) must equal hash(Int(1))"
    );

    // 0.5 must NOT equal 0
    let f05 = PyKey::Float(0.5f64.to_bits());
    let i0 = PyKey::Int(0);
    assert_ne!(f05, i0, "Float(0.5) must not equal Int(0)");

    // Float(-1.0) == Int(-1)
    let fn1 = PyKey::Float((-1.0f64).to_bits());
    let in1 = PyKey::Int(-1);
    assert_eq!(fn1, in1, "Float(-1.0) must equal Int(-1)");
    assert_eq!(key_hash(&fn1), key_hash(&in1), "hash contract for -1.0/-1");

    // Float(1.0) == Bool(true)
    let bt = PyKey::Bool(true);
    assert_eq!(f1, bt, "Float(1.0) must equal Bool(true)");
    assert_eq!(key_hash(&f1), key_hash(&bt), "hash contract for 1.0/true");
}

#[test]
fn pykey_real_valued_complex_unifies_with_the_numeric_types() {
    // #2900: `1+0j` stays a complex key but shares one dict slot with
    // `1`, `1.0` and `True`, so it must compare *and* hash equal to them.
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    fn key_hash(k: &PyKey) -> u64 {
        let mut h = DefaultHasher::new();
        k.hash(&mut h);
        h.finish()
    }

    let z1 = PyKey::Complex(1.0, 0.0);
    for equivalent in [
        PyKey::Int(1),
        PyKey::Float(1.0f64.to_bits()),
        PyKey::Bool(true),
    ] {
        assert_eq!(z1, equivalent, "1+0j must equal {equivalent:?}");
        assert_eq!(equivalent, z1, "equality must be symmetric");
        assert_eq!(
            key_hash(&z1),
            key_hash(&equivalent),
            "Hash+Eq contract for 1+0j vs {equivalent:?}"
        );
        assert_eq!(
            py_hash_pykey(&z1),
            py_hash_pykey(&equivalent),
            "CPython hash(1+0j) == hash({equivalent:?})"
        );
    }

    // A negative zero imaginary part still counts as real-valued, and
    // `-0.0` real still unifies with `0` / `False`.
    assert_eq!(PyKey::Complex(1.0, -0.0), PyKey::Int(1));
    assert_eq!(PyKey::Complex(-0.0, 0.0), PyKey::Int(0));
    assert_eq!(PyKey::Complex(-0.0, 0.0), PyKey::Bool(false));
    assert_eq!(
        key_hash(&PyKey::Complex(-0.0, 0.0)),
        key_hash(&PyKey::Int(0))
    );

    // A non-zero imaginary part is a separate key.
    assert_ne!(PyKey::Complex(1.0, 1.0), PyKey::Int(1));
    assert_ne!(PyKey::Complex(1.0, 1.0), PyKey::Float(1.0f64.to_bits()));
    assert_ne!(PyKey::Complex(0.5, 0.0), PyKey::Int(0));

    // Large integer-valued reals unify across the BigInt arm, matching
    // `{1e20: 'a', 10**20: 'b'}` collapsing to one entry.
    let big = num_bigint::BigInt::from(10u8).pow(20);
    let z_big = PyKey::Complex(1e20, 0.0);
    assert_eq!(z_big, PyKey::BigInt(Box::new(big.clone())));
    assert_eq!(
        key_hash(&z_big),
        key_hash(&PyKey::BigInt(Box::new(big))),
        "Hash+Eq contract for complex(1e20) vs 10**20"
    );

    // NaN: a complex NaN key finds itself, but is not equal to the plain
    // float NaN (CPython reports `complex(nan, 0) == nan` as False).
    let nan_bits = f64::NAN.to_bits();
    assert_eq!(PyKey::Complex(f64::NAN, 0.0), PyKey::Complex(f64::NAN, 0.0));
    assert_ne!(PyKey::Complex(f64::NAN, 0.0), PyKey::Float(nan_bits));
}

#[test]
fn pykey_distinct_nan_floats_are_distinct_keys() {
    // #2911/#2954: two independently boxed NaNs occupy two dict slots and
    // expose distinct non-zero Python hashes, while a NaN still finds and
    // hashes like itself.  `0.0` / `-0.0` must keep collapsing.
    let a = Value::float(f64::NAN).to_key().expect("NaN is hashable");
    let b = Value::float(f64::NAN).to_key().expect("NaN is hashable");

    assert_eq!(a, a.clone(), "a NaN key must find itself");
    assert_ne!(a, b, "two distinct NaN objects must be distinct keys");
    let a_hash = py_hash_pykey(&a);
    let b_hash = py_hash_pykey(&b);
    assert_ne!(a_hash, 0, "a NaN hash must be non-zero");
    assert_ne!(b_hash, 0, "a NaN hash must be non-zero");
    assert_ne!(a_hash, b_hash, "distinct NaNs must hash apart");
    assert_eq!(a_hash, py_hash_pykey(&a.clone()), "alias hash is stable");

    let (PyKey::Float(a_bits), PyKey::Float(b_bits)) = (&a, &b) else {
        unreachable!("boxed NaNs must produce float keys")
    };
    assert_eq!(a_hash, py_hash_nan(f64::from_bits(*a_bits)));
    assert_eq!(b_hash, py_hash_nan(f64::from_bits(*b_bits)));
    assert_ne!(
        py_hash_pykey(&PyKey::Complex(f64::from_bits(*a_bits), 1.0)),
        py_hash_pykey(&PyKey::Complex(f64::from_bits(*b_bits), 1.0)),
        "distinct NaN-bearing complex values must hash apart"
    );

    // The helper applies Python's reserved hash-sentinel remap even for a
    // hostile NaN bit pattern that rotates to -1.
    assert_eq!(py_hash_nan(f64::from_bits(u64::MAX)), -2);

    let mut set = PySet::default();
    set.insert(a.clone());
    set.insert(b.clone());
    set.insert(a.clone());
    assert_eq!(set.len(), 2, "distinct NaNs occupy two slots, repeats none");
    assert!(set.contains(&a) && set.contains(&b));

    // Value-equal floats still collapse — the identity rule is NaN-only.
    let mut zeros = PySet::default();
    for key in [
        Value::float(0.0).to_key().unwrap(),
        Value::float(-0.0).to_key().unwrap(),
        Value::int(0).to_key().unwrap(),
        Value::bool_(false).to_key().unwrap(),
    ] {
        zeros.insert(key);
    }
    assert_eq!(zeros.len(), 1, "0.0/-0.0/0/False must stay one key");
}

#[test]
fn pykey_frozenset_hash_is_order_independent_and_cached() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    fn key_hash(key: &PyKey) -> u64 {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish()
    }

    let mut left_items = PySet::default();
    left_items.insert(PyKey::Int(1));
    left_items.insert(PyKey::Int(2));

    let mut right_items = PySet::default();
    right_items.insert(PyKey::Int(2));
    right_items.insert(PyKey::Int(1));

    let left_backing = Rc::new(FrozenSetKey::new(Rc::new(left_items)));
    let right_backing = Rc::new(FrozenSetKey::new(Rc::new(right_items)));
    let left = PyKey::FrozenSet(Rc::clone(&left_backing));
    let right = PyKey::FrozenSet(right_backing);

    assert_eq!(left, right);
    assert_eq!(key_hash(&left), key_hash(&right));
    assert_eq!(py_hash_pykey(&left), py_hash_pykey(&right));

    let PyKey::FrozenSet(shared) = left.clone() else {
        unreachable!();
    };
    assert!(Rc::ptr_eq(&left_backing, &shared));
}

#[test]
fn pykey_layout_is_at_most_24_bytes() {
    assert!(
        std::mem::size_of::<PyKey>() <= 24,
        "PyKey grew beyond 24 bytes: {}",
        std::mem::size_of::<PyKey>()
    );
}

#[test]
fn value_key_ref_matches_pykey_hash_and_eq_across_full_cross_product() {
    use super::{BuiltinState, PyHasher, ValueKeyRef, py_hash_none};
    use num_bigint::BigInt;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{BuildHasher, Hash, Hasher};

    struct MockFrozenSetOps;
    static MOCK_FROZENSET_OPS: MockFrozenSetOps = MockFrozenSetOps;

    impl BuiltinTypeOps for MockFrozenSetOps {
        fn type_name(&self) -> &'static str {
            "frozenset"
        }

        fn canonical_class_tag(&self) -> Option<CanonicalClassTag> {
            Some(CanonicalClassTag::Frozenset)
        }

        fn to_key(&self, state: &BuiltinState) -> Option<PyKey> {
            let borrow = state.borrow();
            borrow
                .downcast_ref::<Rc<FrozenSetKey>>()
                .map(|k| PyKey::FrozenSet(Rc::clone(k)))
        }

        fn with_frozenset_key(
            &self,
            state: &BuiltinState,
            f: &mut dyn FnMut(&FrozenSetKey),
        ) -> bool {
            let borrow = state.borrow();
            if let Some(k) = borrow.downcast_ref::<Rc<FrozenSetKey>>() {
                f(k);
                true
            } else {
                false
            }
        }
    }

    fn mock_frozenset(items: PySet) -> Value {
        let key = Rc::new(FrozenSetKey::new(Rc::new(items)));
        Value::builtin_object(&MOCK_FROZENSET_OPS, Box::new(key))
    }

    fn fx_hash<T: Hash>(v: &T) -> u64 {
        PyHasher::default().hash_one(v)
    }

    fn default_hash<T: Hash>(v: &T) -> u64 {
        let mut h = DefaultHasher::new();
        v.hash(&mut h);
        h.finish()
    }

    let nan_a = Value::float(f64::NAN);
    let nan_b = Value::float(f64::NAN);
    let nan_a_bits = match nan_a.kind() {
        ValueKind::Float(f) => f,
        _ => unreachable!(),
    };
    let nan_b_bits = match nan_b.kind() {
        ValueKind::Float(f) => f,
        _ => unreachable!(),
    };

    let mut fs_12 = PySet::default();
    fs_12.insert(PyKey::Int(1));
    fs_12.insert(PyKey::Int(2));
    let mut fs_21 = PySet::default();
    fs_21.insert(PyKey::Int(2));
    fs_21.insert(PyKey::Int(1));
    let mut fs_empty = PySet::default();
    let _ = &mut fs_empty;

    let big_pos = BigInt::from(10u64).pow(20);
    let big_neg = -&big_pos;

    let pure_values: Vec<Value> = vec![
        Value::int(0),
        Value::int(1),
        Value::int(-1),
        Value::int(42),
        Value::int(i64::MIN),
        Value::int(i64::MAX),
        Value::bool_(false),
        Value::bool_(true),
        Value::float(0.0),
        Value::float(-0.0),
        Value::float(1.0),
        Value::float(-1.0),
        Value::float(1.5),
        Value::float(1e20),
        Value::float(-1e20),
        Value::float(f64::INFINITY),
        Value::float(f64::NEG_INFINITY),
        nan_a.clone(),
        nan_a.clone(),
        nan_b.clone(),
        Value::bigint(big_pos.clone()),
        Value::bigint(big_neg.clone()),
        Value::bigint(BigInt::from(1)),
        Value::bigint(BigInt::from(0)),
        Value::string(""),
        Value::string("abc"),
        Value::string("1"),
        Value::none(),
        Value::ellipsis(),
        Value::bytes(vec![]),
        Value::bytes(b"abc".to_vec()),
        Value::complex(0.0, 0.0),
        Value::complex(-0.0, -0.0),
        Value::complex(1.0, 0.0),
        Value::complex(1.0, -0.0),
        Value::complex(1.0, 2.0),
        Value::complex(1e20, 0.0),
        Value::complex(nan_a_bits, 0.0),
        Value::complex(nan_a_bits, 1.0),
        Value::complex(nan_b_bits, 1.0),
        Value::tuple(vec![]),
        Value::tuple(vec![Value::int(1), Value::string("a")]),
        Value::tuple(vec![Value::float(1.0), Value::string("a")]),
        Value::tuple(vec![Value::bool_(true), Value::string("a")]),
        Value::tuple(vec![
            Value::int(1),
            Value::tuple(vec![Value::float(0.0), Value::complex(1.0, 0.0)]),
        ]),
        Value::tuple(vec![
            Value::bool_(true),
            Value::tuple(vec![Value::int(0), Value::int(1)]),
        ]),
        Value::tuple(vec![nan_a.clone()]),
        Value::tuple(vec![nan_b.clone()]),
        mock_frozenset(fs_12),
        mock_frozenset(fs_21),
        mock_frozenset(fs_empty),
    ];

    // Build the full target PyKey list including PyKey::Object.
    let cls = Rc::new(RefCell::new(PyClass::new("K", "K", None, IndexMap::new())));
    let inst = plain_instance(Rc::clone(&cls));
    let mut target_keys: Vec<PyKey> = pure_values
        .iter()
        .map(|v| v.to_key().expect("pure value must convert to PyKey"))
        .collect();
    target_keys.push(PyKey::Object {
        hash: 1,
        value: inst.clone(),
    });
    target_keys.push(PyKey::Object {
        hash: py_hash_none() as u64,
        value: plain_instance(Rc::clone(&cls)),
    });
    // Also verify a tuple containing a PyKey::Object never matches a pure tuple.
    target_keys.push(PyKey::Tuple(
        vec![PyKey::Object {
            hash: 1,
            value: inst.clone(),
        }]
        .into_boxed_slice(),
    ));

    for (i, val) in pure_values.iter().enumerate() {
        let probe = ValueKeyRef::try_new(val)
            .unwrap_or_else(|| panic!("pure value #{i} ({}) rejected by try_new", val.repr_raw()));
        let owned = val.to_key().expect("pure value must convert to PyKey");

        assert_eq!(
            fx_hash(&probe),
            fx_hash(&owned),
            "FxHasher mismatch for value #{i} ({})",
            val.repr_raw()
        );
        assert_eq!(
            default_hash(&probe),
            default_hash(&owned),
            "DefaultHasher mismatch for value #{i} ({})",
            val.repr_raw()
        );

        for (j, target) in target_keys.iter().enumerate() {
            assert_eq!(
                indexmap::Equivalent::equivalent(&probe, target),
                owned == *target,
                "Equivalent mismatch for value #{i} ({}) vs key #{j} ({target:?})",
                val.repr_raw()
            );
        }
    }

    // Non-pure and unhashable values MUST be rejected by ValueKeyRef::try_new.
    let mut dynamic_fs = PySet::default();
    dynamic_fs.insert(PyKey::Object {
        hash: 1,
        value: inst.clone(),
    });
    let mut deep_tuple = Value::int(1);
    for _ in 0..70 {
        deep_tuple = Value::tuple(vec![deep_tuple]);
    }
    let rejected_values: Vec<Value> = vec![
        Value::list(vec![Value::int(1)]),
        Value::dict(PyDict::default()),
        Value::set(PySet::default()),
        Value::range(0, 10, 1),
        Value::not_implemented(),
        Value::py_class(Rc::clone(&cls)),
        inst.clone(),
        Value::tuple(vec![Value::int(1), Value::list(vec![])]),
        Value::tuple(vec![Value::int(1), inst.clone()]),
        mock_frozenset(dynamic_fs),
        deep_tuple,
    ];
    for (i, val) in rejected_values.iter().enumerate() {
        assert!(
            ValueKeyRef::try_new(val).is_none(),
            "non-pure value #{i} ({}) must be rejected by ValueKeyRef::try_new",
            val.repr_raw()
        );
    }

    // Verify PyDict::get_mut / Value::dict_replace_existing preserves metadata & version.
    let dict_val = Value::dict(PyDict::default());
    let tup_val = Value::tuple(vec![Value::int(1), Value::string("x")]);
    dict_val
        .dict_insert(tup_val.to_key().unwrap(), Value::int(10))
        .unwrap();
    let ver_before = dict_val.dict_with(|d| d.structural_version()).unwrap();
    let probe = ValueKeyRef::try_new(&tup_val).unwrap();
    let old = dict_val
        .dict_replace_existing(&probe, false, None, Value::int(20))
        .unwrap()
        .expect("existing tuple key must hit");
    assert_eq!(old, Value::int(10));
    assert_eq!(
        dict_val.dict_with(|d| d.structural_version()).unwrap(),
        ver_before
    );
    assert!(!dict_val.dict_with(|d| d.may_have_dynamic_key()).unwrap());
    assert_eq!(
        dict_val.dict_with(|d| d.get(&probe).cloned()).unwrap(),
        Some(Value::int(20))
    );
}
