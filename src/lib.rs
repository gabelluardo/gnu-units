//! Safe, high-level Rust interface to GNU Units.
//!
//! Provides unit parsing, conversion, and definition listing backed by the
//! vendored C library exposed through [`gnu_units_sys`].

use std::ffi::CStr;
use std::fmt;
use std::mem::MaybeUninit;
use std::os::raw::c_int;

pub use gnu_units_sys;

#[cfg(feature = "currency-update")]
pub mod currency_update;

mod definitions;
mod ffi;
mod units;

#[cfg(feature = "currency-update")]
use definitions::load_definitions;

use definitions::{DEFINITIONS, ensure_definitions};

pub use definitions::{Definition, DefinitionKind};

#[cfg(feature = "currency-update")]
pub use currency_update::{
    CurrencySource, CurrencyUpdateOptions, UpdateError, fetch_currency_updates,
};

/// `UnitsError` wraps a raw error code returned by the GNU units C library.
///
/// Every fallible operation in this crate returns [`Result<T>`], which resolves
/// to `Err(UnitsError)` when the underlying C function signals failure. Inspect
/// [`code`](UnitsError::code) against the `E_*` constants re-exported from
/// [`gnu_units_sys`] to identify the specific error kind.
///
/// # Examples
///
/// ```no_run
/// use gnu_units::Unit;
///
/// # fn main() -> gnu_units::Result<()> {
/// match Unit::parse(")") {
///     Ok(_) => {}
///     Err(e) => println!("parse failed: {e}"),
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct UnitsError {
    /// Raw error code from the GNU units C library.
    ///
    /// Compare against the `E_*` constants exported by `gnu_units_sys`
    /// (e.g. `gnu_units_sys::E_PARSE`, `gnu_units_sys::E_BADSUM`) to identify
    /// the failure mode.
    pub code: c_int,
}

impl UnitsError {
    fn from_code(code: c_int) -> Option<Self> {
        if code == gnu_units_sys::E_NORMAL as c_int {
            return None;
        }

        Some(Self { code })
    }

    /// Returns `true` when the error indicates that a unit is not
    /// dimensionless (it still carries base dimensions after reduction).
    ///
    /// This error typically arises in two scenarios:
    /// - A failed [`Unit::convert_to`] where the source and target have
    ///   incompatible dimensions (conformability mismatch).
    /// - A [`Unit::to_number`] call on a unit that still has dimensions.
    pub fn is_not_dimensionless(&self) -> bool {
        self.code == gnu_units_sys::E_NOTANUMBER as c_int
    }

    /// Returns `true` when the error indicates that the input could not
    /// be resolved to a valid unit, either because parsing failed
    /// (`E_PARSE`) or because the unit name is not in the database
    /// (`E_UNKNOWNUNIT`).
    pub fn is_invalid_unit(&self) -> bool {
        self.code == gnu_units_sys::E_UNKNOWNUNIT as c_int
            || self.code == gnu_units_sys::E_PARSE as c_int
    }
}

impl fmt::Display for UnitsError {
    /// Formats the error as `"GNU units error code <N>"`.
    ///
    /// `<N>` is the raw integer value of [`UnitsError::code`]; compare it
    /// against the `E_*` constants in `gnu_units_sys` to identify the specific
    /// failure mode.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GNU units error code {}", self.code)
    }
}

impl std::error::Error for UnitsError {}

/// Convenience alias for [`std::result::Result<T, UnitsError>`].
///
/// All fallible functions in this crate return `Result<T>` rather than
/// spelling out the full error type. Use `Result<()>` for operations that
/// only signal success or failure, and `Result<f64>` (or another concrete
/// type) when a value is also produced.
///
/// # Examples
///
/// ```no_run
/// use gnu_units::{parse, Result};
///
/// # fn main() -> gnu_units::Result<()> {
/// let unit = parse("km")?;
/// println!("factor: {}", unit.factor());
/// # Ok(())
/// # }
/// ```
pub type Result<T> = std::result::Result<T, UnitsError>;

/// `Unit` wraps a GNU units `unittype` for safe use from Rust.
///
/// A `Unit` represents a dimensional quantity: a numeric factor paired with
/// zero or more base dimensions (length, time, mass, …). Instances are
/// constructed via [`Unit::new`] (dimensionless, factor 1) or [`Unit::parse`]
/// (from a GNU units expression string). All arithmetic operations mutate
/// `self` in place and return [`Result<()>`].
///
/// `Unit` owns the memory allocated by the C library; it is freed
/// automatically when the value is dropped.
///
/// # Examples
///
/// ```no_run
/// use gnu_units::Unit;
///
/// # fn main() -> gnu_units::Result<()> {
/// let mut area = Unit::parse("3 m")?;
/// area.multiply(Unit::parse("4 m")?)?;
/// println!("factor: {}", area.factor()); // 12.0
/// # Ok(())
/// # }
/// ```
pub struct Unit {
    pub(crate) raw: gnu_units_sys::unittype,
}

// SAFETY: All FFI calls that read or mutate `unittype` fields are serialized
// through `GNU_UNITS_MUTEX` in `ffi.rs`. The raw pointers inside `unittype` point to
// C heap allocations that are only accessed under the same lock.
unsafe impl Send for Unit {}
unsafe impl Sync for Unit {}

