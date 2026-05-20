pub(crate) static ELEMENTS: &str = include_str!("../gnu-units-sys/vendor/units/elements.units");

#[cfg(feature = "currency-update")]
pub(crate) static CURRENCY: &str = include_str!("../gnu-units-sys/vendor/units/currency.units");

#[cfg(feature = "currency-update")]
pub(crate) static CRYPTO: &str = include_str!("../gnu-units-sys/vendor/units/crypto.units");

#[cfg(feature = "currency-update")]
pub(crate) static METAL_PRICES: &str =
    include_str!("../gnu-units-sys/vendor/units/metal_prices.units");

#[cfg(feature = "currency-update")]
pub(crate) static CPI: &str = include_str!("../gnu-units-sys/vendor/units/cpi.units");
