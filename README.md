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
