//! Safe, high-level Rust interface to GNU Units.
//!
//! By default this crate uses a native pure-Rust unit engine.
//! Build with `--no-default-features --features vendored` to use the vendored C library instead.

use std::fmt;

/// The vendored C-bindings crate. Only available when `features = ["vendored"]`.
#[cfg(feature = "vendored")]
pub use gnu_units_sys;

#[cfg(feature = "currency-update")]
pub mod currency_update;

pub(crate) mod definitions;
mod engine;
mod units;

use self::definitions::{DEFINITIONS, ensure_definitions};
pub use self::definitions::{Definition, DefinitionKind};

#[cfg(feature = "currency-update")]
use self::definitions::load_definitions;

#[cfg(feature = "currency-update")]
pub use self::currency_update::{
    CurrencySource, CurrencyUpdateOptions, UpdateError, fetch_currency_updates,
};

/// Numeric error codes matching the GNU units C library values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ErrorCode {
    /// Successful conversion (no error).
    Normal = 0,
    /// Expression could not be parsed.
    Parse = 1,
    /// Incompatible dimensions in addition or subtraction.
    BadSum = 5,
    /// Result is not a dimensionless number.
    NotANumber = 6,
    /// Root of a non-integer dimension.
    NotRoot = 7,
    /// Unit name not found in the database.
    UnknownUnit = 8,
    /// Function argument outside its valid domain.
    NotInDomain = 10,
    /// Function argument has wrong dimensions.
    BadFuncArg = 11,
    /// Dimensional unit raised to an irrational exponent.
    IrrationalExponent = 12,
    /// Expression is not a function or table (used for fallback logic).
    NotAFunc = 13,
    /// Invalid numeric literal.
    BadNum = 20,
}

impl ErrorCode {
    #[cfg(feature = "vendored")]
    pub(crate) fn from_raw(code: i32) -> Self {
        match code {
            0 => Self::Normal,
            1 => Self::Parse,
            5 => Self::BadSum,
            6 => Self::NotANumber,
            7 => Self::NotRoot,
            8 => Self::UnknownUnit,
            10 => Self::NotInDomain,
            11 => Self::BadFuncArg,
            12 => Self::IrrationalExponent,
            13 => Self::NotAFunc,
            20 => Self::BadNum,
            _ => Self::Parse,
        }
    }
}

/// Wraps a raw error code returned by the units engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitsError(ErrorCode);

impl UnitsError {
    /// Returns the error code as defined in [`ErrorCode`].
    pub fn code(&self) -> ErrorCode {
        self.0
    }

    /// Returns `true` when the error indicates a dimensionless reduction failed.
    pub fn is_not_dimensionless(&self) -> bool {
        self.0 == ErrorCode::NotANumber
    }

    /// Returns `true` when the expression was invalid (unknown/unparseable).
    pub fn is_invalid_unit(&self) -> bool {
        self.0 == ErrorCode::UnknownUnit || self.0 == ErrorCode::Parse
    }
}

impl fmt::Display for UnitsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GNU units error code {}", self.0 as i32)
    }
}

impl std::error::Error for UnitsError {}

/// Convenience alias for [`std::result::Result<T, UnitsError>`].
pub type Result<T> = std::result::Result<T, UnitsError>;

/// Represents a dimensional quantity: a numeric factor paired with zero or
/// more base dimensions.
pub struct Unit {
    raw: engine::RawUnit,
}

// SAFETY: For the vendored backend, all FFI calls are serialised through a
// global mutex. The raw unittype is not accessed concurrently.
#[cfg(feature = "vendored")]
unsafe impl Send for Unit {}
#[cfg(feature = "vendored")]
unsafe impl Sync for Unit {}

impl Unit {
    /// Creates a freshly initialised unit with factor `1.0` and no dimensions.
    pub fn new() -> Self {
        Self {
            raw: engine::unit_new(),
        }
    }

    /// Parses a GNU units expression string and returns the resulting [`Unit`].
    pub fn parse(input: &str) -> Result<Self> {
        ensure_definitions();
        engine::unit_parse(input).map(|raw| Self { raw })
    }

