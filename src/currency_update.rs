use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs;
use std::path::Path;

use chrono::Utc;
use reqwest::blocking::Client;
use serde::Deserialize;

/// `CurrencySource` lists the external services that provide live exchange rates.
///
/// Choose a source based on your requirements for API-key usage and supported base
/// currencies. Pass the chosen variant through [`CurrencyUpdateOptions::source`] when
/// calling [`update_currency_file`].
///
/// # Examples
///
/// ```no_run
/// use gnu_units::currency_update::CurrencySource;
///
/// let free = CurrencySource::Floatrates;
/// let paid = CurrencySource::ExchangerateApi;
/// ```
#[derive(Debug, Clone, Copy)]
pub enum CurrencySource {
    /// `ExchangerateApi` fetches rates from [exchangerate-api.com](https://www.exchangerate-api.com).
    ///
    /// When [`CurrencyUpdateOptions::api_key`] is `None`, the free open endpoint at
    /// `open.er-api.com` is used (rate-limited). When an API key is supplied, the
    /// authenticated v6 endpoint is used instead, which supports more base currencies
    /// and higher request quotas.
    ExchangerateApi,
    /// `Floatrates` fetches rates from [floatrates.com](https://www.floatrates.com).
    ///
    /// No API key is required. The base currency selects the appropriate JSON feed;
    /// inverse rates are reported relative to that base. [`CurrencyUpdateOptions::api_key`]
    /// is ignored when this variant is selected.
    Floatrates,
}

impl CurrencySource {
    fn label(self) -> &'static str {
        match self {
            Self::ExchangerateApi => "exchangerate-api.com",
            Self::Floatrates => "floatrates.com",
        }
    }
}

/// `CurrencyUpdateOptions` holds the configuration for a single currency file update.
///
/// Construct this struct directly or rely on the [`Default`] implementation, which
/// selects [`CurrencySource::ExchangerateApi`] with a USD base and no API key.
///
/// # Examples
///
/// ```no_run
/// use gnu_units::currency_update::{CurrencySource, CurrencyUpdateOptions};
///
/// let opts = CurrencyUpdateOptions {
///     source: CurrencySource::Floatrates,
///     base: "EUR",
///     api_key: None,
/// };
/// ```
#[derive(Debug, Clone)]
pub struct CurrencyUpdateOptions<'a> {
    /// Currency rate provider to use when fetching exchange rates.
    pub source: CurrencySource,
    /// Base currency [ISO 4217](https://en.wikipedia.org/wiki/ISO_4217) code (e.g. `"USD"`).
    ///
    /// All exchange rates written to the output file are expressed relative to this
    /// currency. The value is normalised to upper-case before use, so `"usd"` and
    /// `"USD"` are equivalent.
    pub base: &'a str,
    /// Optional API key for the [`CurrencySource::ExchangerateApi`] provider.
    ///
    /// When `None`, the free open endpoint (`open.er-api.com`) is queried and may be
    /// rate-limited. When `Some`, the authenticated v6 endpoint is used. This field is
    /// ignored when [`source`][CurrencyUpdateOptions::source] is
    /// [`CurrencySource::Floatrates`].
    pub api_key: Option<&'a str>,
}

impl<'a> Default for CurrencyUpdateOptions<'a> {
    fn default() -> Self {
        Self {
            source: CurrencySource::ExchangerateApi,
            base: "USD",
            api_key: None,
        }
    }
}

