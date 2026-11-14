use intersect_model::definitions::{
    bundle::{DefinitionSource, SourceKind},
    rcore_behavior::{
        ast::Literal,
        types::{Scalar, Type, TypeRef, TypeTable},
    },
    resources::DeclarationId,
};

fn number(ty: Scalar, value: &str) -> bool {
    ty.check_literal(&Literal::Number(value.into())).is_ok()
}

#[test]
fn every_fixed_integer_boundary_is_exact_including_beyond_json_precision() {
    use Scalar::*;
    for (ty, min, max, below, above) in [
        (Int8, "-128", "127", "-129", "128"),
        (Int16, "-32768", "32767", "-32769", "32768"),
        (
            Int32,
            "-2147483648",
            "2147483647",
            "-2147483649",
            "2147483648",
        ),
        (
            Int64,
            "-9223372036854775808",
            "9223372036854775807",
            "-9223372036854775809",
            "9223372036854775808",
        ),
        (UInt8, "0", "255", "-1", "256"),
        (UInt16, "0", "65535", "-1", "65536"),
        (UInt32, "0", "4294967295", "-1", "4294967296"),
        (
            UInt64,
            "0",
            "18446744073709551615",
            "-1",
            "18446744073709551616",
        ),
    ] {
        assert!(number(ty, min), "{ty:?} {min}");
        assert!(number(ty, max), "{ty:?} {max}");
        assert!(!number(ty, below), "{ty:?} {below}");
        assert!(!number(ty, above), "{ty:?} {above}");
        assert!(!number(ty, "0.5"));
        assert!(number(ty, "-0"));
    }
    for value in -300..=300 {
        assert_eq!(
            number(Int8, &value.to_string()),
            (-128..=127).contains(&value)
        );
        assert_eq!(
            number(UInt8, &value.to_string()),
            (0..=255).contains(&value)
        );
    }
    assert_eq!(Scalar::named("Int"), Scalar::named("Int64"));
    assert_eq!(Scalar::named("Integer"), None);
}

#[test]
fn normalized_decimal_spelling_and_huge_exponents_do_not_round_or_expand() {
    for text in [
        "127.000",
        "1.27e2",
        "12700e-2",
        "000127",
        "0e-99999999999999999999999",
    ] {
        assert!(number(Scalar::Int8, text), "{text}");
    }
    for text in [
        "127.00000000000000000000000000000001",
        "1.28e2",
        "127e-1",
        "1e99999999999999999999999",
        "1e-99999999999999999999999",
    ] {
        assert!(!number(Scalar::Int8, text), "{text}");
    }
    assert!(number(Scalar::Int64, "9223372036854775807.0000"));
    assert!(!number(Scalar::Int64, "9223372036854775807.0001"));
    assert!(number(
        Scalar::BigInt,
        "18446744073709551616000000000000000000001"
    ));
    assert!(number(
        Scalar::BigInt,
        "1e99999999999999999999999999999999999999"
    ));
    assert!(!number(
        Scalar::BigInt,
        "1e-99999999999999999999999999999999999999"
    ));
    assert!(number(
        Scalar::Decimal,
        "1.00000000000000000000000000000000001e-99999999999999999999999999999999999999"
    ));
    let zeros = "0".repeat(20_000);
    assert!(number(Scalar::Int8, &format!("127{zeros}e-20000")));
    assert!(!number(Scalar::Int8, &format!("128{zeros}e-20000")));
}

#[test]
fn float_literals_have_explicit_overflow_underflow_and_signed_zero_rules() {
    assert!(number(Scalar::Float32, "3.4028234663852886e38"));
    assert!(!number(Scalar::Float32, "3.5e38"));
    assert!(number(Scalar::Float32, "1.401298464324817e-45"));
    assert!(!number(Scalar::Float32, "1e-46"));
    assert!(number(Scalar::Float64, "1.7976931348623157e308"));
    assert!(!number(Scalar::Float64, "1.8e308"));
    assert!(number(Scalar::Float64, "4.9406564584124654e-324"));
    assert!(!number(Scalar::Float64, "1e-325"));
    for ty in [Scalar::Float32, Scalar::Float64] {
        for text in ["0", "-0", "-0.000e-999999999999999999", "0.1"] {
            assert!(number(ty, text), "{ty:?} {text}");
        }
        assert!(!number(ty, "1e999999999999999999999999"));
        assert!(!number(ty, "1e-999999999999999999999999"));
    }
}