    /// Returns the numeric factor of the unit.
    pub fn factor(&self) -> f64 {
        engine::unit_factor(&self.raw)
    }

    /// Returns the base dimensions as a human-readable string.
    pub fn base_units(&self) -> String {
        engine::unit_base_units(&self.raw)
    }

    /// Multiplies `self` by `rhs` in place.
    pub fn multiply(&mut self, rhs: Unit) -> Result<()> {
        engine::unit_multiply(&mut self.raw, &rhs.raw)
    }

    /// Divides `self` by `rhs` in place.
    pub fn divide(&mut self, rhs: Unit) -> Result<()> {
        engine::unit_divide(&mut self.raw, &rhs.raw)
    }

    /// Adds `rhs` to `self` in place.
    pub fn add(&mut self, rhs: Unit) -> Result<()> {
        engine::unit_add(&mut self.raw, &rhs.raw)
    }

    /// Inverts `self` in place (reciprocal).
    pub fn invert(&mut self) {
        engine::unit_invert(&mut self.raw);
    }

    /// Raises `self` to a non-negative integer `power` in place.
    pub fn pow(&mut self, power: i32) -> Result<()> {
        engine::unit_pow(&mut self.raw, power)
    }

    /// Takes the `n`-th root of `self` in place.
    pub fn root(&mut self, n: i32) -> Result<()> {
        engine::unit_root(&mut self.raw, n)
    }

    /// Converts a dimensionless unit to its numeric value.
    pub fn to_number(&self) -> Result<f64> {
        engine::unit_to_number(&self.raw)
    }

    /// Converts `self` into the unit expressed by `to`, returning the numeric
    /// conversion factor.
    pub fn convert_to(mut self, to: Unit) -> Result<f64> {
        self.divide(to)?;
        self.to_number()
    }

    /// Returns `true` when `self` and `other` have the same base dimensions.
    pub fn is_conformable(&self, other: &Unit) -> bool {
        engine::unit_is_conformable(&self.raw, &other.raw)
    }
}

