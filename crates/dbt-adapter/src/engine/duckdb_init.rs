//! DuckDB initialization for [`super::AdbcEngine`]: registering the profile's custom extension
//! repositories (DuckDB 2.0+), then running the rest of the init SQL.
//!
//! Unlike the rest of the init SQL, registration needs the database's current state: each
//! declaration is compared with the existing registration before anything is created, because
//! `CREATE EXTENSION REPOSITORY IF NOT EXISTS` keeps a conflicting registration as it is.

use std::hash::{DefaultHasher, Hash, Hasher};

use adbc_core::error::{Error, Status};
use arrow_array::cast::AsArray;
use arrow_array::{Array, RecordBatch};
use dbt_adapter_core::AdapterType;
use dbt_adapter_sql::ident::{escape_string_literal, quote_identifier};
use dbt_adbc::{Connection, Database};
use dbt_auth::{
    AdapterConfig, DuckDbInit, DuckDbRepositoryDeclaration, DuckDbRepositoryKey,
    DuckDbUserRepositories,
};
use dbt_common::AdapterResult;

use crate::errors::{
    AdapterError, AdapterErrorKind, adbc_error_to_adapter_error, auth_error_to_adapter_error,
};

/// Initializes a newly created database with the profile's DuckDB init SQL, followed by
/// `catalog_attach_stmts`. Uses a temporary connection, opened only if there is work to do.
pub(crate) fn initialize(
    database: &dyn Database,
    config: &AdapterConfig,
    catalog_attach_stmts: Vec<String>,
) -> AdapterResult<()> {
    let init = DuckDbInit::from_config(config).map_err(auth_error_to_adapter_error)?;
    let mut conn = None;
    let new_connection = |conn: &mut Option<Box<dyn Connection>>| -> AdapterResult<()> {
        if conn.is_none() {
            *conn = Some(
                database
                    .new_connection()
                    .map_err(adbc_error_to_adapter_error)?,
            );
        }
        Ok(())
    };

    // Repositories are registered before extensions are installed from them.
    let user_repositories = if init.uses_user_repositories() {
        new_connection(&mut conn)?;
        register(conn.as_deref_mut().unwrap(), &init)?
    } else {
        DuckDbUserRepositories::Unsupported
    };

    let stmts = init.render(user_repositories);
    for (idx, sql) in stmts.iter().chain(&catalog_attach_stmts).enumerate() {
        new_connection(&mut conn)?;
        execute(conn.as_deref_mut().unwrap(), sql).map_err(|e| {
            adbc_error_to_adapter_error(Error::with_message_and_status(
                format!("DuckDB init SQL statement {} failed: {e}", idx + 1),
                Status::Internal,
            ))
        })?;
    }
    Ok(())
}

/// Validates `config` as [`initialize`] would and identifies the repository declarations it
/// initializes a database with. Run before reusing a cached database, so that the database is
/// never handed to a configuration that initialization rejects, or reused for different
/// declarations without registering them.
pub(crate) fn init_identity(config: &AdapterConfig) -> AdapterResult<u64> {
    let init = DuckDbInit::from_config(config).map_err(auth_error_to_adapter_error)?;
    let mut hasher = DefaultHasher::new();
    init.extension_repositories().hash(&mut hasher);
    Ok(hasher.finish())
}

/// The error for reusing a cached database whose [`init_identity`] differs.
pub(crate) fn init_identity_conflict() -> AdapterError {
    AdapterError::new(
        AdapterErrorKind::Configuration,
        "this DuckDB database was already initialized in this process with different \
         extension_repositories; start a new dbt process to apply them",
    )
}

