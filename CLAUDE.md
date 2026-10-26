# libcommon

Read `../CLAUDE.md` and `../AGENTS.md` first.
This crate contains game-independent Rust primitives used by libsms/libpikmin/libssbm.
No game data, simulation, graphics API dependencies or host-specific types.
Modules: `texture` (+ `texture::gx` decoding), `binary`, `disc`, `audio::afc`, `anim`, `tev`. libssbm-assets still has its own texture decoder and Dat reader (see README).
Prefer small extractions with real consumers over unused general frameworks.
Keep GX disk codes distinct from native-view ABI codes; they differ for clamp/repeat.
Test shared behavior and affected consumers with `cargo test --release`.
Work on `modern/1.5.0`; commit only your own files by explicit path.
