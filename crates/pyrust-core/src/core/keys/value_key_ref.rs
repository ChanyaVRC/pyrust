// ─────────────────────────────────────────────────────────────────────────────
// ValueKeyRef — zero-alloc probe type for pure non-dynamic `Value` lookups
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum tuple nesting depth accepted by [`ValueKeyRef::try_new`]. Deeper
/// tuples fall back to `value_to_pykey` so stack usage stays bounded.
const MAX_VALUE_KEY_REF_DEPTH: usize = 64;

/// Borrowed `&Value` wrapper that hashes and compares identically to `PyKey`
/// for pure non-dynamic hashable values, allowing zero-allocation probes into
/// `IndexMap<PyKey, V>` / `IndexSet<PyKey>` (#454).
///
/// Only constructed via [`ValueKeyRef::try_new`], which accepts pure
/// non-dynamic values (`Int`, `Bool`, `Float`, `BigInt`, `Str`, `None`,
/// `Ellipsis`, `Bytes`, `Complex`, pure `FrozenSet`, and `Tuple`s recursively
/// containing only accepted values) and rejects every unhashable or dynamic /
/// `PyKey::Object`-producing value.
#[derive(Copy, Clone)]
pub struct ValueKeyRef<'a>(&'a Value);

impl<'a> ValueKeyRef<'a> {
    /// Wrap `value` as a zero-allocation `PyKey` probe if it is a pure
    /// non-dynamic hashable value.
    #[inline]
    pub fn try_new(value: &'a Value) -> Option<Self> {
        Self::is_pure_key(value, 0).then_some(Self(value))
    }

    #[inline]
    fn is_pure_key(value: &Value, depth: usize) -> bool {
        match value.kind() {
            ValueKind::Int(_)
            | ValueKind::Bool(_)
            | ValueKind::Float(_)
            | ValueKind::BigInt(_)
            | ValueKind::Str(_)
            | ValueKind::None
            | ValueKind::Ellipsis
            | ValueKind::Bytes(_)
            | ValueKind::Complex(_, _) => true,
            ValueKind::Tuple(items) => {
                depth < MAX_VALUE_KEY_REF_DEPTH
                    && items.iter().all(|item| Self::is_pure_key(item, depth + 1))
            }
            ValueKind::BuiltinObject { ops, state } => {
                if ops.canonical_class_tag() != Some(CanonicalClassTag::Frozenset) {
                    return false;
                }
                let mut is_pure = false;
                let matched = ops.with_frozenset_key(state, &mut |fs| {
                    is_pure = !fs.contains_object();
                });
                matched && is_pure
            }
            _ => false,
        }
    }

    #[inline(never)]
    fn hash_slow<H: Hasher>(&self, state: &mut H) {
        match self.0.kind() {
            ValueKind::Bool(b) => hash_i64_key(b as i64, state),
            ValueKind::Float(f) => hash_float_bits(f.to_bits(), state),
            ValueKind::BigInt(n) => hash_bigint_key(n, state),
            ValueKind::None => {
                (py_hash_none() as u64).hash(state);
            }
            ValueKind::Ellipsis => {
                (py_hash_ellipsis() as u64).hash(state);
            }
            ValueKind::Bytes(b) => {
                6u8.hash(state);
                b.as_ref().hash(state);
            }
            ValueKind::Complex(re, im) => hash_complex_key(re, im, state),
            ValueKind::Tuple(items) => {
                5u8.hash(state);
                items.len().hash(state);
                for item in items {
                    ValueKeyRef(item).hash(state);
                }
            }
            ValueKind::BuiltinObject {
                ops,
                state: builtin_state,
            } => {
                let mut py_hash = 0i64;
                let matched = ops.with_frozenset_key(builtin_state, &mut |fs| {
                    py_hash = fs.py_hash();
                });
                debug_assert!(matched, "ValueKeyRef only accepts frozenset BuiltinObjects");
                4u8.hash(state);
                py_hash.hash(state);
            }
            ValueKind::Int(_) | ValueKind::Str(_) => unreachable!(),
            _ => unreachable!("ValueKeyRef invariant violated"),
        }
    }

