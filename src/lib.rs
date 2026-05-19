use std::ffi::{CStr, CString};
use std::fmt;
use std::mem::MaybeUninit;
use std::os::raw::c_int;
use std::ptr;
use std::sync::{LazyLock, Mutex};

pub use gnu_units_sys;

#[cfg(feature = "currency-update")]
pub mod currency_update;

mod definitions;
use definitions::{DEFINITIONS, load_definitions};
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
    raw: gnu_units_sys::unittype,
}

// SAFETY: All FFI calls that read or mutate `unittype` fields are serialized
// through `FFI_LOCK`. The raw pointers inside `unittype` point to C heap
// allocations that are only accessed under the same lock.
unsafe impl Send for Unit {}
unsafe impl Sync for Unit {}

/// Loads the embedded GNU units definitions exactly once into the C library's
/// global database.
///
/// `DB` is a [`LazyLock`] whose initialiser parses the compile-time embedded
/// `definitions.units` content and registers each unit, prefix, table, and
/// function directly with the C library's global hash tables. All subsequent
/// accesses are no-ops, the `LazyLock` guarantees the closure runs at most
/// once, even under concurrent access.
static DB: LazyLock<Vec<Definition>> = LazyLock::new(|| {
    let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: mylocale/progname point to static C strings that live for the
    // duration of the process. utf8mode is a plain C int. All writes happen
    // under FFI_LOCK before any other FFI call can observe the globals.
    unsafe {
        gnu_units_sys::mylocale = c"en_US".as_ptr() as *mut std::os::raw::c_char;
        gnu_units_sys::progname = c"gnu-units".as_ptr() as *mut std::os::raw::c_char;
        gnu_units_sys::utf8mode = 1;
    }

    let mut defs = load_definitions(DEFINITIONS, c"definitions.units");
    // Emulate C last-write-wins: reverse so last-in-file entries come first
    defs.reverse();
    // Stable sort by canonical_name + kind; preserves reverse order within each group
    defs.sort();
    // Remove duplicates; keeps first of each group = last-in-file entry
    defs.dedup();
    // Final alphabetical ordering for the public API
    defs.sort_by(|a, b| a.name.cmp(&b.name));
    defs
});

/// Global mutex that serializes every FFI call into the GNU units C library.
///
/// The C library uses process-wide mutable globals (e.g. `unitcount`,
/// `lastunitset`, `lastunit`, `parameter_value`). Every call site that
/// touches those globals must hold this lock for the duration of the call.
static FFI_LOCK: Mutex<()> = Mutex::new(());