impl Unit {
    /// Creates a freshly initialized unit with factor `1.0` and no dimensions.
    ///
    /// The underlying C function `initializeunit` zeroes all fields of the
    /// `unittype` struct, producing the multiplicative identity, equivalent
    /// to the dimensionless number `1`.
    ///
    /// Prefer [`Unit::parse`] when you want a unit with a specific value or
    /// dimensions.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use gnu_units::Unit;
    ///
    /// let unit = Unit::new();
    /// assert_eq!(unit.factor(), 1.0);
    /// ```
    pub fn new() -> Self {
        let mut raw = MaybeUninit::<gnu_units_sys::unittype>::zeroed();
        // SAFETY: zeroed() ensures all pointer slots are null (valid for
        // *mut c_char). initializeunit then sets factor=1.0 and the first
        // terminator slots. The function only writes to the passed struct
        // without accessing any global state, no lock is needed.
        unsafe {
            gnu_units_sys::initializeunit(raw.as_mut_ptr());
            Self {
                raw: raw.assume_init(),
            }
        }
    }

    /// Parses a GNU units expression string and returns the resulting [`Unit`].
    ///
    /// `input` is passed to the underlying C function `parseunit`. The string
    /// is first converted to a null-terminated C string; a null byte anywhere
    /// in `input` causes an immediate `E_PARSE` error without reaching the C
    /// layer.
    ///
    /// # Errors
    ///
    /// Returns `Err(UnitsError)` with `code == E_PARSE` when:
    ///
    /// - `input` contains a null byte (`\0`).
    /// - `input` is not a valid GNU units expression (e.g. unbalanced
    ///   parentheses, unknown unit name).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use gnu_units::Unit;
    ///
    /// # fn main() -> gnu_units::Result<()> {
    /// let km = Unit::parse("km")?;
    /// println!("factor: {}", km.factor());
    ///
    /// assert!(Unit::parse(")").is_err());
    /// # Ok(())
    /// # }
    /// ```
    pub fn parse(input: &str) -> Result<Self> {
        ensure_definitions();
        ffi::parseunit(input)
    }

    /// Multiplies `self` by `rhs` in place, consuming `rhs`.
    ///
    /// Delegates to the C function `multunit`. Ownership of `rhs` is
    /// transferred to the C layer, which merges its dimension arrays into
    /// `self`. `rhs` is dropped after the call; the underlying C allocation
    /// is freed safely because `multunit` leaves the `rhs` struct in a
    /// defined (empty) state.
    ///
    /// # Errors
    ///
    /// Returns `Err(UnitsError)` if the multiplication cannot be represented
    /// (e.g. a dimensional overflow reported by the C library).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use gnu_units::Unit;
    ///
    /// # fn main() -> gnu_units::Result<()> {
    /// let mut lhs = Unit::parse("3")?;
    /// lhs.multiply(Unit::parse("4")?)?;
    /// assert_eq!(lhs.factor(), 12.0);
    /// # Ok(())
    /// # }
    /// ```
    pub fn multiply(&mut self, mut rhs: Unit) -> Result<()> {
        ffi::multunit(self, &mut rhs)
    }

    /// Divides `self` by `rhs` in place, consuming `rhs`.
    ///
    /// Delegates to the C function `divunit`. Ownership of `rhs` is
    /// transferred to the C layer, which inverts `rhs` and merges its
    /// dimension arrays into `self`. `rhs` is dropped after the call; the
    /// underlying C allocation is freed safely because `divunit` leaves the
    /// `rhs` struct in a defined (empty) state.
    ///
    /// # Errors
    ///
    /// Returns `Err(UnitsError)` if the division cannot be represented (e.g.
    /// a dimensional inconsistency reported by the C library).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use gnu_units::Unit;
    ///
    /// # fn main() -> gnu_units::Result<()> {
    /// let mut lhs = Unit::parse("10")?;
    /// lhs.divide(Unit::parse("2")?)?;
    /// assert_eq!(lhs.factor(), 5.0);
    /// # Ok(())
    /// # }
    /// ```
    pub fn divide(&mut self, mut rhs: Unit) -> Result<()> {
        ffi::divunit(self, &mut rhs)
    }

    /// Adds `rhs` to `self` in place, consuming `rhs`.
    ///
    /// Delegates to the C function `addunit`. Both units must be
    /// dimensionally compatible (same base dimensions). Ownership of `rhs`
    /// is transferred to the C layer; `rhs` is dropped after the call.
    ///
    /// # Errors
    ///
    /// Returns `Err(UnitsError)` when the two units have incompatible
    /// dimensions (e.g. adding a length to a mass).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use gnu_units::Unit;
    ///
    /// # fn main() -> gnu_units::Result<()> {
    /// let mut lhs = Unit::parse("3")?;
    /// lhs.add(Unit::parse("7")?)?;
    /// assert_eq!(lhs.factor(), 10.0);
    /// # Ok(())
    /// # }
    /// ```
    pub fn add(&mut self, mut rhs: Unit) -> Result<()> {
        ffi::addunit(self, &mut rhs)
    }

