//! Bounded numerical routines for use by a language's Run semantics.
use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericError {
    InvalidControls,
    NonFinite,
    Unbracketed,
    WorkLimit,
    NoProgress,
}

impl fmt::Display for NumericError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::InvalidControls => "Invalid numerical solver controls",
            Self::NonFinite => "The numerical solver received or produced a non-finite value",
            Self::Unbracketed => "The residual has the same sign at both bracket endpoints",
            Self::WorkLimit => "The numerical solver reached its work limit",
            Self::NoProgress => "The numerical solver cannot advance at this precision",
        })
    }
}

impl Error for NumericError {}

fn finite(values: &[f64]) -> Result<(), NumericError> {
    if values.iter().all(|value| value.is_finite()) {
        Ok(())
    } else {
        Err(NumericError::NonFinite)
    }
}

/// Finds the first qualifying tick in an established crossing bracket.
/// The predicate is false at `at`, true at `until`, and stays true after
/// the crossing within that bracket. Endpoints are checked by the caller.
pub fn first_crossing_tick<E>(
    mut at: u64,
    mut until: u64,
    mut crossed_at: impl FnMut(u64) -> Result<bool, E>,
) -> Result<u64, E> {
    while until.saturating_sub(at) > 1 {
        let midpoint = at + (until - at) / 2;
        if crossed_at(midpoint)? {
            until = midpoint;
        } else {
            at = midpoint;
        }
    }
    Ok(until)
}

/// Finds a zero of a continuous scalar residual in a caller-supplied bracket.
/// Success means the absolute residual is within tolerance. Each iteration
/// evaluates the residual once, after evaluating the two endpoints.
pub fn bisect(
    mut left: f64,
    mut right: f64,
    tolerance: f64,
    max_iterations: u32,
    mut residual: impl FnMut(f64) -> f64,
) -> Result<f64, NumericError> {
    finite(&[left, right, tolerance])?;
    if left >= right || tolerance <= 0.0 || max_iterations == 0 {
        return Err(NumericError::InvalidControls);
    }
    let mut at_left = residual(left);
    let at_right = residual(right);
    finite(&[at_left, at_right])?;
    if at_left.abs() <= tolerance {
        return Ok(left);
    }
    if at_right.abs() <= tolerance {
        return Ok(right);
    }
    if at_left.is_sign_negative() == at_right.is_sign_negative() {
        return Err(NumericError::Unbracketed);
    }
    for _ in 0..max_iterations {
        let middle = left * 0.5 + right * 0.5;
        if middle <= left || middle >= right {
            return Err(NumericError::NoProgress);
        }
        let value = residual(middle);
        finite(&[value])?;
        if value.abs() <= tolerance {
            return Ok(middle);
        }
        if value.is_sign_negative() == at_left.is_sign_negative() {
            left = middle;
            at_left = value;
        } else {
            right = middle;
        }
    }
    Err(NumericError::WorkLimit)
}

/// Integrates an initial-value problem with classical fourth-order Runge–Kutta.
/// The profile provides a step size appropriate to its dynamics. This routine
/// divides the interval into equal substeps no larger than `max_step`. It
/// refuses a request beyond `max_steps` before calling the derivative.
pub fn rk4(
    start: f64,
    end: f64,
    initial: &[f64],
    max_step: f64,
    max_steps: u32,
    mut derivative: impl FnMut(f64, &[f64], &mut [f64]),
) -> Result<Vec<f64>, NumericError> {
    finite(&[start, end, max_step])?;
    finite(initial)?;
    if start > end || initial.is_empty() || max_step <= 0.0 || max_steps == 0 {
        return Err(NumericError::InvalidControls);
    }
    if start == end {
        return Ok(initial.to_vec());
    }
    let span = end - start;
    finite(&[span])?;
    let count = (span / max_step).ceil().max(1.0);
    if count > f64::from(max_steps) {
        return Err(NumericError::WorkLimit);
    }
    let step = span / count;
    if step == 0.0 || start + step == start {
        return Err(NumericError::NoProgress);
    }
    let half = step * 0.5;
    let mut value = initial.to_vec();
    let mut stage = vec![0.0; value.len()];
    let mut slopes: [Vec<f64>; 4] = std::array::from_fn(|_| vec![0.0; value.len()]);
    let mut evaluate = |time: f64, input: &[f64], output: &mut [f64]| {
        finite(&[time])?;
        finite(input)?;
        output.fill(f64::NAN);
        derivative(time, input, output);
        finite(output)
    };
    for index in 0..count as u32 {
        let time = start + f64::from(index) * step;
        let next = if index + 1 == count as u32 {
            end
        } else {
            time + step
        };
        if next <= time {
            return Err(NumericError::NoProgress);
        }
        evaluate(time, &value, &mut slopes[0])?;
        for (previous, factor, sample_time) in [
            (0, half, time + half),
            (1, half, time + half),
            (2, step, next),
        ] {
            for (slot, (current, slope)) in
                stage.iter_mut().zip(value.iter().zip(&slopes[previous]))
            {
                *slot = current + factor * slope;
            }
            evaluate(sample_time, &stage, &mut slopes[previous + 1])?;
        }
        for (index, value) in value.iter_mut().enumerate() {
            *value += (step / 6.0)
                * (slopes[0][index]
                    + 2.0 * slopes[1][index]
                    + 2.0 * slopes[2][index]
                    + slopes[3][index]);
        }
        finite(&value)?;
    }
    Ok(value)
}
