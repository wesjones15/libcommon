# libcommon

Shared Rust support for the project's native runtimes: binary parsing, disc access, textures, animation math, audio, mesh processing and FFI diagnostics.

This is a Cargo source dependency used by [libsms](https://github.com/wesjones15/libsms), [libpikmin](https://github.com/wesjones15/libpikmin) and [libssbm](https://github.com/wesjones15/libssbm). It is compiled into its consumers and has no separate runtime DLL. [`ror2-common`](https://github.com/wesjones15/ror2-common) is a separate C# repository for host integration.

Format-specific interpretation, simulation and rendering remain in the consuming libraries. This crate contains no original game assets or Unity types and has no graphics API dependency.

## Modules

| Module | Responsibility |
| --- | --- |
| [`disc`](src/disc.rs) | ISO/GCM and CISO access, validated file-system tables, file/range reads and DOL reads; synthetic images for tests. |
| [`binary`](src/binary.rs) | Bounds-checked big-endian random-access and cursor readers. |
| [`texture`](src/texture.rs) | Addressing, wrap-code conversion, nearest and bilinear RGBA8 sampling. |
| [`texture::gx`](src/texture/gx.rs) | GX texture/palette decoding, including indexed formats and CMPR. |
| [`anim`](src/anim.rs) | Cubic Hermite interpolation. |
| [`tev`](src/tev.rs) | CPU color/alpha combiner arithmetic and register handling. |
| [`audio::afc`](src/audio/afc.rs) | AFC ADPCM decoding with explicit overflow behavior. |
| [`audio::out`](src/audio/out.rs) | Optional CPAL output with device-change and stream-loss handling. |
| [`vr_arms`](src/vr_arms.rs) | Weighted-mesh selection, cached arm cuts and first-person body visibility. |
| [`ffi`](src/ffi.rs) | Panic guards, failure values and diagnostic messages for native exports. |
| [`scratch`](src/scratch.rs) | Workspace-local test scratch directories. |

Source documentation describes each module's contract. The crate itself forbids unsafe Rust.

## Integration

Use a Rust toolchain supporting edition 2024. The maintained branch is `modern/1.5.0`. Keep this repository beside the consumer and add a Cargo path dependency:

```toml
[dependencies]
libcommon = { path = "../libcommon" }
```

Nested crates can inherit that path from their workspace dependencies. Optional device output is enabled with:

```toml
libcommon = { path = "../libcommon", features = ["cpal"] }
```

The default build has no external dependencies. The `cpal` feature adds the platform audio backend; mixing and sequencing remain the consumer's responsibility.

## Shared contracts

- **Readers:** truncated reads return errors. A failed cursor read does not advance it. Disc parsing validates bounds, paths and directory extents; consumers select accepted disc IDs/revisions and interpret the files.
- **Textures:** GX and native-view ABI codes differ for clamp/repeat; convert explicitly. Mirroring applies to all RGBA channels. GX channel expansion uses bit replication.
- **Materials and animation:** TEV shares arithmetic, not material interpretation or shaders. Hermite helpers share the curve, not tangent units, loop rules or clocks.
- **Audio:** AFC callers choose clamp or wrap behavior explicitly. Other ADPCM formats retain their own readers.
- **Meshes:** consumers adapt their rigs and triangles to `vr_arms::Mesh`. Cached cuts and arm-local coordinates are shared; rig names, rendering and host/controller placement are not.
- **FFI:** `ffi::guard(name, fallback, body)` records caught panic diagnostics and returns the supplied fallback. It does not roll back mutations; the consumer owns handle validity and recovery.
- **Tests:** `scratch::dir` writes beneath the owning workspace's `target/test-scratch/`, rather than a system temporary directory.

Some consumers intentionally retain different implementations. `libssbm-assets` keeps its texture decoder's existing rounding and its relocation-aware DAT reader. `libsms-present` and `libpikmin-present` share CPU combiner arithmetic while their viewer shaders remain separate. AFC stream overflow is selected by the caller. Preserve these differences unless a migration is deliberate and checked against existing output.

## Checks

```powershell
cargo test --release
cargo test --release --features cpal
```

Tests live alongside the modules. The feature-enabled run checks that configuration, not live device behavior.

Run affected consumers' release checks after shared changes. Their asset/texture regressions cover differences that primitive-level tests cannot. Disc-dependent consumer checks require a local cache and report skips when it is absent. Prefer small shared implementations with real consumers.

## License and credits

Original contributions by **wesleyasaurus** are dedicated to the public domain under
[CC0 1.0 Universal](LICENSE), to the extent wesleyasaurus owns the relevant rights.
Attribution for those contributions is appreciated but not required. Third-party software
and original game content retain their respective rights and terms.
See [CREDITS.md](CREDITS.md) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
