# libcommon

Read `../CLAUDE.md` and `../AGENTS.md` first.
This crate contains game-independent Rust primitives used by libsms/libpikmin/libssbm.
No game data, simulation, graphics API dependencies or host-specific types.
Modules: `texture` (+ `texture::gx` decoding), `binary`, `disc`, `audio::afc`, `anim`, `tev`, `vr_arms` (the VR forearm-and-hand cut every core bakes on first run), `ffi` (the panic guard every core's C ABI export runs its body through: `guard(name, fallback, || ..)`, messages for the host via each core's `*_take_panic`), `scratch` (test scratch folders under the workspace's `target/test-scratch/`; tests must not write to `std::env::temp_dir()`, which a sandboxed run cannot reach). libssbm-assets still has its own texture decoder and Dat reader (see README).
Prefer small extractions with real consumers over unused general frameworks.
Keep GX disk codes distinct from native-view ABI codes; they differ for clamp/repeat.
Test shared behavior and affected consumers with `cargo test --release`.
Work on `modern/1.5.0`; commit only your own files by explicit path.