impl Default for Unit {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for Unit {
    fn clone(&self) -> Self {
        Self {
            raw: engine::unit_clone(&self.raw),
        }
    }
}

impl Drop for Unit {
    fn drop(&mut self) {
        engine::unit_drop(&mut self.raw);
    }
}

/// Convenience wrapper around [`Unit::parse`].
pub fn parse(input: &str) -> Result<Unit> {
    Unit::parse(input)
}

/// Parses `from` and `to` as GNU units expressions and returns the numeric
/// conversion factor.
///
/// First attempts a function or table conversion; if neither applies
/// ([`ErrorCode::NotAFunc`]), falls back to standard unit division.
pub fn convert(from: &str, to: &str) -> Result<f64> {
    ensure_definitions();
    match engine::convert_func(from, to) {
        Err(e) if e.code() == ErrorCode::NotAFunc => {}
        result => return result,
    }
    Unit::parse(from)?.convert_to(Unit::parse(to)?)
}

/// Returns all unit definitions conformable with `expr` in alphabetical order.
pub fn conformable(expr: &str) -> Result<Vec<String>> {
    let target = Unit::parse(expr)?;
    let defs = list_definitions();
    let names = defs
        .iter()
        .filter(|d| d.kind == DefinitionKind::Unit)
        .filter_map(|d| {
            let parsed = Unit::parse(&d.name).ok()?;
            if parsed.is_conformable(&target) {
                Some(d.name.clone())
            } else {
                None
            }
        })
        .collect();
    Ok(names)
}

/// Returns all unit definitions from the embedded GNU units database, sorted
/// alphabetically.
pub fn list_definitions() -> Vec<Definition> {
    ensure_definitions();
    DEFINITIONS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Reloads currency unit definitions from a GNU units currency file string.
#[cfg(feature = "currency-update")]
pub fn reload_currency(content: &str) {
    ensure_definitions();
    let new_defs = load_definitions(content, c"currency.units");
    let mut defs = DEFINITIONS.write().unwrap_or_else(|e| e.into_inner());
    defs.retain(|d| d.kind != DefinitionKind::Unit || !new_defs.iter().any(|n| n.name == d.name));
    defs.extend(new_defs);
    defs.sort_by(|a, b| a.name.cmp(&b.name));
}

// Required so that `Result<Unit, UnitsError>::unwrap_err()` compiles in tests
// (unwrap_err needs T: Debug to format the unexpected Ok value).
#[cfg(test)]
impl fmt::Debug for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Unit {{ factor: {} }}", self.factor())
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::definitions::{ensure_definitions, replace_operators};

    #[test]
    fn units_error_display() {
        let err = UnitsError(ErrorCode::BadNum);

        let s = format!("{err}");

        assert!(s.contains("20"));
    }

    #[rstest]
    #[case::eq_same_code(ErrorCode::Parse, ErrorCode::Parse, true)]
    #[case::ne_different_code(ErrorCode::Parse, ErrorCode::BadSum, false)]
    fn units_error_eq_semantics(#[case] a: ErrorCode, #[case] b: ErrorCode, #[case] equal: bool) {
        let ea = UnitsError(a);
        let eb = UnitsError(b);

        assert_eq!(ea == eb, equal);
    }

    #[test]
    fn units_error_copy_semantics() {
        let original = UnitsError(ErrorCode::BadSum);

        let copy = original;

        assert_eq!(copy, original);
    }

    #[rstest]
    #[case::notanumber(ErrorCode::NotANumber, true)]
    #[case::e_parse(ErrorCode::Parse, false)]
    #[case::e_badsum(ErrorCode::BadSum, false)]
    fn is_not_dimensionless(#[case] code: ErrorCode, #[case] expected: bool) {
        let err = UnitsError(code);

        assert_eq!(err.is_not_dimensionless(), expected);
    }

    #[rstest]
    #[case::e_parse(ErrorCode::Parse, true)]
    #[case::e_unknownunit(ErrorCode::UnknownUnit, true)]
    #[case::e_notanumber(ErrorCode::NotANumber, false)]
    fn is_invalid_unit(#[case] code: ErrorCode, #[case] expected: bool) {
        let err = UnitsError(code);

        assert_eq!(err.is_invalid_unit(), expected);
    }

    #[rstest]
    #[case::via_new(Unit::new())]
    #[case::via_default(Unit::default())]
    fn initial_factor_is_one(#[case] unit: Unit) {
        assert_eq!(unit.factor(), 1.0);
    }

    #[test]
    fn error_on_unary_plus() {
        let result = Unit::parse("+5");

        assert!(result.is_err());
    }

    #[rstest]
    #[case::hex_literal("0xff", 255.0)]
    #[case::negative_exponent_dimensionless("2^-1", 0.5)]
    fn parse_numeric(#[case] input: &str, #[case] expected: f64) {
        let result = Unit::parse(input);

        assert!(result.is_ok());
        assert!((result.unwrap().factor() - expected).abs() < 1e-6);
    }

    #[test]
    fn parse_hex_without_valid_digits_is_zero() {
        let result = Unit::parse("0xGG");

        assert!(result.is_ok());
        assert_eq!(result.unwrap().factor(), 0.0);
    }

    /// Malformed numeric literals and trailing input must produce `E_PARSE`,
    /// not `E_UNKNOWNUNIT` — ensures the parser gate is hit before unit lookup.
    #[rstest]
    #[case::error_on_trailing_paren("2 )")]
    fn parse_error_code_is_e_parse(#[case] input: &str) {
        let result = Unit::parse(input);

        let err = result.unwrap_err();
        assert_eq!(err.code(), ErrorCode::Parse);
    }

    #[test]
    fn parse_m_inverse_has_inverse_dimension() {
        let unit = Unit::parse("m^-1").unwrap();

        let base = unit.base_units();

        assert_eq!(unit.factor(), 1.0);
        assert!(
            base.contains("/ m"),
            "expected '/ m' in base_units, got: {base}"
        );
    }

    #[test]
    fn clone_preserves_factor() {
        let original = Unit::parse("5").unwrap();

        let cloned = original.clone();

        assert_eq!(cloned.factor(), original.factor());
    }

    #[test]
    fn to_number_returns_factor() {
        let unit = Unit::parse("42").unwrap();

        let result = unit.to_number();

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42.0);
    }

