//! Unit operations for the pure-Rust native engine.
//!
//! Implements the uniform engine API consumed by `engine/mod.rs`, translating
//! between [`UnitValue`] internals and the crate-level error types.

use std::collections::HashMap;

use crate::UnitsError;
use crate::error_codes as E;

use super::database;
use super::parser::{ParseError, parseunit as parser_parseunit, parseunit_with_vars};
use super::types::UnitValue;

pub(crate) type RawUnit = UnitValue;

fn parse_err() -> UnitsError {
    UnitsError { code: E::E_PARSE }
}

fn unknown_unit_err() -> UnitsError {
    UnitsError {
        code: E::E_UNKNOWNUNIT,
    }
}

fn not_a_number_err() -> UnitsError {
    UnitsError {
        code: E::E_NOTANUMBER,
    }
}

fn bad_sum_err() -> UnitsError {
    UnitsError { code: E::E_BADSUM }
}

fn not_root_err() -> UnitsError {
    UnitsError { code: E::E_NOTROOT }
}

fn map_parse(e: ParseError) -> UnitsError {
    if e.0.contains("unknown unit") {
        return unknown_unit_err();
    }

    parse_err()
}

pub(crate) fn unit_new() -> RawUnit {
    UnitValue::one()
}

pub(crate) fn unit_parse(input: &str) -> crate::Result<RawUnit> {
    parser_parseunit(input).map_err(map_parse)
}

pub(crate) fn unit_factor(unit: &RawUnit) -> f64 {
    unit.factor
}

pub(crate) fn unit_base_units(unit: &RawUnit) -> String {
    unit.base_units_string()
}

pub(crate) fn unit_multiply(lhs: &mut RawUnit, rhs: &RawUnit) -> crate::Result<()> {
    lhs.multiply_assign(rhs);
    Ok(())
}

pub(crate) fn unit_divide(lhs: &mut RawUnit, rhs: &RawUnit) -> crate::Result<()> {
    lhs.divide_assign(rhs);
    Ok(())
}

pub(crate) fn unit_add(lhs: &mut RawUnit, rhs: &RawUnit) -> crate::Result<()> {
    if !lhs.add_assign(rhs) {
        return Err(bad_sum_err());
    }
    Ok(())
}

pub(crate) fn unit_invert(unit: &mut RawUnit) {
    unit.invert();
}

pub(crate) fn unit_pow(unit: &mut RawUnit, power: i32) -> crate::Result<()> {
    if power < 0 {
        return Err(UnitsError { code: E::E_BADNUM });
    }
    unit.pow_assign(power);
    Ok(())
}

pub(crate) fn unit_root(unit: &mut RawUnit, n: i32) -> crate::Result<()> {
    if n <= 0 {
        return Err(not_root_err());
    }
    if !unit.root_assign(n) {
        return Err(not_root_err());
    }
    Ok(())
}

pub(crate) fn unit_to_number(unit: &RawUnit) -> crate::Result<f64> {
    if !unit.is_dimensionless() {
        return Err(not_a_number_err());
    }
    Ok(unit.factor)
}

pub(crate) fn unit_is_conformable(a: &RawUnit, b: &RawUnit) -> bool {
    let mut ratio = a.clone();
    ratio.divide_assign(b);
    ratio.is_dimensionless()
}

pub(crate) fn unit_clone(src: &RawUnit) -> RawUnit {
    src.clone()
}

#[inline]
pub(crate) fn unit_drop(_unit: &mut RawUnit) {}

pub(crate) fn convert_func(from: &str, to: &str) -> Option<f64> {
    enum FuncLookup {
        Function(String),
        Table(String),
    }

    let from_val = parser_parseunit(from).ok()?;

    let lookup = {
        let db = database::read();
        if let Some(func) = db.functions.get(to) {
            func.reverse
                .as_ref()
                .map(|r| FuncLookup::Function(r.clone()))
        } else {
            db.tables
                .get(to)
                .map(|table| FuncLookup::Table(table.unit.clone()))
        }
    };

    match lookup? {
        FuncLookup::Function(reverse) => {
            let vars = HashMap::from([(to.to_owned(), from_val)]);
            let result = parseunit_with_vars(&reverse, &vars).ok()?;
            Some(result.factor)
        }
        FuncLookup::Table(unit_expr) => {
            let unit_val = parser_parseunit(&unit_expr).ok()?;
            let mut ratio = from_val;
            ratio.divide_assign(&unit_val);
            if !ratio.is_dimensionless() {
                return None;
            }
            let db = database::read();
            let table = db.tables.get(to)?;
            table.reverse_interpolate(ratio.factor)
        }
    }
}
