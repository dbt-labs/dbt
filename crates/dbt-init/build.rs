//! Packs the `dbt init` sample projects for embedding.
//!
//! Each project under `assets/` becomes one Zstd-compressed tar archive in `OUT_DIR`,
//! which `src/init.rs` embeds and unpacks when `dbt init` runs. The projects stay
//! plain files in the repository; only the binary carries them compressed, which
//! keeps their seed data (over 1 MiB of CSV) from being stored in it as-is.

use std::path::{Path, PathBuf};
use std::{env, fs, io};

/// The directories under `assets/` to pack. Keep in sync with
/// `ProjectTemplateAsset` in `src/init.rs`.
const TEMPLATES: &[&str] = &["jaffle_shop", "moms_flower_shop"];

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets");

    let assets =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo")).join("assets");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("set by cargo"));

    for template in TEMPLATES {
        let root = assets.join(template);
        let mut files = Vec::new();
        collect_files(&root, &mut files)?;
        // Sorted paths and fixed header fields make the archive depend only on the
        // files' paths and contents, so the build is reproducible.
        files.sort();

        let mut archive = tar::Builder::new(Vec::new());
        for path in files {
            let data = fs::read(&path)?;
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            let relative_path = path.strip_prefix(&root).expect("collected under root");
            archive.append_data(&mut header, relative_path, data.as_slice())?;
        }
        let archive = archive.into_inner()?;

        // The highest level costs build time only: Zstd unpacks at the same speed
        // whatever level packed the data.
        let mut compressed = vec![0; zstd_safe::compress_bound(archive.len())];
        let written = zstd_safe::compress(
            compressed.as_mut_slice(),
            &archive,
            zstd_safe::max_c_level(),
        )
        .map_err(|code| io::Error::other(zstd_safe::get_error_name(code)))?;
        compressed.truncate(written);
        fs::write(out_dir.join(format!("{template}.tar.zst")), compressed)?;
    }

    Ok(())
}

/// Collects every file under `dir`, recursively.
fn collect_files(dir: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}
