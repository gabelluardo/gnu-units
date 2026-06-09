//! Parses the embedded `definitions.units` file and registers entries into
//! either the vendored C-library database (via FFI) or the pure-Rust native
//! database, depending on the active feature.
//!
//! Produces a sorted [`Vec<Definition>`] exposed through the public API.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

#[cfg(feature = "vendored")]
use std::ffi::{CStr, CString};
#[cfg(feature = "vendored")]
use std::os::raw::c_int;
#[cfg(feature = "vendored")]
use std::sync::Mutex;

#[cfg(feature = "vendored")]
use crate::engine::ffi;

#[cfg(feature = "native")]
use crate::engine::native::database::{Database, init as db_init};

use crate::units;

/// The kind of a definition entry in the GNU units database.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum DefinitionKind {
    /// A plain unit (e.g. `meter`, `second`).
    Unit,
    /// A unit prefix (e.g. `kilo-`, `mega-`).
    Prefix,
    /// A conversion function (e.g. `tempC(x)`).
    Function,
    /// A piecewise unit table (e.g. `dBV[...]`).
    Table,
    /// A unit list alias (defined via `!unitlist`).
    Alias,
}

impl DefinitionKind {
    fn from_name(name: &str) -> DefinitionKind {
        match name {
            _ if name.ends_with('-') => DefinitionKind::Prefix,
            _ if name.contains('[') => DefinitionKind::Table,
            _ if name.contains('(') => DefinitionKind::Function,
            _ => DefinitionKind::Unit,
        }
    }
}

/// A single entry from the GNU units definitions database.
#[derive(Debug, Clone, Eq)]
pub struct Definition {
    /// The name of the unit, prefix, function, table, or alias.
    pub name: String,
    /// The definition string for this entry.
    pub definition: String,
    /// What kind of definition this is.
    pub kind: DefinitionKind,
}

impl PartialEq for Definition {
    fn eq(&self, other: &Self) -> bool {
        self.canonical_name() == other.canonical_name() && self.kind == other.kind
    }
}

impl Ord for Definition {
    fn cmp(&self, other: &Self) -> Ordering {
        self.canonical_name()
            .cmp(other.canonical_name())
            .then_with(|| self.kind.cmp(&other.kind))
    }
}

impl PartialOrd for Definition {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Definition {
    /// Returns the canonical lookup name used for deduplication:
    /// - Prefixes: trailing `-` stripped (`kilo-` → `kilo`)
    /// - Functions: stripped from `(` onward (`tempC(x)` → `tempC`)
    /// - Tables: stripped from `[` onward (`gasmark[degR]` → `gasmark`)
    /// - Units and aliases: full name as-is
    pub fn canonical_name(&self) -> &str {
        if let Some(idx) = self.name.find('(') {
            &self.name[..idx]
        } else if let Some(idx) = self.name.find('[') {
            &self.name[..idx]
        } else {
            self.name.trim_end_matches('-')
        }
    }
}

/// Sorted list of all known definitions, populated once at start-up.
pub(crate) static DEFINITIONS: LazyLock<RwLock<Vec<Definition>>> = LazyLock::new(|| {
    #[cfg(feature = "vendored")]
    {
        let _guard = ffi::lock();
        // SAFETY: static C strings live for the process lifetime.
        // LazyLock guarantees single-threaded execution here.
        unsafe {
            gnu_units_sys::mylocale = c"en_US".as_ptr() as *mut std::os::raw::c_char;
            gnu_units_sys::progname = c"gnu-units".as_ptr() as *mut std::os::raw::c_char;
            gnu_units_sys::utf8mode = 1;
        }
    }

    #[cfg(feature = "native")]
    let mut native_db = Database::default();

    let content = include_str!("../data/definitions.units");
    let mut env: HashMap<String, String> = HashMap::new();

    #[cfg(feature = "vendored")]
    let mut defs = load_core(content, c"definitions.units", &mut env);
    #[cfg(feature = "native")]
    let mut defs = load_core(content, c"definitions.units", &mut env, &mut native_db);

    #[cfg(feature = "native")]
    db_init(native_db);

    // Emulate C last-write-wins: reverse so last-in-file entries come first.
    defs.reverse();
    defs.sort();
    defs.dedup();
    defs.sort_by(|a, b| a.name.cmp(&b.name));
    RwLock::new(defs)
});

/// Forces the definitions to be loaded exactly once.
pub(crate) fn ensure_definitions() {
    LazyLock::force(&DEFINITIONS);
}

#[cfg(feature = "vendored")]
static FILE_PTRS: LazyLock<Mutex<HashMap<Vec<u8>, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(feature = "vendored")]
fn intern_filename(filename: &CStr) -> *mut std::os::raw::c_char {
    let key = filename.to_bytes().to_vec();
    let mut map = FILE_PTRS.lock().unwrap_or_else(|e| e.into_inner());
    let addr = *map
        .entry(key)
        .or_insert_with(|| CString::new(filename.to_bytes()).unwrap().into_raw() as usize);
    addr as *mut std::os::raw::c_char
}