/// Registers `init`'s declared repositories and reports whether the loaded DuckDB supports
/// user-provided repositories, if `init` uses any.
fn register(
    conn: &mut dyn Connection,
    init: &DuckDbInit<'_>,
) -> AdapterResult<DuckDbUserRepositories> {
    if !init.uses_user_repositories() {
        return Ok(DuckDbUserRepositories::Unsupported);
    }
    let declarations = init.extension_repositories();

    // Ask the loaded driver rather than assume a version from Fusion's own pins.
    let batch = query(
        conn,
        "SELECT version(), EXISTS (SELECT 1 FROM duckdb_functions() \
         WHERE function_name = 'duckdb_extension_repositories')",
    )?;
    let version = strings(&batch, 0)?.next().flatten().unwrap_or_default();
    let supported = batch
        .column(1)
        .as_boolean_opt()
        .ok_or_else(|| unexpected("support check"))?
        .value(0);
    if !supported {
        if declarations.is_empty() {
            // An extension `repo` that DuckDB doesn't know fails at `INSTALL` with DuckDB's error.
            return Ok(DuckDbUserRepositories::Unsupported);
        }
        return Err(AdapterError::new(
            AdapterErrorKind::NotSupported,
            format!(
                "extension_repositories requires DuckDB 2.0 or later, but the loaded DuckDB \
                 driver is {version}"
            ),
        ));
    }

    // 'forbidden' distrusts every user-provided repository, including ones registered earlier,
    // so their extensions could not be loaded even when nothing needs to be created.
    // https://github.com/duckdb/duckdb/blob/d54cbbe3df17e8e5ce68312f483b4155836112aa/src/main/extension/extension_helper.cpp#L877-L882
    let batch = query(
        conn,
        "SELECT current_setting('allow_extension_repositories')",
    )?;
    let access = strings(&batch, 0)?
        .next()
        .flatten()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if access == "forbidden" {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            "the profile uses custom extension repositories, but allow_extension_repositories is \
             'forbidden', so DuckDB neither adds nor trusts them",
        ));
    }

    let missing = unregistered(conn, declarations)?;
    // Using custom repositories in the profile opts in to them. Leave 'undecided' unless
    // creating a repository or installing from one requires 'allowed'; DuckDB permits changing
    // it from 'undecided' while the database is running.
    // https://github.com/duckdb/duckdb/blob/d54cbbe3df17e8e5ce68312f483b4155836112aa/src/main/settings/custom_settings.cpp#L215-L222
    let needs_allowed = !missing.is_empty() || init.installs_from_user_repositories();
    if access == "undecided" && needs_allowed {
        execute(conn, "SET allow_extension_repositories = 'allowed'")
            .map_err(adbc_error_to_adapter_error)?;
    }
    create_and_verify(conn, &missing)?;
    Ok(DuckDbUserRepositories::Supported)
}

/// Creates `missing`, then compares them with what is registered afterwards: another process
/// may have registered the same name since it was found missing, and `CREATE ... IF NOT EXISTS`
/// keeps that registration.
fn create_and_verify(
    conn: &mut dyn Connection,
    missing: &[&DuckDbRepositoryDeclaration<'_>],
) -> AdapterResult<()> {
    if missing.is_empty() {
        return Ok(());
    }
    for declaration in missing {
        execute(conn, &create_sql(declaration)).map_err(|e| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!(
                    "extension_repositories: failed to register '{}': {}",
                    declaration.name, e.message
                ),
            )
        })?;
    }

    if let Some(declaration) = unregistered(conn, missing.iter().copied())?.first() {
        return Err(AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            format!(
                "extension_repositories: '{}' is not registered after creating it",
                declaration.name
            ),
        ));
    }
    Ok(())
}

/// Checks the declarations that are registered, and returns the ones that are not.
fn unregistered<'d, 'a>(
    conn: &mut dyn Connection,
    declarations: impl IntoIterator<Item = &'d DuckDbRepositoryDeclaration<'a>>,
) -> AdapterResult<Vec<&'d DuckDbRepositoryDeclaration<'a>>>
where
    'a: 'd,
{
    let registered = Registered::query(conn)?;
    let mut missing = Vec::new();
    for declaration in declarations {
        match Registered::find(&registered, declaration.name) {
            Some(existing) => existing.check(declaration)?,
            None => missing.push(declaration),
        }
    }
    Ok(missing)
}

/// `CREATE EXTENSION REPOSITORY IF NOT EXISTS` for `declaration`. `IF NOT EXISTS` keeps a
/// conflicting existing repository, so the result must be compared afterwards.
fn create_sql(declaration: &DuckDbRepositoryDeclaration<'_>) -> String {
    let mut sql = format!(
        "CREATE EXTENSION REPOSITORY IF NOT EXISTS {} WITH PREFIX '{}'",
        quote_identifier(declaration.name, AdapterType::DuckDB),
        escape_string_literal(declaration.prefix, AdapterType::DuckDB)
    );
    if let DuckDbRepositoryKey::Pinned(key) = declaration.key {
        sql.push_str(&format!(
            " USING PUBLIC KEY '{}'",
            escape_string_literal(key, AdapterType::DuckDB)
        ));
    }
    sql
}