    #[test]
    fn all_definitions_have_non_empty_names() {
        let defs = list_definitions();

        for d in defs.iter() {
            assert!(!d.name.is_empty(), "found a definition with an empty name");
        }
    }

    #[rstest]
    #[case::kilo_prefix("kilo-")]
    #[case::hms("hms")]
    fn list_definitions_contains_known_entry(#[case] name: &str) {
        let defs = list_definitions();

        assert!(
            defs.iter().any(|d| d.name == name),
            "expected '{name}' in definitions"
        );
    }

    #[rstest]
    #[case::tempc("tempC", DefinitionKind::Function)]
    #[case::gasmark("gasmark", DefinitionKind::Table)]
    fn list_definitions_contains_known_kind_entry(
        #[case] canonical: &str,
        #[case] kind: DefinitionKind,
    ) {
        let defs = list_definitions();

        assert!(
            defs.iter()
                .any(|d| d.canonical_name() == canonical && d.kind == kind),
            "expected '{canonical}' with kind {kind:?} in definitions"
        );
    }

    #[test]
    fn definition_kind_name_invariant() {
        let defs = list_definitions();

        for d in defs.iter() {
            match d.kind {
                DefinitionKind::Prefix => {
                    assert!(
                        d.name.ends_with('-'),
                        "prefix '{}' must end with '-'",
                        d.name
                    );
                }
                DefinitionKind::Table => {
                    assert!(d.name.contains('['), "table '{}' must contain '['", d.name);
                }
                DefinitionKind::Function => {
                    assert!(
                        d.name.contains('('),
                        "function '{}' must contain '('",
                        d.name
                    );
                }
                _ => {}
            }
        }
    }

    #[rstest]
    #[case::simple_meter("m", "m")]
    #[case::compound("kg m/s^2", " / ")]
    fn base_units_contains_expected(#[case] expr: &str, #[case] contains: &str) {
        let unit = Unit::parse(expr).unwrap();

        let base = unit.base_units();

        assert!(
            base.contains(contains),
            "base_units('{expr}') = '{base}', expected it to contain '{contains}'"
        );
    }

    #[test]
    fn base_units_dimensionless_is_empty() {
        let unit = Unit::parse("42").unwrap();

        let base = unit.base_units();

        assert_eq!(base, "");
    }

    #[test]
    fn conformable_does_not_contain_wrong_domain() {
        let result = conformable("km").unwrap();

        assert!(!result.contains(&"kg".to_owned()));
        assert!(!result.contains(&"s".to_owned()));
    }

    #[rstest]
    #[case::en_dash('\u{2013}', "-")]
    #[case::minus_sign('\u{2212}', "-")]
    #[case::times_sign('\u{00D7}', "*")]
    #[case::middle_dot('\u{00B7}', "*")]
    #[case::division_sign('\u{00F7}', "/")]
    #[case::fraction_slash('\u{2044}', "|")]
    #[case::nbsp('\u{00A0}', " ")]
    #[case::zero_width_space('\u{200B}', "")]
    fn replace_operators_cases(#[case] input: char, #[case] expected: &str) {
        let result = replace_operators(&input.to_string());

        assert_eq!(result, expected);
    }

    #[test]
    fn root_even_negative_is_error() {
        let mut unit = Unit::parse("-4").unwrap();

        let result = unit.root(2);

        assert!(result.is_err(), "even root of negative should fail");
    }

    #[test]
    fn root_odd_negative_is_error() {
        let mut unit = Unit::parse("-8").unwrap();

        let result = unit.root(3);

        assert!(
            result.is_err(),
            "odd root of negative should fail (matching C behavior)"
        );
    }

