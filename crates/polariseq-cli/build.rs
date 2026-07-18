use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("progress_key.bin");

    let key: [u8; 32] = match env::var("POLARISEQ_PROGRESS_KEY") {
        Ok(val) => {
            let mut key = [0u8; 32];
            for i in 0..4u8 {
                let mut hasher = DefaultHasher::new();
                val.hash(&mut hasher);
                i.hash(&mut hasher);
                let h = hasher.finish();
                key[(i as usize) * 8..(i as usize + 1) * 8].copy_from_slice(&h.to_le_bytes());
            }
            key
        }
        Err(_) => {
            let seed = format!("polariseq-progress-{}", env!("CARGO_PKG_VERSION"));
            let mut key = [0u8; 32];
            for i in 0..4u8 {
                let mut hasher = DefaultHasher::new();
                seed.hash(&mut hasher);
                i.hash(&mut hasher);
                let h = hasher.finish();
                key[(i as usize) * 8..(i as usize + 1) * 8].copy_from_slice(&h.to_le_bytes());
            }
            key
        }
    };

    fs::write(&dest_path, key).unwrap();
    println!("cargo:rerun-if-env-changed=POLARISEQ_PROGRESS_KEY");
    println!("cargo:rerun-if-changed=build.rs");
}
