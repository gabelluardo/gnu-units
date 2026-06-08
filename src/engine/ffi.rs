//! Safe wrappers around `gnu_units_sys` FFI calls, implementing the uniform engine API.
//!
//! Functions that access global C state acquire [`GNU_UNITS_MUTEX`] internally.

use std::ffi::{CStr, CString};
use std::os::raw::c_int;
use std::ptr;
use std::sync::Mutex;

use crate::ErrorCode;
use crate::{Result, UnitsError};

static GNU_UNITS_MUTEX: Mutex<()> = Mutex::new(());

pub(crate) type RawUnit = gnu_units_sys::unittype;

/// Acquires `GNU_UNITS_MUTEX` and returns the guard. Recovers from poison.
pub(crate) fn lock() -> std::sync::MutexGuard<'static, ()> {
    GNU_UNITS_MUTEX.lock().unwrap_or_else(|e| e.into_inner())
}

fn from_code(code: c_int) -> Option<UnitsError> {
    if code == ErrorCode::Normal as i32 {
        return None;
    }
    Some(UnitsError(ErrorCode::from_raw(code)))
}

pub(crate) fn unit_new() -> RawUnit {
    let mut raw = std::mem::MaybeUninit::<gnu_units_sys::unittype>::zeroed();
    // SAFETY: zeroed ensures all pointer slots are null. initializeunit sets
    // factor=1.0 and the first terminator slots without accessing global state.
    unsafe {
        gnu_units_sys::initializeunit(raw.as_mut_ptr());
        raw.assume_init()
    }
}

pub(crate) fn unit_parse(input: &str) -> Result<RawUnit> {
    let _guard = lock();
    let mut raw = unit_new();
    let c_input = CString::new(input).map_err(|_| UnitsError(ErrorCode::Parse))?;
    // SAFETY: raw is freshly initialized; c_input is a valid NUL-terminated string.
    // GNU_UNITS_MUTEX is held.
    let code = unsafe {
        gnu_units_sys::parseunit(&mut raw, c_input.as_ptr(), ptr::null_mut(), ptr::null_mut())
    };
    if let Some(err) = from_code(code) {
        unit_drop(&mut raw);
        return Err(err);
    }
    Ok(raw)
}

pub(crate) fn unit_factor(unit: &RawUnit) -> f64 {
    unit.factor
}

pub(crate) fn unit_base_units(unit: &RawUnit) -> String {
    // SAFETY: NULLUNIT is a valid sentinel pointer defined by the C library.
    let null_sentinel = unsafe { gnu_units_sys::NULLUNIT };
    let mut numerators: Vec<String> = Vec::new();
    let mut denominators: Vec<String> = Vec::new();

    for &ptr in unit.numerator.iter() {
        if ptr.is_null() {
            break;
        }
        if ptr == null_sentinel {
            continue;
        }
        // SAFETY: ptr is a valid NUL-terminated C string owned by the C lib.
        let s = unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() };
        numerators.push(s);
    }
    for &ptr in unit.denominator.iter() {
        if ptr.is_null() {
            break;
        }
        if ptr == null_sentinel {
            continue;
        }
        // SAFETY: ptr is a valid NUL-terminated C string owned by the C lib.
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
        "1".to_owned()
    } else {
        numerators.join(" ")
    };
    format!("{num_part} / {}", denominators.join(" "))
}

pub(crate) fn unit_multiply(lhs: &mut RawUnit, rhs: &RawUnit) -> Result<()> {
    // multunit moves pointers from rhs to lhs (via moveproduct), so we clone rhs first.
    let mut rhs_copy = unit_clone(rhs);
    // SAFETY: Both units are valid. multunit moves pointers from rhs_copy to lhs.
    // No global state is accessed.
    let code = unsafe { gnu_units_sys::multunit(lhs, &mut rhs_copy) };
    unit_drop(&mut rhs_copy);
    from_code(code).map_or(Ok(()), Err)
}

pub(crate) fn unit_divide(lhs: &mut RawUnit, rhs: &RawUnit) -> Result<()> {
    // divunit moves pointers from rhs to lhs (via moveproduct), so we clone rhs first.
    let mut rhs_copy = unit_clone(rhs);
    // SAFETY: Both units are valid. divunit moves pointers from rhs_copy to lhs.
    // No global state is accessed.
    let code = unsafe { gnu_units_sys::divunit(lhs, &mut rhs_copy) };
    unit_drop(&mut rhs_copy);
    from_code(code).map_or(Ok(()), Err)
}

pub(crate) fn unit_add(lhs: &mut RawUnit, rhs: &RawUnit) -> Result<()> {
    let _guard = lock();
    // addunit calls completereduce on both args and frees rhs on success.
    // Clone rhs so callers retain a valid immutable reference.
    let mut rhs_copy = unit_clone(rhs);
    // SAFETY: Both units are valid. addunit accesses global state. GNU_UNITS_MUTEX is held.
    let code = unsafe { gnu_units_sys::addunit(lhs, &mut rhs_copy) };
    // freeunit is idempotent: safe to call even if addunit already freed rhs_copy on success.
    unit_drop(&mut rhs_copy);
    from_code(code).map_or(Ok(()), Err)
}

