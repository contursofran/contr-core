// Embeds the kit, for `herdr-contr-feed install`, so the helper alone can install it. The files are the repo's
// kit/ folder at build time.
use std::fs;
use std::path::{Path, PathBuf};

fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, PathBuf)>) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut entries: Vec<_> = fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())).map(|e| e.unwrap().path()).collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        if p.is_dir() {
            walk(&p, base, out);
            continue;
        }
        if name == ".DS_Store" {
            continue;
        }
        let rel = p.strip_prefix(base).unwrap().components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
        out.push((rel, p));
    }
}

include!("../build-stamp.rs");

fn main() {
    build_stamp();
    let mut src = String::new();
    // The kit (the Claude Code plugin of a coordinator and its workers), for <home>/herdr-contr/kit.
    let kit_base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../kit");
    assert!(kit_base.join(".claude-plugin/plugin.json").is_file(), "kit/ must sit next to xplat/");
    let mut kit = Vec::new();
    walk(&kit_base, &kit_base, &mut kit);
    kit.sort();
    src.push_str("/// Every file of the kit, as (path relative to the kit folder, bytes), sorted by path.\npub static KIT: &[(&str, &[u8])] = &[\n");
    for (rel, p) in &kit {
        println!("cargo:rerun-if-changed={}", p.display());
        src.push_str(&format!("    ({rel:?}, include_bytes!({:?})),\n", p.display().to_string()));
    }
    src.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    fs::write(out.join("embedded.rs"), src).unwrap();
}

