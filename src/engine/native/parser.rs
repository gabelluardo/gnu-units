//! Pure-Rust expression parser for GNU units expressions.
//!
//! Grammar (lowest → highest precedence):
//!
//! ```text
//! expr     ::= additive
//! additive ::= product (('+' | '-') product)*
//! product  ::= power (('*' | '/' | '|' | <juxtaposition>) power)*
//! power    ::= unary ('^' | '**') signed_integer
//! unary    ::= ('-' | '+') unary | atom
//! atom     ::= NUMBER | NAME ('(' expr ')')? | '(' expr ')'
//! ```
//!
//! The `|` operator is equivalent to `/` (reciprocal fraction).  Adjacent
//! tokens (juxtaposition) imply multiplication.  Powers associate left to right
//! with multiplication / division.

use std::collections::{HashMap, HashSet};

use super::database::{Database, FunctionDef, UnitEntry, read as db_read};
use super::types::UnitValue;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "parse error: {}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Num(f64),
    Name(String),
    Star,
    Slash,
    Pipe,
    Caret,
    LParen,
    RParen,
    Minus,
    Plus,
    Tilde,
    PlusMinus,
    Eof,
}

fn tokenize(input: &str) -> Result<Vec<Token>, ParseError> {
    let bytes = input.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\n' | b'\r' => {
                i += 1;
            }
            b'*' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                    tokens.push(Token::Caret);
                    i += 2;
                } else {
                    tokens.push(Token::Star);
                    i += 1;
                }
            }
            b'/' => {
                tokens.push(Token::Slash);
                i += 1;
            }
            b'^' => {
                tokens.push(Token::Caret);
                i += 1;
            }
            b'|' => {
                tokens.push(Token::Pipe);
                i += 1;
            }
            b'(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            b')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            b'-' => {
                tokens.push(Token::Minus);
                i += 1;
            }
            b'+' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'-' {
                    tokens.push(Token::PlusMinus);
                    i += 2;
                } else {
                    tokens.push(Token::Plus);
                    i += 1;
                }
            }
            b'0'..=b'9' | b'.' => {
                let start = i;
                if bytes[i] == b'0'
                    && i + 1 < bytes.len()
                    && (bytes[i + 1] == b'x' || bytes[i + 1] == b'X')
                {
                    i += 2;
                    while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                        i += 1;
                    }
                    let hex = &input[start + 2..i];
                    let val = i64::from_str_radix(hex, 16)
                        .map_err(|_| ParseError("invalid hex literal".to_owned()))?;
                    tokens.push(Token::Num(val as f64));
                } else {
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i < bytes.len() && bytes[i] == b'.' {
                        i += 1;
                        while i < bytes.len() && bytes[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                    // Scientific notation — only consume e/E when followed by
                    // sign+digits or digits, so "eV" isn't swallowed.
                    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
                        let mut j = i + 1;
                        if j < bytes.len() && (bytes[j] == b'+' || bytes[j] == b'-') {
                            j += 1;
                        }
                        if j < bytes.len() && bytes[j].is_ascii_digit() {
                            i = j;
                            while i < bytes.len() && bytes[i].is_ascii_digit() {
                                i += 1;
                            }
                        }
                    }
                    let num_str = &input[start..i];
                    let val: f64 = num_str
                        .parse()
                        .map_err(|_| ParseError(format!("invalid number: {num_str}")))?;
                    tokens.push(Token::Num(val));
                }
            }
            b if b.is_ascii_alphabetic()
                || b == b'_'
                || b == b'%'
                || b == b'$'
                || b == b'&'
                || !b.is_ascii() =>
            {
                let start = i;
                i += 1;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric()
                        || bytes[i] == b'_'
                        || bytes[i] == b'\''
                        || bytes[i] == b'$'
                        || bytes[i] == b'&'
                        || !bytes[i].is_ascii())
                {
                    i += 1;
                }
                tokens.push(Token::Name(input[start..i].to_owned()));
            }
            b'~' => {
                tokens.push(Token::Tilde);
                i += 1;
            }
            _ => {
                let ch = input[i..].chars().next().unwrap_or('?');
                return Err(ParseError(format!("unexpected character: '{ch}'")));
            }
        }
    }

    tokens.push(Token::Eof);
    Ok(tokens)
}

