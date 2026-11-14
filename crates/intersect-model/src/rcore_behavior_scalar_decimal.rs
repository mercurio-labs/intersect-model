use super::*;
use num_integer::Integer;
use num_traits::Signed;

fn decode(v: &Value, budget: &mut NumericOperation<'_>) -> Result<(BigInt, i32)> {
    let coefficient = v
        .payload
        .get("coefficient")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            error(
                ArithmeticErrorKind::InvalidValue,
                "Missing decimal coefficient",
            )
        })?;
    let scale = v
        .payload
        .get("scale")
        .and_then(|v| v.as_i64())
        .and_then(|s| i32::try_from(s).ok())
        .ok_or_else(|| error(ArithmeticErrorKind::InvalidValue, "Invalid decimal scale"))?;
    Ok((integer_text(coefficient, budget)?, scale))
}
fn finish(coefficient: BigInt, scale: i32, budget: &mut NumericOperation<'_>) -> Result<Value> {
    if digits(&coefficient) > budget.limits.max_digits {
        return Err(resource("Numeric result digit limit exceeded"));
    }
    budget.finish(value(
        Scalar::Decimal,
        json!({"kind":"decimal","coefficient":coefficient.to_str_radix(10),"scale":scale}),
    ))
}
fn scale(n: i64) -> Result<i32> {
    i32::try_from(n)
        .map_err(|_| error(ArithmeticErrorKind::ScaleOverflow, "Decimal scale overflow"))
}
fn pad(n: BigInt, count: i64, budget: &mut NumericOperation<'_>) -> Result<BigInt> {
    let count =
        usize::try_from(count).map_err(|_| resource("Decimal scale alignment limit exceeded"))?;
    if count > budget.limits.max_scale_alignment {
        return Err(resource("Decimal scale alignment limit exceeded"));
    }
    budget.intermediate(sum_size(digits(&n), count)?)?;
    let projected = sum_size(digits(&n), count)?;
    budget.work(product_work(projected, projected)?)?;
    let exponent =
        u32::try_from(count).map_err(|_| resource("Decimal scale alignment limit exceeded"))?;
    Ok(n * BigInt::from(10u8).pow(exponent))
}
// Compare virtual zero-padded magnitudes, without allocating powers of ten.
fn ordering(
    a: &BigInt,
    sa: i32,
    b: &BigInt,
    sb: i32,
    budget: &mut NumericOperation<'_>,
) -> Result<Ordering> {
    if a.sign() != b.sign() {
        return Ok(a.sign().cmp(&b.sign()));
    }
    if a.is_zero() {
        return Ok(Ordering::Equal);
    }
    let at = a.abs().to_str_radix(10);
    let bt = b.abs().to_str_radix(10);
    budget.work(sum_size(at.len(), bt.len())?)?;
    let ae = i64::try_from(at.len()).map_err(|_| resource("Numeric size estimate overflow"))?
        - i64::from(sa);
    let be = i64::try_from(bt.len()).map_err(|_| resource("Numeric size estimate overflow"))?
        - i64::from(sb);
    let mut result = ae.cmp(&be);
    if result == Ordering::Equal {
        for i in 0..at.len().max(bt.len()) {
            budget.checkpoint()?;
            let ac = at.as_bytes().get(i).copied().unwrap_or(b'0');
            let bc = bt.as_bytes().get(i).copied().unwrap_or(b'0');
            result = ac.cmp(&bc);
            if result != Ordering::Equal {
                break;
            }
        }
    }
    Ok(if a.is_negative() {
        result.reverse()
    } else {
        result
    })
}
pub(super) fn unary(
    op: UnaryOp,
    input: &Value,
    budget: &mut NumericOperation<'_>,
) -> Result<Value> {
    let (n, s) = decode(input, budget)?;
    finish(if op == UnaryOp::Negate { -n } else { n }, s, budget)
}
pub(super) fn binary(
    op: BinaryOp,
    left: &Value,
    right: &Value,
    budget: &mut NumericOperation<'_>,
) -> Result<Value> {
    let (a, sa) = decode(left, budget)?;
    let (b, sb) = decode(right, budget)?;
    if comparison(op) {
        let result = compare(op, ordering(&a, sa, &b, sb, budget)?)?;
        return budget.finish(boolean(result));
    }
    if matches!(op, BinaryOp::Divide | BinaryOp::Remainder) && b.is_zero() {
        return Err(error(
            ArithmeticErrorKind::DivisionByZero,
            "Decimal division or remainder by zero",
        ));
    }
    match op {
        BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Remainder => {
            let s = sa.max(sb);
            let a = pad(a, i64::from(s) - i64::from(sa), budget)?;
            let b = pad(b, i64::from(s) - i64::from(sb), budget)?;
            budget.intermediate(sum_size(digits(&a).max(digits(&b)), 1)?)?;
            budget.work(if op == BinaryOp::Remainder {
                product_work(digits(&a), digits(&b))?
            } else {
                digits(&a).max(digits(&b))
            })?;
            finish(
                match op {
                    BinaryOp::Add => a + b,
                    BinaryOp::Subtract => a - b,
                    _ => a % b,
                },
                s,
                budget,
            )
        }
        BinaryOp::Multiply => {
            let s = scale(i64::from(sa) + i64::from(sb))?;
            budget.intermediate(sum_size(digits(&a), digits(&b))?)?;
            budget.work(product_work(digits(&a), digits(&b))?)?;
            finish(a * b, s, budget)
        }
        BinaryOp::Divide => {
            let preferred = i64::from(sa) - i64::from(sb);
            if a.is_zero() {
                return finish(a, scale(preferred)?, budget);
            }
            let ad = digits(&a);
            let bd = digits(&b);
            // Conservative Euclidean work bound is charged before backend execution.
            budget.work(product_work(product_work(ad, bd)?, sum_size(ad, bd)?)?)?;
            let gcd = a.gcd(&b);
            let mut n = a / &gcd;
            let mut d = b / gcd;
            if d.is_negative() {
                d = -d;
                n = -n
            }
            let mut powers = [0usize; 2];
            for (slot, factor) in [2u8, 5u8].into_iter().enumerate() {
                let divisor = BigInt::from(factor);
                loop {
                    budget.work(digits(&d))?;
                    if !(&d % &divisor).is_zero() {
                        break;
                    }
                    powers[slot] = powers[slot]
                        .checked_add(1)
                        .ok_or_else(|| resource("Decimal division expansion limit exceeded"))?;
                    if powers[slot] > budget.limits.max_scale_alignment {
                        return Err(resource("Decimal division expansion limit exceeded"));
                    }
                    d /= &divisor;
                }
            }
            if d != BigInt::from(1u8) {
                return Err(error(
                    ArithmeticErrorKind::NonTerminatingDecimal,
                    "Decimal division has no exact terminating result",
                ));
            }
            let k = powers[0].max(powers[1]);
            budget.intermediate(sum_size(digits(&n), k)?)?;
            let projected = sum_size(digits(&n), k)?;
            budget.work(product_work(projected, projected)?)?;
            for (slot, factor) in [2u8, 5u8].into_iter().enumerate() {
                budget.checkpoint()?;
                let exponent = u32::try_from(k - powers[slot])
                    .map_err(|_| resource("Decimal division expansion limit exceeded"))?;
                n *= BigInt::from(factor).pow(exponent);
            }
            finish(
                n,
                scale(
                    preferred
                        + i64::try_from(k)
                            .map_err(|_| resource("Decimal division expansion limit exceeded"))?,
                )?,
                budget,
            )
        }
        _ => Err(error(
            ArithmeticErrorKind::TypeMismatch,
            "Unsupported decimal operator",
        )),
    }
}