    #[rstest]
    #[case::gasmark1("gasmark(1)", "degR", 734.67, 0.1)]
    #[case::gasmark5("gasmark(5)", "degR", 834.67, 0.1)]
    #[case::gasmark10("gasmark(10)", "degR", 959.67, 0.1)]
    fn table_parse_factor(
        #[case] input: &str,
        #[case] to: &str,
        #[case] expected_factor_in_degr: f64,
        #[case] tol: f64,
    ) {
        let result = convert(input, to);

        assert!(
            result.is_ok(),
            "convert({input:?}, {to:?}) failed: {:?}",
            result.err()
        );
        let factor = result.unwrap();
        assert!(
            (factor - expected_factor_in_degr).abs() < tol,
            "convert({input:?}, {to:?}) = {factor}, expected {expected_factor_in_degr}\u{00b1}{tol}"
        );
    }

    #[rstest]
    #[case::tempc_inverse("~tempC(0 K)", -273.15, 0.01)]
    fn inverse_function_parses(#[case] input: &str, #[case] expected: f64, #[case] tol: f64) {
        let unit = Unit::parse(input);

        assert!(unit.is_ok(), "parse({input:?}) failed: {:?}", unit.err());
        let factor = unit.unwrap().factor();
        assert!(
            (factor - expected).abs() < tol,
            "parse({input:?}).factor() = {factor}, expected {expected}\u{00b1}{tol}"
        );
    }

    #[rstest]
    #[case::tempc_0("tempC(0)", 273.15, 0.01)]
    #[case::tempc_100("tempC(100)", 373.15, 0.01)]
    #[case::tempf_32("tempF(32)", 273.15, 0.01)]
    #[case::tempf_212("tempF(212)", 373.15, 0.01)]
    fn forward_function_to_kelvin(
        #[case] input: &str,
        #[case] expected_kelvin: f64,
        #[case] tol: f64,
    ) {
        let result = convert(input, "K");

        assert!(
            result.is_ok(),
            "convert({input:?}, \"K\") failed: {:?}",
            result.err()
        );
        let factor = result.unwrap();
        assert!(
            (factor - expected_kelvin).abs() < tol,
            "convert({input:?}, \"K\") = {factor}, expected {expected_kelvin}\u{00b1}{tol}"
        );
    }

    #[rstest]
    #[case::kilogram("kilogram", "gram", 1000.0, 1e-9)]
    #[case::milligram("milligram", "gram", 0.001, 1e-9)]
    #[case::megabyte("megabyte", "byte", 1e6, 1e-3)]
    #[case::microsecond("microsecond", "second", 1e-6, 1e-15)]
    fn prefix_resolution(
        #[case] from: &str,
        #[case] to: &str,
        #[case] expected: f64,
        #[case] tol: f64,
    ) {
        let result = convert(from, to);

        assert!(
            result.is_ok(),
            "convert({from:?}, {to:?}) failed: {:?}",
            result.err()
        );
        let factor = result.unwrap();
        assert!(
            (factor - expected).abs() < tol,
            "convert({from:?}, {to:?}) = {factor}, expected {expected}\u{00b1}{tol}"
        );
    }

    #[rstest]
    #[case::reciprocal("1|2", 0.5)]
    #[case::fraction_5_9("5|9", 5.0 / 9.0)]
    #[case::numdiv_3_8("3|8", 0.375)]
    fn pipe_factor(#[case] input: &str, #[case] expected: f64) {
        let unit = Unit::parse(input).unwrap();

        assert!(
            (unit.factor() - expected).abs() < 1e-12,
            "{input} factor = {}, expected {expected}",
            unit.factor()
        );
    }

    #[test]
    fn list_definitions_returns_independent_copy() {
        let mut first = list_definitions();
        let original_len = first.len();
        first.clear();

        let second = list_definitions();

        assert_eq!(second.len(), original_len);
    }

    /// `m^(1|3)` must fail: the native engine uses integer dimension exponents,
    /// so root-3 of `m^1` is not representable (1 % 3 != 0).
    /// GAP: to support fractional-power dimensional units, the engine would need
    /// rational exponents in `UnitValue::dimensions`.
    #[test]
    fn rational_exponent_one_third() {
        ensure_definitions();

        let result = Unit::parse("m^(1|3)");

        assert!(
            result.is_err(),
            "m^(1|3) should fail (integer-only dimension exponents)"
        );
    }