/// A user-provided repository as `duckdb_extension_repositories()` reports it.
struct Registered {
    name: String,
    prefix: String,
    /// Keys in DuckDB's compact form: the base64 body of the PEM, without whitespace.
    public_keys: Vec<String>,
    key_fingerprints: Vec<String>,
}

impl Registered {
    fn query(conn: &mut dyn Connection) -> AdapterResult<Vec<Self>> {
        let batches = batches(
            conn,
            "SELECT repository_name, prefix, public_keys, key_fingerprints \
             FROM duckdb_extension_repositories() WHERE type = 'USER_PROVIDED'",
        )?;
        let mut out = Vec::new();
        for batch in &batches {
            let names = strings(batch, 0)?;
            let prefixes = strings(batch, 1)?;
            let keys = string_lists(batch, 2)?;
            let fingerprints = string_lists(batch, 3)?;
            for (((name, prefix), public_keys), key_fingerprints) in
                names.zip(prefixes).zip(keys).zip(fingerprints)
            {
                out.push(Registered {
                    name: name.unwrap_or_default().to_owned(),
                    prefix: prefix.unwrap_or_default().to_owned(),
                    public_keys,
                    key_fingerprints,
                });
            }
        }
        Ok(out)
    }

    /// DuckDB stores user repository names in lowercase.
    // https://github.com/duckdb/duckdb/blob/d54cbbe3df17e8e5ce68312f483b4155836112aa/src/main/extension/extension_repository_manager.cpp#L252-L263
    fn find<'r>(registered: &'r [Self], name: &str) -> Option<&'r Self> {
        registered
            .iter()
            .find(|existing| existing.name.eq_ignore_ascii_case(name))
    }

    /// Fails if this registration's trust configuration differs from `declaration`. A
    /// registration is never replaced: that would change which keys DuckDB trusts.
    fn check(&self, declaration: &DuckDbRepositoryDeclaration<'_>) -> AdapterResult<()> {
        let mismatch = |what: String| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!(
                    "extension_repositories: repository '{}' is already registered with {what}. \
                     Run DROP EXTENSION REPOSITORY \"{}\", or use another name, to change it.",
                    declaration.name, self.name
                ),
            )
        };
        // DuckDB appends `/<version>/<platform>/...` to the prefix, so a trailing slash does
        // not select a different repository.
        // https://github.com/duckdb/duckdb/blob/d54cbbe3df17e8e5ce68312f483b4155836112aa/src/main/extension/extension_install.cpp#L218-L233
        if self.prefix.trim_end_matches('/') != declaration.prefix.trim_end_matches('/') {
            return Err(mismatch(format!(
                "prefix '{}', which does not match the configured prefix '{}'",
                self.prefix, declaration.prefix
            )));
        }
        // Without a configured key, the key DuckDB pinned at registration stays trusted.
        if let DuckDbRepositoryKey::Pinned(key) = declaration.key {
            let mut configured = compact_public_keys(key);
            configured.sort_unstable();
            let mut stored = self.public_keys.clone();
            stored.sort_unstable();
            if configured != stored {
                return Err(mismatch(format!(
                    "public keys [{}], which do not match the configured public_key",
                    self.key_fingerprints.join(", ")
                )));
            }
        }
        Ok(())
    }
}

/// The compact forms of the keys in a `public_key` value, as DuckDB stores them: the base64
/// body of each PEM block, or of the whole value without PEM armor, without whitespace.
// https://github.com/duckdb/duckdb/blob/d54cbbe3df17e8e5ce68312f483b4155836112aa/src/main/extension/extension_repository_manager.cpp#L48-L104
fn compact_public_keys(key: &str) -> Vec<String> {
    const PEM_HEADER: &str = "-----BEGIN PUBLIC KEY-----";
    const PEM_FOOTER: &str = "-----END PUBLIC KEY-----";
    let bodies: Vec<&str> = if key.contains(PEM_HEADER) {
        key.split(PEM_HEADER)
            .skip(1)
            .map(|block| block.split(PEM_FOOTER).next().unwrap_or(block))
            .collect()
    } else {
        vec![key]
    };
    bodies
        .into_iter()
        .map(|body| {
            body.chars()
                .filter(|c| !matches!(c, ' ' | '\t' | '\n' | '\r'))
                .collect()
        })
        .collect()
}

