# Starstream

## Entry points

* `make` to build everything
    * `cargo build` to build Rust crates
* `./run` to run prototype VM and tests

## Implementation structure

* `starstream_vm`: Host and example/test code
    * Uses [wasmi](https://docs.rs/wasmi/0.31.2/wasmi/) as WASM interpreter
* `starstream_sys`: WASM-side Rust bindings for Starstream VM exports
* TODO: `starstream_compiler`: Compiler from Starstream language to WASM

## Examples

* `example_contract`
* `example_coordination`

Note that a single WASM file can contain any combination of coordination scripts, UTXOs, and tokens. This allows them to ship together as a unit. The limitations on what each type of contract can do are enforced dynamically.
