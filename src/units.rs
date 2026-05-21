pub(crate) static ELEMENTS: &str = include_str!("../data/elements.units");

#[cfg(feature = "currency-update")]
pub(crate) static CURRENCY: &str = include_str!("../data/currency.units");

#[cfg(feature = "currency-update")]
pub(crate) static CRYPTO: &str = include_str!("../data/crypto.units");

#[cfg(feature = "currency-update")]
pub(crate) static METAL_PRICES: &str = include_str!("../data/metal_prices.units");

#[cfg(feature = "currency-update")]
pub(crate) static CPI: &str = include_str!("../data/cpi.units");
