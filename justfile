set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Run tests with default features
test-default:
    cargo nextest run --workspace

# Run tests with currency-update feature
test-currency-update:
    cargo nextest run --workspace --features currency-update

# Run tests with vendored feature only
test-vendored:
    cargo nextest run --workspace --no-default-features --features vendored

# Run tests with vendored + bindgen features
test-vendored-bindgen:
    cargo nextest run --workspace --no-default-features --features vendored,bindgen

# Run all feature combinations (mirrors CI rust-tests matrix)
test-all: test-default test-currency-update test-vendored test-vendored-bindgen

# Create an annotated tag vVERSION and push it to origin
tag-release VERSION:
    @[ -n "{{VERSION}}" ] || (echo "ERROR: VERSION is empty" >&2; exit 1)
    @case "{{VERSION}}" in *" "*) echo "ERROR: VERSION must not contain spaces" >&2; exit 1;; esac
    @case "{{VERSION}}" in v*) echo "ERROR: VERSION must not start with 'v' (prefix is added automatically)" >&2; exit 1;; esac
    git tag -a "v{{VERSION}}" -m "Release v{{VERSION}}"
    git push origin "v{{VERSION}}"

# Publish gnu-units-sys to crates.io
publish-sys:
    cargo publish -p gnu-units-sys

# Publish gnu-units to crates.io
publish:
    cargo publish -p gnu-units

# Publish both crates
publish-all: publish-sys
    cargo publish -p gnu-units
