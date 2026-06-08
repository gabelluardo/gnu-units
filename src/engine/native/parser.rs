//! Pure-Rust expression parser for GNU units expressions, backed by [`pest`].
//!
//! The PEG grammar lives in `parser.pest` (same directory).
//!
//! Operator precedence (lowest → highest):
//! ```text
//! additive  ::= product (('+' | '-' | '+-') product)*
//! product   ::= power (('*' | '/' | 'per' | <juxtaposition>) power)*
//!               ('|' requires both operands to be dimensionless)
//! power     ::= unary ('^' | '**') unary
//! unary     ::= ('-' | 'per') unary | atom
//! atom      ::= NUMBER | '~' NAME '(' expr ')' | NAME '(' expr ')' | NAME | '(' expr ')'
//! ```

use std::collections::{HashMap, HashSet};

use pest::{Parser, iterators::Pair};
use pest_derive::Parser;

use super::database::{Database, FunctionDef, UnitEntry, read as db_read};
use super::types::{UnitValue, float2rat};

#[derive(Parser)]
#[grammar = "engine/native/parser.pest"]
struct UnitParser;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ParseError {
    #[error("unknown unit: {0}")]
    UnknownUnit(String),

    #[error("{func}: argument {arg} not in domain")]
    NotInDomain { func: String, arg: usize },

    #[error("{func}: argument {arg} has wrong dimensions")]
    WrongDimensions { func: String, arg: usize },

    #[error("irrational exponent for dimensional unit")]
    IrrationalExponent,

    #[error("incompatible dimensions in {0}")]
    IncompatibleDimensions(&'static str),

    #[error("{0}: argument must be dimensionless")]
    DimensionlessRequired(String),

    #[error("{func}: {reason}")]
    Undefined {
        func: &'static str,
        reason: &'static str,
    },

    #[error("{func}: {constraint}")]
    OutOfRange {
        func: &'static str,
        constraint: &'static str,
    },

    #[error("circular definition: {0}")]
    CircularDefinition(String),

    #[error("expression too deeply nested")]
    TooDeep,

    #[error("max recursion depth while resolving: {0}")]
    MaxRecursion(String),

    #[error("invalid number: {0}")]
    InvalidNumber(String),

    #[error("empty expression")]
    EmptyExpression,

    #[error("{func}: expected {expected} argument(s), got {got}")]
    ArityMismatch {
        func: String,
        expected: usize,
        got: usize,
    },

    #[error("function has no inverse: {0}")]
    NoInverse(String),

    #[error("{func}: inverse not supported for zero-arg or multivariate functions")]
    InverseNotSupported { func: String },

    #[error("base unit not divisible by root")]
    NotDivisibleByRoot,

    #[error("exponent must be dimensionless")]
    ExponentNotDimensionless,

    #[error("{0}")]
    Grammar(String),
}

/// Maps a pest parse error to [`ParseError`], translating EOI failures into
/// the "unexpected trailing input" message expected by callers and tests.
fn map_pest_error(e: pest::error::Error<Rule>) -> ParseError {
    if let pest::error::ErrorVariant::ParsingError { ref positives, .. } = e.variant
        && positives.contains(&Rule::EOI)
    {
        return ParseError::Grammar("unexpected trailing input".to_owned());
    }
    ParseError::Grammar(e.to_string())
}

/// Parses `input` as a `Rule::input` and returns the inner `Rule::expr` pair.
fn parse_to_expr_pair(input: &str) -> Result<Pair<'_, Rule>, ParseError> {
    let mut pairs = UnitParser::parse(Rule::input, input).map_err(map_pest_error)?;
    let Some(input_pair) = pairs.next() else {
        return Err(ParseError::Grammar("expected input pair".to_owned()));
    };
    let Some(expr_pair) = input_pair.into_inner().find(|p| p.as_rule() == Rule::expr) else {
        return Err(ParseError::Grammar("expected expression".to_owned()));
    };
    Ok(expr_pair)
}

const MAX_DEPTH: usize = 256;

struct Evaluator<'db> {
    db: &'db Database,
    /// Variable bindings used when evaluating function bodies.
    vars: HashMap<String, UnitValue>,
    /// Unit names currently being resolved (cycle detection).
    resolving: HashSet<String>,
    depth: usize,
}

/// Applies `base ^ exp`, replicating `unitpower` from the GNU units C source.
///
/// - Exponent must be dimensionless; otherwise an error is returned.
/// - Dimensionless base: `factor = factor.powf(exp.factor)`.
/// - Dimensional base: exponent is approximated as `p/q` via continued fractions
///   (`float2rat`).  If not rational, an error is returned.  If `q != 1`,
///   `root_assign(q)` is applied first.  Then `pow_assign(|p|)`, and if `p < 0`
///   the result is inverted.
fn apply_power(mut base: UnitValue, exp: UnitValue) -> Result<UnitValue, ParseError> {
    if !exp.is_dimensionless() {
        return Err(ParseError::ExponentNotDimensionless);
    }
    if base.is_dimensionless() {
        base.factor = base.factor.powf(exp.factor);
        return Ok(base);
    }
    let Some((p, q)) = float2rat(exp.factor) else {
        return Err(ParseError::IrrationalExponent);
    };
    if q != 1 && !base.root_assign(q) {
        return Err(ParseError::NotDivisibleByRoot);
    }
    base.pow_assign(p.unsigned_abs() as i32);
    if p < 0 {
        base.invert();
    }
    Ok(base)
}

