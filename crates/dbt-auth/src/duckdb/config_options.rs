use crate::AuthError;
use crate::config::{AdapterConfig, YmlValue};
use dbt_adbc::database;

pub(super) struct ConfigOptions<'a> {
    entries: Vec<(&'a str, ConfigValue<'a>)>,
}

enum ConfigValue<'a> {
    Scalar(&'a YmlValue),
    List(&'a [YmlValue]),
}

impl<'a> ConfigOptions<'a> {
    pub(super) fn from_config(config: &'a AdapterConfig) -> Result<Self, AuthError> {
        let mapping = match config.get("config_options") {
            None | Some(YmlValue::Null(_)) => return Ok(Self { entries: vec![] }),
            Some(YmlValue::Mapping(mapping, _)) => mapping,
            Some(_) => return Err(AuthError::config("config_options: expected a mapping")),
        };
        let mut entries: Vec<(&str, ConfigValue<'a>)> = Vec::with_capacity(mapping.len());
        for (key, value) in mapping {
            let key = key.as_str().filter(|key| !key.is_empty() && !key.contains('\0'))
                .ok_or_else(|| AuthError::config("config_options: option names must be non-empty strings without NUL characters"))?;
            match key.to_ascii_lowercase().as_str() {
                "path"
                | "uri"
                | "username"
                | "password"
                | "motherduck_token"
                | "motherduck_saas_mode" => {
                    return Err(AuthError::config(format!(
                        "config_options.{key}: this connection option is managed by the adapter; use the profile's path or MotherDuck settings instead"
                    )));
                }
                _ => {}
            }
            if let Some((existing, _)) = entries
                .iter()
                .find(|(existing, _)| key.eq_ignore_ascii_case(existing))
            {
                return Err(AuthError::config(format!(
                    "config_options: option names '{existing}' and '{key}' differ only in case"
                )));
            }
            let value = match value {
                YmlValue::Sequence(values, _) => {
                    for value in values {
                        ConfigValue::validate_scalar(key, value)?;
                    }
                    ConfigValue::List(values)
                }
                value => {
                    ConfigValue::validate_scalar(key, value)?;
                    ConfigValue::Scalar(value)
                }
            };
            entries.push((key, value));
        }
        // Profile maps can be deserialized through HashMap. Keep the option order
        // stable so equivalent profiles produce the same database fingerprint.
        entries.sort_unstable_by_key(|(key, _)| *key);
        Ok(Self { entries })
    }

    pub(super) fn apply(self, builder: &mut database::Builder) -> Result<(), AuthError> {
        for (key, value) in self.entries {
            builder.with_named_option(key, value.into_driver_string())?;
        }
        Ok(())
    }
}

impl ConfigValue<'_> {
    fn validate_scalar(key: &str, value: &YmlValue) -> Result<(), AuthError> {
        match value {
            YmlValue::String(value, _) if !value.contains('\0') => Ok(()),
            YmlValue::Bool(_, _) => Ok(()),
            YmlValue::Number(value, _) if value.as_f64().is_some_and(f64::is_finite) => Ok(()),
            _ => Err(AuthError::config(format!(
                "config_options.{key}: expected a string without NUL characters, boolean, finite number, or a flat list of these values"
            ))),
        }
    }

    fn into_driver_string(self) -> String {
        match self {
            Self::Scalar(value) => value
                .as_scalar_string()
                .expect("validated scalar")
                .into_owned(),
            Self::List(values) => {
                let values: Vec<String> = values
                    .iter()
                    .map(|value| match value {
                        // This is DuckDB's VARCHAR-to-LIST representation, not SQL.
                        // Its cast parser uses backslash escaping inside quoted strings:
                        // https://github.com/duckdb/duckdb/blob/d54cbbe3df17e8e5ce68312f483b4155836112aa/src/function/cast/vector_cast_helpers.cpp#L118
                        YmlValue::String(value, _) => {
                            format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
                        }
                        value => value
                            .as_scalar_string()
                            .expect("validated scalar")
                            .into_owned(),
                    })
                    .collect();
                format!("[{}]", values.join(", "))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::duckdb::DuckDbAuth;
    use crate::test_options::other_option_value;
    use crate::{Auth, NoopAuthWarningPrinter};
    use dbt_adbc::Backend;

    fn configure(yaml: &str) -> Result<database::Builder, AuthError> {
        let mapping = dbt_yaml::from_str(yaml).unwrap();
        DuckDbAuth::new(Backend::DuckDB, Box::new(NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(mapping))
    }

    #[test]
    fn scalar_options_reach_the_database_builder_as_strings() {
        let builder = configure(
            r#"
path: ':memory:'
config_options:
  allow_unsigned_extensions: true
  threads: 2
  memory_limit: '1GB'
  checkpoint_threshold: '16MB'
  preserve_insertion_order: 'false'
  allocator_flush_threshold: 123.5
"#,
        )
        .unwrap();
        for (key, expected) in [
            ("allow_unsigned_extensions", "true"),
            ("threads", "2"),
            ("memory_limit", "1GB"),
            ("checkpoint_threshold", "16MB"),
            ("preserve_insertion_order", "false"),
            ("allocator_flush_threshold", "123.5"),
            ("path", ":memory:"),
        ] {
            assert_eq!(other_option_value(&builder, key), Some(expected), "{key}");
        }
    }

    #[test]
    fn lists_use_duckdb_cast_encoding() {
        let builder = configure(r#"
config_options:
  extension_directories: ['/tmp/a,b', "/tmp/o'brien", 'C:\ext', 'NULL', '', "line\nbreak", '"quoted"']
  allowed_paths: []
  example_list: [true, 2, 3.5, 'false']
"#).unwrap();
        assert_eq!(
            other_option_value(&builder, "extension_directories"),
            Some(
                "['/tmp/a,b', '/tmp/o\\'brien', 'C:\\\\ext', 'NULL', '', 'line\nbreak', '\"quoted\"']"
            )
        );
        assert_eq!(other_option_value(&builder, "allowed_paths"), Some("[]"));
        assert_eq!(
            other_option_value(&builder, "example_list"),
            Some("[true, 2, 3.5, 'false']")
        );
    }

    #[test]
    fn absent_null_and_empty_options_preserve_existing_configuration() {
        let baseline = configure("path: dev.duckdb").unwrap();
        for value in ["{}", "null"] {
            let builder = configure(&format!("path: dev.duckdb\nconfig_options: {value}")).unwrap();
            assert_eq!(
                database::Builder::fingerprint(builder.other.iter()),
                database::Builder::fingerprint(baseline.other.iter())
            );
        }
    }

    #[test]
    fn rejects_invalid_shapes_without_echoing_values() {
        for options in [
            "super-secret",
            "[super-secret]",
            "{1: super-secret}",
            "{'': super-secret}",
            r#"{"bad\0key": super-secret}"#,
            "{memory_limit: {secret: super-secret}}",
            "{memory_limit: null}",
            "{allowed_paths: [[super-secret]]}",
            "{allowed_paths: [null]}",
            r#"{memory_limit: "super-secret\0"}"#,
            "{threads: .nan}",
            "{threads: .inf}",
        ] {
            let error = configure(&format!("config_options: {options}"))
                .unwrap_err()
                .msg()
                .to_owned();
            assert!(error.contains("config_options"), "{error}");
            assert!(!error.contains("super-secret"), "{error}");
        }
    }

    #[test]
    fn rejects_managed_connection_options_even_with_different_case() {
        for key in [
            "path",
            "uri",
            "username",
            "password",
            "motherduck_token",
            "motherduck_saas_mode",
            "PATH",
            "MOTHERDUCK_TOKEN",
        ] {
            for path in [":memory:", "md:my_db?motherduck_token=existing-secret"] {
                let error = configure(&format!(
                    "path: '{path}'\nconfig_options:\n  {key}: super-secret"
                ))
                .unwrap_err()
                .msg()
                .to_owned();
                assert!(error.contains(key), "{error}");
                assert!(error.contains("managed by the adapter"), "{error}");
                assert!(!error.contains("secret"), "{error}");
            }
        }
    }

    #[test]
    fn rejects_case_insensitive_duplicate_option_names() {
        for options in [
            "{threads: 2, THREADS: 4}",
            "{THREADS: 4, threads: 2}",
            "{threads: 2, memory_limit: 1GB, ThReAdS: 2}",
        ] {
            let error = configure(&format!("config_options: {options}"))
                .unwrap_err()
                .msg()
                .to_owned();
            assert!(error.starts_with("config_options:"), "{error}");
            assert!(error.contains("'threads'"), "{error}");
            assert!(
                error.contains("'THREADS'") || error.contains("'ThReAdS'"),
                "{error}"
            );
            assert!(error.contains("differ only in case"), "{error}");
        }
        let builder = configure("config_options: {THREADS: 2}").unwrap();
        assert_eq!(other_option_value(&builder, "THREADS"), Some("2"));
    }

    #[test]
    fn options_have_stable_order_and_keep_settings_separate() {
        let first =
            configure("config_options: {threads: 2, memory_limit: 1GB}\nsettings: {threads: 1}")
                .unwrap();
        let second = configure("config_options: {memory_limit: 1GB, threads: 2}").unwrap();
        assert_eq!(
            database::Builder::fingerprint(first.other.iter()),
            database::Builder::fingerprint(second.other.iter())
        );
        assert_eq!(other_option_value(&first, "threads"), Some("2"));
        let config = AdapterConfig::new(
            dbt_yaml::from_str("config_options: {threads: 2}\nsettings: {threads: 1}").unwrap(),
        );
        assert_eq!(
            crate::duckdb::init::generate_duckdb_init_sql(&config).unwrap(),
            vec!["SET threads = 1"]
        );
    }

    #[test]
    fn options_preserve_motherduck_credentials_and_backend() {
        for backend in [Backend::DuckDB, Backend::DuckDBExtended] {
            let config = AdapterConfig::new(
                dbt_yaml::from_str(
                    r#"
path: 'md:my_db?motherduck_token=existing-secret'
settings:
  motherduck_saas_mode: true
config_options:
  threads: 2
"#,
                )
                .unwrap(),
            );
            let builder = DuckDbAuth::new(backend, Box::new(NoopAuthWarningPrinter))
                .configure(&config)
                .unwrap();
            assert_eq!(builder.backend, backend);
            assert_eq!(
                other_option_value(&builder, "motherduck_token"),
                Some("existing-secret")
            );
            assert_eq!(
                other_option_value(&builder, "motherduck_saas_mode"),
                Some("true")
            );
            assert_eq!(other_option_value(&builder, "threads"), Some("2"));
        }
    }

    #[test]
    #[ignore = "loads the DuckDB ADBC driver from the system, local cache, or CDN"]
    fn connect_time_options_work_with_duckdb() {
        use adbc_core::options::AdbcVersion;
        use arrow_array::cast::AsArray;
        use arrow_array::types::Int64Type;
        use dbt_adbc::{connection, driver};

        let mut driver =
            driver::Builder::new(Backend::DuckDB, driver::LoadStrategy::SystemThenCdnCache)
                .with_adbc_version(AdbcVersion::V110)
                .try_load()
                .unwrap();
        let builder = configure(
            r#"
config_options:
  allow_unsigned_extensions: true
  threads: 2
  extension_directories: ['/tmp/a,b', "/tmp/o'brien", 'C:\ext']
"#,
        )
        .unwrap();
        let mut database = builder.build(&mut driver).unwrap();
        println!(
            "DuckDB driver version: {}",
            database.vendor_version().unwrap()
        );
        let mut conn = connection::Builder::default().build(&mut database).unwrap();
        let mut stmt = conn.new_statement().unwrap();
        stmt.set_sql_query("SELECT current_setting('allow_unsigned_extensions'), current_setting('threads'), current_setting('extension_directories')").unwrap();
        let batch = stmt.execute().unwrap().next().unwrap().unwrap();
        assert!(batch.column(0).as_boolean().value(0));
        assert_eq!(batch.column(1).as_primitive::<Int64Type>().value(0), 2);
        let paths = batch.column(2).as_list::<i32>().value(0);
        assert_eq!(
            paths.as_string::<i32>().iter().collect::<Vec<_>>(),
            vec![Some("/tmp/a,b"), Some("/tmp/o'brien"), Some(r"C:\ext")]
        );

        stmt.set_sql_query("SET allow_unsigned_extensions = true")
            .unwrap();
        assert!(
            stmt.execute().is_err(),
            "enabling unsigned extensions must happen before database open"
        );

        let config = AdapterConfig::new(dbt_yaml::from_str("settings: {threads: 1}").unwrap());
        for sql in crate::duckdb::init::generate_duckdb_init_sql(&config).unwrap() {
            stmt.set_sql_query(&sql).unwrap();
            stmt.execute().unwrap();
        }
        stmt.set_sql_query("SELECT current_setting('threads')")
            .unwrap();
        let batch = stmt.execute().unwrap().next().unwrap().unwrap();
        assert_eq!(batch.column(0).as_primitive::<Int64Type>().value(0), 1);

        for options in ["{not_a_duckdb_option: true}", "{threads: not_an_integer}"] {
            let invalid = configure(&format!("config_options: {options}")).unwrap();
            assert!(invalid.build(&mut driver).is_err());
        }
    }
}