/// `UpdateError` enumerates every error that can occur during a currency file update.
///
/// All variants are constructable from the underlying error types via `From`
/// implementations, so `?` propagation works naturally in functions that return
/// [`Result<T>`].
///
/// The [`fmt::Display`] implementation produces human-readable messages prefixed by
/// the error category:
///
/// | Variant | Display prefix |
/// |---|---|
/// | `Io` | `"io error: …"` |
/// | `Http` | `"http error: …"` |
/// | `Json` | `"json error: …"` |
/// | `InvalidTemplate` | `"invalid currency template: …"` |
/// | `Provider` | `"currency provider error: …"` |
///
/// # Examples
///
/// ```no_run
/// use gnu_units::currency_update::UpdateError;
///
/// fn describe(err: UpdateError) -> String {
///     match err {
///         UpdateError::Io(e) => format!("filesystem problem: {e}"),
///         UpdateError::Http(e) => format!("network problem: {e}"),
///         UpdateError::Json(e) => format!("bad JSON: {e}"),
///         UpdateError::InvalidTemplate(msg) => format!("template broken: {msg}"),
///         UpdateError::Provider(msg) => format!("upstream rejected: {msg}"),
///     }
/// }
/// ```
#[derive(Debug)]
pub enum UpdateError {
    /// `Io` wraps a [`std::io::Error`] raised when reading or writing the currency file.
    Io(std::io::Error),
    /// `Http` wraps a [`reqwest::Error`] raised during the HTTP request to the rate provider.
    ///
    /// This includes connection failures, TLS errors, and non-2xx HTTP status codes.
    Http(reqwest::Error),
    /// `Json` wraps a [`serde_json::Error`] raised when the provider response cannot be parsed.
    Json(serde_json::Error),
    /// `InvalidTemplate` indicates the bundled `currency.units` template is structurally invalid.
    ///
    /// The `&'static str` payload names the specific element that is missing or malformed
    /// (e.g. `"missing rates section marker"`).
    InvalidTemplate(&'static str),
    /// `Provider` carries an error message returned by the upstream rate provider.
    ///
    /// This covers logical errors such as an unrecognised API key, an unsupported base
    /// currency, or an explicit error field in the provider's JSON response.
    Provider(String),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io error: {}", err),
            Self::Http(err) => write!(f, "http error: {}", err),
            Self::Json(err) => write!(f, "json error: {}", err),
            Self::InvalidTemplate(msg) => write!(f, "invalid currency template: {}", msg),
            Self::Provider(msg) => write!(f, "currency provider error: {}", msg),
        }
    }
}

impl std::error::Error for UpdateError {}

impl From<std::io::Error> for UpdateError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<reqwest::Error> for UpdateError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

impl From<serde_json::Error> for UpdateError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

/// `Result<T>` is a convenience alias for [`std::result::Result<T, UpdateError>`].
///
/// All public functions in this module return this type, so callers can handle
/// errors uniformly without repeating the error type annotation.
///
/// # Examples
///
/// ```no_run
/// use gnu_units::currency_update::Result;
///
/// fn example() -> Result<()> {
///     Ok(())
/// }
/// ```
pub type Result<T> = std::result::Result<T, UpdateError>;

#[derive(Debug)]
struct TemplateData {
    prelude: String,
    code_to_unit: Vec<(String, String)>,
    existing_definitions: HashMap<String, String>,
}

/// `update_currency_file` fetches live exchange rates and writes a GNU units currency file to `path`.
///
/// The function reads the bundled `currency.units` template shipped with the GNU units data
/// files, queries the provider configured in `opts` for current rates keyed to `opts.base`,
/// renders the result, and overwrites `path` with the updated file.
///
/// # Errors
///
/// Returns `Err` in the following cases:
///
/// * [`UpdateError::InvalidTemplate`] — the bundled template is structurally malformed.
/// * [`UpdateError::Http`] — the HTTP request to the rate provider failed or returned a
///   non-2xx status.
/// * [`UpdateError::Json`] — the provider response could not be deserialised.
/// * [`UpdateError::Provider`] — the provider returned a logical error (e.g. invalid API
///   key, unsupported base currency).
/// * [`UpdateError::Io`] — writing the output file failed.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use gnu_units::currency_update::{update_currency_file, CurrencySource, CurrencyUpdateOptions};
///
/// # fn main() -> gnu_units::currency_update::Result<()> {
/// let opts = CurrencyUpdateOptions {
///     source: CurrencySource::Floatrates,
///     base: "USD",
///     api_key: None,
/// };
/// update_currency_file(Path::new("/tmp/currency.units"), &opts)?;
/// # Ok(())
/// # }
/// ```
pub fn update_currency_file(path: &Path, opts: &CurrencyUpdateOptions<'_>) -> Result<()> {
    use crate::definitions::CURRENCY_UNITS;

    let template = parse_template(CURRENCY_UNITS)?;
    let base = opts.base.to_ascii_uppercase();
    let rates = fetch_rates(opts.source, &base, opts.api_key)?;
    let today = Utc::now().date_naive().format("%Y-%m-%d").to_string();
    let out = render_currency_file(&template, &rates, &base, opts.source.label(), &today)?;
    fs::write(path, out)?;
    Ok(())
}