    /// Swaps the numerator and denominator of `self` in place.
    ///
    /// Delegates to the C function `invertunit`, which negates the exponent
    /// of every base dimension and takes the reciprocal of the numeric
    /// factor. The operation is always well-defined and cannot fail.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use gnu_units::Unit;
    ///
    /// # fn main() -> gnu_units::Result<()> {
    /// let mut unit = Unit::parse("5")?;
    /// unit.invert();
    /// assert_eq!(unit.factor(), 0.2);
    /// # Ok(())
    /// # }
    /// ```
    pub fn invert(&mut self) {
        // SAFETY: self is a valid initialized unit. invertunit only swaps
        // the numerator and denominator arrays in place and reciprocates
        // the factor, no global state is accessed.
        unsafe {
            gnu_units_sys::invertunit(self.as_mut_ptr());
        }
    }

    /// Raises `self` to a non-negative integer `power` in place.
    ///
    /// Delegates to the C function `expunit`, which multiplies the exponent
    /// of every base dimension by `power` and raises the numeric factor to
    /// `power`.
    ///
    /// # Errors
    ///
    /// Returns `Err(UnitsError)` when:
    ///
    /// - `power` is negative (negative exponents are not supported).
    /// - The resulting dimensions cannot be represented (e.g. an exponent
    ///   overflow reported by the C library).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use gnu_units::Unit;
    ///
    /// # fn main() -> gnu_units::Result<()> {
    /// let mut unit = Unit::parse("3")?;
    /// unit.pow(2)?;
    /// assert_eq!(unit.factor(), 9.0);
    /// # Ok(())
    /// # }
    /// ```
    pub fn pow(&mut self, power: c_int) -> Result<()> {
        if power < 0 {
            return Err(UnitsError {
                code: gnu_units_sys::E_BADNUM as c_int,
            });
        }
        ffi::expunit(self, power)
    }

    /// Takes the `n`th root of `self` in place.
    ///
    /// Delegates to the C function `rootunit`. The root must be exact: every
    /// base-dimension exponent in `self` must be divisible by `n`. If it is
    /// not, the C library returns an error rather than producing a fractional
    /// exponent.
    ///
    /// # Errors
    ///
    /// Returns `Err(UnitsError)` when `n` is not positive (greater than zero),
    /// when the root is not exact (i.e. a dimension exponent is not divisible
    /// by `n`), or when the C library signals another failure.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use gnu_units::Unit;
    ///
    /// # fn main() -> gnu_units::Result<()> {
    /// let mut unit = Unit::parse("9")?;
    /// unit.root(2)?;
    /// assert_eq!(unit.factor(), 3.0);
    /// # Ok(())
    /// # }
    /// ```
    pub fn root(&mut self, n: c_int) -> Result<()> {
        if n <= 0 {
            return Err(UnitsError {
                code: gnu_units_sys::E_NOTROOT as c_int,
            });
        }
        ffi::rootunit(self, n)
    }

    /// Converts a dimensionless unit to its numeric value.
    ///
    /// Internally clones `self` and calls the C function `unit2num` on the
    /// clone, so the original unit is not mutated. The returned `f64` is
    /// the numeric factor of the dimensionless quantity.
    ///
    /// # Errors
    ///
    /// Returns `Err(UnitsError)` when `self` carries non-zero base
    /// dimensions (e.g. metres, kilograms). Use [`Unit::factor`] to read
    /// the numeric factor unconditionally regardless of dimensions.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use gnu_units::Unit;
    ///
    /// # fn main() -> gnu_units::Result<()> {
    /// let unit = Unit::parse("42")?;
    /// assert_eq!(unit.to_number()?, 42.0);
    /// # Ok(())
    /// # }
    /// ```
    pub fn to_number(&self) -> Result<f64> {
        let mut tmp = self.clone();
        ffi::unit2num(&mut tmp)?;
        Ok(tmp.raw.factor)
    }

    /// Returns the numeric factor of the unit.
    ///
    /// The factor is the `double factor` field of the underlying `unittype`
    /// struct. For a dimensionless unit it is the plain numeric value; for
    /// a dimensional unit it is the SI conversion factor (e.g. `1000.0`
    /// for `km` when the base unit is metres).
    ///
    /// This accessor is always infallible. For a strict dimensionless
    /// check, use [`Unit::to_number`] instead.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use gnu_units::Unit;
    ///
    /// # fn main() -> gnu_units::Result<()> {
    /// let unit = Unit::parse("5")?;
    /// assert_eq!(unit.factor(), 5.0);
    /// # Ok(())
    /// # }
    /// ```
    pub fn factor(&self) -> f64 {
        self.raw.factor
    }

    /// Converts `self` into the unit expressed by `to`, returning the numeric
    /// conversion factor.
    ///
    /// Both `self` and `to` are consumed by this call. Internally, `to` is
    /// divided out of `self` using [`Unit::divide`], and the dimensionless
    /// result is extracted with [`Unit::to_number`].
    ///
    /// # Errors
    ///
    /// Returns `Err(UnitsError)` when:
    ///
    /// - `self` and `to` have incompatible dimensions (e.g. converting
    ///   kilometres to kilograms), in which case [`Unit::to_number`] reports a
    ///   dimensional mismatch.
    /// - The division itself fails (e.g. a dimensional overflow reported by the
    ///   C library).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use gnu_units::Unit;
    ///
    /// # fn main() -> gnu_units::Result<()> {
    /// let factor = Unit::parse("5 km")?.convert_to(Unit::parse("miles")?)?;
    /// println!("{factor}"); // ≈ 3.1069
    /// # Ok(())
    /// # }
    /// ```
    pub fn convert_to(mut self, to: Unit) -> Result<f64> {
        self.divide(to)?;
        self.to_number()
    }

