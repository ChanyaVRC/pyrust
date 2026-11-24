use crate::errors::PyError;
use crate::numeric_ops::{
    NumericBinOp, NumericCmpOp, NumericOps, NumericSlot, NumericUnaryOp,
    bigint_to_float_or_overflow, dispatch_numeric_binop, dispatch_numeric_bool,
    dispatch_numeric_compare, dispatch_numeric_unary,
};
use crate::object_model::{PyBigInt, Value, ValueKind};

fn assert_named_err<T: std::fmt::Debug>(res: crate::errors::Result<T>, expected_type: &str) {
    match res {
        Err(PyError::Named(cls, _)) => assert_eq!(cls.as_ref(), expected_type),
        other => panic!("expected {expected_type}, got {other:?}"),
    }
}

#[test]
fn binary_arithmetic_across_all_five_numeric_types() {
    let big = Value::bigint(PyBigInt::from(1i64) << 70);
    let one_j = Value::complex(0.0, 1.0);
    let c = Value::complex(1.0, 2.0);

    // Int overflow promotion to BigInt.
    let overflow_add =
        dispatch_numeric_binop(NumericBinOp::Add, &Value::int(i64::MAX), &Value::int(1))
            .unwrap()
            .unwrap();
    assert_eq!(overflow_add, Value::bigint(PyBigInt::from(i64::MAX) + 1));

    let overflow_sub =
        dispatch_numeric_binop(NumericBinOp::Sub, &Value::int(i64::MIN), &Value::int(1))
            .unwrap()
            .unwrap();
    assert_eq!(overflow_sub, Value::bigint(PyBigInt::from(i64::MIN) - 1));

    let overflow_mul =
        dispatch_numeric_binop(NumericBinOp::Mul, &Value::int(i64::MAX), &Value::int(2))
            .unwrap()
            .unwrap();
    assert_eq!(overflow_mul, Value::bigint(PyBigInt::from(i64::MAX) * 2));

    let overflow_fdiv = dispatch_numeric_binop(
        NumericBinOp::FloorDiv,
        &Value::int(i64::MIN),
        &Value::int(-1),
    )
    .unwrap()
    .unwrap();
    assert_eq!(overflow_fdiv, Value::bigint(-PyBigInt::from(i64::MIN)));

    // Bool arithmetic yields Int / Float / Complex.
    let bt = dispatch_numeric_binop(NumericBinOp::Add, &Value::bool_(true), &Value::bool_(true))
        .unwrap()
        .unwrap();
    assert!(matches!(bt.kind(), ValueKind::Int(2)));

    let bf = dispatch_numeric_binop(NumericBinOp::Add, &Value::bool_(true), &Value::float(2.5))
        .unwrap()
        .unwrap();
    assert_eq!(bf, Value::float(3.5));

    let bc = dispatch_numeric_binop(NumericBinOp::Add, &Value::bool_(true), &one_j)
        .unwrap()
        .unwrap();
    assert_eq!(bc, Value::complex(1.0, 1.0));

    // Complex arithmetic (+, -, *, /, **) with all 5 numeric kinds.
    assert_eq!(
        dispatch_numeric_binop(NumericBinOp::Add, &c, &Value::int(3))
            .unwrap()
            .unwrap(),
        Value::complex(4.0, 2.0)
    );
    assert_eq!(
        dispatch_numeric_binop(NumericBinOp::Sub, &Value::float(5.0), &c)
            .unwrap()
            .unwrap(),
        Value::complex(4.0, -2.0)
    );
    assert_eq!(
        dispatch_numeric_binop(NumericBinOp::Mul, &c, &Value::complex(3.0, -4.0))
            .unwrap()
            .unwrap(),
        Value::complex(11.0, 2.0)
    );
    assert_eq!(
        dispatch_numeric_binop(
            NumericBinOp::Div,
            &Value::complex(0.0, 1e200),
            &Value::complex(0.0, 1e200),
        )
        .unwrap()
        .unwrap(),
        Value::complex(1.0, 0.0)
    );
    assert_named_err(
        dispatch_numeric_binop(NumericBinOp::Div, &c, &Value::complex(0.0, 0.0)).unwrap(),
        "ZeroDivisionError",
    );

    // BigInt + Complex and BigInt overflow -> OverflowError.
    let big_c = dispatch_numeric_binop(NumericBinOp::Add, &big, &one_j)
        .unwrap()
        .unwrap();
    assert_eq!(big_c, Value::complex((1u128 << 70) as f64, 1.0));

    let huge = Value::bigint(PyBigInt::from(10i64).pow(400));
    assert_named_err(
        dispatch_numeric_binop(NumericBinOp::Add, &huge, &one_j).unwrap(),
        "OverflowError",
    );
    assert_named_err(
        dispatch_numeric_binop(NumericBinOp::Add, &huge, &Value::float(1.0)).unwrap(),
        "OverflowError",
    );

    // Non-numeric RHS returns None (NotImplemented).
    assert!(
        dispatch_numeric_binop(NumericBinOp::Mul, &Value::int(3), &Value::string("ab")).is_none()
    );
    assert!(dispatch_numeric_binop(NumericBinOp::Add, &one_j, &Value::string("x")).is_none());
}

