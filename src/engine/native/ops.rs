//! Unit operations for the pure-Rust native engine.
//!
//! Implements the uniform engine API consumed by `engine/mod.rs`, translating
//! between [`UnitValue`] internals and the crate-level error types.

use std::collections::HashMap;

use crate::ErrorCode;
use crate::UnitsError;

use super::database;
use super::parser::{ParseError, parseunit as parser_parseunit, parseunit_with_vars};
use super::types::UnitValue;

pub(crate) type RawUnit = UnitValue;

fn map_parse(e: ParseError) -> UnitsError {
    match e {
        ParseError::UnknownUnit(_) => UnitsError(ErrorCode::UnknownUnit),
        ParseError::NotInDomain { .. } => UnitsError(ErrorCode::NotInDomain),
        ParseError::WrongDimensions { .. } => UnitsError(ErrorCode::BadFuncArg),
        ParseError::IrrationalExponent => UnitsError(ErrorCode::IrrationalExponent),
        ParseError::NotDivisibleByRoot => UnitsError(ErrorCode::NotRoot),
        ParseError::IncompatibleDimensions(_) => UnitsError(ErrorCode::BadSum),
        _ => UnitsError(ErrorCode::Parse),
    }
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
        return Err(UnitsError(ErrorCode::BadSum));
    }
    Ok(())
}

pub(crate) fn unit_invert(unit: &mut RawUnit) {
    unit.invert();
}

pub(crate) fn unit_pow(unit: &mut RawUnit, power: i32) -> crate::Result<()> {
    if power < 0 {
        return Err(UnitsError(ErrorCode::BadNum));
    }
    unit.pow_assign(power);
    Ok(())
}

pub(crate) fn unit_root(unit: &mut RawUnit, n: i32) -> crate::Result<()> {
    if n <= 0 {
        return Err(UnitsError(ErrorCode::NotRoot));
    }
    if !unit.root_assign(n) {
        return Err(UnitsError(ErrorCode::NotRoot));
    }
    Ok(())
}

pub(crate) fn unit_to_number(unit: &RawUnit) -> crate::Result<f64> {
    if !unit.is_dimensionless() {
        return Err(UnitsError(ErrorCode::NotANumber));
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

pub(crate) fn convert_func(from: &str, to: &str) -> crate::Result<f64> {
    enum FuncLookup {
        Function(String),
        Table(String),
    }

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

    let lookup = lookup.ok_or(UnitsError(ErrorCode::NotAFunc))?;

    let from_val = match parser_parseunit(from) {
        Ok(v) => v,
        Err(_) => return Err(UnitsError(ErrorCode::NotAFunc)),
    };

    match lookup {
        FuncLookup::Function(reverse) => {
            let vars = HashMap::from([(to.to_owned(), from_val)]);
            parseunit_with_vars(&reverse, &vars)
                .map(|v| v.factor)
                .map_err(map_parse)
        }
        FuncLookup::Table(unit_expr) => {
            let unit_val = match parser_parseunit(&unit_expr) {
                Ok(v) => v,
                Err(e) => return Err(map_parse(e)),
            };
            let mut ratio = from_val;
            ratio.divide_assign(&unit_val);
            if !ratio.is_dimensionless() {
                return Err(UnitsError(ErrorCode::BadFuncArg));
            }
            let db = database::read();
            let table = db.tables.get(to).ok_or(UnitsError(ErrorCode::NotAFunc))?;
            table
                .reverse_interpolate(ratio.factor)
                .ok_or(UnitsError(ErrorCode::NotInDomain))
        }
    }
}
