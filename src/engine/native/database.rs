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

/// A closed or half-open numeric interval used for domain and range checking.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Interval {
    pub min: f64,
    pub max: f64,
    /// `true` → lower bound is exclusive (`(`).
    pub min_open: bool,
    /// `true` → upper bound is exclusive (`)`).
    pub max_open: bool,
}

impl Interval {
    pub fn contains(&self, value: f64) -> bool {
        let above_min = if self.min_open {
            value > self.min
        } else {
            value >= self.min
        };
        let below_max = if self.max_open {
            value < self.max
        } else {
            value <= self.max
        };

        above_min && below_max
    }
}

/// A non-linear conversion function from the definitions file.
#[derive(Debug, Clone)]
pub(crate) struct FunctionDef {
    /// Parameter names used in the formula.
    /// - Zero params `[]`: zero-arg alias, body is parsed directly as a unit definition.
    /// - One param `["x"]`: standard single-arg function.
    /// - Multiple params: multivariate function.
    pub params: Vec<String>,
    /// Expression evaluated with params bound to the input's numeric factors.
    pub forward: String,
    /// Inverse expression. Only valid for single-param functions; uses the function
    /// name as the variable (GNU units convention).
    pub reverse: Option<String>,
    /// Per-parameter dimensional unit extracted from `units=[p1,p2;...]`.
    /// `None` for a parameter means no conformability check is performed.
    pub units: Vec<Option<String>>,
    /// Per-parameter domain interval extracted from `domain=[...]`.
    /// `None` for a parameter means no domain check is performed.
    pub domain: Vec<Option<Interval>>,
    /// Output range interval extracted from `range=[...]`.
    /// Used to validate the *input* of the inverse call.
    pub range: Option<Interval>,
    /// Dimensional unit for the inverse input (from `units=` after `;`).
    pub inverse_unit: Option<String>,
    /// When `true`, domain and range checks are suppressed (`noerror` keyword).
    pub noerror: bool,
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
    /// Linear interpolation following GNU units `evalfunc` semantics: a tiny
    /// tolerance (`f64::EPSILON`) is accepted at the endpoints; any input
    /// strictly outside that boundary returns `None` (`E_NOTINDOMAIN`).
    pub fn interpolate(&self, input: f64) -> Option<f64> {
        match self.points.as_slice() {
            [] => None,
            [(_, y)] => Some(*y),
            [(first_x, first_y), .., (last_x, last_y)] => {
                if input >= *last_x && input < last_x + f64::EPSILON {
                    return Some(*last_y);
                }
                if input <= *first_x && input > first_x - f64::EPSILON {
                    return Some(*first_y);
                }
                for window in self.points.windows(2) {
                    let (x0, y0) = window[0];
                    let (x1, y1) = window[1];
                    if x0 <= input && input <= x1 {
                        let t = (input - x0) / (x1 - x0);
                        return Some(y0 + t * (y1 - y0));
                    }
                }
                None
            }
        }
    }