    /// Returns the base dimensions of the unit as a human-readable string.
    ///
    /// Each non-empty, non-sentinel slot in the numerator and denominator
    /// arrays of the underlying `unittype` is read and the corresponding C
    /// string is collected. The terms are primitive base units as resolved by
    /// the parser, but they are **not** sorted or canceled, redundant terms
    /// (e.g. from `"m/s * s"`) may appear in both numerator and denominator.
    ///
    /// Result formats:
    ///
    /// - `"m"`, numerator only
    /// - `"kg m / s s"`, numerator and denominator
    /// - `"1 / s"`, denominator only (numerator is empty)
    /// - `""`, dimensionless (both arrays are empty)
    ///
    pub fn base_units(&self) -> String {
        // SAFETY: The pointer fields inside raw were set by the C library
        // during parse and are immutable for the lifetime of self. NULLUNIT
        // is a process-global constant (`""`) assigned once at file scope,
        // reading its address is safe without the lock. CStr::from_ptr is
        // safe because each non-null, non-NULLUNIT pointer references a
        // valid NUL-terminated C string owned by this Unit.
        let null_sentinel = unsafe { gnu_units_sys::NULLUNIT };

        let mut numerators: Vec<String> = Vec::new();
        let mut denominators: Vec<String> = Vec::new();

        for &ptr in self.raw.numerator.iter() {
            if ptr.is_null() {
                // NULL is the array terminator; everything beyond is uninitialised.
                break;
            }
            if ptr == null_sentinel {
                // NULLUNIT marks a cancelled (dimensionally eliminated) entry; skip it.
                continue;
            }
            // SAFETY: ptr is non-null and not NULLUNIT, so it points to a
            // valid NUL-terminated C string managed by the C library.
            let s = unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() };
            numerators.push(s);
        }

        for &ptr in self.raw.denominator.iter() {
            if ptr.is_null() {
                break;
            }
            if ptr == null_sentinel {
                continue;
            }
            let s = unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() };
            denominators.push(s);
        }

        if numerators.is_empty() && denominators.is_empty() {
            return String::new();
        }

        if denominators.is_empty() {
            return numerators.join(" ");
        }

        let num_part = if numerators.is_empty() {
            "1".to_string()
        } else {
            numerators.join(" ")
        };

        format!("{num_part} / {}", denominators.join(" "))
    }

    /// Returns `true` when `self` and `other` have the same base dimensions.
    ///
    /// Two units are conformable when they describe the same physical quantity
    /// (e.g. `km` and `miles` are both lengths). The check divides a clone of
    /// `self` by a clone of `other` and attempts to reduce the result to a
    /// dimensionless number: success means the units are conformable.
    ///
    /// Neither `self` nor `other` is consumed or mutated.
    pub fn is_conformable(&self, other: &Unit) -> bool {
        let mut ratio = self.clone();
        let mut other_clone = other.clone();
        if ffi::divunit(&mut ratio, &mut other_clone).is_err() {
            return false;
        }
        ffi::unit2num(&mut ratio).is_ok()
    }

    // SAFETY: Caller must ensure no aliasing pointers to self.raw exist
    // for the duration of the FFI call.
    pub(crate) unsafe fn as_mut_ptr(&mut self) -> *mut gnu_units_sys::unittype {
        &mut self.raw
    }
}

impl Default for Unit {
    /// Returns a freshly initialized unit with factor `1.0` and no dimensions.
    ///
    /// Equivalent to [`Unit::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for Unit {
    /// Returns a deep copy of `self`.
    ///
    /// Delegates to the C function `unitcopy`, which duplicates all heap
    /// allocations owned by the `unittype` struct. The cloned `Unit` is
    /// fully independent: dropping either value does not affect the other.
    fn clone(&self) -> Self {
        ffi::unitcopy(self)
    }
}

impl Drop for Unit {
    /// Releases the C heap memory owned by this unit.
    ///
    /// Delegates to the C function `freeunit`, which frees the dimension
    /// arrays allocated by `parseunit` or `unitcopy`. After `drop`, all
    /// pointer fields inside the `unittype` struct are invalid; the struct
    /// itself is on the Rust stack and is reclaimed by Rust after this
    /// function returns.
    fn drop(&mut self) {
        ffi::freeunit(self)
    }
}

/// Convenience free function, equivalent to [`Unit::parse`].
///
/// Parses a GNU units expression string and returns the resulting [`Unit`].
/// This function exists so callers can write `gnu_units::parse("km")` without
/// importing [`Unit`] explicitly.
///
/// # Errors
///
/// Returns `Err(UnitsError)` for the same reasons as [`Unit::parse`]: a null
/// byte in `input`, or an expression the GNU units parser cannot recognise.
///
/// # Examples
///
/// ```no_run
/// # fn main() -> gnu_units::Result<()> {
/// let unit = gnu_units::parse("km")?;
/// println!("factor: {}", unit.factor());
/// # Ok(())
/// # }
/// ```
pub fn parse(input: &str) -> Result<Unit> {
    Unit::parse(input)
}