#[test]
fn pow_and_complex_pow_edge_cases() {
    // 0j ** 0 == 1+0j, 0j ** -1 -> ZeroDivisionError, 0j ** 1j -> ZeroDivisionError.
    assert_eq!(
        dispatch_numeric_binop(NumericBinOp::Pow, &Value::complex(0.0, 0.0), &Value::int(0))
            .unwrap()
            .unwrap(),
        Value::complex(1.0, 0.0)
    );
    assert_named_err(
        dispatch_numeric_binop(
            NumericBinOp::Pow,
            &Value::complex(0.0, 0.0),
            &Value::int(-1),
        )
        .unwrap(),
        "ZeroDivisionError",
    );
    assert_named_err(
        dispatch_numeric_binop(
            NumericBinOp::Pow,
            &Value::complex(0.0, 0.0),
            &Value::complex(0.0, 1.0),
        )
        .unwrap(),
        "ZeroDivisionError",
    );
    // Exact repeated squaring for small integer exponents.
    assert_eq!(
        dispatch_numeric_binop(NumericBinOp::Pow, &Value::complex(1.0, 1.0), &Value::int(2),)
            .unwrap()
            .unwrap(),
        Value::complex(0.0, 2.0)
    );
    // Negative float to fractional power promotes to complex; to NaN stays float NaN.
    let neg_nan = dispatch_numeric_binop(
        NumericBinOp::Pow,
        &Value::float(-2.0),
        &Value::float(f64::NAN),
    )
    .unwrap()
    .unwrap();
    assert!(matches!(neg_nan.kind(), ValueKind::Float(f) if f.is_nan()));

    // Int overflow promotion on **.
    let p64 = dispatch_numeric_binop(NumericBinOp::Pow, &Value::int(2), &Value::int(64))
        .unwrap()
        .unwrap();
    assert_eq!(p64, Value::bigint(PyBigInt::from(1i64) << 64));
}

#[test]
fn bitwise_shifts_and_unsupported_numeric_type_errors() {
    // bool bitwise preserves bool; mixed bool/int returns int.
    let bb = dispatch_numeric_binop(
        NumericBinOp::BitAnd,
        &Value::bool_(true),
        &Value::bool_(false),
    )
    .unwrap()
    .unwrap();
    assert!(matches!(bb.kind(), ValueKind::Bool(false)));
    let bi = dispatch_numeric_binop(NumericBinOp::BitOr, &Value::bool_(true), &Value::int(2))
        .unwrap()
        .unwrap();
    assert!(matches!(bi.kind(), ValueKind::Int(3)));
    let bx = dispatch_numeric_binop(
        NumericBinOp::BitXor,
        &Value::bool_(true),
        &Value::bool_(true),
    )
    .unwrap()
    .unwrap();
    assert!(matches!(bx.kind(), ValueKind::Bool(false)));

    // Shifts with BigInt promotion and negative shift count ValueError.
    let shl = dispatch_numeric_binop(NumericBinOp::LShift, &Value::int(1), &Value::int(80))
        .unwrap()
        .unwrap();
    assert_eq!(shl, Value::bigint(PyBigInt::from(1i64) << 80));
    assert_named_err(
        dispatch_numeric_binop(NumericBinOp::RShift, &Value::int(8), &Value::int(-1)).unwrap(),
        "ValueError",
    );

    // Complex floor-div, mod, bitwise, shift, and Float bitwise/shift -> TypeError.
    let c = Value::complex(1.0, 2.0);
    for op in [
        NumericBinOp::FloorDiv,
        NumericBinOp::Mod,
        NumericBinOp::BitAnd,
        NumericBinOp::BitOr,
        NumericBinOp::BitXor,
        NumericBinOp::LShift,
        NumericBinOp::RShift,
    ] {
        assert_named_err(
            dispatch_numeric_binop(op, &c, &Value::int(2)).unwrap(),
            "TypeError",
        );
        assert_named_err(
            dispatch_numeric_binop(op, &Value::bool_(true), &c).unwrap(),
            "TypeError",
        );
    }
    for op in [
        NumericBinOp::BitAnd,
        NumericBinOp::BitOr,
        NumericBinOp::BitXor,
        NumericBinOp::LShift,
        NumericBinOp::RShift,
    ] {
        assert_named_err(
            dispatch_numeric_binop(op, &Value::float(1.5), &Value::int(1)).unwrap(),
            "TypeError",
        );
    }
}

