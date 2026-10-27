//! Copies one file out of a GameCube disc image (ISO/GCM/CISO), read-only on the image:
//! `cargo run --example disc_cat -- <image> <path on disc> <output file>`.
use libcommon::disc::Disc;
use std::path::Path;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [image, path, out] = a.as_slice() else { panic!("usage: disc_cat <image> <path> <out>") };
    let disc = Disc::open(Path::new(image)).expect("open disc");
    let bytes = disc.read_file(path).expect("read file");
    if let Some(dir) = Path::new(out).parent() { std::fs::create_dir_all(dir).unwrap(); }
    std::fs::write(out, &bytes).unwrap();
    println!("{} bytes -> {out}", bytes.len());
}