    #[rstest]
    #[case::fractional_exponent("8^(1|3)", 2.0)]
    #[case::asin("asin(1)", std::f64::consts::FRAC_PI_2)]
    #[case::cuberoot("cuberoot(27)", 3.0)]
    #[case::round("round(3.7)", 4.0)]
    fn builtin_factor(#[case] input: &str, #[case] expected: f64) {
        ensure_definitions();

        let v = Unit::parse(input).unwrap();

        assert!(
            (v.factor() - expected).abs() < 1e-9,
            "{input} factor = {}, expected {expected}",
            v.factor()
        );
    }

    /// `3|8 m`: the `|` consumes only its two adjacent `power` operands (both
    /// dimensionless), then `m` is juxtaposed onto the 0.375 result.
    #[test]
    fn pipe_numdiv_with_unit() {
        ensure_definitions();

        let v = Unit::parse("3|8 m").unwrap();

        assert!(
            (v.factor() - 0.375).abs() < 1e-12,
            "3|8 m factor = {}, expected 0.375",
            v.factor()
        );
        assert!(
            v.base_units().contains('m'),
            "3|8 m base_units = '{}', expected it to contain 'm'",
            v.base_units()
        );
    }

    /// `per meter` is equivalent to `1/m`; converting between them must give 1.
    #[test]
    fn per_keyword_as_division() {
        ensure_definitions();

        let result = convert("per meter", "1/m");

        assert!(
            result.is_ok(),
            "convert(\"per meter\", \"1/m\") failed: {:?}",
            result.err()
        );
        assert!(
            (result.unwrap() - 1.0).abs() < 1e-9,
            "convert(\"per meter\", \"1/m\") != 1.0"
        );
    }

    #[test]
    fn add_incompatible_dimensions_returns_badsum() {
        let result = convert("m + kg", "m");

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), ErrorCode::BadSum);
    }

    #[cfg(feature = "native")]
    #[test]
    fn non_divisible_root_returns_not_root() {
        ensure_definitions();

        let result = Unit::parse("m^(1|3)");

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), ErrorCode::NotRoot);
    }

    #[cfg(feature = "native")]
    #[test]
    fn table_dimension_mismatch_returns_bad_func_arg() {
        ensure_definitions();

        let result = convert("5 m", "gasmark");

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), ErrorCode::BadFuncArg);
    }

    #[rstest]
    #[case::error_on_ln_of_zero("ln(0)")]
    #[case::error_on_ln_of_negative("ln(-1)")]
    #[case::error_on_log_of_zero("log(0)")]
    #[case::error_on_log_of_negative("log(-1)")]
    #[case::error_on_log2_of_zero("log2(0)")]
    fn error_on_builtin_domain(#[case] expr: &str) {
        let result = convert(expr, "1");

        assert!(result.is_err(), "convert({expr:?}, \"1\") should fail");
    }

    #[rstest]
    #[case::error_on_to_number_dimensional("m", ErrorCode::NotANumber)]
    #[case::error_on_to_number_compound("kg m/s^2", ErrorCode::NotANumber)]
    fn error_on_to_number(#[case] input: &str, #[case] expected_code: ErrorCode) {
        let unit = Unit::parse(input).unwrap();

        let result = unit.to_number();

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), expected_code);
    }

    #[test]
    fn error_on_add_incompatible_dimensions() {
        let mut a = Unit::parse("m").unwrap();
        let b = Unit::parse("kg").unwrap();

        let result = a.add(b);

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), ErrorCode::BadSum);
    }

    #[test]
    fn error_on_convert_to_incompatible_units() {
        let m = Unit::parse("m").unwrap();
        let kg = Unit::parse("kg").unwrap();

        let result = m.convert_to(kg);

        assert!(result.is_err());
    }

    #[rstest]
    #[case::division_by_zero("1/0", f64::INFINITY)]
    #[case::deeply_nested("(((((((((( 1 ))))))))))", 1.0)]
    fn parse_does_not_panic(#[case] input: &str, #[case] expected: f64) {
        let result = Unit::parse(input);

        assert!(result.is_ok(), "parse({input:?}) should not panic or fail");
        assert_eq!(result.unwrap().factor(), expected);
    }
}