pub(crate) fn replace_operators(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let replacement = match ch {
            '\u{2012}' | '\u{2013}' | '\u{2212}' => "-",
            '\u{00D7}' | '\u{2A09}' | '\u{00B7}' | '\u{22C5}' => "*",
            '\u{00F7}' | '\u{2215}' => "/",
            '\u{2044}' => "|",
            '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}' => " ",
            '\u{200B}' | '\u{200C}' => "",
            _ => {
                out.push(ch);
                continue;
            }
        };
        out.push_str(replacement);
    }
    out
}

#[cfg_attr(not(feature = "currency-update"), allow(dead_code))]
pub(crate) fn load_definitions(content: &str, filename: &std::ffi::CStr) -> Vec<Definition> {
    let mut env: HashMap<String, String> = HashMap::new();
    #[cfg(feature = "vendored")]
    {
        load_core(content, filename, &mut env)
    }
    #[cfg(feature = "native")]
    {
        let _ = filename;
        let db_rw = crate::engine::native::database::get();
        let mut db = db_rw.write().unwrap_or_else(|e| e.into_inner());
        load_core(content, c"", &mut env, &mut db)
    }
}

#[cfg(feature = "vendored")]
fn load_core(
    content: &str,
    filename: &std::ffi::CStr,
    env: &mut HashMap<String, String>,
) -> Vec<Definition> {
    let file_ptr = intern_filename(filename);
    // SAFETY: utf8mode is a simple i32 global set during initialization.
    // It is only read here after initialization is complete.
    let utf8mode = unsafe { gnu_units_sys::utf8mode };
    load_lines_vendored(content, env, file_ptr, utf8mode)
}

