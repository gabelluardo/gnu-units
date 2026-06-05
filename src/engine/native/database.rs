//! Unit and prefix database for the pure-Rust native engine.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

#[derive(Debug, Clone)]
pub(crate) enum UnitEntry {
    /// A primitive base unit (defined with bare `!`).
    Primitive,
    /// A dimensionless primitive (defined with `!dimensionless`).
    DimensionlessPrimitive,
    /// A derived unit whose definition must be parsed recursively.
    Derived(String),
}

/// A non-linear conversion function from the definitions file.
#[derive(Debug, Clone)]
pub(crate) struct FunctionDef {
    /// The parameter name used in the formula (e.g. `"x"`).
    pub param: String,
    /// Expression evaluated with `param` bound to the input's numeric factor.
    pub forward: String,
    /// Inverse expression evaluated with `param` bound to the output's numeric factor.
    pub reverse: Option<String>,
}

/// A piecewise lookup table (e.g. `gasmark[degR]`).
#[derive(Debug, Clone)]
pub(crate) struct TableDef {
    /// The output unit (e.g. `"degR"` for `gasmark`).
    pub unit: String,
    /// Sorted pairs of `(input_value, output_value)`.
    pub points: Vec<(f64, f64)>,
}

impl TableDef {
    /// Linear interpolation with clamping at the endpoints.
    pub fn interpolate(&self, input: f64) -> Option<f64> {
        match self.points.as_slice() {
            [] => None,
            [(_, y)] => Some(*y),
            [(first_x, first_y), .., (last_x, last_y)] => {
                if input <= *first_x {
                    return Some(*first_y);
                }
                if input >= *last_x {
                    return Some(*last_y);
                }
                for window in self.points.windows(2) {
                    let (x0, y0) = window[0];
                    let (x1, y1) = window[1];
                    if input >= x0 && input <= x1 {
                        let t = (input - x0) / (x1 - x0);
                        return Some(y0 + t * (y1 - y0));
                    }
                }
                None
            }
        }
    }

    /// Reverse interpolation: given an output value, find the corresponding input.
    pub fn reverse_interpolate(&self, output: f64) -> Option<f64> {
        match self.points.as_slice() {
            [] => None,
            [(x, _)] => Some(*x),
            [(first_x, first_y), .., (last_x, last_y)] => {
                if output <= *first_y {
                    return Some(*first_x);
                }
                if output >= *last_y {
                    return Some(*last_x);
                }
                for window in self.points.windows(2) {
                    let (x0, y0) = window[0];
                    let (x1, y1) = window[1];
                    if (output >= y0 && output <= y1) || (output >= y1 && output <= y0) {
                        if (y1 - y0).abs() < f64::EPSILON {
                            return Some(x0);
                        }
                        let t = (output - y0) / (y1 - y0);
                        return Some(x0 + t * (x1 - x0));
                    }
                }
                None
            }
        }
    }
}

/// In-memory unit database populated once from the embedded definitions file.
#[derive(Debug, Default)]
pub(crate) struct Database {
    /// Unit name → entry.
    pub units: HashMap<String, UnitEntry>,
    /// Prefix bare-name → definition string, sorted longest-first.
    pub prefixes: Vec<(String, String)>,
    /// Function name → definition.
    pub functions: HashMap<String, FunctionDef>,
    /// Table name → piecewise definition.
    pub tables: HashMap<String, TableDef>,
}

static DATABASE: OnceLock<RwLock<Database>> = OnceLock::new();

/// Initialise the global database.  Called once from `definitions::ensure_definitions`.
pub(crate) fn init(db: Database) {
    let _ = DATABASE.set(RwLock::new(db));
}

/// Return a reference to the global `RwLock<Database>`.  Initialises an empty
/// database if `init` has not been called yet (not expected in normal usage).
pub(crate) fn get() -> &'static RwLock<Database> {
    DATABASE.get_or_init(|| RwLock::new(Database::default()))
}

/// Obtain a read guard on the global database.
pub(crate) fn read() -> std::sync::RwLockReadGuard<'static, Database> {
    get().read().unwrap_or_else(|e| e.into_inner())
}

impl Database {
    pub fn insert_unit(&mut self, name: &str, def: &str) {
        let trimmed = def.trim();
        let entry = if trimmed.starts_with('!') {
            let rest = trimmed.trim_start_matches('!').trim();
            if rest == "dimensionless" {
                UnitEntry::DimensionlessPrimitive
            } else {
                UnitEntry::Primitive
            }
        } else {
            UnitEntry::Derived(trimmed.to_owned())
        };
        self.units.insert(name.to_owned(), entry);
    }