fn unexpected(what: &str) -> AdapterError {
    AdapterError::new(
        AdapterErrorKind::UnexpectedResult,
        format!("unexpected DuckDB result for the extension repository {what}"),
    )
}

/// All batches of `sql`'s result.
fn batches(conn: &mut dyn Connection, sql: &str) -> AdapterResult<Vec<RecordBatch>> {
    let mut stmt = conn.new_statement().map_err(adbc_error_to_adapter_error)?;
    stmt.set_sql_query(sql)
        .map_err(adbc_error_to_adapter_error)?;
    let reader = stmt.execute().map_err(adbc_error_to_adapter_error)?;
    reader
        .collect::<Result<_, _>>()
        .map_err(|e| AdapterError::new(AdapterErrorKind::Arrow, e.to_string()))
}

/// The first batch of `sql`'s result, which must have at least one row.
fn query(conn: &mut dyn Connection, sql: &str) -> AdapterResult<RecordBatch> {
    batches(conn, sql)?
        .into_iter()
        .find(|batch| batch.num_rows() > 0)
        .ok_or_else(|| unexpected("query"))
}

fn execute(conn: &mut dyn Connection, sql: &str) -> Result<(), Error> {
    let mut stmt = conn.new_statement()?;
    stmt.set_sql_query(sql)?;
    stmt.execute_update()?;
    Ok(())
}

fn strings(
    batch: &RecordBatch,
    column: usize,
) -> AdapterResult<impl Iterator<Item = Option<&str>>> {
    Ok(batch
        .column(column)
        .as_string_opt::<i32>()
        .ok_or_else(|| unexpected("string column"))?
        .iter())
}

fn string_lists(batch: &RecordBatch, column: usize) -> AdapterResult<Vec<Vec<String>>> {
    let lists = batch
        .column(column)
        .as_list_opt::<i32>()
        .ok_or_else(|| unexpected("list column"))?;
    (0..lists.len())
        .map(|row| {
            if lists.is_null(row) {
                return Ok(vec![]);
            }
            let values = lists.value(row);
            let values = values
                .as_string_opt::<i32>()
                .ok_or_else(|| unexpected("list column"))?;
            Ok(values.iter().flatten().map(str::to_owned).collect())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_BODY: &str = "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAabc+/=";

    #[test]
    fn compact_public_keys_matches_pem_and_base64_forms() {
        let pem = format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n{}\n-----END PUBLIC KEY-----\n",
            &KEY_BODY[..20],
            &KEY_BODY[20..]
        );
        assert_eq!(compact_public_keys(&pem), vec![KEY_BODY]);
        assert_eq!(
            compact_public_keys(&format!("  {KEY_BODY}\r\n")),
            vec![KEY_BODY]
        );
        assert_eq!(
            compact_public_keys(&format!("{pem}{}", pem.replace("abc", "xyz"))),
            vec![KEY_BODY.to_owned(), KEY_BODY.replace("abc", "xyz")]
        );
    }

    fn registered(prefix: &str, keys: &[&str]) -> Registered {
        Registered {
            name: "query_farm".to_owned(),
            prefix: prefix.to_owned(),
            public_keys: keys.iter().map(|k| (*k).to_owned()).collect(),
            key_fingerprints: vec!["sha256:abcd".to_owned()],
        }
    }

    fn declaration<'a>(
        prefix: &'a str,
        key: DuckDbRepositoryKey<'a>,
    ) -> DuckDbRepositoryDeclaration<'a> {
        DuckDbRepositoryDeclaration {
            name: "Query_Farm",
            prefix,
            key,
        }
    }

    #[test]
    fn check_ignores_trailing_slashes_and_accepts_stored_keys_without_a_pin() {
        let existing = registered("https://example.invalid/", &[KEY_BODY]);
        for prefix in ["https://example.invalid", "https://example.invalid/"] {
            existing
                .check(&declaration(prefix, DuckDbRepositoryKey::Discovered))
                .unwrap();
            existing
                .check(&declaration(prefix, DuckDbRepositoryKey::Pinned(KEY_BODY)))
                .unwrap();
        }
        assert_eq!(
            Registered::find(std::slice::from_ref(&existing), "QUERY_FARM").map(|r| &r.name),
            Some(&existing.name)
        );
    }

    /// A throwaway RSA-2048 public key; DuckDB validates explicit keys when registering.
    const THROWAWAY_KEY: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAnWWcITru5BDcUrIeJPNp
