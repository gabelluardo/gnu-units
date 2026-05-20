//! Safe wrappers around `gnu_units_sys` FFI calls.
//!
//! Functions that access global C state acquire [`GNU_UNITS_MUTEX`] internally.

use std::ffi::CString;
use std::os::raw::c_int;
use std::ptr;
use std::sync::Mutex;

use super::{Result, Unit, UnitsError};

static GNU_UNITS_MUTEX: Mutex<()> = Mutex::new(());

/// Acquires `GNU_UNITS_MUTEX` and returns the guard. Recovers from poison.
pub(crate) fn lock() -> std::sync::MutexGuard<'static, ()> {
    GNU_UNITS_MUTEX.lock().unwrap_or_else(|e| e.into_inner())
}

/// Parses `input` and returns the resulting [`Unit`]. Acquires `GNU_UNITS_MUTEX` internally.
pub(crate) fn parseunit(input: &str) -> Result<Unit> {
    let _guard = lock();

    let mut unit = Unit::new();
    let c_input = CString::new(input).map_err(|_| UnitsError {
        code: gnu_units_sys::E_PARSE as c_int,
    })?;
    // SAFETY: unit is freshly initialized. c_input is a valid NUL-terminated
    // C string. parseunit writes into unit without reading uninitialized data.
    // GNU_UNITS_MUTEX is held.
    let code = unsafe {
        gnu_units_sys::parseunit(
            unit.as_mut_ptr(),
            c_input.as_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    UnitsError::from_code(code).map_or(Ok(unit), Err)
}

/// Multiplies `lhs` by `rhs` in place.
pub(crate) fn multunit(lhs: &mut Unit, rhs: &mut Unit) -> Result<()> {
    // SAFETY: Both units are valid. multunit moves pointers from rhs
    // to lhs via moveproduct. No global state is accessed.
    let code = unsafe { gnu_units_sys::multunit(lhs.as_mut_ptr(), rhs.as_mut_ptr()) };
    UnitsError::from_code(code).map_or(Ok(()), Err)
}

/// Divides `lhs` by `rhs` in place.
pub(crate) fn divunit(lhs: &mut Unit, rhs: &mut Unit) -> Result<()> {
    // SAFETY: Both units are valid. divunit moves pointers from rhs
    // to lhs via moveproduct. No global state is accessed.
    let code = unsafe { gnu_units_sys::divunit(lhs.as_mut_ptr(), rhs.as_mut_ptr()) };
    UnitsError::from_code(code).map_or(Ok(()), Err)
}

/// Adds `rhs` to `lhs` in place. Acquires `GNU_UNITS_MUTEX` internally.
pub(crate) fn addunit(lhs: &mut Unit, rhs: &mut Unit) -> Result<()> {
    let _guard = lock();
    // SAFETY: Both units are valid. addunit accesses global state.
    // GNU_UNITS_MUTEX is held.
    let code = unsafe { gnu_units_sys::addunit(lhs.as_mut_ptr(), rhs.as_mut_ptr()) };
    UnitsError::from_code(code).map_or(Ok(()), Err)
}

/// Raises `unit` to `power`. Acquires `GNU_UNITS_MUTEX` internally.
pub(crate) fn expunit(unit: &mut Unit, power: c_int) -> Result<()> {
    let _guard = lock();
    // SAFETY: unit is valid. expunit modifies factor and dimensions.
    // GNU_UNITS_MUTEX is held.
    let code = unsafe { gnu_units_sys::expunit(unit.as_mut_ptr(), power) };
    UnitsError::from_code(code).map_or(Ok(()), Err)
}

/// Takes the nth root of `unit`. Acquires `GNU_UNITS_MUTEX` internally.
pub(crate) fn rootunit(unit: &mut Unit, n: c_int) -> Result<()> {
    let _guard = lock();
    // SAFETY: unit is valid. rootunit modifies factor and dimensions.
    // GNU_UNITS_MUTEX is held.
    let code = unsafe { gnu_units_sys::rootunit(unit.as_mut_ptr(), n) };
    UnitsError::from_code(code).map_or(Ok(()), Err)
}

/// Converts `unit` to a dimensionless number. Acquires `GNU_UNITS_MUTEX` internally.
pub(crate) fn unit2num(unit: &mut Unit) -> Result<()> {
    let _guard = lock();
    // SAFETY: unit is valid. unit2num modifies unit in place.
    // GNU_UNITS_MUTEX is held.
    let code = unsafe { gnu_units_sys::unit2num(unit.as_mut_ptr()) };
    UnitsError::from_code(code).map_or(Ok(()), Err)
}

/// Evaluates the named function `to` on the parsed unit `from`, returning
/// the fully reduced result. Returns `None` when `to` is not a known
/// function, `from` cannot be parsed, or evaluation/reduction fails.
/// Acquires `GNU_UNITS_MUTEX` internally.
pub(crate) fn convert_func(from: &str, to: &str) -> Option<Unit> {
    let _guard = lock();

    let c_name = CString::new(to).ok()?;
    let c_from = CString::new(from).ok()?;
    // SAFETY: c_name is a valid NUL-terminated CString. GNU_UNITS_MUTEX is held.
    let func_ptr = unsafe { gnu_units_sys::fnlookup(c_name.as_ptr()) };
    if func_ptr.is_null() {
        return None;
    }

    let mut unit = Unit::new();
    // SAFETY: unit is freshly initialized, c_from is a valid NUL-terminated
    // CString. parseunit writes into unit without reading uninitialized data.
    // GNU_UNITS_MUTEX is held.
    let code = unsafe {
        gnu_units_sys::parseunit(
            unit.as_mut_ptr(),
            c_from.as_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if UnitsError::from_code(code).is_some() {
        return None;
    }

    // SAFETY: unit is valid and exclusively owned; returns pointer to self.raw.
    let mut unit_ptr = unsafe { unit.as_mut_ptr() };
    // SAFETY: unit_ptr points to a valid parsed unit, func_ptr is verified
    // non-null. evalfunc modifies unit in place through the pointer. GNU_UNITS_MUTEX is held.
    let code = unsafe { gnu_units_sys::evalfunc(1, &mut unit_ptr, func_ptr, 1, 0) };
    if UnitsError::from_code(code).is_some() {
        return None;
    }

    // Verify evalfunc did not reassign the pointer (would indicate UB).
    debug_assert!(std::ptr::eq(unit_ptr, unsafe { unit.as_mut_ptr() }));

    // SAFETY: unit_ptr still points to unit.raw (evalfunc does not reassign
    // the pointer). completereduce reduces it to base units. GNU_UNITS_MUTEX is held.
    let code = unsafe { gnu_units_sys::completereduce(unit_ptr) };
    if UnitsError::from_code(code).is_some() {
        return None;
    }

    Some(unit)
}

/// Deep-copies `src`.
pub(crate) fn unitcopy(src: &Unit) -> Unit {
    let mut dest = Unit::new();
    // SAFETY: C unitcopy only reads from src (via copyproduct/dupstr),
    // never writes to it. The *mut cast satisfies the C binding signature
    // but no mutation occurs. dest is freshly initialized.
    unsafe {
        gnu_units_sys::unitcopy(dest.as_mut_ptr(), &src.raw as *const _ as *mut _);
    }
    dest
}

/// Frees the C heap memory owned by `src`.
pub(crate) fn freeunit(src: &mut Unit) {
    // SAFETY: self.raw was initialised by initializeunit and all pointer
    // fields are either NULL (from zeroed/init) or valid C allocations
    // (from parseunit/unitcopy). freeunit calls free() on those
    // allocations, it only reads NULLUNIT (a constant) and does not
    // access any mutable global state.
    unsafe {
        gnu_units_sys::freeunit(src.as_mut_ptr());
    }
}

/// Registers a new unit prefix. Acquires `GNU_UNITS_MUTEX` internally.
pub(crate) fn newprefix(
    name: &str,
    def: &str,
    linenum: c_int,
    file_ptr: *mut std::os::raw::c_char,
) {
    let _guard = lock();
    let mut name_buf: Vec<u8> = name.bytes().chain(std::iter::once(0)).collect();
    let mut def_buf: Vec<u8> = def.bytes().chain(std::iter::once(0)).collect();
    let mut count: c_int = 0;
    // SAFETY: name_buf and def_buf are null-terminated mutable buffers.
    // The C function copies both strings internally (dupstr).
    // file_ptr is a leaked CString valid for the process lifetime.
    // GNU_UNITS_MUTEX is held.
    unsafe {
        gnu_units_sys::newprefix(
            name_buf.as_mut_ptr() as *mut std::os::raw::c_char,
            def_buf.as_mut_ptr() as *mut std::os::raw::c_char,
            &mut count,
            linenum,
            file_ptr,
            ptr::null_mut(),
            1,
        );
    }
}

/// Registers a new piecewise table. Acquires `GNU_UNITS_MUTEX` internally.
pub(crate) fn newtable(name: &str, def: &str, linenum: c_int, file_ptr: *mut std::os::raw::c_char) {
    let _guard = lock();
    let mut name_buf: Vec<u8> = name.bytes().chain(std::iter::once(0)).collect();
    let mut def_buf: Vec<u8> = def.bytes().chain(std::iter::once(0)).collect();
    let mut count: c_int = 0;
    // SAFETY: name_buf and def_buf are null-terminated mutable buffers.
    // The C function copies both strings internally (dupstr).
    // file_ptr is a leaked CString valid for the process lifetime.
    // GNU_UNITS_MUTEX is held.
    unsafe {
        gnu_units_sys::newtable(
            name_buf.as_mut_ptr() as *mut std::os::raw::c_char,
            def_buf.as_mut_ptr() as *mut std::os::raw::c_char,
            &mut count,
            linenum,
            file_ptr,
            ptr::null_mut(),
            1,
        );
    }
}

/// Registers a new conversion function. Acquires `GNU_UNITS_MUTEX` internally.
pub(crate) fn newfunction(
    name: &str,
    def: &str,
    linenum: c_int,
    file_ptr: *mut std::os::raw::c_char,
) {
    let _guard = lock();
    let mut name_buf: Vec<u8> = name.bytes().chain(std::iter::once(0)).collect();
    let mut def_buf: Vec<u8> = def.bytes().chain(std::iter::once(0)).collect();
    let mut count: c_int = 0;
    // SAFETY: name_buf and def_buf are null-terminated mutable buffers.
    // The C function copies both strings internally (dupstr).
    // file_ptr is a leaked CString valid for the process lifetime.
    // GNU_UNITS_MUTEX is held.
    unsafe {
        gnu_units_sys::newfunction(
            name_buf.as_mut_ptr() as *mut std::os::raw::c_char,
            def_buf.as_mut_ptr() as *mut std::os::raw::c_char,
            &mut count,
            linenum,
            file_ptr,
            ptr::null_mut(),
            1,
        );
    }
}

/// Registers a new unit. Acquires `GNU_UNITS_MUTEX` internally.
pub(crate) fn newunit(name: &str, def: &str, linenum: c_int, file_ptr: *mut std::os::raw::c_char) {
    let _guard = lock();
    let mut name_buf: Vec<u8> = name.bytes().chain(std::iter::once(0)).collect();
    let mut def_buf: Vec<u8> = def.bytes().chain(std::iter::once(0)).collect();
    let mut count: c_int = 0;
    // SAFETY: name_buf and def_buf are null-terminated mutable buffers.
    // The C function copies both strings internally (dupstr).
    // file_ptr is a leaked CString valid for the process lifetime.
    // GNU_UNITS_MUTEX is held.
    unsafe {
        gnu_units_sys::newunit(
            name_buf.as_mut_ptr() as *mut std::os::raw::c_char,
            def_buf.as_mut_ptr() as *mut std::os::raw::c_char,
            &mut count,
            linenum,
            file_ptr,
            ptr::null_mut(),
            1,
            0,
        );
    }
}

/// Registers a new unit list alias. Acquires `GNU_UNITS_MUTEX` internally.
pub(crate) fn newalias(name: &str, def: &str, linenum: c_int, file_ptr: *mut std::os::raw::c_char) {
    let _guard = lock();
    let mut name_buf: Vec<u8> = name.bytes().chain(std::iter::once(0)).collect();
    let mut def_buf: Vec<u8> = def.bytes().chain(std::iter::once(0)).collect();
    // SAFETY: name_buf and def_buf are null-terminated mutable buffers.
    // The C function copies both strings internally (dupstr).
    // file_ptr is a leaked CString valid for the process lifetime.
    // GNU_UNITS_MUTEX is held.
    unsafe {
        gnu_units_sys::newalias(
            name_buf.as_mut_ptr() as *mut std::os::raw::c_char,
            def_buf.as_mut_ptr() as *mut std::os::raw::c_char,
            linenum,
            file_ptr,
            ptr::null_mut(),
        );
    }
}