#[cfg(feature = "vendored")]
fn load_lines_vendored(
    content: &str,
    env: &mut HashMap<String, String>,
    file_ptr: *mut std::os::raw::c_char,
    utf8mode: i32,
) -> Vec<Definition> {
    let mut results = Vec::new();
    let mut wrong_locale = false;
    let mut in_utf8 = false;
    let mut wrong_var = false;

    for (linenum, raw_line) in join_continuations(content) {
        let line = strip_comment(&raw_line);
        let line = line.trim();

        if let Some(rest) = line.strip_prefix('!') {
            let (directive, arg) = split2(rest.trim());
            match directive {
                "locale" => {
                    wrong_locale = arg != "en_US";
                }
                "endlocale" => {
                    wrong_locale = false;
                }
                "utf8" => {
                    in_utf8 = true;
                }
                "endutf8" => {
                    in_utf8 = false;
                }
                "var" => {
                    let (vname, vals) = split2(arg);
                    wrong_var = match lookup_var(vname, env) {
                        Some(v) => !vals.split_whitespace().any(|w| w == v),
                        None => true,
                    };
                }
                "varnot" => {
                    let (vname, vals) = split2(arg);
                    wrong_var = match lookup_var(vname, env) {
                        Some(v) => vals.split_whitespace().any(|w| w == v),
                        None => true,
                    };
                }
                "endvar" => {
                    wrong_var = false;
                }
                "set" if !wrong_locale && !wrong_var => {
                    let (vname, val) = split2(arg);
                    if lookup_var(vname, env).is_none() {
                        env.insert(vname.to_owned(), val.to_owned());
                    }
                }
                "set" | "message" | "prompt" => {}
                "unitlist" if !(wrong_locale || wrong_var || in_utf8 && utf8mode == 0) => {
                    let (name, def) = split2(arg);
                    if !name.is_empty() {
                        ffi::newalias(name, def, linenum as c_int, file_ptr);
                        results.push(Definition {
                            name: name.to_owned(),
                            definition: def.to_owned(),
                            kind: DefinitionKind::Alias,
                        });
                    }
                }
                "unitlist" => {}
                "include" if !(wrong_locale || wrong_var || in_utf8 && utf8mode == 0) => {
                    results.extend(include_vendored(arg, env, utf8mode));
                }
                "include" => {}
                _ => {}
            }
            continue;
        }

        if wrong_locale || wrong_var || (in_utf8 && utf8mode == 0) {
            continue;
        }

        let norm = replace_operators(line);
        let line = norm.trim();
        if line.is_empty() {
            continue;
        }

        let (raw_name, def) = split2(line);
        if raw_name == "-" || def.is_empty() {
            continue;
        }
        let name = raw_name.strip_prefix('+').unwrap_or(raw_name);
        if name.is_empty() {
            continue;
        }

        let lnum = linenum as c_int;
        if name.ends_with('-') {
            ffi::newprefix(name, def, lnum, file_ptr);
        } else if name.contains('[') {
            ffi::newtable(name, def, lnum, file_ptr);
        } else if name.contains('(') {
            ffi::newfunction(name, def, lnum, file_ptr);
        } else {
            ffi::newunit(name, def, lnum, file_ptr);
        }

        results.push(Definition {
            name: name.to_owned(),
            definition: def.to_owned(),
            kind: DefinitionKind::from_name(name),
        });
    }
    results
}

#[cfg(feature = "vendored")]
fn include_vendored(
    arg: &str,
    env: &mut HashMap<String, String>,
    utf8mode: i32,
) -> Vec<Definition> {
    let (content, filename): (&str, &std::ffi::CStr) = match arg {
        "elements.units" => (units::ELEMENTS, c"elements.units"),
        #[cfg(feature = "currency-update")]
        "currency.units" => (units::CURRENCY, c"currency.units"),
        #[cfg(feature = "currency-update")]
        "crypto.units" => (units::CRYPTO, c"crypto.units"),
        #[cfg(feature = "currency-update")]
        "metal_prices.units" => (units::METAL_PRICES, c"metal_prices.units"),
        #[cfg(feature = "currency-update")]
        "cpi.units" => (units::CPI, c"cpi.units"),
        _ => return vec![],
    };
    let file_ptr = intern_filename(filename);
    load_lines_vendored(content, env, file_ptr, utf8mode)
}