fn render_currency_file(
    template: &TemplateData,
    rates: &HashMap<String, f64>,
    base: &str,
    source_label: &str,
    date: &str,
) -> Result<String> {
    let base_unit = template
        .code_to_unit
        .iter()
        .find(|(code, _)| code == base)
        .map(|(_, unit)| unit.as_str())
        .ok_or_else(|| {
            UpdateError::Provider(format!("base currency {} not found in template", base))
        })?;

    let mut out = String::new();
    out.push_str(&template.prelude);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
    out.push_str("# Currency exchange rates \n\n");
    out.push_str(&format!(
        "!message Currency exchange rates from {} ({} base) on {}\n\n",
        source_label, base, date
    ));

    let mut ordered_updates: BTreeMap<String, String> = BTreeMap::new();
    for (code, &rate) in rates {
        ordered_updates.insert(code.clone(), format_rate_definition(rate, base_unit));
    }

    for (code, unit_name) in &template.code_to_unit {
        if let Some(def) = ordered_updates.get(code) {
            out.push_str(&format!("{:<26} {}\n", unit_name, def));
        } else if let Some(existing) = template.existing_definitions.get(unit_name) {
            out.push_str(&format!("{:<26} {}\n", unit_name, existing));
        }
    }

    Ok(out)
}

fn parse_template(input: &str) -> Result<TemplateData> {
    let marker = "# Currency exchange rates";
    let marker_pos = input
        .find(marker)
        .ok_or(UpdateError::InvalidTemplate("missing rates section marker"))?;

    let prelude = input[..marker_pos].trim_end().to_string();
    let mut code_to_unit = Vec::new();

    for line in prelude.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let mut parts = trimmed.split_whitespace();
        let code = match parts.next() {
            Some(code) if code.len() == 3 => code,
            _ => continue,
        };
        let unit_name = match parts.next() {
            Some(name) => name,
            None => continue,
        };

        code_to_unit.push((code.to_string(), unit_name.to_string()));
    }

    let mut existing_definitions = HashMap::new();
    for line in input[marker_pos..].lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('!') {
            continue;
        }

        let mut parts = trimmed.split_whitespace();
        let unit_name = match parts.next() {
            Some(name) => name,
            None => continue,
        };
        let def = trimmed[unit_name.len()..].trim();
        if !def.is_empty() {
            existing_definitions.insert(unit_name.to_string(), def.to_string());
        }
    }

    Ok(TemplateData {
        prelude,
        code_to_unit,
        existing_definitions,
    })
}

fn format_rate_definition(rate: f64, base_unit: &str) -> String {
    format!("1|{} {}", format_float(rate), base_unit)
}

fn format_float(value: f64) -> String {
    let mut s = format!("{:.12}", value);
    while s.contains('.') && s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    s
}

fn fetch_rates(
    source: CurrencySource,
    base: &str,
    api_key: Option<&str>,
) -> Result<HashMap<String, f64>> {
    let client = Client::builder().build()?;
    match source {
        CurrencySource::ExchangerateApi => fetch_exchangerate_api(&client, base, api_key),
        CurrencySource::Floatrates => fetch_floatrates(&client, base),
    }
}

#[derive(Debug, Deserialize)]
struct OpenErApiResponse {
    result: Option<String>,
    #[serde(rename = "error-type")]
    error_type: Option<String>,
    rates: Option<HashMap<String, f64>>,
}

#[derive(Debug, Deserialize)]
struct KeyedErApiResponse {
    result: Option<String>,
    #[serde(rename = "error-type")]
    error_type: Option<String>,
    conversion_rates: Option<HashMap<String, f64>>,
}

