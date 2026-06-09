# gnu-units

Safe Rust bindings for the [GNU units](https://www.gnu.org/software/units/) conversion library.

This crate provides a high-level Rust API for dimensional analysis and unit conversion. By default it uses a pure-Rust engine with no C dependency. Optionally, a vendored GNU units C library backend is available.

## Features

- Parse and convert between thousands of units (length, mass, time, currency, etc.)
- Dimensionless factor extraction
- Conformability checking between units
- List all known unit definitions
- Optional Rust-native currency rate updates (`currency-update` feature)

## Usage

```toml
[dependencies]
gnu-units = "0.2"
```

```rust
use gnu_units::{convert, parse, conformable, Unit, ErrorCode, UnitsError};

// Simple conversion factor
let factor = convert("km", "miles").unwrap();
assert!((factor - 0.62137).abs() < 1e-4);

// Conversion with a value
let km_val = 5.0;
let miles_val = km_val * convert("km", "miles").unwrap();

// Parse and inspect a unit
let unit = parse("kg m/s^2").unwrap();
println!("base units: {}", unit.base_units()); // "kg m / s s"

// Check conformability
let a = Unit::parse("km").unwrap();
let b = Unit::parse("miles").unwrap();
assert!(a.is_conformable(&b));

// Find all conformable units
let lengths = conformable("m").unwrap();
assert!(lengths.contains(&"mile".to_string()));

// Error handling with ErrorCode
let err = Unit::parse("???").unwrap_err();
match err.code() {
    ErrorCode::Parse => println!("parse error"),
    ErrorCode::UnknownUnit => println!("unknown unit"),
    _ => println!("other error: {err}"),
}
```

## Optional features

| Feature            | Description                                                |
| ------------------ | ---------------------------------------------------------- |
| `native` (default) | Pure-Rust unit engine, no C dependency                     |
| `vendored`         | Build and statically link the vendored GNU units C sources |
| `bindgen`          | Regenerate FFI bindings                                    |
| `currency-update`  | Enable Rust-native currency exchange rate updates          |

> **Note:** `native` and `vendored` are mutually exclusive. To use the C backend:
> ```toml
> [dependencies]
> gnu-units = { version = "0.2", default-features = false, features = ["vendored"] }
> ```

## License

Licensed under GPL-3.0-or-later. The vendored GNU units source code is also GPL-3.0-or-later.
