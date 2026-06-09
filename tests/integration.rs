use gnu_units::{ErrorCode, Unit, conformable, convert, list_definitions};
use rstest::rstest;

#[rstest]
#[case::integer("5", 5.0)]
#[case::float("3.15", 3.15)]
#[case::scientific("1e10", 1e10)]
fn parse_numeric_factor(#[case] input: &str, #[case] expected: f64) {
    let result = Unit::parse(input);

    assert!(result.is_ok(), "parse({input:?}) should succeed");
    assert!((result.unwrap().factor() - expected).abs() < 1e-6);
}

#[rstest]
#[case::km("km")]
#[case::m("m")]
#[case::kg("kg")]
fn parse_named_unit(#[case] input: &str) {
    let result = Unit::parse(input);

    assert!(result.is_ok(), "parse({input:?}) should succeed");
    assert!(
        result.unwrap().factor() > 0.0,
        "factor for {input:?} should be positive"
    );
}

#[rstest]
#[case::error_on_null_byte("\0")]
#[case::error_on_close_paren(")")]
fn error_on_invalid_parse(#[case] input: &str) {
    let result = Unit::parse(input);

    assert!(result.is_err(), "parse({input:?}) should fail");
}

#[rstest]
#[case::km_to_m("km", "m", 1000.0, 1e-9)]
#[case::inch_to_cm("inch", "cm", 2.54, 1e-9)]
#[case::minute_to_s("minute", "s", 60.0, 1e-9)]
#[case::five_km_to_mile("5 km", "mile", 3.107, 0.001)]
fn convert_compatible_units(
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
        "got {factor}, expected {expected}±{tol}"
    );
}

#[rstest]
#[case::error_on_incompatible_km_kg("km", "kg")]
#[case::error_on_bad_from("invalidUnitXYZ999", "m")]
#[case::error_on_bad_to("m", "invalidUnitXYZ999")]
fn error_on_convert_incompatible(#[case] from: &str, #[case] to: &str) {
    let result = convert(from, to);

    assert!(result.is_err(), "convert({from:?}, {to:?}) should fail");
}