VEOLNO9N8HV1Af/ztV4MfdaK6ZhVQWTbfLuxhEnCILgmT2NDswSKF3/LCyEyBC73
xYymq9u+AVuIz6k5uCfP/Z1o6/PJlR1Vs1cm7+2nmcH6O3KXIxHf+92QKTvBhiVv
BVD+vkmQROutmziL3C4OH4FtSoNAKOOvabgnI/d3z67Vf8WCsYaNIa5zlte7FpQZ
SeRbcgje4PMA2FCPfVw5EgHGveVn39JwuDD0uPAcbpS0T4j5ltZLvg9SyuQTP6h3
jibWhkf/nLlf0OuEnSgigUr3zhPNeT9UKszrCJTCDyMkWUtp6z8WByVhbySHfvmu
kwIDAQAB
-----END PUBLIC KEY-----
";

    /// `register` finds a repository missing, then another process registers the same name
    /// before `CREATE ... IF NOT EXISTS` runs. Simulated with a second database that shares
    /// the repository directory, registering just before [`create_and_verify`].
    #[test]
    #[ignore = "needs a DuckDB 2.0 ADBC driver, e.g. on LD_LIBRARY_PATH"]
    fn create_and_verify_detects_a_conflicting_concurrent_registration() {
        use dbt_adbc::{Backend, connection, database, driver};

        let repositories = tempfile::tempdir().unwrap();
        let mut driver =
            driver::Builder::new(Backend::DuckDB, driver::LoadStrategy::SystemThenCdnCache)
                .try_load()
                .unwrap();
        let mut open = || {
            let mut builder = database::Builder::new(Backend::DuckDB);
            builder
                .with_named_option("path", ":memory:")
                .unwrap()
                .with_named_option(
                    "extension_repository_directory",
                    repositories.path().to_str().unwrap(),
                )
                .unwrap()
                .with_named_option("allow_extension_repositories", "allowed")
                .unwrap();
            let mut database = builder.build(&mut driver).unwrap();
            connection::Builder::default().build(&mut database).unwrap()
        };
        let mut ours = open();
        let mut other = open();

        let conflicting = DuckDbRepositoryDeclaration {
            name: "race_repo",
            prefix: "https://other.invalid",
            key: DuckDbRepositoryKey::Pinned(THROWAWAY_KEY),
        };
        execute(other.as_mut(), &create_sql(&conflicting)).unwrap();

        let declared = DuckDbRepositoryDeclaration {
            prefix: "https://ours.invalid",
            ..conflicting
        };
        let err = create_and_verify(ours.as_mut(), &[&declared]).unwrap_err();
        assert!(
            err.message()
                .contains("already registered with prefix 'https://other.invalid'"),
            "{err}"
        );
    }

    #[test]
    fn create_sql_quotes_the_name_and_escapes_literals() {
        let discovered = DuckDbRepositoryDeclaration {
            name: "query-farm",
            prefix: "https://o'brien.invalid/x",
            key: DuckDbRepositoryKey::Discovered,
        };
        assert_eq!(
            create_sql(&discovered),
            "CREATE EXTENSION REPOSITORY IF NOT EXISTS \"query-farm\" WITH PREFIX 'https://o''brien.invalid/x'"
        );
        let pinned = DuckDbRepositoryDeclaration {
            key: DuckDbRepositoryKey::Pinned("key'body"),
            ..discovered
        };
        assert!(create_sql(&pinned).ends_with(" USING PUBLIC KEY 'key''body'"));
    }

    #[test]
    fn check_rejects_changed_prefix_and_keys() {
        let existing = registered("https://example.invalid", &[KEY_BODY, "OTHER"]);
        let err = existing
            .check(&declaration(
                "https://other.invalid",
                DuckDbRepositoryKey::Discovered,
            ))
            .unwrap_err();
        assert!(
            err.message().contains("prefix 'https://example.invalid'"),
            "{err}"
        );
        assert!(
            err.message()
                .contains("DROP EXTENSION REPOSITORY \"query_farm\"")
        );

        // The stored trust must be exactly the pinned key, not merely include it.
        for pinned in [KEY_BODY, "UNRELATED"] {
            let err = existing
                .check(&declaration(
                    "https://example.invalid",
                    DuckDbRepositoryKey::Pinned(pinned),
                ))
                .unwrap_err();
            assert!(err.message().contains("sha256:abcd"), "{err}");
            assert!(!err.message().contains(KEY_BODY), "{err}");
        }
    }
}