#[test]
fn malformed_and_nonnumeric_literals_never_become_scalar_values() {
    for text in [
        "", "-", "+1", ".1", "1.", "1e", "1e+", "1e-", "NaN", "inf", "0x10", "1_000", "1 2",
        "1.2.3", "1e1e1", "١", "1\n",
    ] {
        for ty in [
            Scalar::Int64,
            Scalar::Decimal,
            Scalar::BigInt,
            Scalar::Float64,
        ] {
            assert!(!number(ty, text), "{ty:?}: {text:?}");
        }
    }
    assert!(Scalar::Bool.check_literal(&Literal::Boolean(true)).is_ok());
    assert!(
        Scalar::String
            .check_literal(&Literal::String("🙂".into()))
            .is_ok()
    );
    for ty in [Scalar::Bool, Scalar::String, Scalar::Bytes, Scalar::Unit] {
        assert!(!number(ty, "1"));
        assert!(ty.check_literal(&Literal::None).is_err());
    }
    assert!(
        Scalar::Bytes
            .check_literal(&Literal::String("bytes".into()))
            .is_err()
    );
    assert!(
        Scalar::Int64
            .check_literal(&Literal::Boolean(false))
            .is_err()
    );
}

#[test]
fn flat_type_identity_distinguishes_revision_cardinality_and_absence_domains() {
    let mut table = TypeTable::default();
    let int = table.intern(Type::Scalar(Scalar::Int32)).unwrap();
    assert_eq!(int, table.intern(Type::Scalar(Scalar::Int32)).unwrap());
    let optional = table.intern(Type::Option(int)).unwrap();
    let result = table
        .intern(Type::Result {
            ok: int,
            error: int,
        })
        .unwrap();
    assert_ne!(optional, result);
    let source = DefinitionSource::new("memory:model", SourceKind::Ecore, "first");
    let changed = DefinitionSource::new("memory:model", SourceKind::Ecore, "second");
    let model = table
        .intern(Type::Model(DeclarationId {
            source: source.id,
            path: vec![0],
        }))
        .unwrap();
    let other = table
        .intern(Type::Model(DeclarationId {
            source: changed.id,
            path: vec![0],
        }))
        .unwrap();
    assert_ne!(model, other);
    let many = table
        .intern(Type::Many {
            element: model,
            lower: 0,
            upper: -1,
            ordered: true,
            unique: true,
        })
        .unwrap();
    let unordered = table
        .intern(Type::Many {
            element: model,
            lower: 0,
            upper: -1,
            ordered: false,
            unique: true,
        })
        .unwrap();
    let bounded = table
        .intern(Type::Many {
            element: model,
            lower: 1,
            upper: 2,
            ordered: true,
            unique: true,
        })
        .unwrap();
    assert_ne!(many, unordered);
    assert_ne!(many, bounded);
    assert_ne!(model, many);
    let function = table
        .intern(Type::Function {
            parameters: vec![optional, model],
            result,
        })
        .unwrap();
    assert!(matches!(
        table.get(function).unwrap(),
        Type::Function { .. }
    ));
    let encoded = serde_json::to_string(&table).unwrap();
    assert!(encoded.contains("Option"));
    assert!(!encoded.contains("depths"));
}

#[test]
fn type_construction_checks_children_bounds_and_nesting_before_insertion() {
    let mut table = TypeTable::default();
    assert!(table.intern(Type::Option(TypeRef(0))).is_err());
    assert!(table.get(TypeRef(0)).is_err());
    let int = table.intern(Type::Scalar(Scalar::Int64)).unwrap();
    for (lower, upper) in [(0, -2), (0, 0), (0, 1), (4, 3)] {
        assert!(
            table
                .intern(Type::Many {
                    element: int,
                    lower,
                    upper,
                    ordered: true,
                    unique: false
                })
                .is_err()
        );
    }
    let mut nested = int;
    for _ in 1..64 {
        nested = table.intern(Type::Option(nested)).unwrap();
    }
    assert!(
        table
            .intern(Type::Option(nested))
            .unwrap_err()
            .contains("64")
    );
    // A failed insertion neither consumes an ID nor leaves a partial type.
    let string = table.intern(Type::Scalar(Scalar::String)).unwrap();
    assert_eq!(string.0, 64);
    assert!(serde_json::to_string(&table).is_ok());
}