/// Checks that `arg` is dimensionless; returns an error mentioning `name` otherwise.
fn require_dimensionless(name: &str, arg: &UnitValue) -> Result<(), ParseError> {
    if !arg.is_dimensionless() {
        return Err(ParseError::DimensionlessRequired(name.to_owned()));
    }
    Ok(())
}

/// Returns `true` iff `a` and `b` have the same non-zero dimension exponents.
fn are_conformable(a: &UnitValue, b: &UnitValue) -> bool {
    let a_dims: std::collections::HashMap<&String, i32> = a
        .dimensions
        .iter()
        .filter(|(_, v)| **v != 0)
        .map(|(k, v)| (k, *v))
        .collect();
    let b_dims: std::collections::HashMap<&String, i32> = b
        .dimensions
        .iter()
        .filter(|(_, v)| **v != 0)
        .map(|(k, v)| (k, *v))
        .collect();
    a_dims == b_dims
}

/// Error function approximation — Abramowitz & Stegun §7.1.26, max |ε| ≤ 1.5×10⁻⁷.
fn erf_approx(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let poly = t
        * (0.254_829_592
            + t * (-0.284_496_736
                + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    sign * (1.0 - poly * (-x * x).exp())
}

/// Lanczos approximation of Γ(x) with g = 7 (≈15 significant digits).
/// Returns ±∞ at poles (non-positive integers).
fn gamma_lanczos(x: f64) -> f64 {
    const G: f64 = 7.0;
    const P: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_9,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        let denom = (std::f64::consts::PI * x).sin() * gamma_lanczos(1.0 - x);
        if denom == 0.0 {
            return f64::INFINITY;
        }
        return std::f64::consts::PI / denom;
    }
    let z = x - 1.0;
    let mut a = P[0];
    for (i, &pi) in P[1..].iter().enumerate() {
        a += pi / (z + (i as f64) + 1.0);
    }
    let t = z + G + 0.5;
    (2.0 * std::f64::consts::PI).sqrt() * t.powf(z + 0.5) * (-t).exp() * a
}

impl<'db> Evaluator<'db> {
    fn new(db: &'db Database) -> Self {
        Self {
            db,
            vars: HashMap::new(),
            resolving: HashSet::new(),
            depth: 0,
        }
    }

    fn eval_expr(&mut self, pair: Pair<Rule>) -> Result<UnitValue, ParseError> {
        let Some(inner) = pair.into_inner().next() else {
            return Err(ParseError::Grammar("expected additive child".to_owned()));
        };
        self.eval_additive(inner)
    }

    /// Increments the recursion depth, evaluates `pair` as an expression, then
    /// decrements depth — even on error, ensuring consistent state.
    fn eval_nested(&mut self, pair: Pair<Rule>) -> Result<UnitValue, ParseError> {
        if self.depth >= MAX_DEPTH {
            return Err(ParseError::TooDeep);
        }
        self.depth += 1;
        let result = self.eval_expr(pair);
        self.depth -= 1;
        result
    }

    fn eval_additive(&mut self, pair: Pair<Rule>) -> Result<UnitValue, ParseError> {
        let mut inner = pair.into_inner();
        let Some(first) = inner.next() else {
            return Err(ParseError::Grammar("expected first product".to_owned()));
        };
        let mut result = self.eval_product(first)?;

        while let Some(op_pair) = inner.next() {
            let op = op_pair.as_str();
            let Some(rhs_pair) = inner.next() else {
                return Err(ParseError::Grammar("expected rhs product".to_owned()));
            };
            let rhs = self.eval_product(rhs_pair)?;
            if op == "+" {
                if !result.add_assign(&rhs) {
                    return Err(ParseError::IncompatibleDimensions("addition"));
                }
                continue;
            }

            let mut neg = rhs;
            neg.factor = -neg.factor;
            if !result.add_assign(&neg) {
                return Err(ParseError::IncompatibleDimensions("subtraction"));
            }
        }
        Ok(result)
    }

    fn eval_product(&mut self, pair: Pair<Rule>) -> Result<UnitValue, ParseError> {
        let mut inner = pair.into_inner();
        let Some(first) = inner.next() else {
            return Err(ParseError::Grammar("expected first power".to_owned()));
        };
        let mut result = self.eval_power(first)?;

        while let Some(next) = inner.next() {
            match next.as_rule() {
                Rule::mul_op => {
                    let op = next.as_str();
                    let Some(rhs_pair) = inner.next() else {
                        return Err(ParseError::Grammar("expected rhs power".to_owned()));
                    };
                    let rhs = self.eval_power(rhs_pair)?;
                    if op == "/" || op == "per" {
                        result.divide_assign(&rhs);
                    } else if op == "|" {
                        if !result.is_dimensionless() || !rhs.is_dimensionless() {
                            return Err(ParseError::DimensionlessRequired("|".to_owned()));
                        }
                        result.divide_assign(&rhs);
                    } else {
                        result.multiply_assign(&rhs);
                    }
                }
                Rule::juxt_factor => {
                    let Some(inner_power) = next.into_inner().next() else {
                        return Err(ParseError::Grammar(
                            "expected power in juxt_factor".to_owned(),
                        ));
                    };
                    if result.is_dimensionless() && result.factor == 0.0 {
                        match self.eval_power(inner_power) {
                            Ok(rhs) => result.multiply_assign(&rhs),
                            Err(ParseError::UnknownUnit(_)) => {}
                            Err(e) => return Err(e),
                        }
                        continue;
                    }

                    let rhs = self.eval_power(inner_power)?;
                    result.multiply_assign(&rhs);
                }
                _ => unreachable!("unexpected rule in product: {:?}", next.as_rule()),
            }
        }
        Ok(result)
    }

    fn eval_power(&mut self, pair: Pair<Rule>) -> Result<UnitValue, ParseError> {
        let mut inner = pair.into_inner();
        let Some(base_pair) = inner.next() else {
            return Err(ParseError::Grammar("expected unary base".to_owned()));
        };
        let base = self.eval_unary(base_pair)?;

        if let Some(_pow_op) = inner.next() {
            let Some(exp_pair) = inner.next() else {
                return Err(ParseError::Grammar("expected exponent".to_owned()));
            };
            let exp = self.eval_unary(exp_pair)?;
            return apply_power(base, exp);
        }
        Ok(base)
    }

    fn eval_unary(&mut self, pair: Pair<Rule>) -> Result<UnitValue, ParseError> {
        let mut inner = pair.into_inner();
        let Some(first) = inner.next() else {
            return Err(ParseError::Grammar("expected unary child".to_owned()));
        };
        match first.as_rule() {
            Rule::minus => {
                let Some(rhs) = inner.next() else {
                    return Err(ParseError::Grammar("expected unary operand".to_owned()));
                };
                let mut v = self.eval_unary(rhs)?;
                v.factor = -v.factor;
                Ok(v)
            }
            Rule::per => {
                let Some(rhs) = inner.next() else {
                    return Err(ParseError::Grammar("expected operand after per".to_owned()));
                };
                let mut v = self.eval_unary(rhs)?;
                v.invert();
                Ok(v)
            }
            Rule::atom => self.eval_atom(first),
            _ => unreachable!("unexpected rule in unary: {:?}", first.as_rule()),
        }
    }

    fn eval_atom(&mut self, pair: Pair<Rule>) -> Result<UnitValue, ParseError> {
        let Some(child) = pair.into_inner().next() else {
            return Err(ParseError::Grammar("expected atom child".to_owned()));
        };
        match child.as_rule() {
            Rule::number => {
                let Some(num_inner) = child.into_inner().next() else {
                    return Err(ParseError::Grammar("expected number inner".to_owned()));
                };
                let text = num_inner.as_str();
                let val: f64 = match num_inner.as_rule() {
                    Rule::hex_number => {
                        let hex = &text[2..];
                        i64::from_str_radix(hex, 16).map_err(|_| {
                            ParseError::InvalidNumber("invalid hex literal".to_owned())
                        })? as f64
                    }
                    Rule::decimal_number => text
                        .parse()
                        .map_err(|_| ParseError::InvalidNumber(text.to_owned()))?,
                    _ => unreachable!(),
                };
                Ok(UnitValue::from_factor(val))
            }
            Rule::tilde_call => {
                let mut tc = child.into_inner();
                let Some(name_pair) = tc.next() else {
                    return Err(ParseError::Grammar("expected tilde_call name".to_owned()));
                };
                let name = name_pair.as_str().to_owned();
                let Some(expr_pair) = tc.next() else {
                    return Err(ParseError::Grammar("expected tilde_call expr".to_owned()));
                };
                let arg = self.eval_nested(expr_pair)?;
                self.call_inverse_function(&name, arg)
            }
            Rule::func_call => {
                let mut fc = child.into_inner();
                let Some(name_pair) = fc.next() else {
                    return Err(ParseError::Grammar("expected func_call name".to_owned()));
                };
                let name = name_pair.as_str().to_owned();
                let mut args: Vec<UnitValue> = Vec::new();
                if let Some(arg_list_pair) = fc.next() {
                    for expr_pair in arg_list_pair.into_inner() {
                        args.push(self.eval_nested(expr_pair)?);
                    }
                }
                self.call_function_multi(&name, args)
            }
            Rule::name => self.resolve_name(child.as_str()),
            Rule::group => {
                let Some(expr_pair) = child.into_inner().next() else {
                    return Err(ParseError::Grammar("expected group expr".to_owned()));
                };
                self.eval_nested(expr_pair)
            }
            _ => unreachable!("unexpected rule in atom: {:?}", child.as_rule()),
        }
    }

    fn parse_sub(&mut self, expr: &str) -> Result<UnitValue, ParseError> {
        let expr_pair = parse_to_expr_pair(expr)?;
        self.eval_expr(expr_pair)
    }

    fn resolve_name(&mut self, name: &str) -> Result<UnitValue, ParseError> {
        if let Some(val) = self.vars.get(name) {
            return Ok(val.clone());
        }

        if let Some(entry) = self.db.units.get(name).cloned() {
            return self.resolve_entry(name, entry);
        }

        if name.len() > 3 {
            let candidates: Vec<String> = if let Some(base) = name.strip_suffix("ies") {
                vec![format!("{base}y")]
            } else if let Some(base) = name.strip_suffix("es") {
                vec![format!("{base}e"), base.to_owned()]
            } else if let Some(base) = name.strip_suffix('s') {
                vec![base.to_owned()]
            } else {
                Vec::new()
            };
            for singular in candidates {
                if let Some(entry) = self.db.units.get(singular.as_str()).cloned() {
                    return self.resolve_entry(&singular, entry);
                }
            }
        }

        if let Some(prefix_def) = self.db.find_prefix_by_name(name) {
            let def = prefix_def.to_owned();
            return self.parse_sub(&def);
        }

        if let Some((prefix_def, unit_name)) = self.db.find_with_prefix(name) {
            let prefix_str = prefix_def.to_owned();
            let unit_str = unit_name.to_owned();
            let prefix_val = self.parse_sub(&prefix_str)?;
            let unit_entry = self
                .db
                .units
                .get(unit_str.as_str())
                .cloned()
                .ok_or_else(|| ParseError::UnknownUnit(unit_str.clone()))?;
            let mut unit_val = self.resolve_entry(&unit_str, unit_entry)?;
            unit_val.multiply_assign(&prefix_val);
            return Ok(unit_val);
        }

        Err(ParseError::UnknownUnit(name.to_owned()))
    }

    fn resolve_entry(&mut self, name: &str, entry: UnitEntry) -> Result<UnitValue, ParseError> {
        match entry {
            UnitEntry::Primitive => Ok(UnitValue::primitive(name)),
            UnitEntry::DimensionlessPrimitive => Ok(UnitValue::one()),
            UnitEntry::Derived(def) => {
                if self.resolving.contains(name) {
                    return Err(ParseError::CircularDefinition(name.to_owned()));
                }
                if self.depth >= MAX_DEPTH {
                    return Err(ParseError::MaxRecursion(name.to_owned()));
                }
                self.resolving.insert(name.to_owned());
                self.depth += 1;
                let result = self.parse_sub(&def);
                self.depth -= 1;
                self.resolving.remove(name);
                result
            }
        }
    }

    /// Extracts a dimensionless angle value (in radians) for ANGLEIN functions.
    /// If `arg` is already dimensionless, returns its factor unchanged.
    /// If dimensional, divides by `radian` and checks again; errors otherwise.
    fn anglein(&mut self, func: &str, arg: UnitValue) -> Result<f64, ParseError> {
        if arg.is_dimensionless() {
            return Ok(arg.factor);
        }
        let radian = self
            .parse_sub("radian")
            .unwrap_or_else(|_| UnitValue::one());
        let mut divided = arg;
        divided.divide_assign(&radian);
        if divided.is_dimensionless() {
            return Ok(divided.factor);
        }
        Err(ParseError::DimensionlessRequired(func.to_owned()))
    }

    /// Wraps a radian value for ANGLEOUT functions (multiplies by the `radian` unit).
    fn angleout(&mut self, radians: f64) -> UnitValue {
        let mut radian = self
            .parse_sub("radian")
            .unwrap_or_else(|_| UnitValue::one());
        radian.factor *= radians;
        radian
    }

    fn call_function(&mut self, name: &str, arg: UnitValue) -> Result<UnitValue, ParseError> {
        match name {
            "sqrt" => {
                let mut v = arg;
                if !v.root_assign(2) {
                    return Err(ParseError::NotDivisibleByRoot);
                }
                Ok(v)
            }
            "cbrt" | "cuberoot" => {
                let mut v = arg;
                if !v.root_assign(3) {
                    return Err(ParseError::NotDivisibleByRoot);
                }
                Ok(v)
            }
            "abs" => {
                let mut v = arg;
                v.factor = v.factor.abs();
                Ok(v)
            }
            "exp" => {
                require_dimensionless("exp", &arg)?;
                Ok(UnitValue::from_factor(arg.factor.exp()))
            }
            "ln" => {
                require_dimensionless("ln", &arg)?;
                Ok(UnitValue::from_factor(arg.factor.ln()))
            }
            "log" | "log10" => {
                require_dimensionless("log", &arg)?;
                Ok(UnitValue::from_factor(arg.factor.log10()))
            }
            "log2" => {
                require_dimensionless("log2", &arg)?;
                Ok(UnitValue::from_factor(arg.factor.log2()))
            }

            "round" => {
                require_dimensionless("round", &arg)?;
                Ok(UnitValue::from_factor(arg.factor.round()))
            }
            "floor" => {
                require_dimensionless("floor", &arg)?;
                Ok(UnitValue::from_factor(arg.factor.floor()))
            }
            "ceil" => {
                require_dimensionless("ceil", &arg)?;
                Ok(UnitValue::from_factor(arg.factor.ceil()))
            }
            "secant" => {
                let x = self.anglein("secant", arg)?;
                let c = x.cos();
                if c == 0.0 {
                    return Err(ParseError::Undefined {
                        func: "secant",
                        reason: "cos is zero",
                    });
                }
                Ok(UnitValue::from_factor(1.0 / c))
            }
            "sin" => {
                let x = self.anglein("sin", arg)?;
                Ok(UnitValue::from_factor(x.sin()))
            }
            "cos" => {
                let x = self.anglein("cos", arg)?;
                Ok(UnitValue::from_factor(x.cos()))
            }
            "tan" => {
                let x = self.anglein("tan", arg)?;
                Ok(UnitValue::from_factor(x.tan()))
            }
            "csc" => {
                let x = self.anglein("csc", arg)?;
                let s = x.sin();
                if s == 0.0 {
                    return Err(ParseError::Undefined {
                        func: "csc",
                        reason: "sin is zero",
                    });
                }
                Ok(UnitValue::from_factor(1.0 / s))
            }
            "cot" => {
                let x = self.anglein("cot", arg)?;
                let s = x.sin();
                if s == 0.0 {
                    return Err(ParseError::Undefined {
                        func: "cot",
                        reason: "sin is zero",
                    });
                }
                Ok(UnitValue::from_factor(x.cos() / s))
            }

            // --- Inverse trigonometric ANGLEOUT (longer names first) ---
            "asecant" => {
                require_dimensionless("asecant", &arg)?;
                if arg.factor.abs() < 1.0 {
                    return Err(ParseError::OutOfRange {
                        func: "asecant",
                        constraint: "|argument| must be >= 1",
                    });
                }
                Ok(self.angleout((1.0 / arg.factor).acos()))
            }
            "acos" => {
                require_dimensionless("acos", &arg)?;
                if arg.factor.abs() > 1.0 {
                    return Err(ParseError::OutOfRange {
                        func: "acos",
                        constraint: "argument must be in [-1, 1]",
                    });
                }
                Ok(self.angleout(arg.factor.acos()))
            }
            "asin" => {
                require_dimensionless("asin", &arg)?;
                if arg.factor.abs() > 1.0 {
                    return Err(ParseError::OutOfRange {
                        func: "asin",
                        constraint: "argument must be in [-1, 1]",
                    });
                }
                Ok(self.angleout(arg.factor.asin()))
            }
            "atan" => {
                require_dimensionless("atan", &arg)?;
                Ok(self.angleout(arg.factor.atan()))
            }
            "acsc" => {
                require_dimensionless("acsc", &arg)?;
                if arg.factor.abs() < 1.0 {
                    return Err(ParseError::OutOfRange {
                        func: "acsc",
                        constraint: "|argument| must be >= 1",
                    });
                }
                Ok(self.angleout((1.0 / arg.factor).asin()))
            }
            "acot" => {
                require_dimensionless("acot", &arg)?;
                Ok(self.angleout((1.0 / arg.factor).atan()))
            }

            // --- Hyperbolic inverse (longer names first) ---
            "asinh" => {
                require_dimensionless("asinh", &arg)?;
                Ok(UnitValue::from_factor(arg.factor.asinh()))
            }
            "acosh" => {
                require_dimensionless("acosh", &arg)?;
                if arg.factor < 1.0 {
                    return Err(ParseError::OutOfRange {
                        func: "acosh",
                        constraint: "argument must be >= 1",
                    });
                }
                Ok(UnitValue::from_factor(arg.factor.acosh()))
            }
            "atanh" => {
                require_dimensionless("atanh", &arg)?;
                if arg.factor.abs() >= 1.0 {
                    return Err(ParseError::OutOfRange {
                        func: "atanh",
                        constraint: "argument must be in (-1, 1)",
                    });
                }
                Ok(UnitValue::from_factor(arg.factor.atanh()))
            }
            "acsch" => {
                require_dimensionless("acsch", &arg)?;
                if arg.factor == 0.0 {
                    return Err(ParseError::OutOfRange {
                        func: "acsch",
                        constraint: "argument cannot be zero",
                    });
                }
                Ok(UnitValue::from_factor((1.0 / arg.factor).asinh()))
            }
            "asech" => {
                require_dimensionless("asech", &arg)?;
                if arg.factor <= 0.0 || arg.factor > 1.0 {
                    return Err(ParseError::OutOfRange {
                        func: "asech",
                        constraint: "argument must be in (0, 1]",
                    });
                }
                Ok(UnitValue::from_factor((1.0 / arg.factor).acosh()))
            }
            "acoth" => {
                require_dimensionless("acoth", &arg)?;
                if arg.factor.abs() <= 1.0 {
                    return Err(ParseError::OutOfRange {
                        func: "acoth",
                        constraint: "|argument| must be > 1",
                    });
                }
                Ok(UnitValue::from_factor((1.0 / arg.factor).atanh()))
            }

            // --- Hyperbolic (longer names first) ---
            "sinh" => {
                require_dimensionless("sinh", &arg)?;
                Ok(UnitValue::from_factor(arg.factor.sinh()))
            }
            "cosh" => {
                require_dimensionless("cosh", &arg)?;
                Ok(UnitValue::from_factor(arg.factor.cosh()))
            }
            "tanh" => {
                require_dimensionless("tanh", &arg)?;
                Ok(UnitValue::from_factor(arg.factor.tanh()))
            }
            "csch" => {
                require_dimensionless("csch", &arg)?;
                let s = arg.factor.sinh();
                if s == 0.0 {
                    return Err(ParseError::Undefined {
                        func: "csch",
                        reason: "sinh is zero",
                    });
                }
                Ok(UnitValue::from_factor(1.0 / s))
            }
            "sech" => {
                require_dimensionless("sech", &arg)?;
                Ok(UnitValue::from_factor(1.0 / arg.factor.cosh()))
            }
            "coth" => {
                require_dimensionless("coth", &arg)?;
                let s = arg.factor.sinh();
                if s == 0.0 {
                    return Err(ParseError::Undefined {
                        func: "coth",
                        reason: "sinh is zero",
                    });
                }
                Ok(UnitValue::from_factor(arg.factor.cosh() / s))
            }

            // --- Special functions (DIMENSIONLESS) ---
            "erf" => {
                require_dimensionless("erf", &arg)?;
                Ok(UnitValue::from_factor(erf_approx(arg.factor)))
            }
            "erfc" => {
                require_dimensionless("erfc", &arg)?;
                Ok(UnitValue::from_factor(1.0 - erf_approx(arg.factor)))
            }
            "lnGamma" => {
                require_dimensionless("lnGamma", &arg)?;
                Ok(UnitValue::from_factor(gamma_lanczos(arg.factor).abs().ln()))
            }
            "Gamma" => {
                require_dimensionless("Gamma", &arg)?;
                let val = gamma_lanczos(arg.factor);
                if !val.is_finite() {
                    return Err(ParseError::Undefined {
                        func: "Gamma",
                        reason: "non-positive integer argument",
                    });
                }
                Ok(UnitValue::from_factor(val))
            }
            "factorial" => {
                require_dimensionless("factorial", &arg)?;
                if arg.factor < 0.0 {
                    return Err(ParseError::OutOfRange {
                        func: "factorial",
                        constraint: "argument must be >= 0",
                    });
                }
                if arg.factor.fract() != 0.0 {
                    return Err(ParseError::OutOfRange {
                        func: "factorial",
                        constraint: "argument must be a non-negative integer",
                    });
                }
                Ok(UnitValue::from_factor(gamma_lanczos(arg.factor + 1.0)))
            }

            _ => {
                // logN: log base N for any all-digit suffix not already matched (e.g. log3, log16)
                if let Some(base_str) = name.strip_prefix("log")
                    && !base_str.is_empty()
                    && base_str.chars().all(|c| c.is_ascii_digit())
                {
                    let Ok(base) = base_str.parse::<f64>() else {
                        return Err(ParseError::Grammar(format!("{name}: invalid log base")));
                    };
                    if base > 1.0 {
                        require_dimensionless(name, &arg)?;
                        return Ok(UnitValue::from_factor(arg.factor.ln() / base.ln()));
                    }
                }
                if let Some(table) = self.db.tables.get(name) {
                    if !arg.is_dimensionless() {
                        return Err(ParseError::DimensionlessRequired(name.to_owned()));
                    }
                    let output =
                        table
                            .interpolate(arg.factor)
                            .ok_or_else(|| ParseError::NotInDomain {
                                func: name.to_owned(),
                                arg: 1,
                            })?;
                    let unit_expr = format!("{output} {}", table.unit);
                    return self.parse_sub(&unit_expr);
                }
                Err(ParseError::UnknownUnit(name.to_owned()))
            }
        }
    }

    /// Dispatch function call with an argument list of any size.
    ///
    /// - User-defined functions (`db.functions`): support zero or multiple args.
    /// - Built-in functions and tables: require exactly 1 argument.
    fn call_function_multi(
        &mut self,
        name: &str,
        args: Vec<UnitValue>,
    ) -> Result<UnitValue, ParseError> {
        if let Some(func) = self.db.functions.get(name).cloned() {
            return self.call_user_function(name, func, args);
        }
        if args.len() != 1 {
            return Err(ParseError::ArityMismatch {
                func: name.to_owned(),
                expected: 1,
                got: args.len(),
            });
        }
        let arg = args.into_iter().next().expect("args.len() == 1");
        self.call_function(name, arg)
    }

    fn call_user_function(
        &mut self,
        name: &str,
        func: FunctionDef,
        args: Vec<UnitValue>,
    ) -> Result<UnitValue, ParseError> {
        if args.len() != func.params.len() {
            return Err(ParseError::ArityMismatch {
                func: name.to_owned(),
                expected: func.params.len(),
                got: args.len(),
            });
        }
        // Zero-arg: body is a unit alias — parse directly without binding variables.
        if func.params.is_empty() {
            return self.parse_sub(&func.forward);
        }
        if !func.noerror {
            for (i, arg) in args.iter().enumerate() {
                let unit_str = func.units.get(i).and_then(|u| u.as_deref());
                let domain = func.domain.get(i).and_then(|d| d.as_ref());
                self.validate_func_arg(name, i, arg, unit_str, domain)?;
            }
        }
        let mut old_vals: Vec<(String, Option<UnitValue>)> = Vec::new();
        for (param, arg) in func.params.iter().zip(args) {
            let old = self.vars.insert(param.clone(), arg);
            old_vals.push((param.clone(), old));
        }
        let result = self.parse_sub(&func.forward);
        for (param, old) in old_vals {
            if let Some(v) = old {
                self.vars.insert(param, v);
            } else {
                self.vars.remove(&param);
            }
        }
        result
    }

    fn call_inverse_function(
        &mut self,
        name: &str,
        arg: UnitValue,
    ) -> Result<UnitValue, ParseError> {
        let func: FunctionDef = self
            .db
            .functions
            .get(name)
            .cloned()
            .ok_or_else(|| ParseError::UnknownUnit(name.to_owned()))?;
        if func.params.len() != 1 {
            return Err(ParseError::InverseNotSupported {
                func: name.to_owned(),
            });
        }
        let reverse = func
            .reverse
            .ok_or_else(|| ParseError::NoInverse(name.to_owned()))?;
        if !func.noerror {
            self.validate_func_arg(
                name,
                0,
                &arg,
                func.inverse_unit.as_deref(),
                func.range.as_ref(),
            )?;
        }
        let old_val = self.vars.insert(name.to_owned(), arg);
        let result = self.parse_sub(&reverse);
        if let Some(v) = old_val {
            self.vars.insert(name.to_owned(), v);
        } else {
            self.vars.remove(name);
        }
        result
    }

    /// Check that `arg` is dimensionally conformable with `unit_str` (if given)
    /// and that the resulting scalar lies within `domain` (if given).
    ///
    /// `idx` is 0-based and used only for error messages.
    fn validate_func_arg(
        &mut self,
        func_name: &str,
        idx: usize,
        arg: &UnitValue,
        unit_str: Option<&str>,
        domain: Option<&super::database::Interval>,
    ) -> Result<(), ParseError> {
        let value = if let Some(unit_str) = unit_str {
            let unit_val = self.parse_sub(unit_str)?;
            if !are_conformable(arg, &unit_val) {
                return Err(ParseError::WrongDimensions {
                    func: func_name.to_owned(),
                    arg: idx + 1,
                });
            }
            arg.factor / unit_val.factor
        } else {
            arg.factor
        };
        let Some(interval) = domain else {
            return Ok(());
        };
        if !interval.contains(value) {
            return Err(ParseError::NotInDomain {
                func: func_name.to_owned(),
                arg: idx + 1,
            });
        }
        Ok(())
    }
}

pub(crate) fn parseunit(input: &str) -> Result<UnitValue, ParseError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ParseError::EmptyExpression);
    }
    let expr_pair = parse_to_expr_pair(trimmed)?;
    let db = db_read();
    let mut eval = Evaluator::new(&db);
    eval.eval_expr(expr_pair)
}

