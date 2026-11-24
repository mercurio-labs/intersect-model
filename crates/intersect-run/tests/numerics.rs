use intersect_run::numerics::{NumericError, bisect, first_crossing_tick, rk4};

#[test]
fn locates_first_qualifying_tick_and_preserves_probe_errors() -> Result<(), &'static str> {
    let end = u64::MAX;
    let crossing = end - 12;
    assert_eq!(
        first_crossing_tick(end - 30, end, |tick| Ok(tick >= crossing))?,
        crossing
    );
    assert_eq!(
        first_crossing_tick(10, 20, |_| Err("probe failed")),
        Err("probe failed")
    );
    Ok(())
}

#[test]
fn scalar_constraint_checks_bracket_residual_and_work() -> Result<(), NumericError> {
    let root = bisect(0., 2., 1e-12, 48, |x| x * x - 2.)?;
    assert!((root - 2_f64.sqrt()).abs() < 1e-12);
    assert_eq!(
        bisect(0., 2., 1e-10, 10, |x| x * x + 2.),
        Err(NumericError::Unbracketed)
    );
    assert_eq!(
        bisect(0., 2., 1e-12, 1, |x| x * x - 2.),
        Err(NumericError::WorkLimit)
    );
    assert_eq!(
        bisect(0., 2., 1e-5, 10, |_| f64::NAN),
        Err(NumericError::NonFinite)
    );
    Ok(())
}

#[test]
fn coupled_ode_matches_harmonic_motion_with_bounded_work() -> Result<(), NumericError> {
    let derivative = |_: f64, state: &[f64], out: &mut [f64]| {
        out[0] = state[1];
        out[1] = -state[0];
    };
    let end = rk4(
        0.,
        std::f64::consts::FRAC_PI_2,
        &[1., 0.],
        0.01,
        200,
        derivative,
    )?;
    assert!(end[0].abs() < 1e-8);
    assert!((end[1] + 1.).abs() < 1e-8);
    assert_eq!(
        rk4(0., 1., &[1., 0.], 0.01, 99, derivative),
        Err(NumericError::WorkLimit)
    );
    assert_eq!(
        rk4(0., 1., &[1., 0.], 1., 1, |_, _, output| output[0] = 0.),
        Err(NumericError::NonFinite)
    );
    Ok(())
}
