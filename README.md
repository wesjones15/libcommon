# libcommon

Small, game-independent Rust primitives shared by the GameCube character libraries.
This is a source dependency compiled into each native library, with no extra runtime DLL.

The first module is `texture`: clamp/repeat/mirror addressing, explicit GX/native-view
wrap-code conversion, and nearest RGBA8 sampling with independent texture axes.
Mirroring reflects color and alpha together, assembling quadrant textures without
changing their authored opacity. `Wrap::index` also serves bilinear CPU samplers.

Initial consumer: libsms asset parsing, particle baking/sampling and host wrap metadata.
The Sunshine viewer and RoR2 adapter consume that same metadata. libpikmin and
libssbm can adopt these primitives without depending on Sunshine's particle runtime.

Use a sibling checkout and a Cargo path dependency:

```toml
libcommon = { path = "../libcommon" }
```

Run `cargo test --release` here, then each changed consumer's release tests.
Keep behavior-preserving extraction and source-specific regression tests together.
Game-specific particle formats, emission rules, clocks and simulation belong in their
game libraries. No extracted assets or platform/Unity types belong in this crate.

## `texture::gx`

GX texture decoding to RGBA8 (`GxFormat`, `decode`, plus `rgb565`/`rgb5a3`/`ia8` for palette
entries): I4/I8/IA4/IA8, RGB565, RGB5A3, RGBA8, C4/C8/C14X2 (palette passed in, out-of-range
indices decode transparent), CMPR and Z8. Channel expansion replicates high bits as the
hardware does; intensity formats fill all four channels.

Consumers: libsms-assets and libpikmin-assets (each keeps its own format numbering and error
type and maps to `GxFormat`). Their pixel hashes over pseudo-random data are pinned in each
crate's `decoded_pixels_match_baseline` test and were recorded before the move.

Not yet migrated: libssbm-assets expands 5/6/3-bit channels by `v * 255 / max` rather than bit
replication, so some values differ by 1 (5-bit 13 gives 106, hardware gives 107). Moving it
changes its decoded pixels, so it waits for a deliberate decision, not a silent swap.

## `binary`

Bounds-checked big-endian readers: `Be` (random access at absolute offsets) and `Cursor`
(forward, with `seek`/`skip`/`align`). Reads never panic, including offsets that overflow
`usize`; a failure is `Truncated { offset }` where the read began, and a failed cursor read
leaves the position alone. Each consumer converts with `From<Truncated>` into its own error.

Consumers: libsms-assets (`Be`; its J3D name table stays there as `NameTable`) and
libpikmin-assets (`Cursor` as `Reader`; vectors, counts and `readString` stay there as
`ReaderExt`). Not migrated: libssbm-assets' `Dat` readers (relocation-aware, string errors)
and the small `Option`-returning helpers in libsms `skeleton.rs` and libssbm `sms.rs`.

## `audio::afc`

AFC ADPCM for Pikmin and Sunshine (4-bit and 2-bit frames, bank waves, the 16-pair predictor
table). `Overflow` makes the one real difference explicit: the DSP saturates (bank waves, so
`decode_wave` clamps) while the CPU stream decoder (`Jac_Decode_ADPCM`) truncates to 16 bits.
Consumers: libsms-assets (bank waves clamp, streams wrap) and libpikmin-assets (bank waves
clamp; streams currently clamp too, which differs from the decomp on overflow; change
`OVERFLOW` in `sound/stream.rs` to `Wrap` when that is wanted). Melee's DSP-ADPCM is a
different format and stays in libssbm-assets.

## `anim`

`hermite` / `hermite_s`: the cubic Hermite curve between two keys, with tangents in value per
time unit. Consumers keep their key formats, tangent units, loop rules and timing:
libsms (J3D tracks, JPA key frames) and libssbm (fobj tracks, the Sunshine rig reader) call it
with their own tangents and match their old results bit for bit (pinned in each crate);
libpikmin's `.anm` (tangents per second at 30 fps) and model clips sum in a different order, so
they match to f32 rounding, not bit for bit (checked against copies of the old formulas here).

Not shared: transform building. libpikmin's `srt_matrix` is a port of `makeSRT` and libsms and
libssbm use glam's ZYX Euler conversion; they are one-liners with different roots.

## `tev`

GX TEV stage arithmetic for CPU evaluators: `Regs` (PREV and REG0..2), `konst`, `color_in`,
`alpha_in`, and `combine_color` / `combine_alpha` (add, subtract and the compare ops, with
bias, scale and clamping). Texture reads, rasterized color and material interpretation stay
with each game's material reader. `texture::sample_rgba8_bilinear` is the matching CPU
sampler, built on `Wrap::index`.

Consumers: libpikmin-present and libsms-present. Each pinned every baked texture of its real
extracted models before the move (59 and 58 textures, `tests/tev_bakes.rs`, skipped without
`assets/`), and both still produce them hash for hash. The shared combiner also handles the
compare ops libpikmin's old one lacked; none of its shipped materials use them. The viewers'
GLSL interpreters are separate and unchanged.
