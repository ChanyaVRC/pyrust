use indexmap::IndexMap;
use pyrust_core::{BuiltinState, BuiltinTypeOps, Value, ValueKind};

use super::{BYTEARRAY_OPS, as_bytearray_snapshot, bytearray};

fn state(value: &Value) -> &BuiltinState {
    match value.kind() {
        ValueKind::BuiltinObject { state, .. } => state,
        _ => panic!("expected bytearray BuiltinObject"),
    }
}

#[test]
fn slice_assignment_materializes_aliased_rhs_before_mutation() {
    let value = bytearray(b"abcd".to_vec());
    let whole_slice = crate::slice::make_slice(None, None, None);

    BYTEARRAY_OPS
        .set_item(state(&value), &whole_slice, value.clone())
        .expect("self slice assignment should not overlap RefCell borrows");

    assert_eq!(as_bytearray_snapshot(&value).unwrap(), b"abcd");
}

#[test]
fn extended_slice_delete_compacts_in_one_pass() {
    let original: Vec<u8> = (0u8..100).collect();
    let value = bytearray(original.clone());
    let every_third = crate::slice::make_slice(None, None, Some(Value::int(3)));

    BYTEARRAY_OPS
        .delete_item(state(&value), &every_third)
        .unwrap();

    let expected: Vec<u8> = original
        .into_iter()
        .enumerate()
        .filter_map(|(index, byte)| (index % 3 != 0).then_some(byte))
        .collect();
    assert_eq!(as_bytearray_snapshot(&value).unwrap(), expected);
}

#[test]
fn pop_bool_out_of_range_is_an_error_not_a_vec_panic() {
    let value = bytearray(vec![b'x']);
    let result = BYTEARRAY_OPS.call_method(
        state(&value),
        "pop",
        vec![Value::bool_(true)],
        &IndexMap::new(),
    );

    assert!(result.is_err());
    assert_eq!(as_bytearray_snapshot(&value).unwrap(), b"x");
}

#[test]
fn minimum_i64_index_is_an_error_not_a_negation_panic() {
    let value = bytearray(vec![b'x']);

    assert!(
        BYTEARRAY_OPS
            .get_item(state(&value), &Value::int(i64::MIN))
            .is_err()
    );
    assert!(
        BYTEARRAY_OPS
            .call_method(
                state(&value),
                "pop",
                vec![Value::int(i64::MIN)],
                &IndexMap::new(),
            )
            .is_err()
    );
    assert_eq!(as_bytearray_snapshot(&value).unwrap(), b"x");
}