pub(crate) fn unit_invert(unit: &mut RawUnit) {
    // SAFETY: unit is a valid initialized unit. invertunit does not access global state.
    unsafe {
        gnu_units_sys::invertunit(unit);
    }
}

pub(crate) fn unit_pow(unit: &mut RawUnit, power: i32) -> Result<()> {
    if power < 0 {
        return Err(UnitsError(ErrorCode::BadNum));
    }
    let _guard = lock();
    // SAFETY: unit is valid. expunit modifies factor and dimensions. GNU_UNITS_MUTEX is held.
    let code = unsafe { gnu_units_sys::expunit(unit, power) };
    from_code(code).map_or(Ok(()), Err)
}

pub(crate) fn unit_root(unit: &mut RawUnit, n: i32) -> Result<()> {
    if n <= 0 {
        return Err(UnitsError(ErrorCode::NotRoot));
    }
    let _guard = lock();
    // SAFETY: unit is valid. rootunit modifies factor and dimensions. GNU_UNITS_MUTEX is held.
    let code = unsafe { gnu_units_sys::rootunit(unit, n) };
    from_code(code).map_or(Ok(()), Err)
}

pub(crate) fn unit_to_number(unit: &RawUnit) -> Result<f64> {
    let mut tmp = unit_clone(unit);
    let _guard = lock();
    // SAFETY: tmp is valid. unit2num reduces it and calls freeunit internally on success.
    // GNU_UNITS_MUTEX is held.
    let code = unsafe { gnu_units_sys::unit2num(&mut tmp) };
    let factor = tmp.factor;
    // Call freeunit regardless: idempotent after unit2num success, necessary after failure.
    // SAFETY: tmp was initialised by unit_clone. freeunit frees C-allocated string pointers.
    unsafe { gnu_units_sys::freeunit(&mut tmp) };
    from_code(code).map_or(Ok(factor), Err)
}

pub(crate) fn unit_is_conformable(a: &RawUnit, b: &RawUnit) -> bool {
    let mut ratio = unit_clone(a);
    if unit_divide(&mut ratio, b).is_err() {
        unit_drop(&mut ratio);
        return false;
    }
    let result = unit_to_number(&ratio).is_ok();
    unit_drop(&mut ratio);
    result
}

pub(crate) fn unit_clone(src: &RawUnit) -> RawUnit {
    let mut dest = unit_new();
    // SAFETY: C unitcopy reads from src (via copyproduct/dupstr), never writes to it.
    // The *mut cast satisfies the C binding signature but no mutation occurs.
    // dest is freshly initialized.
    unsafe {
        gnu_units_sys::unitcopy(&mut dest, src as *const _ as *mut _);
    }
    dest
}

pub(crate) fn unit_drop(unit: &mut RawUnit) {
    // SAFETY: unit was initialized by unit_new or unitcopy. freeunit frees C-allocated
    // string pointers. Safe to call multiple times (idempotent).
    unsafe {
        gnu_units_sys::freeunit(unit);
    }
}

pub(crate) fn convert_func(from: &str, to: &str) -> crate::Result<f64> {
    let _guard = lock();

    let c_name = CString::new(to).map_err(|_| UnitsError(ErrorCode::NotAFunc))?;
    let c_from = CString::new(from).map_err(|_| UnitsError(ErrorCode::NotAFunc))?;
    // SAFETY: c_name is a valid NUL-terminated CString. GNU_UNITS_MUTEX is held.
    let func_ptr = unsafe { gnu_units_sys::fnlookup(c_name.as_ptr()) };
    if func_ptr.is_null() {
        return Err(UnitsError(ErrorCode::NotAFunc));
    }

    let mut unit = unit_new();
    // SAFETY: unit is freshly initialized, c_from is valid. GNU_UNITS_MUTEX is held.
    let code = unsafe {
        gnu_units_sys::parseunit(&mut unit, c_from.as_ptr(), ptr::null_mut(), ptr::null_mut())
    };
    if let Some(err) = from_code(code) {
        unit_drop(&mut unit);
        return Err(err);
    }

    let mut unit_ptr = &mut unit as *mut RawUnit;
    // SAFETY: unit_ptr points to a valid parsed unit, func_ptr is verified non-null.
    // evalfunc modifies unit in place through the pointer. GNU_UNITS_MUTEX is held.
    let code = unsafe { gnu_units_sys::evalfunc(1, &mut unit_ptr, func_ptr, 1, 0) };
    if let Some(err) = from_code(code) {
        unit_drop(&mut unit);
        return Err(err);
    }

    debug_assert!(std::ptr::eq(unit_ptr, &mut unit as *mut _));

    // SAFETY: unit_ptr still points to unit (evalfunc does not reassign the pointer).
    // completereduce reduces to base units. GNU_UNITS_MUTEX is held.
    let code = unsafe { gnu_units_sys::completereduce(unit_ptr) };
    if let Some(err) = from_code(code) {
        unit_drop(&mut unit);
        return Err(err);
    }

    let factor = unit.factor;
    unit_drop(&mut unit);
    Ok(factor)
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
