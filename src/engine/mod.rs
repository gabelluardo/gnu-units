//! Backend dispatcher — provides a uniform engine API regardless of backend.

#[cfg(all(feature = "native", feature = "vendored"))]
compile_error!("Features `native` and `vendored` are mutually exclusive");

#[cfg(not(any(feature = "native", feature = "vendored")))]
compile_error!("Enable either the `native` or `vendored` feature");

#[cfg(feature = "vendored")]
pub(crate) mod ffi;
#[cfg(feature = "native")]
pub(crate) mod native;

// Re-export the active backend's items under a uniform name.
#[cfg(feature = "vendored")]
use self::ffi as backend;
#[cfg(feature = "native")]
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