/// Ensures the GNU units database is loaded exactly once.
///
/// Forces the [`DB`] lazy initialiser, which embeds `definitions.units` at
/// compile time and loads it into the C library's global database on first
/// call. All subsequent calls are instant no-ops.
fn ensure_db() {
    LazyLock::force(&DB);
}

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
        ensure_db();

        let c_input = CString::new(input).map_err(|_| UnitsError {
            code: gnu_units_sys::E_PARSE as c_int,
        })?;

        let mut unit = Self::new();
        let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: unit is freshly initialized via new(). c_input is a valid
        // NUL-terminated C string. parseunit writes into unit without
        // reading uninitialised data. The null pointers for errstr/errloc
        // are accepted by parseunit (optional out-params). FFI_LOCK is held.
        let code = unsafe {
            gnu_units_sys::parseunit(
                unit.as_mut_ptr(),
                c_input.as_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };

        if let Some(err) = UnitsError::from_code(code) {
            return Err(err);
        }

        Ok(unit)
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
        // SAFETY: self and rhs are valid initialized units. multunit moves
        // pointer ownership from rhs to self via moveproduct, which sets
        // source entries to NULLUNIT, no global state is accessed.
        let code = unsafe { gnu_units_sys::multunit(self.as_mut_ptr(), rhs.as_mut_ptr()) };
        UnitsError::from_code(code).map_or(Ok(()), Err)
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
        // SAFETY: self and rhs are valid initialized units. divunit moves
        // pointer ownership from rhs to self via moveproduct, which sets
        // source entries to NULLUNIT, no global state is accessed.
        let code = unsafe { gnu_units_sys::divunit(self.as_mut_ptr(), rhs.as_mut_ptr()) };
        UnitsError::from_code(code).map_or(Ok(()), Err)
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
        // SAFETY: self is a valid initialized unit. rhs is consumed by this
        // call, addunit frees rhs internals.
        // rhs is dropped after this call; freeunit on the now-emptied rhs is safe.
        // FFI_LOCK is held for the duration.
        let code = {
            let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            unsafe { gnu_units_sys::addunit(self.as_mut_ptr(), rhs.as_mut_ptr()) }
        };
        UnitsError::from_code(code).map_or(Ok(()), Err)
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
        let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: self is a valid initialized unit. expunit modifies the
        // unit in place, adjusting factor and dimension arrays.
        // FFI_LOCK is held for the duration.
        let code = unsafe { gnu_units_sys::expunit(self.as_mut_ptr(), power) };
        UnitsError::from_code(code).map_or(Ok(()), Err)
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
        let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: self is a valid initialized unit. rootunit modifies the
        // unit in place. Returns an error code if the root is not exact.
        // FFI_LOCK is held for the duration.
        let code = unsafe { gnu_units_sys::rootunit(self.as_mut_ptr(), n) };
        UnitsError::from_code(code).map_or(Ok(()), Err)
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
        let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: tmp is a deep copy via clone(). unit2num converts the
        // unit to a dimensionless number, modifying tmp in place. The
        // original self is not touched. FFI_LOCK is held for the duration.
        let code = unsafe { gnu_units_sys::unit2num(tmp.as_mut_ptr()) };
        if let Some(err) = UnitsError::from_code(code) {
            return Err(err);
        }
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
        // SAFETY: divunit only moves pointers between the two structs via
        // moveproduct, no global state is accessed.
        let div_code =
            unsafe { gnu_units_sys::divunit(ratio.as_mut_ptr(), other_clone.as_mut_ptr()) };
        if UnitsError::from_code(div_code).is_some() {
            return false;
        }
        let num_code = {
            let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            // SAFETY: ratio is a valid unit after divunit. unit2num checks
            // whether the ratio is dimensionless, modifying ratio in place.
            // FFI_LOCK is held for the duration.
            unsafe { gnu_units_sys::unit2num(ratio.as_mut_ptr()) }
        };
        UnitsError::from_code(num_code).is_none()
    }

    // SAFETY: Caller must ensure no aliasing pointers to self.raw exist
    // for the duration of the FFI call. For C functions that access global
    // state, the caller must also hold FFI_LOCK.
    unsafe fn as_mut_ptr(&mut self) -> *mut gnu_units_sys::unittype {
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
        let mut dest = Self::new();
        let mut src = self.raw;
        let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: dest is freshly initialized. src is a copy of self.raw
        // whose pointer fields still point to valid C strings owned by self.
        // unitcopy deep-copies all strings, so dest gets independent allocations.
        // FFI_LOCK is held for the duration.
        unsafe {
            gnu_units_sys::unitcopy(dest.as_mut_ptr(), &mut src as *mut _);
        }
        dest
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
        // SAFETY: self.raw was initialised by initializeunit and all pointer
        // fields are either NULL (from zeroed/init) or valid C allocations
        // (from parseunit/unitcopy). freeunit calls free() on those
        // allocations, it only reads NULLUNIT (a constant) and does not
        // access any mutable global state.
        unsafe {
            gnu_units_sys::freeunit(self.as_mut_ptr());
        }
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
/// existing entry with the same name. Call this after writing an updated
/// currency file via [`update_currency_file`] to make the new rates effective
/// for all subsequent [`parse`] and [`convert`] calls.
#[cfg(feature = "currency-update")]
pub fn reload_currency(content: &str) {
    ensure_db();
    let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load_definitions(content, c"currency.units");
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
        .into_iter()
        .filter(|d| d.kind == DefinitionKind::Unit)
        .filter_map(|d| {
            let parsed = Unit::parse(&d.name).ok()?;
            if parsed.is_conformable(&target) {
                Some(d.name)
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
pub fn list_definitions() -> Vec<Definition> {
    ensure_db();
    DB.clone()
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

    #[test]
    fn new_factor_is_one() {
        let unit = Unit::new();

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

    #[test]
    fn error_on_null_byte_in_input() {
        let result = Unit::parse("foo\0bar");

        let err = result.err().expect("expected Err, got Ok");
        assert_eq!(
            err,
            UnitsError {
                code: gnu_units_sys::E_PARSE as c_int,
            }
        );
    }

    #[test]
    fn error_on_unparsable_expression() {
        // Standalone `)` is an unexpected token for the bison grammar → E_PARSE
        let result = Unit::parse(")");

        let err = result.err().expect("expected Err, got Ok");
        assert_eq!(err.code, gnu_units_sys::E_PARSE as c_int);
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

    #[test]
    fn error_on_pow_negative() {
        let mut unit = Unit::parse("3").unwrap();

        let result = unit.pow(-1);

        assert_eq!(
            result,
            Err(UnitsError {
                code: gnu_units_sys::E_BADNUM as c_int
            })
        );
    }

    #[test]
    fn error_on_pow_min_int() {
        let mut unit = Unit::parse("3").unwrap();

        let result = unit.pow(c_int::MIN);

        assert_eq!(
            result,
            Err(UnitsError {
                code: gnu_units_sys::E_BADNUM as c_int
            })
        );
    }

    #[test]
    fn error_on_root_zero() {
        let mut unit = Unit::parse("9").unwrap();

        let result = unit.root(0);

        assert_eq!(
            result,
            Err(UnitsError {
                code: gnu_units_sys::E_NOTROOT as c_int
            })
        );
    }

    #[test]
    fn error_on_root_negative() {
        let mut unit = Unit::parse("9").unwrap();

        let result = unit.root(-1);

        assert_eq!(
            result,
            Err(UnitsError {
                code: gnu_units_sys::E_NOTROOT as c_int
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
    fn factor_returns_parsed_value() {
        let unit = Unit::parse("5").unwrap();

        assert_eq!(unit.factor(), 5.0);
    }

    #[test]
    fn default_factor_is_one() {
        let unit = Unit::default();

        assert_eq!(unit.factor(), 1.0);
    }

    #[test]
    fn free_parse_delegates_to_unit_parse() {
        let unit = parse("5").unwrap();

        assert_eq!(unit.factor(), 5.0);
    }

    #[rstest]
    #[case::dimensionless_to_itself("5", "1", 5.0)]
    #[case::km_to_m("km", "m", 1000.0)]
    #[case::identity_m_to_m("m", "m", 1.0)]
    fn convert_to_compatible_units(#[case] from: &str, #[case] to: &str, #[case] expected: f64) {
        let from_unit = Unit::parse(from).unwrap();
        let to_unit = Unit::parse(to).unwrap();

        let result = from_unit.convert_to(to_unit).unwrap();

        assert_eq!(result, expected);
    }

    #[test]
    fn convert_to_with_numeric_prefix() {
        let from_unit = Unit::parse("5 km").unwrap();
        let to_unit = Unit::parse("miles").unwrap();

        let result = from_unit.convert_to(to_unit).unwrap();

        // 5 km ≈ 3.10686 miles
        assert!((result - 3.10686).abs() < 1e-4);
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

    #[test]
    fn convert_km_to_m_returns_factor() {
        let result = convert("km", "m").unwrap();

        assert_eq!(result, 1000.0);
    }

    #[test]
    fn error_on_convert_invalid_from_expression() {
        let result = convert(")", "m");

        assert_eq!(
            result,
            Err(UnitsError {
                code: gnu_units_sys::E_PARSE as c_int
            })
        );
    }

    #[test]
    fn error_on_convert_invalid_to_expression() {
        let result = convert("m", ")");

        assert_eq!(
            result,
            Err(UnitsError {
                code: gnu_units_sys::E_PARSE as c_int
            })
        );
    }

    #[test]
    fn error_on_convert_incompatible_dimensions() {
        let result = convert("km", "kg");

        assert_eq!(
            result,
            Err(UnitsError {
                code: gnu_units_sys::E_NOTANUMBER as c_int
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

    #[test]
    fn parse_known_prefix_kilo() {
        let unit = Unit::parse("kilogram").unwrap();

        let kg = unit.convert_to(Unit::parse("gram").unwrap()).unwrap();

        assert_eq!(kg, 1000.0);
    }

    #[test]
    fn parse_temperature_diff_unit() {
        // degF is registered as a plain unit via newunit in definitions.units.
        // Its definition uses the fraction-slash `|` (e.g. `5|9 degC`), which
        // replace_operators converts to `|` at load time.
        let result = convert("degF", "degC").unwrap();

        assert!((result - 5.0 / 9.0).abs() < 1e-10);
    }

    #[test]
    fn parse_table_unit_gasmark() {
        // gasmark[degR] is registered via newtable in definitions.units.
        // Parsing a specific table value (gasmark1) returns a unit in degR.
        let unit = Unit::parse("gasmark1").unwrap();

        assert!(unit.factor() > 0.0);
    }

    #[test]
    fn parse_element_from_included_file() {
        // mercury is defined as a weighted sum of isotopic masses in elements.units;
        // converting to the dimensionless unit "1" yields the standard atomic weight.
        let result = convert("mercury", "1").unwrap();

        assert!((result - 200.59).abs() < 0.01);
    }

    #[test]
    fn parse_utf8_unit_loaded() {
        // µ- (U+00B5) is registered as an alias for micro inside a !utf8 block;
        // utf8mode=1 ensures the block is processed.
        let result = convert("\u{00B5}m", "m").unwrap();

        assert_eq!(result, 1e-6);
    }

    #[test]
    fn parse_knot_in_meters_per_second() {
        // knot = nauticalmile / hr = 1852 m / 3600 s ≈ 0.5144 m/s
        let result = convert("knot", "m/s").unwrap();

        assert!((result - 0.514_444).abs() < 1e-4);
    }

    #[test]
    fn parse_function_definition_tempf() {
        // tempF(x) is registered via newfunction; parsing an invocation
        // confirms function definitions were loaded correctly.
        let unit = Unit::parse("tempF(32)").unwrap();

        assert!((unit.factor() - 273.15).abs() < 0.01);
    }

    #[cfg(feature = "currency-update")]
    #[test]
    fn parse_unit_from_currency_include() {
        // currency.units is loaded via !include, USD is defined there.
        let unit = Unit::parse("USD").unwrap();

        assert!(unit.factor() > 0.0);
    }

    #[cfg(feature = "currency-update")]
    #[test]
    fn parse_unit_from_cpi_include() {
        // cpi.units is loaded via !include, UScpi_now is a simple numeric
        // constant defined there, confirming the include was processed.
        let unit = Unit::parse("UScpi_now").unwrap();

        assert!(unit.factor() > 0.0);
    }

    #[test]
    fn convert_inches_to_cm() {
        // inch = 2.54 cm exactly; verifies prefix (centi-) + base unit (m).
        let result = convert("inch", "cm").unwrap();

        assert!((result - 2.54).abs() < 1e-10);
    }

    #[test]
    fn convert_hour_to_s() {
        // hour = 60 min = 3600 s
        let result = convert("hour", "s").unwrap();

        assert_eq!(result, 3600.0);
    }

    #[test]
    fn parse_line_continuation_unit() {
        // spherevolume(r) is defined with a line-continuation backslash;
        // successful parsing confirms the continuation-joining logic works.
        let in_m3 = convert("spherevolume(1 m)", "m^3").unwrap();

        assert!((in_m3 - 4.18879).abs() < 1e-4);
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

        for def in defs {
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

    #[test]
    fn all_prefix_entries_have_names_ending_with_dash() {
        let defs = list_definitions();

        for def in defs.iter().filter(|d| d.kind == DefinitionKind::Prefix) {
            assert!(
                def.name.ends_with('-'),
                "prefix entry '{}' does not end with '-'",
                def.name
            );
        }
    }

    #[test]
    fn all_table_entries_have_bracket_in_name() {
        let defs = list_definitions();

        for def in defs.iter().filter(|d| d.kind == DefinitionKind::Table) {
            assert!(
                def.name.contains('['),
                "table entry '{}' does not contain '['",
                def.name
            );
        }
    }

    #[test]
    fn all_function_entries_have_paren_in_name() {
        let defs = list_definitions();

        for def in defs.iter().filter(|d| d.kind == DefinitionKind::Function) {
            assert!(
                def.name.contains('('),
                "function entry '{}' does not contain '('",
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

    #[test]
    fn base_units_meter_contains_m() {
        let unit = Unit::parse("m").unwrap();

        let result = unit.base_units();

        assert!(result.contains('m'), "expected 'm' in {:?}", result);
    }

    #[test]
    fn base_units_compound_contains_slash() {
        let unit = Unit::parse("kg m/s^2").unwrap();

        let result = unit.base_units();

        assert!(result.contains(" / "), "expected ' / ' in {:?}", result);
    }

    #[test]
    fn base_units_inverse_starts_with_one() {
        let unit = Unit::parse("1/s").unwrap();

        let result = unit.base_units();

        assert!(
            result.starts_with("1 / "),
            "expected '1 / ' prefix in {:?}",
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

    #[test]
    fn conformable_m_contains_length_units() {
        let result = conformable("m").unwrap();

        // "km" is a prefix+base combination, not a standalone DB entry; use full names
        assert!(
            result.contains(&"meter".to_string()),
            "meter missing from {:?}",
            result
        );
        assert!(
            result.contains(&"mile".to_string()),
            "mile missing from {:?}",
            result
        );
        assert!(
            result.contains(&"ft".to_string()),
            "ft missing from {:?}",
            result
        );
        assert!(
            result.contains(&"inch".to_string()),
            "inch missing from {:?}",
            result
        );
    }

    #[test]
    fn conformable_m_does_not_contain_wrong_domain() {
        let result = conformable("m").unwrap();

        assert!(!result.contains(&"kg".to_string()), "kg should not appear");
        assert!(
            !result.contains(&"second".to_string()),
            "second should not appear"
        );
    }

    #[test]
    fn error_on_conformable_invalid_expression() {
        // ")" is an invalid token for the bison grammar → E_PARSE propagated from Unit::parse
        let result = conformable(")");

        assert!(result.is_err(), "expected Err for invalid expression");
    }

    #[test]
    fn conformable_kg_contains_mass_units() {
        let result = conformable("kg").unwrap();

        assert!(
            result.contains(&"lb".to_string()),
            "lb missing from {:?}",
            result
        );
        assert!(
            result.contains(&"g".to_string()),
            "g missing from {:?}",
            result
        );
    }
}
