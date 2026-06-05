use gnu_units::{Unit, conformable, convert, list_definitions};
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
