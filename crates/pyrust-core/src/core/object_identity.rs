use std::any::TypeId;
use std::cell::{Cell, RefCell};

use num_bigint::{BigInt, Sign};

// Monotonic identity shared by the container/method representations that do
// not carry a backing allocation address.  `None` is the exhausted state: the
// last `u64` id is returned once, and the next allocation fails loudly instead
// of wrapping to a live object's id.
thread_local! {
    static OBJ_ID_COUNTER: Cell<Option<u64>> = const { Cell::new(Some(1)) };
}

fn take_next_obj_id(counter: &Cell<Option<u64>>) -> u64 {
    let id = counter
        .get()
        .expect("object identity counter exhausted without wrapping");
    counter.set(id.checked_add(1));
    id
}

pub(crate) fn next_obj_id() -> u64 {
    OBJ_ID_COUNTER.with(take_next_obj_id)
}

/// Exact internal identity before it is exposed as Python's numeric `id()`.
///
/// Variants are semantic namespaces, not presentation tags.  Equality is a
/// cheap, allocation-free comparison used by `is`; [`Self::encode`] maps the
/// same key injectively to a non-negative Python integer.  Keeping those two
/// operations on one key makes their equivalence structural rather than a
/// pair of implementations that tests merely sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ObjectIdentity {
    /// Address of the live allocation that is the Python object.
    Allocation(u64),
    /// Monotonic identity captured by a representation without a suitable
    /// allocation address (list/tuple/set backing, bound methods, and super).
    Counter(u64),
    /// Complete NaN-box bits for tagged inline values and inline strings.
    RawValue(u64),
    /// Complete untagged float box, including minted NaN identity payloads.
    Float(u64),
    /// Both component bit patterns; pyrust's complex `is` compares these.
    Complex { real: u64, imag: u64 },
    /// A built-in implementation type plus the custom identity payload
    /// supplied by its typed `BuiltinTypeOps::identity_payload` hook.
    Builtin { type_id: TypeId, payload: u64 },
}

pub(crate) enum EncodedObjectIdentity {
    Unsigned(u64),
    Wide(BigInt),
}

// One collision-free numeric namespace per concrete BuiltinTypeOps type.
// Values are thread-bound, so the append-only TLS registry is sufficient and
// avoids hashing TypeId into a fixed word (which would reintroduce collisions).
thread_local! {
    static BUILTIN_ID_TYPES: RefCell<Vec<TypeId>> = const { RefCell::new(Vec::new()) };
}

fn builtin_type_namespace(type_id: TypeId) -> u64 {
    BUILTIN_ID_TYPES.with(|types| {
        let mut types = types.borrow_mut();
        let index = match types.iter().position(|known| *known == type_id) {
            Some(index) => index,
            None => {
                let index = types.len();
                types.push(type_id);
                index
            }
        };
        u64::try_from(index).expect("built-in identity namespace exhausted")
    })
}

/// Encode one 64-bit payload in a numbered 64-bit-wide chunk.
///
/// A single `u128 -> BigInt` conversion avoids the temporary BigInts produced
/// by a shift followed by an addition on each `id()` call.
fn chunk_id(namespace: u64, payload: u64) -> BigInt {
    BigInt::from((u128::from(namespace) << 64) | u128::from(payload))
}

fn complex_id(real: u64, imag: u64) -> BigInt {
    // 1 || real || imag: an exact 129-bit positive integer in
    // [2^128, 2^129), built in one conversion.
    let mut bytes = [0_u8; 17];
    bytes[0] = 1;
    bytes[1..9].copy_from_slice(&real.to_be_bytes());
    bytes[9..17].copy_from_slice(&imag.to_be_bytes());
    BigInt::from_bytes_be(Sign::Plus, &bytes)
}

