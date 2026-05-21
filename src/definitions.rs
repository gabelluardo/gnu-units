//! Parses the embedded `definitions.units` file and registers entries via FFI.
//!
//! Handles prefixes, tables, functions, units, and aliases, producing a sorted
//! [`Vec<Definition>`] used by the public API.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_int;
use std::sync::{LazyLock, Mutex, RwLock};

use crate::ffi;
use crate::units;

/// Loads the embedded GNU units definitions exactly once into the C library's
/// global database.
///
/// `DEFINITIONS` is a [`LazyLock`] whose initialiser parses the compile-time embedded
/// `definitions.units` content and registers each unit, prefix, table, and
/// function directly with the C library's global hash tables. The `RwLock`
/// allows [`reload_currency`] to update the list at runtime.
pub(crate) static DEFINITIONS: LazyLock<RwLock<Vec<Definition>>> = LazyLock::new(|| {
    {
        let _guard = ffi::lock();
        // SAFETY: mylocale/progname point to static C strings that live for the
        // duration of the process. utf8mode is a plain C int. LazyLock guarantees
        // this closure runs at most once, so no other FFI call can observe the
        // globals mid-write.
        unsafe {
            gnu_units_sys::mylocale = c"en_US".as_ptr() as *mut std::os::raw::c_char;
            gnu_units_sys::progname = c"gnu-units".as_ptr() as *mut std::os::raw::c_char;
            gnu_units_sys::utf8mode = 1;
        }
    }

    let definitions = include_str!("../data/definitions.units");
    let mut defs = load_definitions(definitions, c"definitions.units");
    // Emulate C last-write-wins: reverse so last-in-file entries come first
    defs.reverse();
    // Stable sort by canonical_name + kind; preserves reverse order within each group
    defs.sort();
    // Remove duplicates; keeps first of each group = last-in-file entry
    defs.dedup();
    // Final alphabetical ordering for the public API
    defs.sort_by(|a, b| a.name.cmp(&b.name));
    RwLock::new(defs)
});

/// Ensures the GNU units definitions are loaded exactly once.
///
/// Forces the [`DEFINITIONS`] lazy initialiser, which embeds `definitions.units` at
/// compile time and loads it into the C library's global definitions on first
/// call. All subsequent calls are instant no-ops.
pub(crate) fn ensure_definitions() {
    LazyLock::force(&DEFINITIONS);
}

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
    /// Returns the canonical lookup name used by the C library for deduplication.
    ///
    /// The C library normalizes names before hash-table lookup:
    /// - Prefixes: trailing `-` is removed (`kilo-` → `kilo`)
    /// - Functions: everything from `(` onward is stripped (`tempC(x)` → `tempC`)
    /// - Tables: everything from `[` onward is stripped (`gasmark[degR]` → `gasmark`)
    /// - Units and aliases: the full name is used as-is
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

pub(crate) fn load_definitions(content: &str, filename: &std::ffi::CStr) -> Vec<Definition> {
    let mut env: HashMap<String, String> = HashMap::new();
    load_definitions_inner(content, filename, &mut env)
}

fn lookup_var(name: &str, env: &HashMap<String, String>) -> Option<String> {
    env.get(name).cloned().or_else(|| std::env::var(name).ok())
}

static FILE_PTRS: LazyLock<Mutex<HashMap<Vec<u8>, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn intern_filename(filename: &CStr) -> *mut std::os::raw::c_char {
    let key = filename.to_bytes().to_vec();
    let mut map = FILE_PTRS.lock().unwrap_or_else(|e| e.into_inner());
    let addr = *map
        .entry(key)
        .or_insert_with(|| CString::new(filename.to_bytes()).unwrap().into_raw() as usize);
    addr as *mut std::os::raw::c_char
}