    /// Reverse interpolation following GNU units `evalfunc` semantics (inverse
    /// path): respects ascending/descending table direction, accepts
    /// `f64::EPSILON` tolerance at endpoints, and returns `None` for out-of-
    /// domain values (`E_NOTINDOMAIN`).
    pub fn reverse_interpolate(&self, output: f64) -> Option<f64> {
        match self.points.as_slice() {
            [] => None,
            [(x, _)] => Some(*x),
            [(first_x, first_y), .., (last_x, last_y)] => {
                let dir = if last_y > first_y { 1.0_f64 } else { -1.0_f64 };
                if dir * output >= dir * last_y && dir * output < dir * last_y + f64::EPSILON {
                    return Some(*last_x);
                }
                if dir * output <= dir * first_y && dir * output > dir * first_y - f64::EPSILON {
                    return Some(*first_x);
                }
                for window in self.points.windows(2) {
                    let (x0, y0) = window[0];
                    let (x1, y1) = window[1];
                    if dir * y0 <= dir * output && dir * output <= dir * y1 {
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
        let entry = match trimmed.strip_prefix('!') {
            Some(rest) if rest.trim() == "dimensionless" => UnitEntry::DimensionlessPrimitive,
            Some(_) => UnitEntry::Primitive,
            None => UnitEntry::Derived(trimmed.to_owned()),
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
        let params: Vec<String> = name
            .split_once('(')
            .and_then(|(_, rest)| rest.split_once(')'))
            .map(|(p, _)| {
                let trimmed = p.trim();
                if trimmed.is_empty() {
                    return Vec::new();
                }
                trimmed.split(',').map(|s| s.trim().to_owned()).collect()
            })
            .unwrap_or_else(|| vec!["x".to_owned()]);
        if let Some(func) = parse_function_def(&params, def) {
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

/// Parse up to `max_count` consecutive intervals (`[a,b]`, `(a,b]`, etc.)
/// from the start of `s`.  Returns the parsed intervals (open/closed bounds,
/// `NEG_INFINITY`/`INFINITY` for missing values) and the remaining string
/// after the last interval consumed.
///
/// Intervals may be separated by an optional `,` (e.g. `[0,1],[2,3]`), or
/// juxtaposed without any separator (e.g. `(0,)(0,)`).
fn parse_intervals_multi(s: &str, max_count: usize) -> (Vec<Option<Interval>>, &str) {
    let mut s = s;
    let mut intervals = Vec::new();

    while intervals.len() < max_count {
        let trimmed = s.trim_start();
        if !trimmed.starts_with('[') && !trimmed.starts_with('(') {
            break;
        }
        let min_open = trimmed.starts_with('(');
        let Some(close_pos) = trimmed.find([']', ')']) else {
            break;
        };
        let max_open = trimmed.as_bytes()[close_pos] == b')';
        let inner = &trimmed[1..close_pos];
        s = &trimmed[close_pos + 1..];

        // Skip an optional `,` separator only when followed by another interval.
        let next = s.trim_start();
        if let Some(after_comma) = next.strip_prefix(',') {
            let after_comma = after_comma.trim_start();
            if after_comma.starts_with('[') || after_comma.starts_with('(') {
                s = after_comma;
            }
        }

        let Some(comma_pos) = inner.find(',') else {
            intervals.push(None);
            continue;
        };
        let min_str = inner[..comma_pos].trim();
        let max_str = inner[comma_pos + 1..].trim();
        let min = if min_str.is_empty() {
            f64::NEG_INFINITY
        } else {
            min_str.parse().unwrap_or(f64::NEG_INFINITY)
        };
        let max = if max_str.is_empty() {
            f64::INFINITY
        } else {
            max_str.parse().unwrap_or(f64::INFINITY)
        };
        intervals.push(Some(Interval {
            min,
            max,
            min_open,
            max_open,
        }));
    }

    (intervals, s)
}

/// Collected metadata extracted from a GNU units function definition string.
struct FuncMeta {
    forward_units: Vec<Option<String>>,
    inverse_unit: Option<String>,
    domain: Vec<Option<Interval>>,
    range: Option<Interval>,
    noerror: bool,
    /// Remaining string after all metadata keywords — the formula body.
    formula: String,
}

/// Extract GNU units function metadata (`units=[...]`, `domain=[...]`,
/// `range=[...]`, `noerror`) from the front of `def` and return a
/// [`FuncMeta`] containing the parsed values and the residual formula string.
fn parse_func_metadata(params_count: usize, def: &str) -> FuncMeta {
    let mut s = def.trim();
    let mut forward_units = vec![None; params_count];
    let mut inverse_unit: Option<String> = None;
    let mut domain = vec![None; params_count];
    let mut range: Option<Interval> = None;
    let mut noerror = false;

    loop {
        let before = s;
        s = s.trim_start();

        // `noerror ` — keyword must be followed by whitespace or end-of-string.
        if s.starts_with("noerror") {
            let rest = &s[7..];
            if rest.is_empty()
                || matches!(rest.as_bytes().first(), Some(b)
                    if !b.is_ascii_alphanumeric() && *b != b'_')
            {
                s = rest;
                noerror = true;
                continue;
            }
        }

        // `units=[p1,p2;result]`
        if s.starts_with("units=[") {
            let after_open = &s[7..];
            let Some(close) = after_open.find(']') else {
                break;
            };
            let content = &after_open[..close];
            s = &after_open[close + 1..];
            let (params_part, inv_part) = match content.find(';') {
                Some(pos) => (&content[..pos], Some(content[pos + 1..].trim())),
                None => (content, None),
            };
            for (i, part) in params_part.split(',').enumerate() {
                let part = part.trim();
                if i < forward_units.len() && !part.is_empty() && part != "*" {
                    forward_units[i] = Some(part.to_owned());
                }
            }
            if let Some(inv) = inv_part
                && !inv.is_empty()
                && inv != "*"
            {
                inverse_unit = Some(inv.to_owned());
            }
            continue;
        }

        // `domain=[...]` or `domain=(...)`, possibly multiple intervals.
        if s.starts_with("domain=") {
            let (intervals, rest) = parse_intervals_multi(&s[7..], params_count);
            s = rest;
            for (i, interval) in intervals.into_iter().enumerate() {
                if i < domain.len() {
                    domain[i] = interval;
                }
            }
            continue;
        }

        // `range=[...]` or `range=(...)` — single interval.
        if s.starts_with("range=") {
            let (intervals, rest) = parse_intervals_multi(&s[6..], 1);
            s = rest;
            range = intervals.into_iter().next().flatten();
            continue;
        }

        if std::ptr::eq(s.as_ptr(), before.as_ptr()) {
            break;
        }
    }

    FuncMeta {
        forward_units,
        inverse_unit,
        domain,
        range,
        noerror,
        formula: s.to_owned(),
    }
}

/// Find the top-level `;` separator in a function body.
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

fn parse_function_def(params: &[String], raw_def: &str) -> Option<FunctionDef> {
    let meta = parse_func_metadata(params.len(), raw_def);
    let body = meta.formula.trim().to_owned();
    if body.is_empty() {
        return None;
    }
    let (forward, reverse) = if let Some(sep) = find_formula_sep(&body) {
        (
            body[..sep].trim().to_owned(),
            Some(body[sep + 1..].trim().to_owned()),
        )
    } else {
        (body, None)
    };
    if forward.is_empty() {
        return None;
    }
    Some(FunctionDef {
        params: params.to_vec(),
        forward,
        reverse,
        units: meta.forward_units,
        domain: meta.domain,
        range: meta.range,
        inverse_unit: meta.inverse_unit,
        noerror: meta.noerror,
    })
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Returns a string label for the discriminant of a `UnitEntry`.
    #[track_caller]
    fn variant_label(entry: &UnitEntry) -> &'static str {
        match entry {
            UnitEntry::Primitive => "primitive",
            UnitEntry::DimensionlessPrimitive => "dimensionless",
            UnitEntry::Derived(_) => "derived",
        }
    }

    #[rstest]
    #[case::primitive("base", "!", "primitive")]
    #[case::dimensionless_primitive("rad", "!dimensionless", "dimensionless")]
    #[case::derived("km", "1000 m", "derived")]
    fn insert_unit_variant(#[case] name: &str, #[case] def: &str, #[case] expected: &str) {
        let mut db = Database::default();

        db.insert_unit(name, def);

        let entry = db
            .units
            .get(name)
            .expect("unit should be present after insert");
        assert_eq!(variant_label(entry), expected);
    }

    #[test]
    fn insert_unit_dimensionless_with_interior_spaces() {
        let mut db = Database::default();

        db.insert_unit("rad", "!  dimensionless");

        let entry = db.units.get("rad").unwrap();
        assert!(
            matches!(entry, UnitEntry::DimensionlessPrimitive),
            "expected DimensionlessPrimitive, got {entry:?}"
        );
    }

    #[test]
    fn insert_unit_derived_stores_trimmed_definition() {
        let mut db = Database::default();

        db.insert_unit("foo", "  100 kg  ");

        let entry = db.units.get("foo").unwrap();
        assert!(
            matches!(entry, UnitEntry::Derived(s) if s == "100 kg"),
            "expected Derived(\"100 kg\"), got {entry:?}"
        );
    }
}