#[test]
fn unary_and_bool_slots_cover_all_five_types() {
    // Negation incl. i64::MIN promotion.
    assert_eq!(
        dispatch_numeric_unary(NumericUnaryOp::Neg, &Value::int(i64::MIN))
            .unwrap()
            .unwrap(),
        Value::bigint(-PyBigInt::from(i64::MIN))
    );
    assert_eq!(
        dispatch_numeric_unary(NumericUnaryOp::Neg, &Value::bool_(true))
            .unwrap()
            .unwrap(),
        Value::int(-1)
    );
    assert_eq!(
        dispatch_numeric_unary(NumericUnaryOp::Neg, &Value::complex(1.0, -2.0))
            .unwrap()
            .unwrap(),
        Value::complex(-1.0, 2.0)
    );

    // Positive preserves bool -> int coercion and NaN float identity (#2911).
    let pos_true = dispatch_numeric_unary(NumericUnaryOp::Pos, &Value::bool_(true))
        .unwrap()
        .unwrap();
    assert!(matches!(pos_true.kind(), ValueKind::Int(1)));
    let nan = Value::float(f64::NAN);
    let pos_nan = dispatch_numeric_unary(NumericUnaryOp::Pos, &nan)
        .unwrap()
        .unwrap();
    assert!(pos_nan.is_identical_to(&nan));

    // Invert works on int/bool/bigint and raises TypeError on float/complex.
    assert_eq!(
        dispatch_numeric_unary(NumericUnaryOp::Invert, &Value::bool_(true))
            .unwrap()
            .unwrap(),
        Value::int(-2)
    );
    assert_named_err(
        dispatch_numeric_unary(NumericUnaryOp::Invert, &Value::float(1.5)).unwrap(),
        "TypeError",
    );
    assert_named_err(
        dispatch_numeric_unary(NumericUnaryOp::Invert, &Value::complex(1.0, 2.0)).unwrap(),
        "TypeError",
    );

    // Absolute value incl. i64::MIN promotion, bool -> int, complex hypot.
    assert_eq!(
        dispatch_numeric_unary(NumericUnaryOp::Abs, &Value::int(i64::MIN))
            .unwrap()
            .unwrap(),
        Value::bigint(-PyBigInt::from(i64::MIN))
    );
    let abs_true = dispatch_numeric_unary(NumericUnaryOp::Abs, &Value::bool_(true))
        .unwrap()
        .unwrap();
    assert!(matches!(abs_true.kind(), ValueKind::Int(1)));
    assert_eq!(
        dispatch_numeric_unary(NumericUnaryOp::Abs, &Value::complex(3.0, 4.0))
            .unwrap()
            .unwrap(),
        Value::float(5.0)
    );

    // nb_bool across all 5 types.
    assert_eq!(dispatch_numeric_bool(&Value::int(0)), Some(false));
    assert_eq!(dispatch_numeric_bool(&Value::int(7)), Some(true));
    assert_eq!(dispatch_numeric_bool(&Value::bool_(false)), Some(false));
    assert_eq!(dispatch_numeric_bool(&Value::float(-0.0)), Some(false));
    assert_eq!(dispatch_numeric_bool(&Value::float(f64::NAN)), Some(true));
    assert_eq!(
        dispatch_numeric_bool(&Value::bigint(PyBigInt::from(1i64) << 70)),
        Some(true)
    );
    assert_eq!(
        dispatch_numeric_bool(&Value::complex(0.0, -0.0)),
        Some(false)
    );
    assert_eq!(dispatch_numeric_bool(&Value::complex(0.0, 1.0)), Some(true));
    assert_eq!(dispatch_numeric_bool(&Value::string("hi")), None);
}