/// Reloads currency unit definitions from a GNU units currency file string.
///
/// Parses `content` as a GNU units definitions file and registers every
/// definition found into the C library's global hash tables, overwriting any
/// existing entry with the same name. Also updates the in-memory definitions
/// list returned by [`list_definitions`].
///
/// Call this after writing an updated currency file via
/// [`update_currency_file`] to make the new rates effective for all subsequent
/// [`parse`], [`convert`], and [`list_definitions`] calls.
#[cfg(feature = "currency-update")]
pub fn reload_currency(content: &str) {
    ensure_definitions();
    let new_defs = load_definitions(content, c"currency.units");
    let mut defs = DEFINITIONS.write().unwrap_or_else(|e| e.into_inner());
    // Remove old entries from the same file, then merge new ones
    defs.retain(|d| d.kind != DefinitionKind::Unit || !new_defs.iter().any(|n| n.name == d.name));
    defs.extend(new_defs);
    defs.sort_by(|a, b| a.name.cmp(&b.name));
}

/// Parses `from` and `to` as GNU units expressions and returns the numeric
/// conversion factor from `from` to `to`.
///
/// This is a convenience wrapper around [`Unit::parse`] and
/// [`Unit::convert_to`]. Callers can write `gnu_units::convert("km", "miles")`
/// without importing [`Unit`] explicitly.
///
/// # Errors
///
/// Returns `Err(UnitsError)` when:
///
/// - Either `from` or `to` cannot be parsed (see [`Unit::parse`] for the
///   exact conditions).
/// - The two units have incompatible dimensions (e.g. kilometres and
///   kilograms).
///
/// # Examples
///
/// ```no_run
/// # fn main() -> gnu_units::Result<()> {
/// let factor = gnu_units::convert("km", "miles")?;
/// println!("{factor}"); // ≈ 0.62137
/// # Ok(())
/// # }
/// ```
pub fn convert(from: &str, to: &str) -> Result<f64> {
    ensure_definitions();
    if let Some(unit) = ffi::convert_func(from, to) {
        return Ok(unit.factor());
    }
    Unit::parse(from)?.convert_to(Unit::parse(to)?)
}