fn parse_open_er_response(body: OpenErApiResponse) -> Result<HashMap<String, f64>> {
    if body.result.as_deref() != Some("success") {
        return Err(UpdateError::Provider(
            body.error_type
                .unwrap_or_else(|| "unknown exchangerate-api error".to_string()),
        ));
    }
    body.rates.ok_or(UpdateError::Provider(
        "missing rates in response".to_string(),
    ))
}

fn parse_keyed_er_response(body: KeyedErApiResponse) -> Result<HashMap<String, f64>> {
    if body.result.as_deref() != Some("success") {
        return Err(UpdateError::Provider(
            body.error_type
                .unwrap_or_else(|| "unknown exchangerate-api error".to_string()),
        ));
    }
    body.conversion_rates.ok_or(UpdateError::Provider(
        "missing conversion_rates in response".to_string(),
    ))
}

fn fetch_exchangerate_api(
    client: &Client,
    base: &str,
    api_key: Option<&str>,
) -> Result<HashMap<String, f64>> {
    if let Some(key) = api_key {
        let url = format!("https://v6.exchangerate-api.com/v6/{}/latest/{}", key, base);
        let response = client.get(url).send()?.error_for_status()?;
        let body: KeyedErApiResponse = response.json()?;
        return parse_keyed_er_response(body);
    }

    let url = format!("https://open.er-api.com/v6/latest/{}", base);
    let response = client.get(url).send()?.error_for_status()?;
    let body: OpenErApiResponse = response.json()?;
    parse_open_er_response(body)
}

#[derive(Debug, Deserialize)]
struct FloatRateEntry {
    code: String,
    #[serde(rename = "inverseRate")]
    inverse_rate: f64,
}

fn parse_floatrates_response(
    map: HashMap<String, FloatRateEntry>,
    base: &str,
) -> HashMap<String, f64> {
    let mut out = HashMap::new();
    for entry in map.values() {
        out.insert(entry.code.to_ascii_uppercase(), entry.inverse_rate);
    }
    out.insert(base.to_string(), 1.0);
    out
}