pub(crate) fn parseunit_with_vars(
    input: &str,
    vars: &HashMap<String, UnitValue>,
) -> Result<UnitValue, ParseError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ParseError::EmptyExpression);
    }
    let expr_pair = parse_to_expr_pair(trimmed)?;
    let db = db_read();
    let mut eval = Evaluator::new(&db);
    eval.vars = vars.clone();
    eval.eval_expr(expr_pair)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rstest::rstest;

    use super::*;
    use crate::engine::native::types::UnitValue;

    #[test]
    fn parse_simple_number() {
        let v = parseunit("7").unwrap();

        assert_eq!(v.factor, 7.0);
        assert!(v.is_dimensionless());
    }

    #[test]
    fn parse_multiplication() {
        let v = parseunit("2 * 3").unwrap();

        assert_eq!(v.factor, 6.0);
        assert!(v.is_dimensionless());
    }

    #[test]
    fn parse_division() {
        let v = parseunit("10 / 2").unwrap();

        assert_eq!(v.factor, 5.0);
        assert!(v.is_dimensionless());
    }

    #[test]
    fn parse_pipe_operator() {
        let v = parseunit("1|2").unwrap();

        assert!((v.factor - 0.5).abs() < 1e-12);
        assert!(v.is_dimensionless());
    }

    #[test]
    fn parse_unary_minus() {
        let v = parseunit("-5").unwrap();

        assert_eq!(v.factor, -5.0);
        assert!(v.is_dimensionless());
    }

    #[test]
    fn call_builtin_sqrt() {
        let v = parseunit("sqrt(4)").unwrap();

        assert!((v.factor - 2.0).abs() < 1e-12);
        assert!(v.is_dimensionless());
    }

    #[test]
    fn parse_power() {
        crate::definitions::ensure_definitions();

        let v = parseunit("m^3").unwrap();

        assert_eq!(v.factor, 1.0);
        assert_eq!(v.dimensions.get("m"), Some(&3));
    }

    #[test]
    fn parse_double_star_power() {
        crate::definitions::ensure_definitions();

        let v = parseunit("kg**2").unwrap();

        assert_eq!(v.factor, 1.0);
        assert_eq!(v.dimensions.get("kg"), Some(&2));
    }

    #[test]
    fn parse_parentheses() {
        crate::definitions::ensure_definitions();

        let v = parseunit("(2 * m)").unwrap();

        assert_eq!(v.factor, 2.0);
        assert_eq!(v.dimensions.get("m"), Some(&1));
    }

    #[test]
    fn parse_juxtaposition() {
        crate::definitions::ensure_definitions();

        let v = parseunit("5 m").unwrap();

        assert_eq!(v.factor, 5.0);
        assert_eq!(v.dimensions.get("m"), Some(&1));
    }

    #[test]
    fn resolve_prefix() {
        crate::definitions::ensure_definitions();

        let v = parseunit("kilogram").unwrap();

        assert!((v.factor - 1.0).abs() < 1e-9);
        assert_eq!(v.dimensions.get("kg"), Some(&1));
    }

    #[test]
    fn call_user_function() {
        crate::definitions::ensure_definitions();

        let v = parseunit("square(3)").unwrap();

        assert!((v.factor - 9.0).abs() < 1e-12);
    }

    #[rstest]
    #[case::empty("")]
    #[case::whitespace_only("   ")]
    fn error_on_empty(#[case] input: &str) {
        let result = parseunit(input);

        assert!(result.is_err());
    }

    #[rstest]
    #[case::missing_close("(2")]
    #[case::stray_close(")")]
    fn error_on_unbalanced_parens(#[case] input: &str) {
        let result = parseunit(input);

        assert!(result.is_err());
    }

    #[test]
    fn error_on_trailing_input() {
        let result = parseunit("2 )");

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("trailing"),
            "expected trailing-input error, got: {msg}"
        );
    }

    #[test]
    fn parseunit_with_vars_resolves_binding() {
        let vars = HashMap::from([("x".to_owned(), UnitValue::from_factor(3.0))]);

        let v = parseunit_with_vars("x", &vars).unwrap();

        assert!((v.factor - 3.0).abs() < 1e-12);
        assert!(v.is_dimensionless());
    }

    #[test]
    fn error_on_parseunit_with_vars_trailing_input() {
        let vars = HashMap::new();

        let result = parseunit_with_vars("2 )", &vars);

        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("trailing"),
            "error should mention trailing input"
        );
    }

    // --- Signed-integer exponents (regression guard for pest migration) ---

    #[rstest]
    #[case::negative("m^-1", "m", -1)]
    #[case::double_star_negative("m**-2", "m", -2)]
    fn power_signed_exponent(#[case] input: &str, #[case] dim: &str, #[case] exp: i32) {
        crate::definitions::ensure_definitions();

        let v = parseunit(input).unwrap();

        assert_eq!(v.dimensions.get(dim), Some(&exp));
    }

    #[test]
    fn parse_power_negative_dimensionless() {
        let v = parseunit("2^-1").unwrap();

        assert!((v.factor - 0.5).abs() < 1e-12);
        assert!(v.is_dimensionless());
    }

    // --- Hex literals (regression guard for pest migration) ---

    #[rstest]
    #[case::lowercase("0xff", 255.0)]
    #[case::uppercase_digits("0xFF", 255.0)]
    #[case::zero("0x0", 0.0)]
    fn parse_hex_number(#[case] input: &str, #[case] expected: f64) {
        let v = parseunit(input).unwrap();

        assert_eq!(v.factor, expected);
        assert!(v.is_dimensionless());
    }

    #[test]
    fn parse_malformed_hex_matches_c_strtod() {
        // `0xGG`: hex_number fails (G is not hex), decimal_number matches "0",
        // "GG" is juxtaposed but swallowed because factor is 0 — mirrors C strtod.
        let v = parseunit("0xGG").unwrap();

        assert_eq!(v.factor, 0.0);
        assert!(v.is_dimensionless());
    }

    #[test]
    fn parse_malformed_hex_does_not_acquire_dimensions() {
        crate::definitions::ensure_definitions();

        let v = parseunit("0xkg").unwrap();

        assert_eq!(v.factor, 0.0);
        assert!(
            v.is_dimensionless(),
            "0xkg must be dimensionless (C compat), got: {:?}",
            v.dimensions
        );
    }

    #[test]
    fn parse_malformed_hex_0xm_is_dimensionless_zero() {
        crate::definitions::ensure_definitions();

        let v = parseunit("0xm").unwrap();

        assert_eq!(v.factor, 0.0);
        assert!(v.is_dimensionless());
    }

    #[test]
    fn zero_factor_does_not_swallow_parse_error() {
        let result = parseunit("0 )");

        assert!(result.is_err());
    }

    // --- Unary plus ---

    #[test]
    fn error_on_unary_plus_in_exponent() {
        crate::definitions::ensure_definitions();

        let result = parseunit("m^+2");

        assert!(result.is_err());
    }

    #[test]
    fn error_on_unary_plus() {
        let result = parseunit("+5");

        assert!(result.is_err());
    }

    // --- Error messages ---

    #[test]
    fn error_on_unknown_unit_message() {
        let result = parseunit("ZZZNOTAUNIT");

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("unknown"),
            "expected 'unknown' in error message, got: {msg}"
        );
    }

    #[test]
    fn error_on_incompatible_dimensions_addition() {
        crate::definitions::ensure_definitions();

        let result = parseunit("m + kg");

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("incompatible"),
            "expected incompatible-dimensions error, got: {msg}"
        );
    }
}
