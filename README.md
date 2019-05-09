avr-device
==========
Auto-generated wrappers around registers for avr chips.

## Usage
You need to have [atdf2svd](https://github.com/Rahix/atdf2svd), [svd2rust](https://github.com/rust-embedded/svd2rust), [form](https://github.com/djmcgill/form), and [rustfmt](https://github.com/rust-lang/rustfmt) installed:
```bash
rustup component add rustfmt
cargo install form
cargo install svd2rust
git clone https://github.com/Rahix/atdf2svd
cd atdf2svd
cargo install --path .
```

Next, clone this repo and build the device definitions:
```bash
git clone https://github.com/Rahix/avr-device
cd avr-device
make
# You can build for just one specific chip using
make src/devices/<chip>/mod.rs
# I suggest building documentation as well
cargo +nightly doc --features <chip> --open
```

In your project add the following lines to `Cargo.toml`:
```toml
[dependencies.avr-device]
path = "path/to/avr-device/"
features = ["atmega32u4"]
```

Via the feature you can select which chip you want the register specifications for.  The following list is what is currently supported:
* `atmega328p`
* `atmega32u4`
* `attiny85`

## Internals
*avr-device* is generated using [`atdf2svd`](https://github.com/Rahix/atdf2svd) and [`svd2rust`](https://github.com/rust-embedded/svd2rust).  The vendor-provided *atdf* files can be found in `vendor/`.  Later on, we intend to add support for patching the svd files because some information from the provided atdfs is not quite as good as it should be (mainly undescriptive names and missing descriptions).

## License
*avr-device* is licensed under either of

 * Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

The vendored *atdf* files are licensed under the Apache License, Version 2.0 ([LICENSE-VENDOR](vendor/LICENSE)).