fn builtin_id(type_id: TypeId, payload: u64) -> BigInt {
    // 1 at bit 192 || a collision-free 64-bit type namespace || payload.
    // The gap above the complex namespace is intentional: the complete
    // 128-bit typed payload remains visually and mechanically separate.
    let namespace = builtin_type_namespace(type_id);
    let mut bytes = [0_u8; 25];
    bytes[0] = 1;
    bytes[9..17].copy_from_slice(&namespace.to_be_bytes());
    bytes[17..25].copy_from_slice(&payload.to_be_bytes());
    BigInt::from_bytes_be(Sign::Plus, &bytes)
}

impl ObjectIdentity {
    /// Inject this typed identity key into Python's non-negative integer space.
    ///
    /// ```text
    /// allocation: [0*2^64, 1*2^64)
    /// counter:    [1*2^64, 2*2^64)
    /// raw value:  [2*2^64, 3*2^64)
    /// float:      [3*2^64, 4*2^64)
    /// complex:    [2^128,   2^129)
    /// built-in:   2^192 | (type_namespace << 64) | payload
    /// ```
    pub(crate) fn encode(self) -> EncodedObjectIdentity {
        match self {
            Self::Allocation(address) => EncodedObjectIdentity::Unsigned(address),
            Self::Counter(id) => EncodedObjectIdentity::Wide(chunk_id(1, id)),
            Self::RawValue(bits) => EncodedObjectIdentity::Wide(chunk_id(2, bits)),
            Self::Float(bits) => EncodedObjectIdentity::Wide(chunk_id(3, bits)),
            Self::Complex { real, imag } => EncodedObjectIdentity::Wide(complex_id(real, imag)),
            Self::Builtin { type_id, payload } => {
                EncodedObjectIdentity::Wide(builtin_id(type_id, payload))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EncodedObjectIdentity, ObjectIdentity, take_next_obj_id};
    use num_bigint::BigInt;
    use std::any::TypeId;
    use std::cell::Cell;

    fn as_bigint(identity: ObjectIdentity) -> BigInt {
        match identity.encode() {
            EncodedObjectIdentity::Unsigned(id) => BigInt::from(id),
            EncodedObjectIdentity::Wide(id) => id,
        }
    }

    #[test]
    fn exact_numeric_identity_namespaces_are_disjoint() {
        let allocation_max = as_bigint(ObjectIdentity::Allocation(u64::MAX));
        let counter_min = as_bigint(ObjectIdentity::Counter(0));
        let counter_max = as_bigint(ObjectIdentity::Counter(u64::MAX));
        let raw_min = as_bigint(ObjectIdentity::RawValue(0));
        let raw_max = as_bigint(ObjectIdentity::RawValue(u64::MAX));
        let float_min = as_bigint(ObjectIdentity::Float(0));
        let float_max = as_bigint(ObjectIdentity::Float(u64::MAX));
        let complex_min = as_bigint(ObjectIdentity::Complex { real: 0, imag: 0 });
        let builtin_min = as_bigint(ObjectIdentity::Builtin {
            type_id: TypeId::of::<u8>(),
            payload: 0,
        });

        assert!(allocation_max < counter_min);
        assert!(counter_min < counter_max);
        assert!(counter_max < raw_min);
        assert!(raw_min < raw_max);
        assert!(raw_max < float_min);
        assert!(float_min < float_max);
        assert!(float_max < complex_min);
        assert!(complex_min < builtin_min);
    }

    #[test]
    fn builtin_type_namespace_is_exact_not_a_type_hash() {
        let first = as_bigint(ObjectIdentity::Builtin {
            type_id: TypeId::of::<u8>(),
            payload: 7,
        });
        let alias = as_bigint(ObjectIdentity::Builtin {
            type_id: TypeId::of::<u8>(),
            payload: 7,
        });
        let other_type = as_bigint(ObjectIdentity::Builtin {
            type_id: TypeId::of::<u16>(),
            payload: 7,
        });
        assert_eq!(first, alias);
        assert_ne!(first, other_type);
    }

    #[test]
    fn monotonic_identity_exhaustion_never_wraps() {
        let counter = Cell::new(Some(u64::MAX));
        assert_eq!(take_next_obj_id(&counter), u64::MAX);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                take_next_obj_id(&counter)
            }))
            .is_err()
        );
    }
}