const MAX_DEPTH: usize = 256;

struct Parser<'db> {
    tokens: Vec<Token>,
    pos: usize,
    db: &'db Database,
    /// Variable bindings used when evaluating function bodies.
    vars: HashMap<String, UnitValue>,
    /// Set of unit names currently being resolved (cycle detection).
    resolving: HashSet<String>,
    depth: usize,
}

impl<'db> Parser<'db> {
    fn new(tokens: Vec<Token>, db: &'db Database) -> Self {
        Self {
            tokens,
            pos: 0,
            db,
            vars: HashMap::new(),
            resolving: HashSet::new(),
            depth: 0,
        }
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn advance(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    /// True when the current token can start a multiplicative factor
    /// (triggering juxtaposition multiplication).
    fn can_start_factor(&self) -> bool {
        matches!(
            self.peek(),
            Token::Num(_) | Token::Name(_) | Token::LParen | Token::Tilde
        )
    }

    fn parse_expr(&mut self) -> Result<UnitValue, ParseError> {
        self.parse_additive()
    }

    fn parse_additive(&mut self) -> Result<UnitValue, ParseError> {
        let mut lhs = self.parse_product()?;
        loop {
            match self.peek() {
                Token::Plus => {
                    self.advance();
                    let rhs = self.parse_product()?;
                    if !lhs.add_assign(&rhs) {
                        return Err(ParseError("incompatible dimensions in addition".to_owned()));
                    }
                }
                Token::Minus => {
                    self.advance();
                    let mut rhs = self.parse_product()?;
                    rhs.factor = -rhs.factor;
                    if !lhs.add_assign(&rhs) {
                        return Err(ParseError(
                            "incompatible dimensions in subtraction".to_owned(),
                        ));
                    }
                }
                Token::PlusMinus => {
                    self.advance();
                    let mut rhs = self.parse_product()?;
                    rhs.factor = -rhs.factor;
                    if !lhs.add_assign(&rhs) {
                        return Err(ParseError(
                            "incompatible dimensions in subtraction".to_owned(),
                        ));
                    }
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    fn parse_product(&mut self) -> Result<UnitValue, ParseError> {
        let mut lhs = self.parse_power()?;

        loop {
            match self.peek() {
                Token::Star => {
                    self.advance();
                    let rhs = self.parse_power()?;
                    lhs.multiply_assign(&rhs);
                }
                Token::Slash => {
                    self.advance();
                    let rhs = self.parse_power()?;
                    lhs.divide_assign(&rhs);
                }
                Token::Pipe => {
                    self.advance();
                    let rhs = self.parse_power()?;
                    lhs.divide_assign(&rhs);
                }
                // Juxtaposition
                _ if self.can_start_factor() => {
                    let rhs = self.parse_power()?;
                    lhs.multiply_assign(&rhs);
                }
                _ => break,
            }
        }

        Ok(lhs)
    }

    fn parse_power(&mut self) -> Result<UnitValue, ParseError> {
        let mut base = self.parse_unary()?;

        while matches!(self.peek(), Token::Caret) {
            self.advance();
            let neg = if matches!(self.peek(), Token::Minus) {
                self.advance();
                true
            } else {
                if matches!(self.peek(), Token::Plus) {
                    self.advance();
                }
                false
            };
            let exp = match self.advance() {
                Token::Num(n) => {
                    if (n - n.round()).abs() > f64::EPSILON {
                        return Err(ParseError(format!("exponent must be an integer, got {n}")));
                    }
                    let e = n.round() as i32;
                    if neg { -e } else { e }
                }
                _ => return Err(ParseError("expected integer after '^'".to_owned())),
            };
            base.pow_assign(exp);
        }

        Ok(base)
    }

    fn parse_unary(&mut self) -> Result<UnitValue, ParseError> {
        if matches!(self.peek(), Token::Minus) {
            self.advance();
            let mut v = self.parse_unary()?;
            v.factor = -v.factor;
            return Ok(v);
        }
        if matches!(self.peek(), Token::Plus) {
            self.advance();
            return self.parse_unary();
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Result<UnitValue, ParseError> {
        match self.peek().clone() {
            Token::Num(n) => {
                self.advance();
                Ok(UnitValue::from_factor(n))
            }
            Token::Name(name) => {
                self.advance();
                if matches!(self.peek(), Token::LParen) {
                    self.advance(); // consume '('
                    if self.depth >= MAX_DEPTH {
                        return Err(ParseError("expression too deeply nested".to_owned()));
                    }
                    self.depth += 1;
                    let arg = self.parse_expr()?;
                    self.depth -= 1;
                    if !matches!(self.peek(), Token::RParen) {
                        return Err(ParseError("expected ')'".to_owned()));
                    }
                    self.advance(); // consume ')'
                    self.call_function(&name, arg)
                } else {
                    self.resolve_name(&name)
                }
            }
            Token::LParen => {
                self.advance();
                if self.depth >= MAX_DEPTH {
                    return Err(ParseError("expression too deeply nested".to_owned()));
                }
                self.depth += 1;
                let v = self.parse_expr()?;
                self.depth -= 1;
                if !matches!(self.peek(), Token::RParen) {
                    return Err(ParseError("expected ')'".to_owned()));
                }
                self.advance(); // consume ')'
                Ok(v)
            }
            Token::Tilde => {
                self.advance();
                match self.peek().clone() {
                    Token::Name(name) => {
                        self.advance();
                        if !matches!(self.peek(), Token::LParen) {
                            return Err(ParseError("expected '(' after ~function".to_owned()));
                        }
                        self.advance(); // consume '('
                        if self.depth >= MAX_DEPTH {
                            return Err(ParseError("expression too deeply nested".to_owned()));
                        }
                        self.depth += 1;
                        let arg = self.parse_expr()?;
                        self.depth -= 1;
                        if !matches!(self.peek(), Token::RParen) {
                            return Err(ParseError("expected ')'".to_owned()));
                        }
                        self.advance(); // consume ')'
                        self.call_inverse_function(&name, arg)
                    }
                    _ => Err(ParseError("expected function name after '~'".to_owned())),
                }
            }
            Token::Eof => Err(ParseError("unexpected end of expression".to_owned())),
            other => Err(ParseError(format!("unexpected token: {other:?}"))),
        }
    }

    fn resolve_name(&mut self, name: &str) -> Result<UnitValue, ParseError> {
        // Variable binding (function parameter or injected constant).
        if let Some(val) = self.vars.get(name) {
            return Ok(val.clone());
        }

        // Direct unit lookup.
        if let Some(entry) = self.db.units.get(name).cloned() {
            return self.resolve_entry(name, entry);
        }

        // Plural stripping: try common English plural suffixes.
        if name.len() > 3 {
            let candidates: Vec<String> = if let Some(base) = name.strip_suffix("ies") {
                vec![format!("{base}y")]
            } else if let Some(base) = name.strip_suffix("es") {
                vec![format!("{base}e"), base.to_owned()]
            } else if let Some(base) = name.strip_suffix('s') {
                vec![base.to_owned()]
            } else {
                Vec::new()
            };
            for singular in candidates {
                if let Some(entry) = self.db.units.get(singular.as_str()).cloned() {
                    return self.resolve_entry(&singular, entry);
                }
            }
        }

        // A bare prefix name used standalone (e.g. "kilo" from "k-   kilo").
        if let Some(prefix_def) = self.db.find_prefix_by_name(name) {
            let def = prefix_def.to_owned();
            return self.parse_sub(&def);
        }

        // Prefix + remainder greedy match.
        if let Some((prefix_def, unit_name)) = self.db.find_with_prefix(name) {
            let prefix_str = prefix_def.to_owned();
            let unit_str = unit_name.to_owned();
            let prefix_val = self.parse_sub(&prefix_str)?;
            let unit_entry = self
                .db
                .units
                .get(unit_str.as_str())
                .cloned()
                .ok_or_else(|| ParseError(format!("unknown unit: {unit_str}")))?;
            let mut unit_val = self.resolve_entry(&unit_str, unit_entry)?;
            unit_val.multiply_assign(&prefix_val);
            return Ok(unit_val);
        }

        Err(ParseError(format!("unknown unit or constant: {name}")))
    }

    fn resolve_entry(&mut self, name: &str, entry: UnitEntry) -> Result<UnitValue, ParseError> {
        match entry {
            UnitEntry::Primitive => Ok(UnitValue::primitive(name)),
            UnitEntry::DimensionlessPrimitive => Ok(UnitValue::one()),
            UnitEntry::Derived(def) => {
                if self.resolving.contains(name) {
                    return Err(ParseError(format!("circular definition: {name}")));
                }
                if self.depth >= MAX_DEPTH {
                    return Err(ParseError(format!(
                        "max recursion depth while resolving: {name}"
                    )));
                }
                self.resolving.insert(name.to_owned());
                self.depth += 1;
                let result = self.parse_sub(&def);
                self.depth -= 1;
                self.resolving.remove(name);
                result
            }
        }
    }

    /// Parse a sub-expression string by temporarily swapping the token stream.
    fn parse_sub(&mut self, expr: &str) -> Result<UnitValue, ParseError> {
        let sub_tokens = tokenize(expr)?;
        let saved_tokens = std::mem::replace(&mut self.tokens, sub_tokens);
        let saved_pos = std::mem::replace(&mut self.pos, 0);
        let result = self.parse_expr();
        self.tokens = saved_tokens;
        self.pos = saved_pos;
        result
    }

    fn call_function(&mut self, name: &str, arg: UnitValue) -> Result<UnitValue, ParseError> {
        match name {
            "sqrt" => {
                let mut v = arg;
                if !v.root_assign(2) {
                    return Err(ParseError(
                        "sqrt: dimension exponents not divisible by 2".to_owned(),
                    ));
                }
                Ok(v)
            }
            "cbrt" => {
                let mut v = arg;
                if !v.root_assign(3) {
                    return Err(ParseError(
                        "cbrt: dimension exponents not divisible by 3".to_owned(),
                    ));
                }
                Ok(v)
            }
            "abs" => {
                let mut v = arg;
                v.factor = v.factor.abs();
                Ok(v)
            }
            "exp" => {
                if !arg.is_dimensionless() {
                    return Err(ParseError("exp: argument must be dimensionless".to_owned()));
                }
                Ok(UnitValue::from_factor(arg.factor.exp()))
            }
            "ln" => {
                if !arg.is_dimensionless() {
                    return Err(ParseError("ln: argument must be dimensionless".to_owned()));
                }
                Ok(UnitValue::from_factor(arg.factor.ln()))
            }
            "log" | "log10" => {
                if !arg.is_dimensionless() {
                    return Err(ParseError("log: argument must be dimensionless".to_owned()));
                }
                Ok(UnitValue::from_factor(arg.factor.log10()))
            }
            "log2" => {
                if !arg.is_dimensionless() {
                    return Err(ParseError(
                        "log2: argument must be dimensionless".to_owned(),
                    ));
                }
                Ok(UnitValue::from_factor(arg.factor.log2()))
            }
            "sin" => {
                if !arg.is_dimensionless() {
                    return Err(ParseError("sin: argument must be dimensionless".to_owned()));
                }
                Ok(UnitValue::from_factor(arg.factor.sin()))
            }
            "cos" => {
                if !arg.is_dimensionless() {
                    return Err(ParseError("cos: argument must be dimensionless".to_owned()));
                }
                Ok(UnitValue::from_factor(arg.factor.cos()))
            }
            "tan" => {
                if !arg.is_dimensionless() {
                    return Err(ParseError("tan: argument must be dimensionless".to_owned()));
                }
                Ok(UnitValue::from_factor(arg.factor.tan()))
            }
            _ => {
                // Check if it's a table call: tablename(numeric_value)
                if let Some(table) = self.db.tables.get(name) {
                    if !arg.is_dimensionless() {
                        return Err(ParseError(format!(
                            "table {name}: argument must be dimensionless"
                        )));
                    }
                    let output = table
                        .interpolate(arg.factor)
                        .ok_or_else(|| ParseError(format!("table {name}: interpolation failed")))?;
                    let unit_expr = format!("{output} {}", table.unit);
                    return self.parse_sub(&unit_expr);
                }
                self.call_user_function(name, arg)
            }
        }
    }

    fn call_user_function(&mut self, name: &str, arg: UnitValue) -> Result<UnitValue, ParseError> {
        let func: FunctionDef = self
            .db
            .functions
            .get(name)
            .cloned()
            .ok_or_else(|| ParseError(format!("unknown function: {name}")))?;

        let old_val = self.vars.insert(func.param.clone(), arg);
        let result = self.parse_sub(&func.forward.clone());
        if let Some(v) = old_val {
            self.vars.insert(func.param.clone(), v);
        } else {
            self.vars.remove(&func.param);
        }
        result
    }

    fn call_inverse_function(
        &mut self,
        name: &str,
        arg: UnitValue,
    ) -> Result<UnitValue, ParseError> {
        let func: FunctionDef = self
            .db
            .functions
            .get(name)
            .cloned()
            .ok_or_else(|| ParseError(format!("unknown function for inverse: {name}")))?;
        let reverse = func
            .reverse
            .ok_or_else(|| ParseError(format!("function has no inverse: {name}")))?;
        let old_val = self.vars.insert(name.to_owned(), arg);
        let result = self.parse_sub(&reverse);
        if let Some(v) = old_val {
            self.vars.insert(name.to_owned(), v);
        } else {
            self.vars.remove(name);
        }
        result
    }
}

/// Parse a GNU units expression string into a fully-reduced [`UnitValue`].
///
/// All derived units are recursively expanded; only primitive base units
/// (defined with `!`) remain in `dimensions`.
pub(crate) fn parseunit(input: &str) -> Result<UnitValue, ParseError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ParseError("empty expression".to_owned()));
    }
    let tokens = tokenize(trimmed)?;
    let db = db_read();
    let mut parser = Parser::new(tokens, &db);
    let result = parser.parse_expr()?;
    // Ensure the entire input was consumed.
    if !matches!(parser.peek(), Token::Eof) {
        return Err(ParseError(format!(
            "unexpected trailing input: {:?}",
            parser.peek()
        )));
    }
    Ok(result)
}

/// Parse a sub-expression with pre-existing variable bindings (used for
/// function evaluation at the call site in `mod.rs`).
pub(crate) fn parseunit_with_vars(
    input: &str,
    vars: &HashMap<String, UnitValue>,
) -> Result<UnitValue, ParseError> {
    let tokens = tokenize(input.trim())?;
    let db = db_read();
    let mut parser = Parser::new(tokens, &db);
    parser.vars = vars.clone();
    let result = parser.parse_expr()?;
    if !matches!(parser.peek(), Token::Eof) {
        return Err(ParseError(format!(
            "unexpected trailing input: {:?}",
            parser.peek()
        )));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rstest::rstest;

    use super::*;
    use crate::engine::native::types::UnitValue;

    #[test]
    fn tokenize_number() {
        let tokens = tokenize("42").unwrap();

        assert_eq!(tokens[0], Token::Num(42.0));
        assert_eq!(tokens[1], Token::Eof);
    }

    #[test]
    fn tokenize_hex() {
        let tokens = tokenize("0xff").unwrap();

        assert_eq!(tokens[0], Token::Num(255.0));
        assert_eq!(tokens[1], Token::Eof);
    }

    #[test]
    fn tokenize_operators() {
        let tokens = tokenize("* / ^ |").unwrap();

        assert_eq!(tokens[0], Token::Star);
        assert_eq!(tokens[1], Token::Slash);
        assert_eq!(tokens[2], Token::Caret);
        assert_eq!(tokens[3], Token::Pipe);
        assert_eq!(tokens[4], Token::Eof);
    }

    #[test]
    fn parse_simple_number() {
        let v = parseunit("7").unwrap();

        assert_eq!(v.factor, 7.0);
        assert!(v.is_dimensionless());
    }

    #[test]
    fn parse_multiplication() {
        let v = parseunit("2 * 3").unwrap();

        assert_eq!(v.factor, 6.0);
        assert!(v.is_dimensionless());
    }

    #[test]
    fn parse_division() {
        let v = parseunit("10 / 2").unwrap();

        assert_eq!(v.factor, 5.0);
        assert!(v.is_dimensionless());
    }

    #[test]
    fn parse_pipe_operator() {
        let v = parseunit("1|2").unwrap();

        assert!((v.factor - 0.5).abs() < 1e-12);
        assert!(v.is_dimensionless());
    }

    #[test]
    fn parse_unary_minus() {
        let v = parseunit("-5").unwrap();

        assert_eq!(v.factor, -5.0);
        assert!(v.is_dimensionless());
    }

    #[test]
    fn call_builtin_sqrt() {
        let v = parseunit("sqrt(4)").unwrap();

        assert!((v.factor - 2.0).abs() < 1e-12);
        assert!(v.is_dimensionless());
    }

    #[test]
    fn parse_power() {
        crate::definitions::ensure_definitions();

        let v = parseunit("m^3").unwrap();

        assert_eq!(v.factor, 1.0);
        assert_eq!(v.dimensions.get("m"), Some(&3));
    }

    #[test]
    fn parse_double_star_power() {
        crate::definitions::ensure_definitions();

        let v = parseunit("kg**2").unwrap();

        assert_eq!(v.factor, 1.0);
        assert_eq!(v.dimensions.get("kg"), Some(&2));
    }

    #[test]
    fn parse_parentheses() {
        crate::definitions::ensure_definitions();

        let v = parseunit("(2 * m)").unwrap();

        assert_eq!(v.factor, 2.0);
        assert_eq!(v.dimensions.get("m"), Some(&1));
    }

    #[test]
    fn parse_juxtaposition() {
        crate::definitions::ensure_definitions();

        let v = parseunit("5 m").unwrap();

        assert_eq!(v.factor, 5.0);
        assert_eq!(v.dimensions.get("m"), Some(&1));
    }

    #[test]
    fn resolve_prefix() {
        crate::definitions::ensure_definitions();

        let v = parseunit("kilogram").unwrap();

        assert!((v.factor - 1.0).abs() < 1e-9);
        assert_eq!(v.dimensions.get("kg"), Some(&1));
    }

    /// Uses the standard `square(x)` function from definitions.units (x^2)
    /// to exercise the user-function code path without mutating the global DB.
    #[test]
    fn call_user_function() {
        crate::definitions::ensure_definitions();

        let v = parseunit("square(3)").unwrap();

        assert!((v.factor - 9.0).abs() < 1e-12);
    }

    #[rstest]
    #[case::empty("")]
    #[case::whitespace_only("   ")]
    fn error_on_empty(#[case] input: &str) {
        let result = parseunit(input);

        assert!(result.is_err());
    }

    /// Uses numeric-only sub-expressions so no unit-DB lookup is needed.
    #[rstest]
    #[case::missing_close("(2")]
    #[case::stray_close(")")]
    fn error_on_unbalanced_parens(#[case] input: &str) {
        let result = parseunit(input);

        assert!(result.is_err());
    }

    #[test]
    fn error_on_trailing_input() {
        let result = parseunit("2 )");

        assert!(result.is_err());
        let msg = result.unwrap_err().0;
        assert!(
            msg.contains("trailing"),
            "expected trailing-input error, got: {msg}"
        );
    }

    #[test]
    fn parseunit_with_vars_resolves_binding() {
        let vars = HashMap::from([("x".to_owned(), UnitValue::from_factor(3.0))]);

        let v = parseunit_with_vars("x", &vars).unwrap();

        assert!((v.factor - 3.0).abs() < 1e-12);
        assert!(v.is_dimensionless());
    }

    #[test]
    fn error_on_parseunit_with_vars_trailing_input() {
        let vars = HashMap::new();

        let result = parseunit_with_vars("2 )", &vars);

        assert!(result.is_err());
        assert!(
            result.unwrap_err().0.contains("trailing"),
            "error should mention trailing input"
        );
    }
}
