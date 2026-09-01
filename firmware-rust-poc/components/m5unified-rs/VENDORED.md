# Vendored from m5unified-rs

The files in this directory (`CMakeLists.txt`, `idf_component.yml`,
`m5u_shim.cpp`, `m5u_shim.h`, `m5u_shim_stub.cpp`) are copied unmodified from
`crates/m5unified-sys/native/` in
[mfiumara/m5unified-rs](https://github.com/mfiumara/m5unified-rs) version
0.3.8, per that project's own documented integration instructions
(`crates/m5unified-sys/native/README.md`): external consumers of the published
`m5unified`/`m5unified-sys` crates are expected to copy this directory into
their own `components/` folder as an ESP-IDF component, since the crate does
not currently expose the native shim's path to downstream build scripts.

Upstream license: MIT OR Apache-2.0 (see upstream `LICENSE`).

Do not hand-edit these files; re-fetch them if the `m5unified` dependency
version in `Cargo.toml` changes.

## Deviation: `idf_component.yml` version pin

Upstream's `idf_component.yml` requests `m5stack/M5Unified: "^0.2.13"`, which
resolves to the newest matching 0.2.x release. M5Unified 0.2.21 (PR #320,
"Replace the IO expander pull enable API with explicit modes") removed
`IOExpander_Base::enablePull()` and changed `setPullMode()`'s second argument
from `bool` to the `gpio_pull_t` enum. `m5u_shim.cpp` as shipped in
m5unified-sys 0.3.8 still calls the old bool-based API, so building against
0.2.21+ fails with compile errors in `m5u_io_expander_enable_pull`,
`m5u_io_expander_set_pull_mode`, `m5u_pi4ioe5v6408_enable_pull`, and
`m5u_pi4ioe5v6408_set_pull_mode`.

Pinned `m5stack/M5Unified` to the last compatible release, `0.2.20`, and
`m5stack/M5GFX` to the version that resolved successfully alongside it,
`0.2.28`, instead of upstream's version ranges. Revisit this pin (or patch
`m5u_shim.cpp` locally) once a m5unified-sys release fixes the IO expander
shim for current M5Unified.
