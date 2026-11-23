use intersect_run::{
    TraceKind,
    analysis::{
        Comparison, Constraint, ErrorCode, Expression, Quantity, Source, Unit, Variable, check,
        simulation::{Simulation, simulate},
        solve,
    },
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

#[test]
fn continuous_crossing_precedes_coincident_sample_and_fires_once() {
    let mut request = Simulation {
        initial: quantity(1.0, Unit::Litre),
        rate: quantity(2000.0, Unit::MillilitresPerSecond),
        threshold: quantity(6.0, Unit::Litre),
        next_rate: quantity(0.0, Unit::LitresPerSecond),
        end: quantity(5.0, Unit::Second),
        interval: quantity(1.0, Unit::Second),
        source: None,
    };
    let result = simulate(&request).unwrap();
    assert_eq!(result.crossing, Some(quantity(2.5, Unit::Second)));
    let volumes: Vec<_> = result
        .samples
        .iter()
        .map(|sample| sample.volume.value)
        .collect();
    assert_eq!(volumes, [1.0, 3.0, 5.0, 6.0, 6.0, 6.0]);
    let crossings: Vec<_> = result
        .trace
        .entries
        .iter()
        .filter(|entry| entry.kind == TraceKind::EventDelivered && entry.label == "Crossing")
        .collect();
    assert_eq!(crossings.len(), 1);
    assert_eq!(crossings[0].time, 2_500_000_000);

    request.interval = quantity(2500.0, Unit::Millisecond);
    let result = simulate(&request).unwrap();
    let delivered: Vec<_> = result
        .trace
        .entries
        .iter()
        .filter(|entry| entry.kind == TraceKind::EventDelivered && entry.time == 2_500_000_000)
        .map(|entry| entry.label.as_str())
        .collect();
    assert_eq!(delivered, ["Crossing", "Sample"]);
    assert_eq!(result.samples[1].volume, quantity(6.0, Unit::Litre));
    request.rate = quantity(0.0, Unit::LitresPerSecond);
    assert_eq!(simulate(&request).unwrap().crossing, None);
}
