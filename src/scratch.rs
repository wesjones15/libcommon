//! Scratch folders for tests, inside the workspace's own `target/`.
//!
//! Tests that write files used to go through `std::env::temp_dir()`, which sits outside the
//! project. A sandboxed run (or any locked-down account) cannot write there, so those tests
//! failed or needed an elevated run. Everything here stays under `target/test-scratch/`, which
//! `cargo clean` removes along with the rest of the build.

use std::path::{Path, PathBuf};

/// The `target` folder of the workspace that owns `manifest_dir`: `CARGO_TARGET_DIR` when it
/// is set, else `target/` beside the nearest `Cargo.lock` at or above `manifest_dir`.
pub fn target_dir(manifest_dir: &Path) -> PathBuf {
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let dir = PathBuf::from(dir);
        return if dir.is_absolute() { dir } else { manifest_dir.join(dir) };
    }
    manifest_dir
        .ancestors()
        .find(|p| p.join("Cargo.lock").is_file())
        .unwrap_or(manifest_dir)
        .join("target")
}

/// A fresh, empty folder `target/test-scratch/<name>-<pid>` for one test. Pass
/// `env!("CARGO_MANIFEST_DIR")` so the folder lands in the calling crate's workspace. The pid
/// keeps parallel `cargo test` processes apart; give each test its own `name` within a process.
pub fn dir(manifest_dir: &str, name: &str) -> PathBuf {
    let dir = target_dir(Path::new(manifest_dir))
        .join("test-scratch")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("test scratch folder {}: {e}", dir.display()));
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_is_inside_target_and_empty() {
        let d = dir(env!("CARGO_MANIFEST_DIR"), "scratch-self");
        let target = target_dir(Path::new(env!("CARGO_MANIFEST_DIR")));
        assert!(d.starts_with(&target), "{} not under {}", d.display(), target.display());
        std::fs::write(d.join("x"), b"1").unwrap();
        let again = dir(env!("CARGO_MANIFEST_DIR"), "scratch-self");
        assert_eq!(again, d);
        assert!(std::fs::read_dir(&again).unwrap().next().is_none(), "not emptied");
        let _ = std::fs::remove_dir_all(again);
    }
}