fn fetch_floatrates(client: &Client, base: &str) -> Result<HashMap<String, f64>> {
    let base_lower = base.to_ascii_lowercase();
    let url = format!("https://www.floatrates.com/daily/{}.json", base_lower);
    let response = client.get(url).send()?.error_for_status()?;
    let map: HashMap<String, FloatRateEntry> = response.json()?;
    Ok(parse_floatrates_response(map, base))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::collections::HashMap;

    #[rstest]
    #[case::strips_all_decimals(1.0, "1")]
    #[case::strips_trailing_zeros(1.5, "1.5")]
    #[case::keeps_twelve_decimals(0.123456789012, "0.123456789012")]
    #[case::zero_value(0.0, "0")]
    #[case::negative_strips_zeros(-2.5, "-2.5")]
    #[case::large_integer(1000.0, "1000")]
    #[case::small_fraction(0.000000000001, "0.000000000001")]
    #[case::half(0.5, "0.5")]
    fn format_float_strips_trailing_zeros(#[case] input: f64, #[case] expected: &str) {
        let result = format_float(input);

        assert_eq!(result, expected);
    }

    #[rstest]
    #[case::usd_base(0.5, "dollar", "1|0.5 dollar")]
    #[case::eur_base(1.2345, "euro", "1|1.2345 euro")]
    #[case::integer_rate(2.0, "pound", "1|2 pound")]
    fn format_rate_definition_output(
        #[case] rate: f64,
        #[case] base_unit: &str,
        #[case] expected: &str,
    ) {
        let result = format_rate_definition(rate, base_unit);

        assert_eq!(result, expected);
    }

    #[test]
    fn parse_template_extracts_prelude() {
        let input = "some preamble text\n# Currency exchange rates\nrate_line\n";

        let result = parse_template(input).unwrap();

        assert_eq!(result.prelude, "some preamble text");
    }

    #[test]
    fn parse_template_extracts_code_to_unit_pairs() {
        let input = "USD dollar\nEUR euro\n# Currency exchange rates\n";

        let result = parse_template(input).unwrap();

        assert_eq!(
            result.code_to_unit,
            vec![
                ("USD".to_string(), "dollar".to_string()),
                ("EUR".to_string(), "euro".to_string()),
            ]
        );
    }

    #[test]
    fn parse_template_extracts_existing_definitions() {
        let input = "# Currency exchange rates\ndollar 1|1.0 USD\n";

        let result = parse_template(input).unwrap();

        assert_eq!(
            result.existing_definitions.get("dollar"),
            Some(&"1|1.0 USD".to_string())
        );
    }

    #[test]
    fn parse_template_error_on_missing_marker() {
        let input = "no marker here\n";

        let result = parse_template(input);

        assert!(result.is_err());
    }

    #[test]
    fn parse_template_error_on_missing_marker_message() {
        let input = "no marker here\n";

        let result = parse_template(input);

        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("missing rates section marker"));
    }

    #[test]
    fn parse_template_skips_comment_lines_in_prelude() {
        let input = "# this is a comment\nUSD dollar\n# Currency exchange rates\n";

        let result = parse_template(input).unwrap();

        assert_eq!(
            result.code_to_unit,
            vec![("USD".to_string(), "dollar".to_string())]
        );
    }

    #[test]
    fn parse_template_skips_non_three_letter_codes() {
        let input = "US dollar\nEURO euro\nUSD usdollar\n# Currency exchange rates\n";

        let result = parse_template(input).unwrap();

        assert_eq!(
            result.code_to_unit,
            vec![("USD".to_string(), "usdollar".to_string())]
        );
    }

    #[test]
    fn parse_template_skips_code_lines_without_unit_name() {
        let input = "USD\nEUR euro\n# Currency exchange rates\n";

        let result = parse_template(input).unwrap();

        assert_eq!(
            result.code_to_unit,
            vec![("EUR".to_string(), "euro".to_string())]
        );
    }

    #[test]
    fn parse_template_empty_prelude_when_marker_at_start() {
        let input = "# Currency exchange rates\ndollar 1|1.0 USD\n";

        let result = parse_template(input).unwrap();

        assert_eq!(result.prelude, "");
        assert!(result.code_to_unit.is_empty());
    }

    #[test]
    fn parse_template_skips_message_directives_in_rates_section() {
        let input = "# Currency exchange rates\n!message some message\ndollar 1|1.0 USD\n";

        let result = parse_template(input).unwrap();

        assert!(!result.existing_definitions.contains_key("!message"));
    }

    #[test]
    fn parse_template_skips_rate_lines_with_empty_definition() {
        let input = "# Currency exchange rates\ndollar\n";

        let result = parse_template(input).unwrap();

        assert!(!result.existing_definitions.contains_key("dollar"));
    }

    #[test]
    fn parse_template_existing_definitions_do_not_bleed_into_code_to_unit() {
        let input = "JPY japanesyen\n# Currency exchange rates\njapanesyen 1|0.0067 dollar\n";

        let result = parse_template(input).unwrap();

        assert_eq!(
            result.code_to_unit,
            vec![("JPY".to_string(), "japanesyen".to_string())]
        );
        assert!(result.existing_definitions.contains_key("japanesyen"));
    }

    #[rstest]
    #[case::exchangerate_api(CurrencySource::ExchangerateApi, "exchangerate-api.com")]
    #[case::floatrates(CurrencySource::Floatrates, "floatrates.com")]
    fn currency_source_label(#[case] source: CurrencySource, #[case] expected: &str) {
        let result = source.label();

        assert_eq!(result, expected);
    }

    #[test]
    fn currency_update_options_default_values() {
        let opts = CurrencyUpdateOptions::default();

        assert!(matches!(opts.source, CurrencySource::ExchangerateApi));
        assert_eq!(opts.base, "USD");
        assert!(opts.api_key.is_none());
    }

    #[test]
    fn update_error_display_invalid_template() {
        let err = UpdateError::InvalidTemplate("test message");

        let msg = err.to_string();

        assert_eq!(msg, "invalid currency template: test message");
    }

    #[test]
    fn update_error_display_provider() {
        let err = UpdateError::Provider("some provider error".to_string());

        let msg = err.to_string();

        assert_eq!(msg, "currency provider error: some provider error");
    }

    #[test]
    fn update_error_display_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err = UpdateError::Io(io_err);

        let msg = err.to_string();

        assert!(msg.contains("io error"));
        assert!(msg.contains("file not found"));
    }

    #[test]
    fn update_error_display_json() {
        let json_err: serde_json::Error =
            serde_json::from_str::<serde_json::Value>("{invalid}").unwrap_err();
        let err = UpdateError::Json(json_err);

        let msg = err.to_string();

        assert!(msg.starts_with("json error:"));
    }

    #[test]
    fn update_error_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "no access");

        let err = UpdateError::from(io_err);

        assert!(matches!(err, UpdateError::Io(_)));
    }

    #[test]
    fn update_error_from_json_error() {
        let json_err: serde_json::Error =
            serde_json::from_str::<serde_json::Value>("{invalid}").unwrap_err();

        let err = UpdateError::from(json_err);

        assert!(matches!(err, UpdateError::Json(_)));
    }

    fn make_template(
        code_to_unit: Vec<(&str, &str)>,
        existing_definitions: Vec<(&str, &str)>,
    ) -> TemplateData {
        TemplateData {
            prelude: "# Sample prelude".to_string(),
            code_to_unit: code_to_unit
                .into_iter()
                .map(|(c, u)| (c.to_string(), u.to_string()))
                .collect(),
            existing_definitions: existing_definitions
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn render_happy_path() {
        let template = make_template(
            vec![("USD", "dollar"), ("EUR", "euro"), ("GBP", "poundsterling")],
            vec![],
        );
        let mut rates = HashMap::new();
        rates.insert("EUR".to_string(), 0.92);
        rates.insert("GBP".to_string(), 0.79);

        let result = render_currency_file(
            &template,
            &rates,
            "USD",
            "exchangerate-api.com",
            "2024-01-15",
        )
        .unwrap();

        let expected = concat!(
            "# Sample prelude\n",
            "\n",
            "# Currency exchange rates \n",
            "\n",
            "!message Currency exchange rates from exchangerate-api.com (USD base) on 2024-01-15\n",
            "\n",
            "euro                       1|0.92 dollar\n",
            "poundsterling              1|0.79 dollar\n",
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn render_uses_existing_definitions_as_fallback() {
        let template = make_template(
            vec![("USD", "dollar"), ("EUR", "euro"), ("GBP", "poundsterling")],
            vec![("poundsterling", "1|0.8 dollar")],
        );
        let mut rates = HashMap::new();
        rates.insert("EUR".to_string(), 0.92);

        let result = render_currency_file(
            &template,
            &rates,
            "USD",
            "exchangerate-api.com",
            "2024-01-15",
        )
        .unwrap();

        let expected = concat!(
            "# Sample prelude\n",
            "\n",
            "# Currency exchange rates \n",
            "\n",
            "!message Currency exchange rates from exchangerate-api.com (USD base) on 2024-01-15\n",
            "\n",
            "euro                       1|0.92 dollar\n",
            "poundsterling              1|0.8 dollar\n",
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn render_error_on_unknown_base() {
        let template = make_template(vec![("USD", "dollar"), ("EUR", "euro")], vec![]);
        let rates = HashMap::new();

        let result = render_currency_file(&template, &rates, "XXX", "test", "2024-01-15");

        assert!(matches!(result, Err(UpdateError::Provider(_))));
    }

    #[test]
    fn render_preserves_template_order() {
        // code_to_unit order: EUR first, then GBP — output must respect template order
        let template = make_template(
            vec![("USD", "dollar"), ("EUR", "euro"), ("GBP", "poundsterling")],
            vec![],
        );
        let mut rates = HashMap::new();
        rates.insert("GBP".to_string(), 0.79);
        rates.insert("EUR".to_string(), 0.92);

        let result = render_currency_file(&template, &rates, "USD", "test", "2024-01-15").unwrap();

        let expected = concat!(
            "# Sample prelude\n",
            "\n",
            "# Currency exchange rates \n",
            "\n",
            "!message Currency exchange rates from test (USD base) on 2024-01-15\n",
            "\n",
            "euro                       1|0.92 dollar\n",
            "poundsterling              1|0.79 dollar\n",
        );
        assert_eq!(result, expected);
        let euro_pos = result.find("euro").unwrap();
        let gbp_pos = result.find("poundsterling").unwrap();
        assert!(
            euro_pos < gbp_pos,
            "euro should appear before poundsterling"
        );
    }

    #[test]
    fn render_omits_currencies_without_rate_or_existing() {
        let template = make_template(
            vec![("USD", "dollar"), ("EUR", "euro"), ("GBP", "poundsterling")],
            vec![],
        );
        let mut rates = HashMap::new();
        rates.insert("EUR".to_string(), 0.92);

        let result = render_currency_file(&template, &rates, "USD", "test", "2024-01-15").unwrap();

        let expected = concat!(
            "# Sample prelude\n",
            "\n",
            "# Currency exchange rates \n",
            "\n",
            "!message Currency exchange rates from test (USD base) on 2024-01-15\n",
            "\n",
            "euro                       1|0.92 dollar\n",
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn parse_open_er_response_success() {
        let mut rates = HashMap::new();
        rates.insert("EUR".to_string(), 0.92_f64);
        rates.insert("GBP".to_string(), 0.79_f64);
        let body = OpenErApiResponse {
            result: Some("success".to_string()),
            error_type: None,
            rates: Some(rates),
        };

        let result = parse_open_er_response(body);

        let map = result.unwrap();
        assert_eq!(map.get("EUR"), Some(&0.92_f64));
        assert_eq!(map.get("GBP"), Some(&0.79_f64));
    }

    #[test]
    fn parse_open_er_response_error_on_failure_result() {
        let body = OpenErApiResponse {
            result: Some("error".to_string()),
            error_type: Some("invalid-key".to_string()),
            rates: None,
        };

        let result = parse_open_er_response(body);

        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("invalid-key"));
    }

    #[test]
    fn parse_open_er_response_error_on_missing_rates() {
        let body = OpenErApiResponse {
            result: Some("success".to_string()),
            error_type: None,
            rates: None,
        };

        let result = parse_open_er_response(body);

        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("missing rates"));
    }

    #[test]
    fn parse_open_er_response_error_on_none_result() {
        let body = OpenErApiResponse {
            result: None,
            error_type: None,
            rates: None,
        };

        let result = parse_open_er_response(body);

        assert!(matches!(result, Err(UpdateError::Provider(_))));
    }

    #[test]
    fn parse_keyed_er_response_success() {
        let mut rates = HashMap::new();
        rates.insert("EUR".to_string(), 0.92_f64);
        let body = KeyedErApiResponse {
            result: Some("success".to_string()),
            error_type: None,
            conversion_rates: Some(rates),
        };

        let result = parse_keyed_er_response(body);

        let map = result.unwrap();
        assert_eq!(map.get("EUR"), Some(&0.92_f64));
    }

    #[test]
    fn parse_keyed_er_response_error_on_failure_result() {
        let body = KeyedErApiResponse {
            result: Some("error".to_string()),
            error_type: Some("quota-reached".to_string()),
            conversion_rates: None,
        };

        let result = parse_keyed_er_response(body);

        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("quota-reached"));
    }

    #[test]
    fn parse_keyed_er_response_error_on_missing_conversion_rates() {
        let body = KeyedErApiResponse {
            result: Some("success".to_string()),
            error_type: None,
            conversion_rates: None,
        };

        let result = parse_keyed_er_response(body);

        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("missing conversion_rates"));
    }

    #[test]
    fn parse_floatrates_response_converts_entries() {
        let mut map = HashMap::new();
        map.insert(
            "eur".to_string(),
            FloatRateEntry {
                code: "eur".to_string(),
                inverse_rate: 0.92,
            },
        );
        map.insert(
            "gbp".to_string(),
            FloatRateEntry {
                code: "gbp".to_string(),
                inverse_rate: 0.79,
            },
        );

        let result = parse_floatrates_response(map, "USD");

        assert_eq!(result.get("EUR"), Some(&0.92_f64));
        assert_eq!(result.get("GBP"), Some(&0.79_f64));
    }

    #[test]
    fn parse_floatrates_response_includes_base_at_one() {
        let map = HashMap::new();

        let result = parse_floatrates_response(map, "USD");

        assert_eq!(result.get("USD"), Some(&1.0_f64));
    }
}