#[rstest]
#[case::freezing_point_celsius("273.15 K", "tempC", 0.0, 1e-6)]
#[case::boiling_point_celsius("373.15 K", "tempC", 100.0, 1e-6)]
#[case::freezing_point_fahrenheit("273.15 K", "tempF", 32.0, 1e-6)]
fn convert_temperature(
    #[case] from: &str,
    #[case] to: &str,
    #[case] expected: f64,
    #[case] tol: f64,
) {
    let result = convert(from, to);

    assert!(result.is_ok(), "convert({from:?}, {to:?}) should succeed");
    let value = result.unwrap();
    assert!(
        (value - expected).abs() < tol,
        "got {value}, expected {expected}±{tol}"
    );
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
fn invert_five() {
    let mut a = Unit::parse("5").unwrap();

    a.invert();

    assert!((a.factor() - 0.2).abs() < 1e-12);
}

#[test]
fn pow_three_squared() {
    let mut a = Unit::parse("3").unwrap();

    a.pow(2).unwrap();

    assert_eq!(a.factor(), 9.0);
}

#[test]
fn root_sqrt_nine() {
    let mut a = Unit::parse("9").unwrap();

    a.root(2).unwrap();

    assert!((a.factor() - 3.0).abs() < 1e-12);
}

#[test]
fn error_on_negative_pow() {
    let mut a = Unit::parse("3").unwrap();

    let result = a.pow(-1);

    assert!(result.is_err());
}

#[rstest]
#[case::error_on_zero(0)]
#[case::error_on_negative(-1)]
fn error_on_invalid_root(#[case] n: i32) {
    let mut a = Unit::parse("9").unwrap();

    let result = a.root(n);

    assert!(result.is_err());
}

#[test]
fn km_is_conformable_with_mile() {
    let km = Unit::parse("km").unwrap();
    let miles = Unit::parse("mile").unwrap();

    assert!(km.is_conformable(&miles));
}

#[test]
fn m_is_not_conformable_with_kg() {
    let m = Unit::parse("m").unwrap();
    let kg = Unit::parse("kg").unwrap();

    assert!(!m.is_conformable(&kg));
}

#[test]
fn conformable_km_contains_m_and_mile() {
    let result = conformable("km").unwrap();

    assert!(
        result.contains(&"m".to_owned()),
        "expected 'm' in conformable(\"km\")"
    );
    assert!(
        result.contains(&"mile".to_owned()),
        "expected 'mile' in conformable(\"km\")"
    );
}

#[test]
fn error_on_conformable_invalid_expr() {
    let result = conformable("invalidUnitXYZ999");

    assert!(result.is_err());
}

#[test]
fn list_definitions_not_empty() {
    let defs = list_definitions();

    assert!(defs.len() > 1000);
}

#[test]
fn list_definitions_sorted_alphabetically() {
    let defs = list_definitions();

    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
}

#[rstest]
#[case::m("m")]
#[case::meter("meter")]
fn list_definitions_contains_known_entry(#[case] name: &str) {
    let defs = list_definitions();

    assert!(
        defs.iter().any(|d| d.name == name),
        "expected '{name}' in definitions"
    );
}

#[test]
fn convert_to_km_to_m() {
    let km = Unit::parse("km").unwrap();
    let m = Unit::parse("m").unwrap();

    let result = km.convert_to(m);

    assert!(result.is_ok());
    assert!((result.unwrap() - 1000.0).abs() < 1e-9);
}

#[test]
fn base_units_of_m_contains_m() {
    let unit = Unit::parse("m").unwrap();

    let result = unit.base_units();

    assert!(
        result.contains('m'),
        "base_units of 'm' should contain 'm', got {result:?}"
    );
}

#[test]
fn base_units_of_dimensionless_is_empty() {
    let unit = Unit::parse("42").unwrap();

    let result = unit.base_units();

    assert_eq!(result, "");
}

#[rstest]
#[case::half("1|2", 0.5)]
#[case::three_eighths("3|8", 0.375)]
fn parse_pipe_operator(#[case] input: &str, #[case] expected: f64) {
    let unit = Unit::parse(input).unwrap();

    assert!((unit.factor() - expected).abs() < 1e-12);
}

#[test]
fn per_keyword_inverts() {
    let result = convert("per meter", "1/m");

    assert!(result.is_ok());
    assert!((result.unwrap() - 1.0).abs() < 1e-9);
}

#[test]
fn parse_hex_literal() {
    let unit = Unit::parse("0xff").unwrap();

    assert_eq!(unit.factor(), 255.0);
}

#[test]
fn parse_malformed_hex_is_zero() {
    let unit = Unit::parse("0xGG").unwrap();

    assert_eq!(unit.factor(), 0.0);
}

#[test]
fn error_on_unary_plus() {
    let result = Unit::parse("+5");

    assert!(result.is_err());
}

#[rstest]
#[case::sqrt("sqrt(9)", 3.0)]
#[case::cuberoot("cuberoot(27)", 3.0)]
#[case::abs("abs(-5)", 5.0)]
fn parse_builtin_function(#[case] input: &str, #[case] expected: f64) {
    let unit = Unit::parse(input).unwrap();

    assert!((unit.factor() - expected).abs() < 1e-9);
}

#[rstest]
#[case::tempc_0("tempC(0)", "K", 273.15, 0.01)]
#[case::tempf_212("tempF(212)", "K", 373.15, 0.01)]
fn forward_function_convert(
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
    assert!((result.unwrap() - expected).abs() < tol);
}

#[test]
fn inverse_function_tilde() {
    let unit = Unit::parse("~tempC(0 K)").unwrap();

    assert!((unit.factor() - (-273.15)).abs() < 0.01);
}

#[test]
fn table_lookup_gasmark() {
    let result = convert("gasmark(5)", "degR");

    assert!(result.is_ok());
    assert!((result.unwrap() - 834.67).abs() < 0.1);
}

#[rstest]
#[case::kilo("kilogram", "gram", 1000.0)]
#[case::milli("milligram", "gram", 0.001)]
#[case::micro("microsecond", "second", 1e-6)]
fn prefix_resolution(#[case] from: &str, #[case] to: &str, #[case] expected: f64) {
    let result = convert(from, to);

    assert!(result.is_ok());
    assert!((result.unwrap() - expected).abs() < expected.abs() * 1e-9);
}

#[test]
fn fractional_exponent_dimensionless() {
    let unit = Unit::parse("8^(1|3)").unwrap();

    assert!((unit.factor() - 2.0).abs() < 1e-9);
}

#[cfg(feature = "native")]
#[test]
fn error_code_on_unknown_unit() {
    let err = Unit::parse("invalidUnitXYZ999").err().unwrap();

    assert_eq!(err.code(), ErrorCode::UnknownUnit);
}

#[test]
fn error_code_on_incompatible_add() {
    let result = convert("m + kg", "m");

    assert_eq!(result.unwrap_err().code(), ErrorCode::BadSum);
}
