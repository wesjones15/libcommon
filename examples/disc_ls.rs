//! Lists a GameCube disc image's files (ISO/GCM/CISO), read-only:
//! `cargo run --example disc_ls -- <image> [path prefix]`.
use libcommon::disc::Disc;
use std::path::Path;

fn main() {
    let mut args = std::env::args().skip(1);
    let image = args.next().expect("usage: disc_ls <image> [prefix]");
    let prefix = args.next().unwrap_or_default();
    let disc = Disc::open(Path::new(&image)).expect("open disc");
    println!("{} rev {} ({} files)", disc.game_id, disc.version, disc.len());
    for f in disc.files().iter().filter(|f| f.path.starts_with(&prefix)) {
        println!("{:>10} {}", f.size, f.path);
    }
}
