use std::cmp::Ordering;
use std::collections::HashMap;
use std::ffi::CString;
use std::os::raw::c_int;

pub(crate) static DEFINITIONS: &str =
    include_str!("../gnu-units-sys/vendor/units/definitions.units");
static ELEMENTS_UNITS: &str = include_str!("../gnu-units-sys/vendor/units/elements.units");

#[cfg(feature = "currency-update")]
pub(crate) static CURRENCY_UNITS: &str =
    include_str!("../gnu-units-sys/vendor/units/currency.units");
#[cfg(feature = "currency-update")]
static CRYPTO_UNITS: &str = include_str!("../gnu-units-sys/vendor/units/crypto.units");
#[cfg(feature = "currency-update")]
static METAL_PRICES_UNITS: &str = include_str!("../gnu-units-sys/vendor/units/metal_prices.units");
#[cfg(feature = "currency-update")]
static CPI_UNITS: &str = include_str!("../gnu-units-sys/vendor/units/cpi.units");

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

fn load_definitions_inner(
    content: &str,
    filename: &std::ffi::CStr,
    env: &mut HashMap<String, String>,
) -> Vec<Definition> {
    // SAFETY: file_ptr is leaked so the C library can store it permanently.
    let file_ptr = CString::new(filename.to_bytes()).unwrap().into_raw();

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
                    let mut name_buf: Vec<u8> = name.bytes().chain(std::iter::once(0)).collect();
                    let mut def_buf: Vec<u8> = def.bytes().chain(std::iter::once(0)).collect();
                    // SAFETY: name_buf and def_buf are null-terminated mutable buffers.
                    // The C function copies both strings internally (dupstr), so the
                    // buffers only need to live for the duration of this call.
                    // file_ptr is a leaked CString valid for the process lifetime.
                    unsafe {
                        gnu_units_sys::newalias(
                            name_buf.as_mut_ptr() as *mut std::os::raw::c_char,
                            def_buf.as_mut_ptr() as *mut std::os::raw::c_char,
                            *linenum as c_int,
                            file_ptr,
                            std::ptr::null_mut(),
                        );
                    }
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
                            load_definitions_inner(ELEMENTS_UNITS, c"elements.units", env)
                        }
                        #[cfg(feature = "currency-update")]
                        "currency.units" => {
                            load_definitions_inner(CURRENCY_UNITS, c"currency.units", env)
                        }
                        #[cfg(feature = "currency-update")]
                        "crypto.units" => {
                            load_definitions_inner(CRYPTO_UNITS, c"crypto.units", env)
                        }
                        #[cfg(feature = "currency-update")]
                        "metal_prices.units" => {
                            load_definitions_inner(METAL_PRICES_UNITS, c"metal_prices.units", env)
                        }
                        #[cfg(feature = "currency-update")]
                        "cpi.units" => load_definitions_inner(CPI_UNITS, c"cpi.units", env),
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

        let (name, redefine) = if let Some(stripped) = raw_name.strip_prefix('+') {
            (stripped, 1)
        } else {
            (raw_name, 0)
        };

        if name.is_empty() {
            continue;
        }

        let mut name_buf: Vec<u8> = name.bytes().chain(std::iter::once(0)).collect();
        let mut def_buf: Vec<u8> = def.bytes().chain(std::iter::once(0)).collect();
        let mut count: c_int = 0;

        let name_ptr = name_buf.as_mut_ptr() as *mut std::os::raw::c_char;
        let def_ptr = def_buf.as_mut_ptr() as *mut std::os::raw::c_char;

        // SAFETY: name_buf and def_buf are null-terminated mutable buffers.
        // The C function copies both strings internally (dupstr), so the
        // buffers only need to live for the duration of this call.
        // file_ptr is a leaked CString valid for the process lifetime.
        unsafe {
            match name {
                n if n.ends_with('-') => {
                    gnu_units_sys::newprefix(
                        name_ptr,
                        def_ptr,
                        &mut count,
                        *linenum as c_int,
                        file_ptr,
                        std::ptr::null_mut(),
                        redefine,
                    );
                }
                n if n.contains('[') => {
                    gnu_units_sys::newtable(
                        name_ptr,
                        def_ptr,
                        &mut count,
                        *linenum as c_int,
                        file_ptr,
                        std::ptr::null_mut(),
                        redefine,
                    );
                }
                n if n.contains('(') => {
                    gnu_units_sys::newfunction(
                        name_ptr,
                        def_ptr,
                        &mut count,
                        *linenum as c_int,
                        file_ptr,
                        std::ptr::null_mut(),
                        redefine,
                    );
                }
                _ => {
                    gnu_units_sys::newunit(
                        name_ptr,
                        def_ptr,
                        &mut count,
                        *linenum as c_int,
                        file_ptr,
                        std::ptr::null_mut(),
                        redefine,
                        0,
                    );
                }
            }
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