    pub fn insert_prefix(&mut self, name: &str, def: &str) {
        let bare = name.trim_end_matches('-');
        self.prefixes.retain(|(n, _)| n.as_str() != bare);
        self.prefixes.push((bare.to_owned(), def.trim().to_owned()));
        // longest prefix first → greedy matching
        self.prefixes
            .sort_by_key(|(b, _)| std::cmp::Reverse(b.len()));
    }

    pub fn insert_function(&mut self, name: &str, def: &str) {
        let bare = name.split('(').next().unwrap_or(name);
        let param = name
            .split_once('(')
            .and_then(|(_, rest)| rest.split_once(')'))
            .map(|(p, _)| p.trim().to_owned())
            .unwrap_or_else(|| "x".to_owned());
        if let Some(func) = parse_function_def(&param, def) {
            self.functions.insert(bare.to_owned(), func);
        }
    }

    pub fn insert_table(&mut self, name: &str, def: &str) {
        let Some(bracket_start) = name.find('[') else {
            return;
        };
        let Some(bracket_end) = name.find(']') else {
            return;
        };
        let bare = &name[..bracket_start];
        let unit = &name[bracket_start + 1..bracket_end];

        let nums: Vec<f64> = def
            .split_whitespace()
            .filter_map(|s| s.parse::<f64>().ok())
            .collect();
        let mut points: Vec<(f64, f64)> = nums.chunks_exact(2).map(|c| (c[0], c[1])).collect();
        points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        self.tables.insert(
            bare.to_owned(),
            TableDef {
                unit: unit.to_owned(),
                points,
            },
        );
    }

    /// Greedy prefix look-up: returns `(prefix_def_str, remaining_unit_name)`
    /// or `None` if no prefix matches a known unit in the remainder.
    pub fn find_with_prefix<'a>(&'a self, name: &'a str) -> Option<(&'a str, &'a str)> {
        for (prefix, value) in &self.prefixes {
            let plen = prefix.len();
            if name.len() > plen && name.starts_with(prefix.as_str()) {
                let rest = &name[plen..];
                if self.units.contains_key(rest) {
                    return Some((value.as_str(), rest));
                }
            }
        }
        None
    }

    /// Returns the definition string for a prefix whose bare name equals `name`
    /// exactly (e.g. `find_prefix_by_name("kilo")` returns `Some("1e3")`).
    pub fn find_prefix_by_name<'a>(&'a self, name: &'a str) -> Option<&'a str> {
        self.prefixes
            .iter()
            .find(|(bare, _)| bare.as_str() == name)
            .map(|(_, def)| def.as_str())
    }
}

/// Strip GNU units function metadata (`units=[...]`, `domain=[...]`,
/// `range=[...]`, `noerror`) from the raw definition string and return the
/// formula part.
fn strip_func_metadata(def: &str) -> &str {
    let mut s = def.trim();
    loop {
        let before = s;
        s = s.trim_start();
        let keywords = ["units=", "domain=", "range=", "noerror"];
        for kw in keywords {
            if s.starts_with(kw) {
                s = &s[kw.len()..];
                if kw.ends_with('=') && (s.starts_with('[') || s.starts_with('(')) {
                    // Intervals are single-level: [a,b], [a,b), (a,b], (a,b)
                    if let Some(end) = s.find([')', ']']) {
                        s = &s[end + 1..];
                    }
                }
                break;
            }
        }
        if std::ptr::eq(s.as_ptr(), before.as_ptr()) && s.len() == before.len() {
            break;
        }
    }
    s.trim()
}

/// Find the top-level `;` separator in a function body (ignores `;` inside
/// `[...]` brackets, which appear in `units=[1;K]` metadata).
fn find_formula_sep(s: &str) -> Option<usize> {
    let mut depth: usize = 0;
    for (i, ch) in s.char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            ';' if depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

fn parse_function_def(param: &str, raw_def: &str) -> Option<FunctionDef> {
    let body = strip_func_metadata(raw_def);
    if body.is_empty() {
        return None;
    }
    let (forward, reverse) = if let Some(sep) = find_formula_sep(body) {
        (body[..sep].trim(), Some(body[sep + 1..].trim().to_owned()))
    } else {
        (body, None)
    };
    if forward.is_empty() {
        return None;
    }
    Some(FunctionDef {
        param: param.to_owned(),
        forward: forward.to_owned(),
        reverse,
    })
}
