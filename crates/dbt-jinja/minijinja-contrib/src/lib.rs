//! MiniJinja-Contrib is a utility crate for [MiniJinja](https://github.com/mitsuhiko/minijinja)
//! that adds support for certain utilities that are too specific for the MiniJinja core.  This is
//! usually because they provide functionality that Jinja2 itself does not have.
//!
//! To add all of these to an environment you can use the [`add_to_environment`] function.
//!
//! ```
//! use minijinja::Environment;
//!
//! let mut env = Environment::new();
//! minijinja_contrib::add_to_environment(&mut env);
//! ```
#![cfg_attr(docsrs, feature(doc_cfg))]

use std::collections::BTreeMap;

use minijinja::{Environment, Value};

/// Implements Python methods for better compatibility.
#[cfg(feature = "pycompat")]
pub mod pycompat;

/// Module for creating dynamic refleciton-based Jinja objects that don't need their own Rust
/// types.
pub mod dyn_object;

/// Utility filters.
pub mod filters;

/// Globals
pub mod globals;

/// Datetime & re functino
pub mod modules;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

/// Registers all features of this crate with an [`Environment`].
///
/// All the filters that are available will be added, same with global
/// functions that exist.
///
/// **Note:** the `pycompat` support is intentionally not registered
/// with the environment.
pub fn add_to_environment(env: &mut Environment) {
    env.add_filter("pluralize", filters::pluralize);
    env.add_filter("filesizeformat", filters::filesizeformat);
    env.add_filter("truncate", filters::truncate);
    let mut modules = BTreeMap::new();
    #[cfg(feature = "wordcount")]
    {
        env.add_filter("wordcount", filters::wordcount);
    }
    #[cfg(feature = "wordwrap")]
    {
        env.add_filter("wordwrap", filters::wordwrap);
    }
    #[cfg(feature = "datetime")]
    {
        env.add_filter("datetimeformat", filters::datetimeformat);
        env.add_filter("timeformat", filters::timeformat);
        env.add_filter("dateformat", filters::dateformat);
        let datetime_namespace = crate::modules::py_datetime::create_datetime_module();
        modules.insert(
            "datetime".to_string(),
            Value::from_object(datetime_namespace),
        );
    }
    let re_namespace = crate::modules::re::create_re_namespace();
    modules.insert("re".to_string(), Value::from_object(re_namespace));

    let pytz_namespace = crate::modules::pytz::create_pytz_namespace();
    modules.insert("pytz".to_string(), Value::from_object(pytz_namespace));

    let validation_namespace = crate::modules::validation::create_validation_namespace();
    #[cfg(feature = "rand")]
    {
        env.add_filter("random", filters::random);
        env.add_function("lipsum", globals::lipsum);
        env.add_function("randrange", globals::randrange);
    }
    env.add_function("cycler", globals::cycler);
    env.add_function("joiner", globals::joiner);
    env.add_global("dict", globals::create_dict_namespace());
    env.add_global("modules", Value::from_object(modules));
    env.add_global("validation", Value::from_object(validation_namespace));
}
