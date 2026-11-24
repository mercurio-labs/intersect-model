use intersect_run::analysis::{
    Comparison, Constraint, ErrorCode, Expression, Quantity, Source, Unit, Variable, check, solve,
};

fn quantity(value: f64, unit: Unit) -> Quantity {
    Quantity { value, unit }
}

fn literal(value: f64, unit: Unit) -> Box<Expression> {
    Box::new(Expression::Literal {
        quantity: quantity(value, unit),
    })
}

fn reference(name: &str) -> Box<Expression> {
    Box::new(Expression::Variable { name: name.into() })
}

#[test]
fn checked_affine_units_and_unknowns() {
    let source = Source {
        uri: "memory:equation".into(),
        start: 1,
        end: 5,
    };
    let mut equation = Constraint {
        left: *literal(6.0, Unit::Litre),
        comparison: Comparison::Equal,
        right: Expression::Add {
            left: literal(1000.0, Unit::Millilitre),
            right: Box::new(Expression::Multiply {
                left: reference("flow"),
                right: reference("time"),
            }),
        },
        source: Some(source.clone()),
    };
    let mut variables = [
        (
            "flow".into(),
            Variable {
                value: Some(2.0),
                unit: Unit::LitresPerSecond,
                source: None,
            },
        ),
        (
            "time".into(),
            Variable {
                value: Some(4000.0),
                unit: Unit::Millisecond,
                source: None,
            },
        ),
    ]
    .into();
    assert_eq!(
        solve(&equation, &variables, "flow").unwrap(),
        quantity(1.25, Unit::LitresPerSecond)
    );
    assert!(!check(&equation, &variables, 0.0).unwrap());
    variables.get_mut("flow").unwrap().value = Some(1.25);
    assert!(check(&equation, &variables, 0.0).unwrap());

    equation.left = *literal(6.0, Unit::Second);
    let error = solve(&equation, &variables, "flow").unwrap_err();
    assert_eq!(error.code, ErrorCode::Dimension);
    assert_eq!(error.source, Some(source));
    equation.left = *literal(6.0, Unit::Litre);
    variables.get_mut("time").unwrap().value = Some(0.0);
    assert_eq!(
        solve(&equation, &variables, "flow").unwrap_err().code,
        ErrorCode::Inconsistent
    );
    equation.left = *literal(1.0, Unit::Litre);
    assert_eq!(
        solve(&equation, &variables, "flow").unwrap_err().code,
        ErrorCode::Underdetermined
    );
}