fn load_definitions_inner(
    content: &str,
    filename: &std::ffi::CStr,
    env: &mut HashMap<String, String>,
) -> Vec<Definition> {
    // SAFETY: each unique filename CString is leaked once, giving the C library
    // a stable pointer it can retain for the process lifetime.
    let file_ptr = intern_filename(filename);

    let mut results: Vec<Definition> = Vec::new();
    let mut wrong_locale = false;
    let mut in_utf8 = false;
    let mut wrong_var = false;
    let utf8mode = unsafe { gnu_units_sys::utf8mode };

    // Join continuation lines first, then process
    let joined_lines = {
        let mut lines = Vec::new();
        let mut iter = content.lines().enumerate();
        while let Some((idx, line)) = iter.next() {
            // Strip UTF-8 BOM from first line
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
    };

    for (linenum, raw_line) in &joined_lines {
        // Strip comments
        let line = if let Some(pos) = raw_line.find('#') {
            &raw_line[..pos]
        } else {
            raw_line.as_str()
        };
        let line = line.trim();

        if let Some(rest) = line.strip_prefix('!') {
            let rest = rest.trim();
            let mut parts = rest.splitn(2, char::is_whitespace);
            let directive = parts.next().unwrap_or("");
            let arg = parts.next().unwrap_or("").trim();

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
                    let mut tokens = arg.splitn(2, char::is_whitespace);
                    let varname = tokens.next().unwrap_or("");
                    let values = tokens.next().unwrap_or("").trim();
                    match lookup_var(varname, env) {
                        Some(val) => {
                            wrong_var = !values.split_whitespace().any(|v| v == val);
                        }
                        None => wrong_var = true,
                    }
                }
                "varnot" => {
                    let mut tokens = arg.splitn(2, char::is_whitespace);
                    let varname = tokens.next().unwrap_or("");
                    let values = tokens.next().unwrap_or("").trim();
                    match lookup_var(varname, env) {
                        Some(val) => {
                            wrong_var = values.split_whitespace().any(|v| v == val);
                        }
                        None => wrong_var = true,
                    }
                }
                "endvar" => {
                    wrong_var = false;
                }
                "set" if !wrong_locale && !wrong_var => {
                    let mut tokens = arg.splitn(2, char::is_whitespace);
                    let varname = tokens.next().unwrap_or("");
                    let value = tokens.next().unwrap_or("").trim();
                    if lookup_var(varname, env).is_none() {
                        env.insert(varname.to_owned(), value.to_owned());
                    }
                }
                "set" => {}
                "message" | "prompt" => {}
                "unitlist" => {
                    if wrong_locale || wrong_var || (in_utf8 && utf8mode == 0) {
                        continue;
                    }
                    let mut tokens = arg.splitn(2, char::is_whitespace);
                    let name = tokens.next().unwrap_or("");
                    let def = tokens.next().unwrap_or("").trim();
                    if name.is_empty() {
                        continue;
                    }
                    ffi::newalias(name, def, *linenum as c_int, file_ptr);
                    results.push(Definition {
                        name: name.to_owned(),
                        definition: def.to_owned(),
                        kind: DefinitionKind::Alias,
                    });
                }
                "include" => {
                    if wrong_locale || wrong_var || (in_utf8 && utf8mode == 0) {
                        continue;
                    }
                    let def = match arg {
                        "elements.units" => {
                            load_definitions_inner(units::ELEMENTS, c"elements.units", env)
                        }
                        #[cfg(feature = "currency-update")]
                        "currency.units" => {
                            load_definitions_inner(units::CURRENCY, c"currency.units", env)
                        }
                        #[cfg(feature = "currency-update")]
                        "crypto.units" => {
                            load_definitions_inner(units::CRYPTO, c"crypto.units", env)
                        }
                        #[cfg(feature = "currency-update")]
                        "metal_prices.units" => {
                            load_definitions_inner(units::METAL_PRICES, c"metal_prices.units", env)
                        }
                        #[cfg(feature = "currency-update")]
                        "cpi.units" => load_definitions_inner(units::CPI, c"cpi.units", env),
                        _ => vec![],
                    };
                    results.extend(def);
                }
                _ => {}
            }
            continue;
        }

        if wrong_locale || wrong_var || (in_utf8 && utf8mode == 0) {
            continue;
        }

        let line = replace_operators(line);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let mut tokens = line.splitn(2, char::is_whitespace);
        let raw_name = tokens.next().unwrap_or("");
        let def = tokens.next().unwrap_or("").trim();

        if raw_name == "-" || def.is_empty() {
            continue;
        }

        let name = raw_name.strip_prefix('+').unwrap_or(raw_name);
        if name.is_empty() {
            continue;
        }

        match name {
            n if n.ends_with('-') => ffi::newprefix(name, def, *linenum as c_int, file_ptr),
            n if n.contains('[') => ffi::newtable(name, def, *linenum as c_int, file_ptr),
            n if n.contains('(') => ffi::newfunction(name, def, *linenum as c_int, file_ptr),
            _ => ffi::newunit(name, def, *linenum as c_int, file_ptr),
        }

        let kind = if name.ends_with('-') {
            DefinitionKind::Prefix
        } else if name.contains('[') {
            DefinitionKind::Table
        } else if name.contains('(') {
            DefinitionKind::Function
        } else {
            DefinitionKind::Unit
        };
        results.push(Definition {
            name: name.to_owned(),
            definition: def.to_owned(),
            kind,
        });
    }

    results
}
