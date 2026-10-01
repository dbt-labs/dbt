//! This crate adds utilities to embed MiniJinja templates
//! directly in the binary.  It is a static version of the
//! `path_loader` function with some optional filtering.
//!
//! First you need to add this as regular and build dependency:
//!
//! ```text
//! cargo add minijinja-embed
//! cargo add minijinja-embed --build
//! ```
//!
//! Afterwards you can embed a template folder in your `build.rs`
//! script.  You can also do this conditional based on a feature
//! flag.  In this example we just embed all templates in the
//! `src/templates` folder:
//!
//! ```rust
//! fn main() {
//!     // ...
//! # if false {
//!     minijinja_embed::embed_templates!("src/templates");
//! # }
//! }
//! ```
//!
//! Later when you create the environment you can load the embedded
//! templates:
//!
//! ```rust,ignore
//! use minijinja::Environment;
//!
//! let mut env = Environment::new();
//! minijinja_embed::load_templates!(&mut env);
//! ```
//!
//! For more information see [`embed_templates`].
#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]
#![allow(clippy::needless_doctest_main)]

use std::fmt::Write;
use std::fs::{self, DirEntry};
use std::io;
use std::path::Path;

/// Utility macro to store templates in a `build.rs` file.
///
/// This needs to be invoked in `build.rs` file with at least
/// the path to the templates.  Optionally it can be filtered
/// by extension and an alternative bundle name can be provided.
///
/// These are all equivalent:
///
/// ```rust
/// # fn foo() {
/// minijinja_embed::embed_templates!("src/templates");
/// minijinja_embed::embed_templates!("src/templates", &[][..]);
/// minijinja_embed::embed_templates!("src/templates", &[][..], "main");
/// # }
/// ```
///
/// To embed different folders, alternative bundle names can be provided.
/// Also templates can be filtered down by extension to avoid accidentally
/// including unexpected templates.
///
/// ```rust
/// # fn foo() {
/// minijinja_embed::embed_templates!("src/templates", &[".html", ".txt"]);
/// # }
/// ```
///
/// Later they can then be loaded into a Jinja environment with
/// the [`load_templates!`] macro.
///
/// # Panics
///
/// This function panics if the templates are not valid (eg: invalid syntax).
/// It's not possible to handle this error by design.  During development you
/// should be using dynamic template loading instead.
#[macro_export]
macro_rules! embed_templates {
    ($path:expr, $exts:expr, $bundle_name:expr) => {{
        let out_dir = ::std::env::var_os("OUT_DIR").unwrap();
        let dst_path = ::std::path::Path::new(&out_dir)
            .join(format!("minijinja_templates_{}.rs", $bundle_name));
        let generated = $crate::_embed_templates($path, $exts);
        println!("cargo:rerun-if-changed={}", $path);
        ::std::fs::write(dst_path, generated).unwrap();
    }};

    ($path:expr) => {
        $crate::embed_templates!($path, &[][..], "main");
    };

    ($path:expr, $exts:expr) => {
        $crate::embed_templates!($path, $exts, "main");
    };
}

/// Loads embedded templates into the environment.
///
/// This macro takes a MiniJinja environment as argument and optionally
/// also the name of a template bundle.  All templates in the bundle are
/// then loaded into the environment.  Templates are eagerly loaded into
/// the environment which means that no loader needs to be enabled.
///
/// ```rust,ignore
/// minijinja_embed::load_templates!(&mut env);
/// ```
///
/// By default the `main` bundled is loaded.  To load a different one
/// pass it as second argument:
///
/// ```rust,ignore
/// minijinja_embed::load_templates!(&mut env, "other_bundle");
/// ```
#[macro_export]
macro_rules! load_templates {
    ($env:expr, $bundle_name:literal) => {{
        let load_template = include!(concat!(
            env!("OUT_DIR"),
            "/minijinja_templates_",
            $bundle_name,
            ".rs"
        ));
        load_template(&mut $env);
    }};

    ($env:expr) => {
        $crate::load_templates!($env, "main");
    };
}

fn visit_dirs(dir: &Path, cb: &mut dyn FnMut(&DirEntry)) -> io::Result<()> {
    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path
                .file_name()
                .and_then(|x| x.to_str())
                .map_or(false, |x| x.starts_with('.'))
            {
                continue;
            }
            if path.is_dir() {
                visit_dirs(&path, cb)?;
            } else {
                cb(&entry);
            }
        }
    }
    Ok(())
}

#[doc(hidden)]
pub fn _embed_templates<P>(path: P, extensions: &[&str]) -> String
where
    P: AsRef<Path>,
{
    let path = path.as_ref().canonicalize().unwrap();
    let mut gen = String::new();
    writeln!(gen, "|env: &mut minijinja::Environment| {{").unwrap();

    visit_dirs(&path, &mut |f| {
        let p = f.path();
        if !extensions.is_empty()
            && !p
                .file_name()
                .and_then(|x| x.to_str())
                .map_or(false, |name| extensions.iter().any(|x| name.ends_with(x)))
        {
            return;
        }

        let contents = fs::read_to_string(&p).unwrap();
        let name = p.strip_prefix(&path).unwrap();

        writeln!(
            gen,
            "env.add_template({:?}, {:?}).expect(\"Embedded an invalid template\");",
            name.to_string_lossy().replace('\\', "/"),
            contents
        )
        .unwrap();
    })
    .unwrap();

    writeln!(gen, "}}").unwrap();

    gen
}
