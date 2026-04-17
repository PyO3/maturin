use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest = Path::new(&out_dir).join("generated_info.txt");
    fs::write(dest, "hello from build.rs").unwrap();
}
