use intersect_model::definitions::rcore_behavior::{
    ir::{BinaryOp, UnaryOp, Value, ValueType},
    types::Scalar,
};
use intersect_model::rcore_behavior_scalar::{
    self, ArithmeticErrorKind, NumericBudget, NumericLimits,
};
use serde_json::{Value as Json, json};
fn v(s: Scalar, payload: Json) -> Value {
    Value {
        ty: ValueType::Scalar(s),
        payload,
    }
}
fn integer(s: Scalar, n: &str) -> Value {
    v(
        s,
        match s {
            Scalar::Int64 => json!({"kind":"int64","value":n}),
            Scalar::UInt64 => json!({"kind":"uint64","value":n}),
            Scalar::BigInt => json!({"kind":"bigint","value":n}),
            _ => serde_json::from_str(n).unwrap(),
        },
    )
}
fn dec(n: &str, s: i32) -> Value {
    v(
        Scalar::Decimal,
        json!({"kind":"decimal","coefficient":n,"scale":s}),
    )
}
fn bin(
    op: BinaryOp,
    a: &Value,
    b: &Value,
) -> Result<Value, rcore_behavior_scalar::ArithmeticError> {
    rcore_behavior_scalar::binary(op, a, b, &mut NumericBudget::default())
}
#[test]
fn every_integer_width_checks_boundaries_and_mathematical_remainder() {
    for (s, min, max) in [
        (Scalar::Int8, "-128", "127"),
        (Scalar::Int16, "-32768", "32767"),
        (Scalar::Int32, "-2147483648", "2147483647"),
        (Scalar::Int64, "-9223372036854775808", "9223372036854775807"),
    ] {
        let one = integer(s, "1");
        let minus = integer(s, "-1");
        let low = integer(s, min);
        assert_eq!(
            bin(BinaryOp::Subtract, &low, &one).unwrap_err().kind,
            ArithmeticErrorKind::Overflow
        );
        for operand in [&low, &integer(s, max)] {
            assert_eq!(
                bin(BinaryOp::Multiply, operand, &integer(s, "2"))
                    .unwrap_err()
                    .kind,
                ArithmeticErrorKind::Overflow
            );
        }
        assert_eq!(
            bin(BinaryOp::Add, &integer(s, max), &one).unwrap_err().kind,
            ArithmeticErrorKind::Overflow
        );
        assert_eq!(
            bin(BinaryOp::Divide, &low, &minus).unwrap_err().kind,
            ArithmeticErrorKind::Overflow
        );
        assert_eq!(
            bin(BinaryOp::Remainder, &low, &minus).unwrap(),
            integer(s, "0")
        );
        assert_eq!(
            rcore_behavior_scalar::unary(UnaryOp::Negate, &low, &mut NumericBudget::default())
                .unwrap_err()
                .kind,
            ArithmeticErrorKind::Overflow
        );
        assert_eq!(
            bin(BinaryOp::Divide, &one, &integer(s, "0"))
                .unwrap_err()
                .kind,
            ArithmeticErrorKind::DivisionByZero
        );
    }
    for (s, max) in [
        (Scalar::UInt8, "255"),
        (Scalar::UInt16, "65535"),
        (Scalar::UInt32, "4294967295"),
        (Scalar::UInt64, "18446744073709551615"),
    ] {
        assert_eq!(
            rcore_behavior_scalar::unary(
                UnaryOp::Negate,
                &integer(s, "1"),
                &mut NumericBudget::default()
            )
            .unwrap_err()
            .kind,
            ArithmeticErrorKind::Overflow
        );
        assert_eq!(
            bin(BinaryOp::Multiply, &integer(s, max), &integer(s, "2"))
                .unwrap_err()
                .kind,
            ArithmeticErrorKind::Overflow
        );
        assert_eq!(
            bin(BinaryOp::Divide, &integer(s, "1"), &integer(s, "0"))
                .unwrap_err()
                .kind,
            ArithmeticErrorKind::DivisionByZero
        );
        assert_eq!(
            bin(BinaryOp::Add, &integer(s, max), &integer(s, "1"))
                .unwrap_err()
                .kind,
            ArithmeticErrorKind::Overflow
        );
        assert_eq!(
            bin(BinaryOp::Subtract, &integer(s, "0"), &integer(s, "1"))
                .unwrap_err()
                .kind,
            ArithmeticErrorKind::Overflow
        );
        assert_eq!(
            rcore_behavior_scalar::unary(
                UnaryOp::Negate,
                &integer(s, "0"),
                &mut NumericBudget::default()
            )
            .unwrap(),
            integer(s, "0")
        );
        assert_eq!(
            bin(BinaryOp::Multiply, &integer(s, max), &integer(s, "1")).unwrap(),
            integer(s, max)
        );
    }
}
#[test]
fn bigint_exactness_and_decimal_scale_math_are_distinct_from_storage_equality() {
    let a = integer(Scalar::BigInt, "999999999999999999999999999999999999");
    // Carry and borrow cross multiple backend limbs; cancellation produces canonical zero.
    let power = integer(Scalar::BigInt, "1000000000000000000000000000000000000");
    let one = integer(Scalar::BigInt, "1");
    assert_eq!(bin(BinaryOp::Add, &a, &one).unwrap(), power);
    assert_eq!(bin(BinaryOp::Subtract, &power, &one).unwrap(), a);
    let zero = integer(Scalar::BigInt, "0");
    assert_eq!(bin(BinaryOp::Subtract, &a, &a).unwrap(), zero);
    assert_eq!(
        rcore_behavior_scalar::unary(UnaryOp::Negate, &zero, &mut NumericBudget::default())
            .unwrap(),
        zero
    );
    assert_eq!(
        bin(BinaryOp::Multiply, &integer(Scalar::BigInt, "-1"), &zero).unwrap(),
        zero
    );
    assert_eq!(
        bin(BinaryOp::Multiply, &a, &integer(Scalar::BigInt, "10")).unwrap(),
        integer(Scalar::BigInt, "9999999999999999999999999999999999990")
    );
    assert_eq!(
        bin(
            BinaryOp::Divide,
            &integer(Scalar::BigInt, "-7"),
            &integer(Scalar::BigInt, "3")
        )
        .unwrap(),
        integer(Scalar::BigInt, "-2")
    );
    assert_eq!(
        bin(
            BinaryOp::Remainder,
            &integer(Scalar::BigInt, "-7"),
            &integer(Scalar::BigInt, "3")
        )
        .unwrap(),
        integer(Scalar::BigInt, "-1")
    );
    assert_eq!(
        bin(BinaryOp::Add, &dec("100", 2), &dec("2", 1)).unwrap(),
        dec("120", 2)
    );
    assert_eq!(
        bin(BinaryOp::Multiply, &dec("100", 2), &dec("20", 1)).unwrap(),
        dec("2000", 3)
    );
    assert_eq!(
        bin(BinaryOp::Divide, &dec("1", 0), &dec("8", 0)).unwrap(),
        dec("125", 3)
    );
    assert_eq!(
        bin(BinaryOp::Divide, &dec("100", 2), &dec("2", 0)).unwrap(),
        dec("50", 2)
    );
    assert_eq!(
        bin(BinaryOp::Divide, &dec("1", 0), &dec("3", 0))
            .unwrap_err()
            .kind,
        ArithmeticErrorKind::NonTerminatingDecimal
    );
    assert_eq!(
        bin(BinaryOp::Remainder, &dec("-70", 1), &dec("3", 0)).unwrap(),
        dec("-10", 1)
    );
    assert_eq!(
        bin(BinaryOp::Equal, &dec("100", 2), &dec("1", 0))
            .unwrap()
            .payload,
        json!(true)
    );
    assert_eq!(
        bin(BinaryOp::Greater, &dec("1", i32::MIN), &dec("1", i32::MAX))
            .unwrap()
            .payload,
        json!(true)
    );
    assert_eq!(
        bin(BinaryOp::Add, &dec("1", i32::MIN), &dec("1", i32::MAX))
            .unwrap_err()
            .kind,
        ArithmeticErrorKind::ResourceLimit
    );
    assert_eq!(
        bin(BinaryOp::Multiply, &dec("1", i32::MAX), &dec("1", 1))
            .unwrap_err()
            .kind,
        ArithmeticErrorKind::ScaleOverflow
    );
}
#[test]
fn ieee_float_precision_nonfinite_comparisons_and_signed_zero() {
    for (s, kind, one, zero, negzero, infinity, nan) in [
        (
            Scalar::Float32,
            "float32",
            "3f800000",
            "00000000",
            "80000000",
            "7f800000",
            "7fc00000",
        ),
        (
            Scalar::Float64,
            "float64",
            "3ff0000000000000",
            "0000000000000000",
            "8000000000000000",
            "7ff0000000000000",
            "7ff8000000000000",
        ),
    ] {
        let f = |bits: &str| v(s, json!({"kind":kind,"bits":bits}));
        assert_eq!(
            bin(BinaryOp::Divide, &f(one), &f(zero)).unwrap(),
            f(infinity)
        );
        assert_eq!(bin(BinaryOp::Divide, &f(zero), &f(zero)).unwrap(), f(nan));
        assert_eq!(bin(BinaryOp::Remainder, &f(one), &f(zero)).unwrap(), f(nan));
        assert_eq!(
            bin(BinaryOp::Equal, &f(zero), &f(negzero)).unwrap().payload,
            json!(true)
        );
        assert_eq!(
            bin(BinaryOp::Equal, &f(nan), &f(nan)).unwrap().payload,
            json!(false)
        );
        assert_eq!(
            bin(BinaryOp::LessEqual, &f(nan), &f(one)).unwrap().payload,
            json!(false)
        );
        assert_eq!(
            rcore_behavior_scalar::unary(UnaryOp::Negate, &f(zero), &mut NumericBudget::default())
                .unwrap(),
            f(negzero)
        );
    }
    let f = |bits: &str| v(Scalar::Float32, json!({"kind":"float32","bits":bits}));
    assert_eq!(
        bin(BinaryOp::Add, &f("4b800000"), &f("3f800000")).unwrap(),
        f("4b800000")
    );
}
#[test]
fn all_opaque_scalar_equalities_validate_their_exact_wire() {
    for a in [
        v(Scalar::Bool, json!(true)),
        v(Scalar::String, json!("hello")),
        v(Scalar::Unit, json!({"kind":"unit"})),
        v(Scalar::Bytes, json!({"kind":"bytes","hex":"00FF"})),
        v(Scalar::Char16, json!({"kind":"char16","unit":55296})),
        v(
            Scalar::DateInstant,
            json!({"kind":"date","milliseconds":"9007199254740993"}),
        ),
    ] {
        assert_eq!(bin(BinaryOp::Equal, &a, &a).unwrap().payload, json!(true));
        assert_eq!(
            bin(BinaryOp::Subtract, &a, &a).unwrap_err().kind,
            ArithmeticErrorKind::TypeMismatch
        );
        // String `+` is concatenation since 6aaa2047; the other opaque
        // scalars still reject it.
        if a.ty == ValueType::Scalar(Scalar::String) {
            assert_eq!(
                bin(BinaryOp::Add, &a, &a).unwrap(),
                v(Scalar::String, json!("hellohello"))
            );
        } else {
            assert_eq!(
                bin(BinaryOp::Add, &a, &a).unwrap_err().kind,
                ArithmeticErrorKind::TypeMismatch
            );
        }
    }
    assert!(
        bin(
            BinaryOp::Equal,
            &v(Scalar::Bytes, json!({"kind":"bytes","hex":"ff"})),
            &v(Scalar::Bytes, json!({"kind":"bytes","hex":"ff"}))
        )
        .is_err()
    );
    assert_eq!(
        bin(
            BinaryOp::Equal,
            &integer(Scalar::Int32, "1"),
            &integer(Scalar::Int64, "1")
        )
        .unwrap_err()
        .kind,
        ArithmeticErrorKind::TypeMismatch
    );
}
#[test]
fn deterministic_budgets_fail_then_fresh_budget_recovers() {
    let a = integer(Scalar::BigInt, "99");
    let mut budget = NumericBudget::new(NumericLimits {
        max_work: 1,
        ..NumericLimits::default()
    });
    assert_eq!(
        rcore_behavior_scalar::binary(BinaryOp::Add, &a, &a, &mut budget)
            .unwrap_err()
            .kind,
        ArithmeticErrorKind::ResourceLimit
    );
    let mut budget = NumericBudget::new(NumericLimits {
        max_digits: 2,
        ..NumericLimits::default()
    });
    assert_eq!(
        rcore_behavior_scalar::binary(BinaryOp::Multiply, &a, &a, &mut budget)
            .unwrap_err()
            .kind,
        ArithmeticErrorKind::ResourceLimit
    );
    assert!(budget.work_used() > 0);
    assert_eq!(
        bin(BinaryOp::Multiply, &a, &a).unwrap(),
        integer(Scalar::BigInt, "9801")
    );
    let mut budget = NumericBudget::new(NumericLimits {
        max_total_value_bytes: 1,
        ..NumericLimits::default()
    });
    assert_eq!(
        rcore_behavior_scalar::unary(UnaryOp::Plus, &a, &mut budget)
            .unwrap_err()
            .kind,
        ArithmeticErrorKind::ResourceLimit
    );
}

