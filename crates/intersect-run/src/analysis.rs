//! Typed scalar analysis shared by language adapters. Values use litres and
//! seconds internally. Equations are linear in one explicitly requested unknown.
pub mod simulation;

use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub uri: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalysisError {
    pub code: ErrorCode,
    pub message: String,
    pub source: Option<Source>,
}

impl AnalysisError {
    pub fn new(code: ErrorCode, message: impl Into<String>, source: Option<&Source>) -> Self {
        Self {
            code,
            message: message.into(),
            source: source.cloned(),
        }
    }
}

impl fmt::Display for AnalysisError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AnalysisError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Invalid,
    Unsupported,
    Unbound,
    Dimension,
    Inconsistent,
    Underdetermined,
    Budget,
    Objective,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    #[serde(rename = "1")]
    Scalar,
    #[serde(rename = "L")]
    Litre,
    #[serde(rename = "mL")]
    Millilitre,
    #[serde(rename = "s")]
    Second,
    #[serde(rename = "ms")]
    Millisecond,
    #[serde(rename = "L/s")]
    LitresPerSecond,
    #[serde(rename = "mL/s")]
    MillilitresPerSecond,
}

impl Unit {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Scalar => "1",
            Self::Litre => "L",
            Self::Millilitre => "mL",
            Self::Second => "s",
            Self::Millisecond => "ms",
            Self::LitresPerSecond => "L/s",
            Self::MillilitresPerSecond => "mL/s",
        }
    }

    fn dimension(self) -> Dimension {
        match self {
            Self::Scalar => Dimension(0, 0),
            Self::Litre | Self::Millilitre => Dimension(1, 0),
            Self::Second | Self::Millisecond => Dimension(0, 1),
            Self::LitresPerSecond | Self::MillilitresPerSecond => Dimension(1, -1),
        }
    }

    fn scale(self) -> f64 {
        match self {
            Self::Millilitre | Self::Millisecond | Self::MillilitresPerSecond => 0.001,
            _ => 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Dimension(i8, i8);

impl Dimension {
    fn product(self, other: Self, divide: bool) -> Option<Self> {
        if divide {
            Some(Self(
                self.0.checked_sub(other.0)?,
                self.1.checked_sub(other.1)?,
            ))
        } else {
            Some(Self(
                self.0.checked_add(other.0)?,
                self.1.checked_add(other.1)?,
            ))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quantity {
    pub value: f64,
    pub unit: Unit,
}

impl Quantity {
    pub fn in_unit(self, unit: Unit, source: Option<&Source>) -> Result<f64, AnalysisError> {
        if self.unit.dimension() != unit.dimension() {
            return Err(AnalysisError::new(
                ErrorCode::Dimension,
                format!("expected {}, found {}", unit.symbol(), self.unit.symbol()),
                source,
            ));
        }
        finite(self.value * (self.unit.scale() / unit.scale()), source)
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.value, self.unit.symbol())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variable {
    pub unit: Unit,
    pub value: Option<f64>,
    pub source: Option<Source>,
}

pub type Variables = BTreeMap<String, Variable>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Expression {
    Literal { quantity: Quantity },
    Variable { name: String },
    Add { left: Box<Self>, right: Box<Self> },
    Subtract { left: Box<Self>, right: Box<Self> },
    Multiply { left: Box<Self>, right: Box<Self> },
    Divide { left: Box<Self>, right: Box<Self> },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    Equal,
    LessOrEqual,
    GreaterOrEqual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Constraint {
    pub left: Expression,
    pub comparison: Comparison,
    pub right: Expression,
    pub source: Option<Source>,
}

pub fn evaluate(
    expression: &Expression,
    variables: &Variables,
    unit: Unit,
    source: Option<&Source>,
) -> Result<Quantity, AnalysisError> {
    let context = Context {
        variables,
        unknown: None,
        source,
    };
    let value = context.expression(expression, 0)?;
    if value.dimension != unit.dimension() {
        return Err(context.error(ErrorCode::Dimension, "expression and result units differ"));
    }
    Ok(Quantity {
        value: finite(value.constant / unit.scale(), source)?,
        unit,
    })
}

/// The tolerance is absolute in the canonical units of the constraint.
pub fn check(
    constraint: &Constraint,
    variables: &Variables,
    tolerance: f64,
) -> Result<bool, AnalysisError> {
    let context = Context {
        variables,
        unknown: None,
        source: constraint.source.as_ref(),
    };
    if !tolerance.is_finite() || tolerance < 0.0 {
        return Err(context.error(
            ErrorCode::Invalid,
            "tolerance must be finite and non-negative",
        ));
    }
    let residual = context.residual(constraint)?.constant;
    Ok(match constraint.comparison {
        Comparison::Equal => residual.abs() <= tolerance,
        Comparison::LessOrEqual => residual <= tolerance,
        Comparison::GreaterOrEqual => residual >= -tolerance,
    })
}

pub fn solve(
    equation: &Constraint,
    variables: &Variables,
    unknown: &str,
) -> Result<Quantity, AnalysisError> {
    let source = equation.source.as_ref();
    let context = Context {
        variables,
        unknown: Some(unknown),
        source,
    };
    if !matches!(equation.comparison, Comparison::Equal) {
        return Err(context.error(ErrorCode::Unsupported, "solving requires an equality"));
    }
    let unit = variables
        .get(unknown)
        .ok_or_else(|| context.error(ErrorCode::Unbound, format!("undeclared unknown {unknown}")))?
        .unit;
    let residual = context.residual(equation)?;
    if residual.coefficient == 0.0 {
        return Err(if residual.constant == 0.0 {
            context.error(
                ErrorCode::Underdetermined,
                "the equation does not determine the unknown",
            )
        } else {
            context.error(ErrorCode::Inconsistent, "the equation cannot be satisfied")
        });
    }
    let canonical = finite(-residual.constant / residual.coefficient, source)?;
    Ok(Quantity {
        value: finite(canonical / unit.scale(), source)?,
        unit,
    })
}

struct Context<'a> {
    variables: &'a Variables,
    unknown: Option<&'a str>,
    source: Option<&'a Source>,
}

#[derive(Clone, Copy)]
struct Affine {
    dimension: Dimension,
    constant: f64,
    coefficient: f64,
}

impl Context<'_> {
    fn error(&self, code: ErrorCode, message: impl Into<String>) -> AnalysisError {
        AnalysisError::new(code, message, self.source)
    }

    fn residual(&self, constraint: &Constraint) -> Result<Affine, AnalysisError> {
        self.add(
            self.expression(&constraint.left, 0)?,
            self.expression(&constraint.right, 0)?,
            -1.0,
        )
    }

    fn expression(&self, expr: &Expression, depth: usize) -> Result<Affine, AnalysisError> {
        if depth >= 64 {
            return Err(self.error(ErrorCode::Budget, "expression depth limit exceeded"));
        }
        let child = |expr: &Expression| self.expression(expr, depth + 1);
        let value = match expr {
            Expression::Literal { quantity } => Affine {
                dimension: quantity.unit.dimension(),
                constant: quantity.value * quantity.unit.scale(),
                coefficient: 0.0,
            },
            Expression::Variable { name } => {
                let variable = self.variables.get(name).ok_or_else(|| {
                    self.error(ErrorCode::Unbound, format!("undeclared variable {name}"))
                })?;
                let unknown = self.unknown == Some(name);
                let constant = if unknown {
                    0.0
                } else {
                    let source = variable.source.as_ref().or(self.source);
                    let bound = variable.value.ok_or_else(|| {
                        AnalysisError::new(
                            ErrorCode::Unbound,
                            format!("no value for {name}"),
                            source,
                        )
                    })?;
                    finite(bound * variable.unit.scale(), source)?
                };
                Affine {
                    dimension: variable.unit.dimension(),
                    constant,
                    coefficient: f64::from(unknown),
                }
            }
            Expression::Add { left, right } => self.add(child(left)?, child(right)?, 1.0)?,
            Expression::Subtract { left, right } => self.add(child(left)?, child(right)?, -1.0)?,
            Expression::Multiply { left, right } => {
                self.multiply(child(left)?, child(right)?, false)?
            }
            Expression::Divide { left, right } => {
                self.multiply(child(left)?, child(right)?, true)?
            }
        };
        finite(value.constant, self.source)?;
        finite(value.coefficient, self.source)?;
        Ok(value)
    }

    fn add(&self, a: Affine, b: Affine, sign: f64) -> Result<Affine, AnalysisError> {
        if a.dimension != b.dimension {
            return Err(self.error(
                ErrorCode::Dimension,
                "operands have incompatible dimensions",
            ));
        }
        Ok(Affine {
            dimension: a.dimension,
            constant: finite(a.constant + sign * b.constant, self.source)?,
            coefficient: finite(a.coefficient + sign * b.coefficient, self.source)?,
        })
    }

    fn multiply(&self, a: Affine, b: Affine, divide: bool) -> Result<Affine, AnalysisError> {
        if b.coefficient != 0.0 && (divide || a.coefficient != 0.0) {
            return Err(self.error(
                ErrorCode::Unsupported,
                "expression is nonlinear in the unknown",
            ));
        }
        if divide && b.constant == 0.0 {
            return Err(self.error(ErrorCode::Invalid, "division by zero"));
        }
        let dimension = a
            .dimension
            .product(b.dimension, divide)
            .ok_or_else(|| self.error(ErrorCode::Budget, "dimension exponent limit exceeded"))?;
        Ok(if divide {
            Affine {
                dimension,
                constant: a.constant / b.constant,
                coefficient: a.coefficient / b.constant,
            }
        } else {
            Affine {
                dimension,
                constant: a.constant * b.constant,
                coefficient: a.coefficient * b.constant + b.coefficient * a.constant,
            }
        })
    }
}

fn finite(value: f64, source: Option<&Source>) -> Result<f64, AnalysisError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(AnalysisError::new(
            ErrorCode::Invalid,
            "non-finite numeric result",
            source,
        ))
    }
}
