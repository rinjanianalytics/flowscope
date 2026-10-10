// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Embed the console data files (avatar parts, documentation, frontend).

use std::fmt::Write;
use std::path::{Path, PathBuf};

/// List the files of a directory, recursively, sorted by name.
fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            files(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let mut code = String::new();
    // `data/docs` is a link to the `docs` directory of the repository. The
    // published package has a copy of it.
    for (name, dir) in [("AVATARS", "data/avatars"), ("DOCS", "data/docs"), ("FRONTEND", "data/frontend")] {
        let dir = manifest.join(dir).canonicalize().unwrap_or_else(|_| manifest.join(dir));
        println!("cargo:rerun-if-changed={}", dir.display());
        let mut list = vec![];
        files(&dir, &mut list);
        writeln!(code, "pub static {name}: &[(&str, &[u8])] = &[").unwrap();
        for path in list {
            let rel = path.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/");
            writeln!(code, "    ({rel:?}, include_bytes!({:?})),", path.display().to_string()).unwrap();
        }
        writeln!(code, "];").unwrap();
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("embedded.rs");
    std::fs::write(out, code).unwrap();
}