/// Finds all unit definitions that are conformable with `expr`.
///
/// Parses `expr` into a [`Unit`], then iterates over every
/// [`DefinitionKind::Unit`] entry returned by [`list_definitions`]. Any entry
/// whose name parses successfully and whose dimensions match those of `expr`
/// is included in the result. The returned names are in alphabetical order.
///
/// # Errors
///
/// Returns `Err(UnitsError)` if `expr` itself cannot be parsed.
///
/// # Examples
///
/// ```no_run
/// # fn main() -> gnu_units::Result<()> {
/// let lengths = gnu_units::conformable("m")?;
/// assert!(lengths.contains(&"km".to_string()));
/// # Ok(())
/// # }
/// ```
pub fn conformable(expr: &str) -> Result<Vec<String>> {
    let target = Unit::parse(expr)?;
    let names = list_definitions()
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

/// Returns all unit definitions from the embedded GNU units database.
///
/// Each entry contains the unit name, its definition string, and what
/// kind of definition it is (unit, prefix, function, table, or alias).
/// The list is sorted alphabetically by name.
pub fn list_definitions() -> std::sync::RwLockReadGuard<'static, Vec<Definition>> {
    ensure_definitions();
    DEFINITIONS.read().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::replace_operators;
    use rstest::rstest;
    use std::os::raw::c_int;

    #[test]
    fn units_error_display() {
        let err = UnitsError { code: 5 };

        let formatted = format!("{err}");

        assert_eq!(formatted, "GNU units error code 5");
    }

    #[test]
    fn units_error_eq_same_code() {
        let a = UnitsError { code: 3 };
        let b = UnitsError { code: 3 };

        assert_eq!(a, b);
    }

    #[test]
    fn units_error_ne_different_code() {
        let a = UnitsError { code: 1 };
        let b = UnitsError { code: 2 };

        assert_ne!(a, b);
    }

    #[test]
    fn units_error_copy_semantics() {
        let original = UnitsError { code: 7 };
        let copied: UnitsError = original;

        assert_eq!(copied, original);
    }

    #[rstest]
    #[case::new(Unit::new())]
    #[case::default(Unit::default())]
    fn initial_factor_is_one(#[case] unit: Unit) {
        assert_eq!(unit.factor(), 1.0);
    }

    #[rstest]
    #[case::integer("5", 5.0)]
    #[case::float("3.15", 3.15)]
    #[case::large("1e10", 1e10)]
    fn parse_numeric(#[case] input: &str, #[case] expected: f64) {
        let unit = Unit::parse(input).unwrap();

        assert_eq!(unit.factor(), expected);
    }

    #[rstest]
    #[case::null_byte("foo\0bar", gnu_units_sys::E_PARSE as c_int)]
    #[case::unparsable(")", gnu_units_sys::E_PARSE as c_int)]
    fn parse_error(#[case] input: &str, #[case] expected_code: c_int) {
        let result = Unit::parse(input);

        assert_eq!(
            result.err().unwrap(),
            UnitsError {
                code: expected_code
            }
        );
    }

    #[test]
    fn clone_preserves_factor() {
        let unit = Unit::parse("7").unwrap();

        let cloned = unit.clone();

        assert_eq!(cloned.factor(), 7.0);
    }

    #[test]
    fn multiply_five_by_three() {
        let mut lhs = Unit::parse("5").unwrap();
        let rhs = Unit::parse("3").unwrap();

        lhs.multiply(rhs).unwrap();

        assert_eq!(lhs.factor(), 15.0);
    }

    #[test]
    fn divide_ten_by_two() {
        let mut lhs = Unit::parse("10").unwrap();
        let rhs = Unit::parse("2").unwrap();

        lhs.divide(rhs).unwrap();

        assert_eq!(lhs.factor(), 5.0);
    }

    #[test]
    fn add_three_and_seven() {
        let mut lhs = Unit::parse("3").unwrap();
        let rhs = Unit::parse("7").unwrap();

        lhs.add(rhs).unwrap();

        assert_eq!(lhs.factor(), 10.0);
    }

    #[test]
    fn invert_five_is_point_two() {
        let mut unit = Unit::parse("5").unwrap();

        unit.invert();

        assert_eq!(unit.factor(), 0.2);
    }

    #[test]
    fn pow_three_squared_is_nine() {
        let mut unit = Unit::parse("3").unwrap();

        unit.pow(2).unwrap();

        assert_eq!(unit.factor(), 9.0);
    }

    #[test]
    fn root_sqrt_nine_is_three() {
        let mut unit = Unit::parse("9").unwrap();

        unit.root(2).unwrap();

        assert_eq!(unit.factor(), 3.0);
    }

    #[rstest]
    #[case::negative(-1, gnu_units_sys::E_BADNUM as c_int)]
    #[case::min_int(c_int::MIN, gnu_units_sys::E_BADNUM as c_int)]
    fn pow_error(#[case] power: c_int, #[case] expected_code: c_int) {
        let mut unit = Unit::parse("3").unwrap();

        let result = unit.pow(power);

        assert_eq!(
            result,
            Err(UnitsError {
                code: expected_code
            })
        );
    }

    #[rstest]
    #[case::zero(0, gnu_units_sys::E_NOTROOT as c_int)]
    #[case::negative(-1, gnu_units_sys::E_NOTROOT as c_int)]
    fn root_error(#[case] n: c_int, #[case] expected_code: c_int) {
        let mut unit = Unit::parse("9").unwrap();

        let result = unit.root(n);

        assert_eq!(
            result,
            Err(UnitsError {
                code: expected_code
            })
        );
    }

    #[test]
    fn to_number_returns_factor() {
        let unit = Unit::parse("42").unwrap();

        let result = unit.to_number().unwrap();

        assert_eq!(result, 42.0);
    }

    #[test]
    fn free_parse_delegates_to_unit_parse() {
        let unit = parse("5").unwrap();

        assert_eq!(unit.factor(), 5.0);
    }

    #[rstest]
    #[case::dimensionless_to_itself("5", "1", 5.0, 1e-10)]
    #[case::km_to_m("km", "m", 1000.0, 1e-10)]
    #[case::identity_m_to_m("m", "m", 1.0, 1e-10)]
    #[case::numeric_prefix("5 km", "miles", 3.10686, 1e-4)]
    fn convert_to_compatible_units(
        #[case] from: &str,
        #[case] to: &str,
        #[case] expected: f64,
        #[case] tolerance: f64,
    ) {
        let from_unit = Unit::parse(from).unwrap();
        let to_unit = Unit::parse(to).unwrap();

        let result = from_unit.convert_to(to_unit).unwrap();

        assert!(
            (result - expected).abs() < tolerance,
            "convert_to({from:?}, {to:?}) = {result}, expected {expected} ±{tolerance}"
        );
    }

    #[test]
    fn error_on_convert_to_incompatible_dimensions() {
        let from_unit = Unit::parse("km").unwrap();
        let to_unit = Unit::parse("kg").unwrap();

        let result = from_unit.convert_to(to_unit);

        assert_eq!(
            result,
            Err(UnitsError {
                code: gnu_units_sys::E_NOTANUMBER as c_int
            })
        );
    }

    #[rstest]
    #[case::invalid_from(")", "m", gnu_units_sys::E_PARSE as c_int)]
    #[case::invalid_to("m", ")", gnu_units_sys::E_PARSE as c_int)]
    #[case::incompatible_dimensions("km", "kg", gnu_units_sys::E_NOTANUMBER as c_int)]
    fn convert_error(#[case] from: &str, #[case] to: &str, #[case] expected_code: c_int) {
        let result = convert(from, to);

        assert_eq!(
            result,
            Err(UnitsError {
                code: expected_code
            })
        );
    }

    #[rstest]
    #[case::figure_dash("\u{2012}x", "-x")]
    #[case::en_dash("\u{2013}y", "-y")]
    #[case::minus_sign("\u{2212}z", "-z")]
    #[case::times("\u{00D7}a", "*a")]
    #[case::nary_times("\u{2A09}b", "*b")]
    #[case::middle_dot("\u{00B7}c", "*c")]
    #[case::dot_operator("\u{22C5}d", "*d")]
    #[case::division_sign("\u{00F7}e", "/e")]
    #[case::division_slash("\u{2215}f", "/f")]
    #[case::fraction_slash("\u{2044}g", "|g")]
    #[case::no_break_space("a\u{00A0}b", "a b")]
    #[case::ogham_space("a\u{1680}b", "a b")]
    #[case::en_quad("a\u{2000}b", "a b")]
    #[case::thin_space("a\u{2009}b", "a b")]
    #[case::hair_space("a\u{200A}b", "a b")]
    #[case::narrow_no_break_space("a\u{202F}b", "a b")]
    #[case::medium_math_space("a\u{205F}b", "a b")]
    #[case::ideographic_space("a\u{3000}b", "a b")]
    #[case::zero_width_space("a\u{200B}b", "ab")]
    #[case::zero_width_non_joiner("a\u{200C}b", "ab")]
    #[case::plain_ascii("hello", "hello")]
    #[case::empty_string("", "")]
    #[case::multiple_replacements("\u{2212}3\u{00D7}4", "-3*4")]
    #[case::preserves_ascii_operators("3*4 + 5/2 - 1", "3*4 + 5/2 - 1")]
    #[case::mixed_unicode_and_ascii("3\u{00D7}4 + 5\u{00F7}2", "3*4 + 5/2")]
    fn replace_operators_cases(#[case] input: &str, #[case] expected: &str) {
        let result = replace_operators(input);

        assert_eq!(result, expected);
    }

    #[rstest]
    #[case::prefix_kilo("kilogram", "gram", 1000.0, 1e-10)]
    #[case::temperature_diff("degF", "degC", 0.555_555_555_6, 1e-8)]
    #[case::element_mercury("mercury", "1", 200.59, 0.01)]
    #[case::utf8_micro("\u{00B5}m", "m", 1e-6, 1e-16)]
    #[case::knot_to_mps("knot", "m/s", 0.514_444, 1e-4)]
    #[case::inches_to_cm("inch", "cm", 2.54, 1e-10)]
    #[case::hour_to_seconds("hour", "s", 3600.0, 1e-10)]
    #[case::line_continuation("spherevolume(1 m)", "m^3", 4.18879, 1e-4)]
    fn definitions_convert(
        #[case] from: &str,
        #[case] to: &str,
        #[case] expected: f64,
        #[case] tolerance: f64,
    ) {
        let result = convert(from, to).unwrap();

        assert!(
            (result - expected).abs() < tolerance,
            "convert({from:?}, {to:?}) = {result}, expected {expected} ±{tolerance}"
        );
    }

    #[rstest]
    #[case::table_gasmark("gasmark1", 1.0, 1e-10)]
    #[case::function_tempf("tempF(32)", 273.15, 0.01)]
    fn definitions_parse_factor(
        #[case] input: &str,
        #[case] expected: f64,
        #[case] tolerance: f64,
    ) {
        let unit = Unit::parse(input).unwrap();

        assert!(
            (unit.factor() - expected).abs() < tolerance,
            "parse({input:?}).factor() = {}, expected {expected} ±{tolerance}",
            unit.factor()
        );
    }

    #[cfg(feature = "currency-update")]
    #[rstest]
    #[case::currency_usd("USD")]
    #[case::cpi_now("UScpi_now")]
    fn definitions_parse_currency(#[case] input: &str) {
        let unit = Unit::parse(input).unwrap();

        assert!(
            unit.factor() > 0.0,
            "parse({input:?}).factor() should be > 0"
        );
    }

    #[test]
    fn list_definitions_is_not_empty() {
        let defs = list_definitions();

        assert!(
            defs.len() > 1000,
            "expected >1000 definitions, got {}",
            defs.len()
        );
    }

    #[test]
    fn list_definitions_is_sorted_alphabetically() {
        let defs = list_definitions();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();

        let mut sorted = names.clone();
        sorted.sort();

        assert_eq!(names, sorted);
    }

    #[test]
    fn all_definitions_have_non_empty_names() {
        let defs = list_definitions();

        for def in defs.iter() {
            assert!(!def.name.is_empty(), "found empty name entry");
        }
    }

    #[rstest]
    #[case::unit_m("m", "!", DefinitionKind::Unit)]
    #[case::unit_meter("meter", "m", DefinitionKind::Unit)]
    #[case::prefix_kilo("kilo-", "1e3", DefinitionKind::Prefix)]
    #[case::alias_hms("hms", "hr;min;sec", DefinitionKind::Alias)]
    fn list_definitions_contains_known_entry(
        #[case] name: &str,
        #[case] expected_def: &str,
        #[case] expected_kind: DefinitionKind,
    ) {
        let defs = list_definitions();
        let found = defs.iter().find(|d| d.name == name);

        assert!(found.is_some(), "entry '{name}' not found in definitions");
        let entry = found.unwrap();
        assert_eq!(entry.definition, expected_def);
        assert_eq!(entry.kind, expected_kind);
    }

    #[rstest]
    #[case::function_tempc("tempC(x)", DefinitionKind::Function)]
    #[case::table_gasmark("gasmark[degR]", DefinitionKind::Table)]
    fn list_definitions_contains_known_kind_entry(
        #[case] name: &str,
        #[case] expected_kind: DefinitionKind,
    ) {
        let defs = list_definitions();
        let found = defs.iter().find(|d| d.name == name);

        assert!(found.is_some(), "entry '{name}' not found in definitions");
        assert_eq!(found.unwrap().kind, expected_kind);
    }

    #[rstest]
    #[case::prefix_ends_with_dash(DefinitionKind::Prefix, '-')]
    #[case::table_contains_bracket(DefinitionKind::Table, '[')]
    #[case::function_contains_paren(DefinitionKind::Function, '(')]
    fn definition_kind_name_invariant(#[case] kind: DefinitionKind, #[case] expected_char: char) {
        let defs = list_definitions();

        for def in defs.iter().filter(|d| d.kind == kind) {
            assert!(
                def.name.contains(expected_char),
                "{kind:?} entry '{}' does not contain '{expected_char}'",
                def.name
            );
        }
    }

    #[rstest]
    #[case::e_notanumber_true(gnu_units_sys::E_NOTANUMBER as c_int, true)]
    #[case::e_parse_false(gnu_units_sys::E_PARSE as c_int, false)]
    #[case::e_unknownunit_false(gnu_units_sys::E_UNKNOWNUNIT as c_int, false)]
    #[case::arbitrary_code_false(42, false)]
    fn is_not_dimensionless(#[case] code: c_int, #[case] expected: bool) {
        let err = UnitsError { code };

        let result = err.is_not_dimensionless();

        assert_eq!(result, expected);
    }

    #[rstest]
    #[case::e_unknownunit_true(gnu_units_sys::E_UNKNOWNUNIT as c_int, true)]
    #[case::e_parse_true(gnu_units_sys::E_PARSE as c_int, true)]
    #[case::e_notanumber_false(gnu_units_sys::E_NOTANUMBER as c_int, false)]
    #[case::arbitrary_code_false(42, false)]
    fn is_invalid_unit(#[case] code: c_int, #[case] expected: bool) {
        let err = UnitsError { code };

        let result = err.is_invalid_unit();

        assert_eq!(result, expected);
    }

    #[rstest]
    #[case::meter_contains_m("m", "m")]
    #[case::compound_contains_slash("kg m/s^2", " / ")]
    #[case::inverse_contains_one_slash("1/s", "1 / ")]
    fn base_units_contains_expected(#[case] input: &str, #[case] expected_substr: &str) {
        let unit = Unit::parse(input).unwrap();

        let result = unit.base_units();

        assert!(
            result.contains(expected_substr),
            "expected '{expected_substr}' in {:?}",
            result
        );
    }

    #[rstest]
    #[case::unit_new_is_empty("")]
    #[case::dimensionless_number_is_empty("")]
    fn base_units_dimensionless_is_empty(#[case] expected: &str) {
        let unit = Unit::new();

        let result = unit.base_units();

        assert_eq!(result, expected);
    }

    #[test]
    fn base_units_pure_number_is_empty() {
        let unit = Unit::parse("5").unwrap();

        let result = unit.base_units();

        assert_eq!(result, "");
    }

    #[rstest]
    #[case::km_and_miles("km", "miles", true)]
    #[case::m_and_kg("m", "kg", false)]
    #[case::velocity_conformable("m/s", "knot", true)]
    fn is_conformable(#[case] a: &str, #[case] b: &str, #[case] expected: bool) {
        let unit_a = Unit::parse(a).unwrap();
        let unit_b = Unit::parse(b).unwrap();

        let result = unit_a.is_conformable(&unit_b);

        assert_eq!(result, expected);
    }

    #[test]
    fn is_conformable_with_itself() {
        let unit = Unit::parse("m").unwrap();

        let result = unit.is_conformable(&unit.clone());

        assert!(result);
    }

    #[rstest]
    #[case::m_contains_meter("m", "meter")]
    #[case::m_contains_mile("m", "mile")]
    #[case::m_contains_ft("m", "ft")]
    #[case::m_contains_inch("m", "inch")]
    #[case::kg_contains_lb("kg", "lb")]
    #[case::kg_contains_g("kg", "g")]
    fn conformable_contains_expected_unit(#[case] expr: &str, #[case] expected_unit: &str) {
        let result = conformable(expr).unwrap();

        assert!(
            result.contains(&expected_unit.to_string()),
            "{} missing from {:?}",
            expected_unit,
            result
        );
    }

    #[rstest]
    #[case::m_not_kg("m", "kg")]
    #[case::m_not_second("m", "second")]
    fn conformable_does_not_contain_wrong_domain(
        #[case] expr: &str,
        #[case] unexpected_unit: &str,
    ) {
        let result = conformable(expr).unwrap();

        assert!(
            !result.contains(&unexpected_unit.to_string()),
            "{} should not appear in {:?}",
            unexpected_unit,
            result
        );
    }

    #[test]
    fn error_on_conformable_invalid_expression() {
        let result = conformable(")");

        assert!(result.is_err(), "expected Err for invalid expression");
    }

    #[rstest]
    #[case::zero_celsius("273.15 K", "tempC", 0.0, 1e-6)]
    #[case::boiling_point_celsius("373.15 K", "tempC", 100.0, 1e-6)]
    #[case::freezing_point_fahrenheit("273.15 K", "tempF", 32.0, 1e-4)]
    #[case::body_temp_fahrenheit("310.15 K", "tempF", 98.6, 0.1)]
    #[case::absolute_zero_kelvin("0 K", "tempK", 0.0, 1e-10)]
    #[case::fallback_non_function("km", "m", 1000.0, 1e-10)]
    fn convert_via_function(
        #[case] from: &str,
        #[case] to: &str,
        #[case] expected: f64,
        #[case] tolerance: f64,
    ) {
        let result = convert(from, to).unwrap();

        assert!(
            (result - expected).abs() < tolerance,
            "convert({from:?}, {to:?}) = {result}, expected {expected} ±{tolerance}"
        );
    }
}