#[cfg(feature = "native")]
fn load_core(
    content: &str,
    _filename: &std::ffi::CStr,
    env: &mut HashMap<String, String>,
    db: &mut Database,
) -> Vec<Definition> {
    let mut results = Vec::new();
    let mut wrong_locale = false;
    let mut in_utf8 = false;
    let mut wrong_var = false;
    const UTF8MODE: i32 = 1;

    for (_linenum, raw_line) in join_continuations(content) {
        let line = strip_comment(&raw_line);
        let line = line.trim();

        if let Some(rest) = line.strip_prefix('!') {
            let (directive, arg) = split2(rest.trim());
            match directive {
                "locale" => {
                    wrong_locale = arg != "en_US";
                }
                "endlocale" => {
                    wrong_locale = false;
                }
                "utf8" => {
                    in_utf8 = true;
                }
                "endutf8" => {
                    in_utf8 = false;
                }
                "var" => {
                    let (vname, vals) = split2(arg);
                    wrong_var = match lookup_var(vname, env) {
                        Some(v) => !vals.split_whitespace().any(|w| w == v),
                        None => true,
                    };
                }
                "varnot" => {
                    let (vname, vals) = split2(arg);
                    wrong_var = match lookup_var(vname, env) {
                        Some(v) => vals.split_whitespace().any(|w| w == v),
                        None => true,
                    };
                }
                "endvar" => {
                    wrong_var = false;
                }
                "set" if !wrong_locale && !wrong_var => {
                    let (vname, val) = split2(arg);
                    if lookup_var(vname, env).is_none() {
                        env.insert(vname.to_owned(), val.to_owned());
                    }
                }
                "set" | "message" | "prompt" => {}
                "unitlist" if !(wrong_locale || wrong_var || in_utf8 && UTF8MODE == 0) => {
                    let (name, def) = split2(arg);
                    if !name.is_empty() {
                        results.push(Definition {
                            name: name.to_owned(),
                            definition: def.to_owned(),
                            kind: DefinitionKind::Alias,
                        });
                    }
                }
                "unitlist" => {}
                "include" if !(wrong_locale || wrong_var || in_utf8 && UTF8MODE == 0) => {
                    results.extend(include_native(arg, env, db));
                }
                "include" => {}
                _ => {}
            }
            continue;
        }

        if wrong_locale || wrong_var || (in_utf8 && UTF8MODE == 0) {
            continue;
        }

        let norm = replace_operators(line);
        let line = norm.trim();
        if line.is_empty() {
            continue;
        }

        let (raw_name, def) = split2(line);
        if raw_name == "-" || def.is_empty() {
            continue;
        }
        let name = raw_name.strip_prefix('+').unwrap_or(raw_name);
        if name.is_empty() {
            continue;
        }

        match name {
            _ if name.ends_with('-') => db.insert_prefix(name, def),
            _ if name.contains('[') => db.insert_table(name, def),
            _ if name.contains('(') => db.insert_function(name, def),
            _ => db.insert_unit(name, def),
        }

        results.push(Definition {
            name: name.to_owned(),
            definition: def.to_owned(),
            kind: DefinitionKind::from_name(name),
        });
    }
    results
}

#[cfg(feature = "native")]
fn include_native(
    arg: &str,
    env: &mut HashMap<String, String>,
    db: &mut Database,
) -> Vec<Definition> {
    let (content, _filename): (&str, &std::ffi::CStr) = match arg {
        "elements.units" => (units::ELEMENTS, c"elements.units"),
        #[cfg(feature = "currency-update")]
        "currency.units" => (units::CURRENCY, c"currency.units"),
        #[cfg(feature = "currency-update")]
        "crypto.units" => (units::CRYPTO, c"crypto.units"),
        #[cfg(feature = "currency-update")]
        "metal_prices.units" => (units::METAL_PRICES, c"metal_prices.units"),
        #[cfg(feature = "currency-update")]
        "cpi.units" => (units::CPI, c"cpi.units"),
        _ => return vec![],
    };
    load_core(content, c"", env, db)
}

fn join_continuations(content: &str) -> Vec<(usize, String)> {
    let mut lines = Vec::new();
    let mut iter = content.lines().enumerate();
    while let Some((idx, line)) = iter.next() {
        let line = if idx == 0 {
            line.trim_start_matches('\u{FEFF}')
        } else {
            line
        };
        let mut buf = line.to_string();
        while buf.ends_with('\\')
            && let Some((_, next)) = iter.next()
        {
            buf.pop();
            buf.push_str(next.trim_start());
        }
        lines.push((idx + 1, buf));
    }
    lines
}

fn strip_comment(line: &str) -> &str {
    if let Some(pos) = line.find('#') {
        return &line[..pos];
    }
    line
}

fn split2(s: &str) -> (&str, &str) {
    let mut parts = s.splitn(2, char::is_whitespace);
    let k = parts.next().unwrap_or("");
    let v = parts.next().unwrap_or("").trim();
    (k, v)
}

fn lookup_var(name: &str, env: &HashMap<String, String>) -> Option<String> {
    env.get(name).cloned().or_else(|| std::env::var(name).ok())
}