#[test]
fn numeric_input_resource_limits_keep_typed_errors_before_codec_allocation() {
    let a = integer(Scalar::BigInt, "123");
    let mut budget = NumericBudget::new(NumericLimits {
        max_digits: 2,
        ..NumericLimits::default()
    });
    assert_eq!(
        rcore_behavior_scalar::unary(UnaryOp::Plus, &a, &mut budget)
            .unwrap_err()
            .kind,
        ArithmeticErrorKind::ResourceLimit
    );
    let mut nested = json!(0);
    for _ in 0..100 {
        nested = json!([nested]);
    }
    let mut budget = NumericBudget::default();
    assert_eq!(
        rcore_behavior_scalar::unary(UnaryOp::Plus, &v(Scalar::Int32, nested), &mut budget)
            .unwrap_err()
            .kind,
        ArithmeticErrorKind::ResourceLimit
    );
    assert_eq!(
        rcore_behavior_scalar::unary(
            UnaryOp::Plus,
            &integer(Scalar::Int32, "1"),
            &mut NumericBudget::default()
        )
        .unwrap(),
        integer(Scalar::Int32, "1")
    );
}

#[test]
fn every_numeric_integer_operator_and_order_has_a_real_width_control() {
    for s in [
        Scalar::Int8,
        Scalar::Int16,
        Scalar::Int32,
        Scalar::Int64,
        Scalar::UInt8,
        Scalar::UInt16,
        Scalar::UInt32,
        Scalar::UInt64,
        Scalar::BigInt,
    ] {
        let a = integer(s, "7");
        let b = integer(s, "3");
        for (op, result) in [
            (BinaryOp::Add, "10"),
            (BinaryOp::Subtract, "4"),
            (BinaryOp::Multiply, "21"),
            (BinaryOp::Divide, "2"),
            (BinaryOp::Remainder, "1"),
        ] {
            assert_eq!(bin(op, &a, &b).unwrap(), integer(s, result), "{s:?} {op:?}");
        }
        for (op, result) in [
            (BinaryOp::Less, false),
            (BinaryOp::LessEqual, false),
            (BinaryOp::Greater, true),
            (BinaryOp::GreaterEqual, true),
            (BinaryOp::Equal, false),
            (BinaryOp::NotEqual, true),
        ] {
            assert_eq!(bin(op, &a, &b).unwrap().payload, json!(result));
        }
        assert_eq!(
            rcore_behavior_scalar::unary(UnaryOp::Plus, &a, &mut NumericBudget::default()).unwrap(),
            a
        );
        assert_eq!(
            bin(BinaryOp::Remainder, &a, &integer(s, "0"))
                .unwrap_err()
                .kind,
            ArithmeticErrorKind::DivisionByZero
        );
    }
    for s in [
        Scalar::Int8,
        Scalar::Int16,
        Scalar::Int32,
        Scalar::Int64,
        Scalar::BigInt,
    ] {
        for (a, b, q, r) in [
            ("7", "3", "2", "1"),
            ("-7", "3", "-2", "-1"),
            ("7", "-3", "-2", "1"),
            ("-7", "-3", "2", "-1"),
        ] {
            let actualq = bin(BinaryOp::Divide, &integer(s, a), &integer(s, b)).unwrap();
            let actualr = bin(BinaryOp::Remainder, &integer(s, a), &integer(s, b)).unwrap();
            assert_eq!(actualq, integer(s, q));
            assert_eq!(actualr, integer(s, r));
            assert_eq!(
                bin(
                    BinaryOp::Add,
                    &bin(BinaryOp::Multiply, &actualq, &integer(s, b)).unwrap(),
                    &actualr
                )
                .unwrap(),
                integer(s, a)
            );
        }
    }
}
#[test]
fn ieee_ties_subnormals_overflow_and_noncanonical_nan_inputs() {
    for (s, kind, one, tie, next, two, max, inf, minsub, negminsub, negzero, nan, canonical) in [
        (
            Scalar::Float32,
            "float32",
            "3f800000",
            "33800000",
            "3f800001",
            "40000000",
            "7f7fffff",
            "7f800000",
            "00000001",
            "80000001",
            "80000000",
            "7fa12345",
            "7fc00000",
        ),
        (
            Scalar::Float64,
            "float64",
            "3ff0000000000000",
            "3ca0000000000000",
            "3ff0000000000001",
            "4000000000000000",
            "7fefffffffffffff",
            "7ff0000000000000",
            "0000000000000001",
            "8000000000000001",
            "8000000000000000",
            "7ff123456789abcd",
            "7ff8000000000000",
        ),
    ] {
        let f = |bits: &str| v(s, json!({"kind":kind,"bits":bits}));
        assert_eq!(bin(BinaryOp::Add, &f(one), &f(tie)).unwrap(), f(one));
        assert_ne!(bin(BinaryOp::Add, &f(next), &f(tie)).unwrap(), f(next));
        assert_eq!(bin(BinaryOp::Multiply, &f(max), &f(two)).unwrap(), f(inf));
        assert_eq!(
            bin(BinaryOp::Divide, &f(negminsub), &f(two)).unwrap(),
            f(negzero)
        );
        assert_eq!(
            bin(BinaryOp::Multiply, &f(minsub), &f(one)).unwrap(),
            f(minsub)
        );
        assert_eq!(bin(BinaryOp::Add, &f(nan), &f(one)).unwrap(), f(canonical));
        for op in [
            BinaryOp::Less,
            BinaryOp::LessEqual,
            BinaryOp::Greater,
            BinaryOp::GreaterEqual,
            BinaryOp::Equal,
        ] {
            assert_eq!(bin(op, &f(nan), &f(one)).unwrap().payload, json!(false));
        }
        assert_eq!(
            bin(BinaryOp::NotEqual, &f(nan), &f(one)).unwrap().payload,
            json!(true)
        );
    }
}
#[test]
fn decimal_scale_edges_zero_cancellation_and_sign_identity() {
    assert_eq!(
        bin(BinaryOp::Add, &dec("-123", 2), &dec("123", 2)).unwrap(),
        dec("0", 2)
    );
    assert_eq!(
        bin(BinaryOp::Subtract, &dec("1", i32::MIN), &dec("1", i32::MIN)).unwrap(),
        dec("0", i32::MIN)
    );
    assert_eq!(
        bin(BinaryOp::Multiply, &dec("1", i32::MIN), &dec("1", 0)).unwrap(),
        dec("1", i32::MIN)
    );
    assert_eq!(
        bin(BinaryOp::Multiply, &dec("1", i32::MIN), &dec("1", -1))
            .unwrap_err()
            .kind,
        ArithmeticErrorKind::ScaleOverflow
    );
    assert_eq!(
        bin(BinaryOp::Divide, &dec("1", i32::MAX), &dec("1", -1))
            .unwrap_err()
            .kind,
        ArithmeticErrorKind::ScaleOverflow
    );
    assert_eq!(
        bin(BinaryOp::Divide, &dec("1", -2), &dec("2", 0)).unwrap(),
        dec("5", -1)
    );
    for op in [BinaryOp::Divide, BinaryOp::Remainder] {
        assert_eq!(
            bin(op, &dec("1", 0), &dec("0", 0)).unwrap_err().kind,
            ArithmeticErrorKind::DivisionByZero
        );
    }
    assert_eq!(
        bin(BinaryOp::Remainder, &dec("7", 0), &dec("-3", 0)).unwrap(),
        dec("1", 0)
    );
    assert_eq!(
        bin(BinaryOp::Remainder, &dec("-7", 0), &dec("-3", 0)).unwrap(),
        dec("-1", 0)
    );
    assert_eq!(
        bin(BinaryOp::Less, &dec("-100", 2), &dec("-2", 0))
            .unwrap()
            .payload,
        json!(false)
    );
}
#[test]
fn exact_work_and_value_boundaries_recover_and_power_preflight_is_charged() {
    let a = dec("1", 0);
    let b = dec("1", 20);
    let mut measured = NumericBudget::default();
    let expected = rcore_behavior_scalar::binary(BinaryOp::Add, &a, &b, &mut measured).unwrap();
    let used = measured.work_used();
    let bytes = measured.value_bytes_used();
    for cap in [used, used + 1] {
        let mut budget = NumericBudget::new(NumericLimits {
            max_work: cap,
            ..NumericLimits::default()
        });
        assert_eq!(
            rcore_behavior_scalar::binary(BinaryOp::Add, &a, &b, &mut budget).unwrap(),
            expected
        );
    }
    let mut budget = NumericBudget::new(NumericLimits {
        max_work: used - 1,
        ..NumericLimits::default()
    });
    assert_eq!(
        rcore_behavior_scalar::binary(BinaryOp::Add, &a, &b, &mut budget)
            .unwrap_err()
            .kind,
        ArithmeticErrorKind::ResourceLimit
    );
    let mut budget = NumericBudget::new(NumericLimits {
        max_total_value_bytes: bytes,
        ..NumericLimits::default()
    });
    assert_eq!(
        rcore_behavior_scalar::binary(BinaryOp::Add, &a, &b, &mut budget).unwrap(),
        expected
    );
    let mut budget = NumericBudget::new(NumericLimits {
        max_total_value_bytes: bytes - 1,
        ..NumericLimits::default()
    });
    assert_eq!(
        rcore_behavior_scalar::binary(BinaryOp::Add, &a, &b, &mut budget)
            .unwrap_err()
            .kind,
        ArithmeticErrorKind::ResourceLimit
    );
    let mut budget = NumericBudget::new(NumericLimits {
        max_work: 10000,
        ..NumericLimits::default()
    });
    assert_eq!(
        rcore_behavior_scalar::binary(BinaryOp::Add, &dec("1", 0), &dec("1", 1000), &mut budget)
            .unwrap_err()
            .kind,
        ArithmeticErrorKind::ResourceLimit
    );
    assert_eq!(bin(BinaryOp::Add, &a, &b).unwrap(), expected);
}
