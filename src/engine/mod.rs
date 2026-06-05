//! Backend dispatcher — provides a uniform engine API regardless of backend.

#[cfg(feature = "vendored")]
pub(crate) mod ffi;
#[cfg(not(feature = "vendored"))]
pub(crate) mod native;

// Re-export the active backend's items under a uniform name.
#[cfg(feature = "vendored")]
use self::ffi as backend;
#[cfg(not(feature = "vendored"))]
use self::native as backend;

pub(crate) use backend::RawUnit;
pub(crate) use backend::convert_func;
pub(crate) use backend::unit_add;
pub(crate) use backend::unit_base_units;
pub(crate) use backend::unit_clone;
pub(crate) use backend::unit_divide;
pub(crate) use backend::unit_drop;
pub(crate) use backend::unit_factor;
pub(crate) use backend::unit_invert;
pub(crate) use backend::unit_is_conformable;
pub(crate) use backend::unit_multiply;
pub(crate) use backend::unit_new;
pub(crate) use backend::unit_parse;
pub(crate) use backend::unit_pow;
pub(crate) use backend::unit_root;
pub(crate) use backend::unit_to_number;