    #[inline(never)]
    fn equivalent_slow(&self, key: &PyKey) -> bool {
        match (self.0.kind(), key) {
            (ValueKind::Int(a), PyKey::Int(b)) => a == *b,
            (ValueKind::Int(a), PyKey::Bool(b)) => a == *b as i64,
            (ValueKind::Int(a), PyKey::Float(bits)) => float_bits_eq_i64(*bits, a),
            (ValueKind::Int(a), PyKey::BigInt(n)) => *n.as_ref() == BigInt::from(a),
            (ValueKind::Int(a), PyKey::Complex(re, im)) => {
                *im == 0.0 && float_bits_eq_i64(re.to_bits(), a)
            }

            (ValueKind::Bool(a), PyKey::Bool(b)) => a == *b,
            (ValueKind::Bool(a), PyKey::Int(b)) => *b == a as i64,
            (ValueKind::Bool(a), PyKey::Float(bits)) => float_bits_eq_i64(*bits, a as i64),
            (ValueKind::Bool(a), PyKey::Complex(re, im)) => {
                *im == 0.0 && float_bits_eq_i64(re.to_bits(), a as i64)
            }

            (ValueKind::Float(a), PyKey::Float(b_bits)) => {
                a.to_bits() == *b_bits || a == f64::from_bits(*b_bits)
            }
            (ValueKind::Float(a), PyKey::Int(i)) => float_bits_eq_i64(a.to_bits(), *i),
            (ValueKind::Float(a), PyKey::Bool(b)) => float_bits_eq_i64(a.to_bits(), *b as i64),
            (ValueKind::Float(a), PyKey::BigInt(n)) => bigint_float_eq(n, a),
            (ValueKind::Float(a), PyKey::Complex(re, im)) => *im == 0.0 && *re == a,

            (ValueKind::BigInt(a), PyKey::BigInt(b)) => a == b.as_ref(),
            (ValueKind::BigInt(a), PyKey::Int(b)) => *a == BigInt::from(*b),
            (ValueKind::BigInt(a), PyKey::Bool(b)) => a.to_i64().is_some_and(|i| i == *b as i64),
            (ValueKind::BigInt(a), PyKey::Float(bits)) => bigint_float_eq(a, f64::from_bits(*bits)),
            (ValueKind::BigInt(a), PyKey::Complex(re, im)) => *im == 0.0 && bigint_float_eq(a, *re),

            (ValueKind::Str(a), PyKey::Str(b)) => b.as_str() == Some(a),
            (ValueKind::None, PyKey::None) => true,
            (ValueKind::Ellipsis, PyKey::Ellipsis) => true,
            (ValueKind::Bytes(a), PyKey::Bytes(b)) => a.as_ref() == b.as_ref(),

            (ValueKind::Complex(ar, ai), PyKey::Complex(br, bi)) => {
                complex_key_components_eq(ar, ai, *br, *bi)
            }
            (ValueKind::Complex(re, im), other) => real_valued_complex_eq(re, im, other),

            (ValueKind::Tuple(items), PyKey::Tuple(keys)) => {
                items.len() == keys.len()
                    && items
                        .iter()
                        .zip(keys.iter())
                        .all(|(v, k)| indexmap::Equivalent::equivalent(&ValueKeyRef(v), k))
            }

            (
                ValueKind::BuiltinObject {
                    ops,
                    state: builtin_state,
                },
                PyKey::FrozenSet(other),
            ) => {
                let mut eq = false;
                let matched = ops.with_frozenset_key(builtin_state, &mut |fs| {
                    eq = std::ptr::eq(fs as *const FrozenSetKey, Rc::as_ptr(other))
                        || fs.items() == other.items();
                });
                debug_assert!(matched, "ValueKeyRef only accepts frozenset BuiltinObjects");
                eq
            }

            _ => false,
        }
    }
}

impl Hash for ValueKeyRef<'_> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self.0.kind() {
            ValueKind::Int(v) => hash_i64_key(v, state),
            ValueKind::Str(s) => {
                2u8.hash(state);
                s.hash(state);
            }
            _ => self.hash_slow(state),
        }
    }
}

impl indexmap::Equivalent<PyKey> for ValueKeyRef<'_> {
    #[inline]
    fn equivalent(&self, key: &PyKey) -> bool {
        match (self.0.kind(), key) {
            (ValueKind::Int(a), PyKey::Int(b)) => a == *b,
            (ValueKind::Str(a), PyKey::Str(b)) => b.as_str() == Some(a),
            _ => self.equivalent_slow(key),
        }
    }
}
