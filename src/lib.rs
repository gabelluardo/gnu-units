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
#[allow(dead_code)]
pub(crate) mod error_codes {
    pub const E_NORMAL: i32 = 0;
    pub const E_PARSE: i32 = 1;
    pub const E_BADSUM: i32 = 5;
    pub const E_NOTANUMBER: i32 = 6;
    pub const E_NOTROOT: i32 = 7;
    pub const E_UNKNOWNUNIT: i32 = 8;
    pub const E_BADNUM: i32 = 20;
}

/// Wraps a raw error code returned by the units engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitsError {
    /// Raw error code as defined in [`error_codes`].
    pub code: i32,
}

impl UnitsError {
    /// Returns `true` when the error indicates a dimensionless reduction failed.
    pub fn is_not_dimensionless(&self) -> bool {
        self.code == error_codes::E_NOTANUMBER
    }

    /// Returns `true` when the expression was invalid (unknown/unparseable).
    pub fn is_invalid_unit(&self) -> bool {
        self.code == error_codes::E_UNKNOWNUNIT || self.code == error_codes::E_PARSE
    }
}

impl fmt::Display for UnitsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GNU units error code {}", self.code)
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
pub fn convert(from: &str, to: &str) -> Result<f64> {
    ensure_definitions();
    if let Some(factor) = engine::convert_func(from, to) {
        return Ok(factor);
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

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::definitions::replace_operators;

    #[test]
    fn units_error_display() {
        let err = UnitsError { code: 42 };

        let s = format!("{err}");

        assert!(s.contains("42"));
    }

    #[rstest]
    #[case::eq_same_code(1, 1, true)]
    #[case::ne_different_code(1, 2, false)]
    fn units_error_eq_semantics(#[case] a: i32, #[case] b: i32, #[case] equal: bool) {
        let ea = UnitsError { code: a };
        let eb = UnitsError { code: b };

        assert_eq!(ea == eb, equal);
    }

    #[test]
    fn units_error_copy_semantics() {
        let original = UnitsError { code: 5 };

        let copy = original;

        assert_eq!(copy, original);
    }

    #[rstest]
    #[case::notanumber(error_codes::E_NOTANUMBER, true)]
    #[case::e_parse(error_codes::E_PARSE, false)]
    #[case::e_badsum(error_codes::E_BADSUM, false)]
    fn is_not_dimensionless(#[case] code: i32, #[case] expected: bool) {
        let err = UnitsError { code };

        assert_eq!(err.is_not_dimensionless(), expected);
    }

    #[rstest]
    #[case::e_parse(error_codes::E_PARSE, true)]
    #[case::e_unknownunit(error_codes::E_UNKNOWNUNIT, true)]
    #[case::e_notanumber(error_codes::E_NOTANUMBER, false)]
    fn is_invalid_unit(#[case] code: i32, #[case] expected: bool) {
        let err = UnitsError { code };

        assert_eq!(err.is_invalid_unit(), expected);
    }

    #[rstest]
    #[case::via_new(Unit::new())]
    #[case::via_default(Unit::default())]
    fn initial_factor_is_one(#[case] unit: Unit) {
        assert_eq!(unit.factor(), 1.0);
    }

    #[rstest]
    #[case::integer("5", 5.0)]
    #[case::float("3.15", 3.15)]
    #[case::scientific("1e10", 1e10)]
    fn parse_numeric(#[case] input: &str, #[case] expected: f64) {
        let result = Unit::parse(input);

        assert!(result.is_ok());
        assert!((result.unwrap().factor() - expected).abs() < 1e-6);
    }

    #[rstest]
    #[case::null_byte("\0")]
    #[case::close_paren(")")]
    fn parse_error(#[case] input: &str) {
        let result = Unit::parse(input);

        assert!(result.is_err());
    }

    #[test]
    fn clone_preserves_factor() {
        let original = Unit::parse("5").unwrap();

        let cloned = original.clone();

        assert_eq!(cloned.factor(), original.factor());
    }

    #[test]
    fn multiply_five_by_three() {
        let mut a = Unit::parse("5").unwrap();
        let b = Unit::parse("3").unwrap();

        a.multiply(b).unwrap();

        assert_eq!(a.factor(), 15.0);
    }

    #[test]
    fn divide_ten_by_two() {
        let mut a = Unit::parse("10").unwrap();
        let b = Unit::parse("2").unwrap();

        a.divide(b).unwrap();

        assert_eq!(a.factor(), 5.0);
    }

    #[test]
    fn add_three_and_seven() {
        let mut a = Unit::parse("3").unwrap();
        let b = Unit::parse("7").unwrap();

        a.add(b).unwrap();

        assert_eq!(a.factor(), 10.0);
    }

    #[test]
    fn invert_five_is_point_two() {
        let mut a = Unit::parse("5").unwrap();

        a.invert();

        assert!((a.factor() - 0.2).abs() < 1e-12);
    }

    #[test]
    fn pow_three_squared_is_nine() {
        let mut a = Unit::parse("3").unwrap();

        a.pow(2).unwrap();

        assert_eq!(a.factor(), 9.0);
    }

    #[test]
    fn root_sqrt_nine_is_three() {
        let mut a = Unit::parse("9").unwrap();

        a.root(2).unwrap();

        assert!((a.factor() - 3.0).abs() < 1e-12);
    }

    #[test]
    fn pow_error() {
        let mut a = Unit::parse("3").unwrap();

        let result = a.pow(-1);

        assert!(result.is_err());
    }

    #[rstest]
    #[case::zero(0)]
    #[case::negative(-1)]
    fn root_error(#[case] n: i32) {
        let mut a = Unit::parse("9").unwrap();

        let result = a.root(n);

        assert!(result.is_err());
    }

    #[test]
    fn to_number_returns_factor() {
        let unit = Unit::parse("42").unwrap();

        let result = unit.to_number();

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42.0);
    }

    #[rstest]
    #[case::km_to_m("km", "m", 1000.0, 1e-9)]
    #[case::five_km_to_miles("5 km", "miles", 3.107, 0.001)]
    #[case::m_to_m("m", "m", 1.0, 1e-12)]
    fn convert_to_compatible_units(
        #[case] from: &str,
        #[case] to: &str,
        #[case] expected: f64,
        #[case] tol: f64,
    ) {
        let result = convert(from, to);

        assert!(result.is_ok(), "convert({from:?}, {to:?}) should succeed");
        let factor = result.unwrap();
        assert!(
            (factor - expected).abs() < tol,
            "got {factor}, expected {expected}\u{00b1}{tol}"
        );
    }

    #[test]
    fn error_on_convert_to_incompatible_dimensions() {
        let result = convert("km", "kg");

        assert!(result.is_err());
    }

    #[rstest]
    #[case::bad_from("invalidUnitXYZ999", "m")]
    #[case::bad_to("m", "invalidUnitXYZ999")]
    #[case::incompatible("km", "kg")]
    fn convert_error(#[case] from: &str, #[case] to: &str) {
        let result = convert(from, to);

        assert!(result.is_err());
    }

    #[test]
    fn list_definitions_is_not_empty() {
        let defs = list_definitions();

        assert!(defs.len() > 1000);
    }

    #[test]
    fn list_definitions_is_sorted_alphabetically() {
        let defs = list_definitions();

        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();

        assert_eq!(names, sorted);
    }

    #[test]
    fn all_definitions_have_non_empty_names() {
        let defs = list_definitions();

        for d in defs.iter() {
            assert!(!d.name.is_empty(), "found a definition with an empty name");
        }
    }

    #[rstest]
    #[case::m("m")]
    #[case::meter("meter")]
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

    #[rstest]
    #[case::km_miles("km", "miles", true)]
    #[case::m_kg("m", "kg", false)]
    fn is_conformable(#[case] a: &str, #[case] b: &str, #[case] expected: bool) {
        let ua = Unit::parse(a).unwrap();
        let ub = Unit::parse(b).unwrap();

        assert_eq!(ua.is_conformable(&ub), expected);
    }

    #[rstest]
    #[case::m("km", "m")]
    #[case::mile("km", "mile")]
    fn conformable_contains_expected_unit(#[case] expr: &str, #[case] should_contain: &str) {
        let result = conformable(expr).unwrap();

        assert!(
            result.contains(&should_contain.to_owned()),
            "conformable('{expr}') should contain '{should_contain}'"
        );
    }

    #[test]
    fn conformable_does_not_contain_wrong_domain() {
        let result = conformable("km").unwrap();

        assert!(!result.contains(&"kg".to_owned()));
        assert!(!result.contains(&"s".to_owned()));
    }

    #[test]
    fn error_on_conformable_invalid_expression() {
        let result = conformable("invalidUnitXYZ999");

        assert!(result.is_err());
    }

    #[rstest]
    #[case::kg_to_g("kilogram", "gram", 1000.0, 1e-9)]
    #[case::inch_to_cm("inch", "cm", 2.54, 1e-9)]
    #[case::minute_to_s("minute", "s", 60.0, 1e-9)]
    fn definitions_convert(
        #[case] from: &str,
        #[case] to: &str,
        #[case] expected: f64,
        #[case] tol: f64,
    ) {
        let result = convert(from, to);

        assert!(result.is_ok(), "convert({from:?}, {to:?}) should succeed");
        let factor = result.unwrap();
        assert!(
            (factor - expected).abs() < tol,
            "got {factor}, expected {expected}\u{00b1}{tol}"
        );
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

    #[rstest]
    #[case::zero_celsius("273.15 K", "tempC", 0.0, 1e-6)]
    #[case::boiling_celsius("373.15 K", "tempC", 100.0, 1e-6)]
    #[case::freezing_fahrenheit("273.15 K", "tempF", 32.0, 1e-4)]
    fn convert_via_function(
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
    #[case::zero_c("273.15 K", "tempC", 0.0, 1e-6)]
    #[case::hundred_c("373.15 K", "tempC", 100.0, 1e-6)]
    #[case::freezing_f("273.15 K", "tempF", 32.0, 0.1)]
    #[case::boiling_f("373.15 K", "tempF", 212.0, 0.1)]
    fn temperature_convert(
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

    #[test]
    fn pipe_operator_reciprocal() {
        let unit = Unit::parse("1|2").unwrap();

        assert!((unit.factor() - 0.5).abs() < 1e-12);
    }

    #[test]
    fn pipe_operator_fraction() {
        let unit = Unit::parse("5|9").unwrap();

        assert!((unit.factor() - (5.0 / 9.0)).abs() < 1e-12);
    }

    #[test]
    fn list_definitions_returns_independent_copy() {
        let mut first = list_definitions();
        let original_len = first.len();
        first.clear();

        let second = list_definitions();

        assert_eq!(second.len(), original_len);
    }
}
