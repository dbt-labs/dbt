use std::fmt::Write as _;
use std::io::Read as _;
use std::path::PathBuf;

use dbt_adbc::install::{self, DriverTriplet, find_expected_checksum, format_driver_url};
use sha2::{Digest, Sha256};

const OS_ARCH: &[(&str, &str)] = &[
    ("apple-darwin", "aarch64"),
    ("apple-darwin", "x86_64"),
    ("manylinux_2_17-linux-gnu", "aarch64"),
    ("manylinux_2_17-linux-gnu", "x86_64"),
    ("manylinux_2_28-linux-gnu", "aarch64"),
    ("manylinux_2_28-linux-gnu", "x86_64"),
    ("pc-windows-msvc", "x86_64"),
];

struct OwnedDriverTriplet {
    os: String,
    arch: String,
    version: String,
}

impl OwnedDriverTriplet {
    fn as_ref(&self) -> DriverTriplet<'_> {
        DriverTriplet {
            os: &self.os,
            arch: &self.arch,
            version: &self.version,
        }
    }
}

/// (backend_name, triplet)
type DriverEntry = (String, OwnedDriverTriplet);
/// (backend_name, triplet, checksum)
type ChecksumEntry = (String, OwnedDriverTriplet, String);

fn parse_backend_and_version(s: &str) -> (&str, &str) {
    // Split on the first '-' only: "bigquery-0.21.0.dev+dbt0.21.7" -> ("bigquery", "0.21.0.dev+dbt0.21.7")
    let pos = s.find('-').expect("expected '-' in backend-version string");
    (&s[..pos], &s[pos + 1..])
}

fn build_driver_entries(toml: &toml::Table) -> Vec<DriverEntry> {
    let drivers = toml["drivers"]
        .as_array()
        .expect("drivers.toml: 'drivers' must be an array");
    let mut entries = Vec::new();
    for value in drivers {
        let bv = value
            .as_str()
            .expect("drivers.toml: each driver entry must be a string");
        let (backend, version) = parse_backend_and_version(bv);
        for &(os, arch) in OS_ARCH {
            // XXX: reduce noise by skipping platforms that don't have a build
            // for manylinux 2.28 yet. This should change in the future when
            // we fix the manylinux labels in all the drivers.
            if os.starts_with("manylinux_2_28") && backend != "dbt" {
                continue;
            }
            entries.push((
                backend.to_string(),
                OwnedDriverTriplet {
                    os: os.to_string(),
                    arch: arch.to_string(),
                    version: version.to_string(),
                },
            ));
        }
    }
    entries.sort_by(|a, b| (&a.0, a.1.as_ref()).cmp(&(&b.0, b.1.as_ref())));
    entries
}

fn download_and_hash(agent: &ureq::Agent, url: &str) -> Result<String, ureq::Error> {
    let mut response = agent.get(url).call()?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 256 * 1024]; // 256 KiB buffer
    let mut reader = response.body_mut().as_reader();
    loop {
        let n = reader.read(&mut buf).map_err(ureq::Error::Io)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let hash = hasher.finalize();
    Ok(format!("{:x}", hash))
}

fn main() {
    // Checksums are of whatever this host serves, so name it: a stray
    // DBT_ADBC_USE_STAGING_CDN would otherwise silently record pre-release
    // hashes.
    eprintln!("Syncing checksums from https://{}", install::cdn_host());

    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output_path = crate_dir.join("src/checksums.rs");

    let toml_path = crate_dir.join("drivers.toml");
    let toml_content = std::fs::read_to_string(&toml_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", toml_path.display()));
    let toml_table: toml::Table = toml_content
        .parse()
        .unwrap_or_else(|e| panic!("failed to parse {}: {e}", toml_path.display()));

    let http_agent = install::build_http_agent();

    let entries = build_driver_entries(&toml_table);
    let mut checksums: Vec<ChecksumEntry> = Vec::new();
    let mut new_count: usize = 0;
    let mut not_found_count: usize = 0;

    for (backend, triplet) in &entries {
        let tr = triplet.as_ref();
        if let Some(hash) = find_expected_checksum(backend, tr) {
            eprintln!(
                "Cached  {}-{} ({}, {})",
                backend, triplet.version, triplet.os, triplet.arch
            );
            checksums.push((
                backend.clone(),
                OwnedDriverTriplet {
                    os: triplet.os.clone(),
                    arch: triplet.arch.clone(),
                    version: triplet.version.clone(),
                },
                hash.to_string(),
            ));
            continue;
        }

        let url = format_driver_url(backend, tr);
        eprint!(
            "Downloading {}-{} ({}, {})... ",
            backend, triplet.version, triplet.os, triplet.arch
        );
        match download_and_hash(&http_agent, &url) {
            Ok(hash) => {
                eprintln!("ok");
                new_count += 1;
                checksums.push((
                    backend.clone(),
                    OwnedDriverTriplet {
                        os: triplet.os.clone(),
                        arch: triplet.arch.clone(),
                        version: triplet.version.clone(),
                    },
                    hash,
                ));
            }
            Err(ureq::Error::StatusCode(status)) => {
                eprintln!("skipped (HTTP {})", status);
                not_found_count += 1;
            }
            Err(e) => {
                eprintln!("FAILED: {}", e);
                std::process::exit(1);
            }
        }
    }

    let mut out = String::new();
    writeln!(out, "// DO NOT EDIT!!!").unwrap();
    writeln!(
        out,
        "// This file is generated by cargo run --bin adbc-sync"
    )
    .unwrap();
    writeln!(out).unwrap();
    writeln!(out, "#[allow(clippy::type_complexity)]").unwrap();
    writeln!(out, "#[rustfmt::skip]").unwrap();
    writeln!(out, "pub static SORTED_CDN_DRIVER_CHECKSUMS: [(").unwrap();
    writeln!(out, "    (").unwrap();
    writeln!(out, "        &str, // backend").unwrap();
    writeln!(out, "        &str, // target_os").unwrap();
    writeln!(out, "        &str, // arch").unwrap();
    writeln!(out, "        &str, // version").unwrap();
    writeln!(out, "    ),").unwrap();
    writeln!(out, "    &str, // checksum").unwrap();
    writeln!(out, "); {}] = [", checksums.len()).unwrap();
    for (backend, triplet, checksum) in &checksums {
        writeln!(out, "    (").unwrap();
        writeln!(
            out,
            "        (\"{}\", \"{}\", \"{}\", \"{}\"),",
            backend, triplet.os, triplet.arch, triplet.version
        )
        .unwrap();
        writeln!(out, "        \"{}\",", checksum).unwrap();
        writeln!(out, "    ),").unwrap();
    }
    writeln!(out, "];").unwrap();

    std::fs::write(&output_path, &out).expect("failed to write checksums.rs");
    eprintln!(
        "Wrote {} checksums ({} new, {} not found). Run git diff fs/sa/crates/dbt-adbc/src/checksums.rs to review.",
        checksums.len(),
        new_count,
        not_found_count,
    );
}