#[test]
fn compare_slot_precision_nan_signed_zero_and_complex_ordering() {
    // Exact int == float and int == complex precision beyond 2^53.
    let i_big = (1i64 << 53) + 1;
    let f_rounded = i_big as f64;
    assert!(
        !dispatch_numeric_compare(
            NumericCmpOp::Eq,
            &Value::int(i_big),
            &Value::float(f_rounded),
        )
        .unwrap()
        .unwrap()
    );
    assert!(
        dispatch_numeric_compare(
            NumericCmpOp::Gt,
            &Value::int(i_big),
            &Value::float(f_rounded),
        )
        .unwrap()
        .unwrap()
    );
    assert!(
        !dispatch_numeric_compare(
            NumericCmpOp::Eq,
            &Value::int(i_big),
            &Value::complex(f_rounded, 0.0),
        )
        .unwrap()
        .unwrap()
    );

    // BigInt vs complex exact precision.
    let pow2_100 = Value::bigint(PyBigInt::from(1i64) << 100);
    let f_pow2_100 = 2.0_f64.powi(100);
    assert!(
        dispatch_numeric_compare(
            NumericCmpOp::Eq,
            &pow2_100,
            &Value::complex(f_pow2_100, 0.0),
        )
        .unwrap()
        .unwrap()
    );
    let ten_30 = Value::bigint(PyBigInt::from(10i64).pow(30));
    assert!(
        !dispatch_numeric_compare(NumericCmpOp::Eq, &ten_30, &Value::complex(1e30, 0.0))
            .unwrap()
            .unwrap()
    );

    // Bool == Complex and -0.0 == 0.0 == 0j.
    assert!(
        dispatch_numeric_compare(
            NumericCmpOp::Eq,
            &Value::bool_(true),
            &Value::complex(1.0, 0.0),
        )
        .unwrap()
        .unwrap()
    );
    assert!(
        dispatch_numeric_compare(
            NumericCmpOp::Eq,
            &Value::float(-0.0),
            &Value::complex(0.0, -0.0),
        )
        .unwrap()
        .unwrap()
    );

    // NaN comparisons.
    let nan = Value::float(f64::NAN);
    assert!(
        !dispatch_numeric_compare(NumericCmpOp::Eq, &nan, &nan)
            .unwrap()
            .unwrap()
    );
    assert!(
        dispatch_numeric_compare(NumericCmpOp::Ne, &nan, &nan)
            .unwrap()
            .unwrap()
    );
    for op in [
        NumericCmpOp::Lt,
        NumericCmpOp::Le,
        NumericCmpOp::Gt,
        NumericCmpOp::Ge,
    ] {
        assert!(
            !dispatch_numeric_compare(op, &nan, &Value::int(0))
                .unwrap()
                .unwrap()
        );
    }

    // Complex ordering raises TypeError across all four order operators.
    let c = Value::complex(1.0, 0.0);
    for op in [
        NumericCmpOp::Lt,
        NumericCmpOp::Le,
        NumericCmpOp::Gt,
        NumericCmpOp::Ge,
    ] {
        assert_named_err(
            NumericSlot::Complex(1.0, 0.0)
                .nb_compare(op, &Value::int(2))
                .unwrap(),
            "TypeError",
        );
        assert_named_err(
            dispatch_numeric_compare(op, &Value::int(2), &c).unwrap(),
            "TypeError",
        );
    }
}

#[test]
fn bigint_to_float_or_overflow_bounds() {
    let small = PyBigInt::from(1i64) << 100;
    assert_eq!(
        bigint_to_float_or_overflow(&small).unwrap(),
        2.0_f64.powi(100)
    );

    let huge = PyBigInt::from(10i64).pow(400);
    assert_named_err(bigint_to_float_or_overflow(&huge), "OverflowError");
}
