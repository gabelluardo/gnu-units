# gnu-units

Safe Rust bindings for the [GNU units](https://www.gnu.org/software/units/) conversion library.

This crate provides a high-level Rust API over the vendored GNU units C library, enabling dimensional analysis and unit conversion without spawning external processes.

## Features

- Parse and convert between thousands of units (length, mass, time, currency, etc.)
- Dimensionless factor extraction
- Conformability checking between units
- List all known unit definitions
- Optional Rust-native currency rate updates (`currency-update` feature)
- Statically links vendored GNU units with no system dependencies

## Usage

```toml
[dependencies]
gnu-units = "0.1"
```

```rust
use gnu_units::{convert, parse, conformable, Unit};

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
```

## Optional features

| Feature              | Description                                                |
| -------------------- | ---------------------------------------------------------- |
| `vendored` (default) | Build and statically link the vendored GNU units C sources |
| `bindgen`            | Regenerate FFI bindings from the C headers                 |
| `currency-update`    | Enable Rust-native currency exchange rate updates          |

## License

Licensed under GPL-3.0-or-later. The vendored GNU units source code is also GPL-3.0-or-later.
