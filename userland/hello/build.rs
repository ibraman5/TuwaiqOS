//! Points the linker at `link.ld` via an absolute path, so this crate
//! builds correctly regardless of the working directory `cargo` is invoked
//! from.

use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let linker_script = manifest_dir.join("link.ld");
    println!("cargo:rustc-link-arg=-T{}", linker_script.display());
    println!("cargo:rerun-if-changed=link.ld");
}
