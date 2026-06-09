//! Pure-Rust unit engine — module declarations and re-exports.
//!
//! Exposes the uniform engine API that mirrors `engine/ffi.rs`, allowing
//! `engine/mod.rs` to dispatch to either backend transparently via `#[cfg]`.

pub(crate) mod database;
pub(crate) mod ops;
pub(crate) mod parser;
pub(crate) mod types;

pub(crate) use self::ops::{
    RawUnit, convert_func, unit_add, unit_base_units, unit_clone, unit_divide, unit_drop,
    unit_factor, unit_invert, unit_is_conformable, unit_multiply, unit_new, unit_parse, unit_pow,
    unit_root, unit_to_number,
};
