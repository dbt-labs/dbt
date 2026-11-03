//! Per-adapter identity a node resolves against during the parse phase.

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use dbt_adapter_core::AdapterType;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_jinja_ctx::{TargetContextMap, to_jinja_btreemap};
use dbt_schemas::{schemas::profiles::DbConfig, state::DbtProfile};
use minijinja::{constants::DIALECT, value::Value as MinijinjaValue};
use serde::Serialize;

use crate::phases::utils::{TargetGlobals, build_target_globals};

/// What a node of one adapter type starts from at parse.
#[derive(Debug, Clone)]
pub struct ParseAdapterScope {
    /// The adapter type of the nodes in this scope.
    pub adapter_type: AdapterType,
    /// The database a node starts from, before `generate_database_name`.
    pub database: String,
    /// The schema a node starts from, before `generate_schema_name`.
    pub schema: String,
    /// Context entries that shadow the env globals of the same name. Empty for the
    /// default adapter.
    overlay: BTreeMap<String, MinijinjaValue>,
}

impl ParseAdapterScope {
    /// The overlay's dialect: `None` for the default, undeclared and LakeCompute adapters.
    pub fn dialect(&self) -> Option<&MinijinjaValue> {
        self.overlay.get(DIALECT)
    }
}

/// The env globals a declared non-default adapter replaces, in `ResolveCore`'s shape.
#[derive(Serialize)]
struct ScopeOverlay {
    target: TargetContextMap,
    env: TargetContextMap,
    database: Option<String>,
    schema: Option<String>,
}

/// One [`ParseAdapterScope`] per adapter type the active target declares.
#[derive(Debug, Clone)]
pub struct ParseAdapterScopes {
    default: ParseAdapterScope,
    /// Declared non-default adapters. An `Err` holds why that connection cannot
    /// form a target context.
    by_type: HashMap<AdapterType, Result<ParseAdapterScope, String>>,
}

impl ParseAdapterScopes {
    /// The scopes of every adapter `profile`'s active target declares.
    pub fn from_profile(profile: &DbtProfile) -> Self {
        let default = ParseAdapterScope {
            adapter_type: profile.default_adapter,
            database: profile.database.clone(),
            schema: profile.schema.clone(),
            overlay: BTreeMap::new(),
        };
        let default_threads = default_target_threads(profile);
        let by_type = profile
            .adapters
            .keys()
            .copied()
            .filter(|adapter_type| *adapter_type != profile.default_adapter)
            .filter_map(|adapter_type| {
                let config = profile.adapter(adapter_type)?;
                Some((
                    adapter_type,
                    declared_scope(profile, adapter_type, config, default_threads.as_ref()),
                ))
            })
            .collect();
        Self { default, by_type }
    }

    /// The scope of `adapter_type`. An undeclared type gets the default's scope.
    pub fn get(&self, adapter_type: AdapterType) -> FsResult<&ParseAdapterScope> {
        Ok(self.declared(adapter_type)?.unwrap_or(&self.default))
    }

    /// The scope of a declared non-default adapter; `None` for the default and undeclared ones.
    pub fn declared(&self, adapter_type: AdapterType) -> FsResult<Option<&ParseAdapterScope>> {
        match self.by_type.get(&adapter_type) {
            None => Ok(None),
            Some(Ok(scope)) => Ok(Some(scope)),
            Some(Err(e)) => Err(fs_err!(
                ErrorCode::InvalidConfig,
                "the '{adapter_type}' connection cannot be used to resolve nodes: {e}"
            )),
        }
    }

    /// `base` with the overlay of `adapter_type` applied. Borrows `base` when there
    /// is nothing to overlay.
    pub fn ctx_for<'a>(
        &self,
        adapter_type: AdapterType,
        base: &'a BTreeMap<String, MinijinjaValue>,
    ) -> FsResult<Cow<'a, BTreeMap<String, MinijinjaValue>>> {
        let Some(scope) = self.declared(adapter_type)? else {
            return Ok(Cow::Borrowed(base));
        };
        let mut ctx = base.clone();
        ctx.extend(scope.overlay.iter().map(|(k, v)| (k.clone(), v.clone())));
        Ok(Cow::Owned(ctx))
    }
}

/// `target.threads` of the default adapter's env global, `None` if it cannot be built.
fn default_target_threads(profile: &DbtProfile) -> Option<MinijinjaValue> {
    let config = profile.default_db_config().clone();
    build_target_globals(&profile.profile, &profile.target, config)
        .ok()?
        .target
        .remove("threads")
}

/// Built the same way `initialize_parse_jinja_environment` builds the env globals.
fn declared_scope(
    profile: &DbtProfile,
    adapter_type: AdapterType,
    config: &DbConfig,
    default_threads: Option<&MinijinjaValue>,
) -> Result<ParseAdapterScope, String> {
    let (database, schema) = config.profile_database_and_schema();
    let TargetGlobals {
        mut target,
        database: overlay_database,
        schema: overlay_schema,
    } = build_target_globals(&profile.profile, &profile.target, config.clone())?;
    // The thread count is target-wide, so every adapter reports the default's.
    if let Some(threads) = default_threads {
        target.insert("threads".to_string(), threads.clone());
    }
    let target = Arc::new(target);
    let mut overlay = to_jinja_btreemap(&ScopeOverlay {
        target: target.clone(),
        env: target,
        database: overlay_database,
        schema: overlay_schema,
    });
    // A LakeCompute node keeps the default's dialect, so its naming macros stay the default's.
    if adapter_type != AdapterType::LakeCompute {
        overlay.insert(
            DIALECT.to_string(),
            MinijinjaValue::from(adapter_type.as_ref()),
        );
    }
    Ok(ParseAdapterScope {
        adapter_type,
        database,
        schema,
        overlay,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use dbt_common::io_args::IoArgs;
    use dbt_schemas::schemas::{
        profiles::{LakeComputeConfig, SnowflakeDbConfig},
        relations::DEFAULT_DBT_QUOTING,
        serde::StringOrInteger,
    };
    use dbt_schemas::state::ProfileAdapter;
    use indexmap::IndexMap;

    use super::*;
    use crate::{
        invocation_args::InvocationArgs, jinja_environment::JinjaEnv, phases::parse::init,
    };

    fn snowflake() -> DbConfig {
        SnowflakeDbConfig {
            account: Some("acct".to_string()),
            database: Some("ANALYTICS".to_string()),
            schema: Some("DBT_X".to_string()),
            ..Default::default()
        }
        .into()
    }

    fn bigquery(dataset: Option<&str>) -> DbConfig {
        let dataset = dataset
            .map(|d| format!("dataset: {d}\n"))
            .unwrap_or_default();
        dbt_yaml::from_str(&format!(
            "type: bigquery\nmethod: oauth\nproject: bq-proj\n{dataset}"
        ))
        .unwrap()
    }

    fn profile(configs: Vec<DbConfig>) -> DbtProfile {
        let default_adapter = configs[0].adapter_type();
        let (database, schema) = configs[0].profile_database_and_schema();
        DbtProfile {
            profile: "p".to_string(),
            target: "dev".to_string(),
            defer_to_target: None,
            allow_clones: true,
            adapters: configs
                .into_iter()
                .map(|c| (c.adapter_type(), ProfileAdapter::single(c)))
                .collect::<IndexMap<_, _>>(),
            default_adapter,
            schema,
            database,
            relative_profile_path: "profiles.yml".into(),
            threads: None,
        }
    }

    fn target_field(scope: &ParseAdapterScope, key: &str) -> MinijinjaValue {
        scope.overlay["target"].get_attr(key).unwrap()
    }

    /// The parse env a single-adapter project with `config` gets.
    fn parse_env(config: DbConfig) -> JinjaEnv {
        let adapter_type = config.adapter_type();
        init::initialize_parse_jinja_environment(
            "root",
            "p",
            "dev",
            adapter_type,
            config,
            vec![adapter_type],
            DEFAULT_DBT_QUOTING,
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            chrono::Utc::now().with_timezone(&chrono_tz::Tz::UTC),
            &InvocationArgs::default(),
            BTreeSet::from(["root".to_string()]),
            IoArgs::default(),
            None,
        )
        .unwrap()
    }

    #[test]
    fn default_and_undeclared_ctx_borrow_base() {
        let scopes =
            ParseAdapterScopes::from_profile(&profile(vec![snowflake(), bigquery(Some("bq_ds"))]));
        let base = BTreeMap::from([("k".to_string(), MinijinjaValue::from(1))]);
        for adapter_type in [AdapterType::Snowflake, AdapterType::Postgres] {
            let ctx = scopes.ctx_for(adapter_type, &base).unwrap();
            assert!(
                matches!(ctx, Cow::Borrowed(b) if std::ptr::eq(b, &base)),
                "{adapter_type}"
            );
        }
    }

    #[test]
    fn default_scope_matches_the_profile_database_and_schema() {
        let profile = profile(vec![snowflake(), bigquery(Some("bq_ds"))]);
        let scopes = ParseAdapterScopes::from_profile(&profile);
        let scope = scopes.get(AdapterType::Snowflake).unwrap();
        assert_eq!(
            (scope.database.as_str(), scope.schema.as_str()),
            (profile.database.as_str(), profile.schema.as_str())
        );
        assert_eq!(
            (scope.database.as_str(), scope.schema.as_str()),
            ("ANALYTICS", "DBT_X")
        );
        assert!(scope.overlay.is_empty());
    }

    #[test]
    fn default_scope_reads_the_profile_fields() {
        let mut profile = profile(vec![snowflake()]);
        profile.database = "db".to_string();
        profile.schema = "dbt_test".to_string();
        let scopes = ParseAdapterScopes::from_profile(&profile);
        for adapter_type in [AdapterType::Snowflake, AdapterType::Postgres] {
            let scope = scopes.get(adapter_type).unwrap();
            assert_eq!(
                (scope.database.as_str(), scope.schema.as_str()),
                ("db", "dbt_test"),
                "{adapter_type}"
            );
        }
    }

    /// The overlay `target` has the value the parse env registers as its global.
    #[test]
    fn declared_target_matches_the_parse_env_global() {
        let env = parse_env(bigquery(Some("bq_ds")));
        let scopes =
            ParseAdapterScopes::from_profile(&profile(vec![snowflake(), bigquery(Some("bq_ds"))]));
        let scope = scopes.get(AdapterType::Bigquery).unwrap();
        for key in ["target", "env", "database", "schema", "dialect"] {
            assert_eq!(scope.overlay[key], env.get_global(key).unwrap(), "{key}");
        }
    }

    #[test]
    fn declared_target_threads_follow_the_default_global() {
        let mut sf = snowflake();
        sf.set_threads(Some(StringOrInteger::Integer(4)));
        let global = parse_env(sf.clone())
            .get_global("target")
            .unwrap()
            .get_attr("threads")
            .unwrap();
        assert_eq!(global, MinijinjaValue::from(4));

        let scopes =
            ParseAdapterScopes::from_profile(&profile(vec![sf.clone(), bigquery(Some("bq_ds"))]));
        let scope = scopes.get(AdapterType::Bigquery).unwrap();
        for key in ["target", "env"] {
            assert_eq!(
                scope.overlay[key].get_attr("threads").unwrap(),
                global,
                "{key}"
            );
        }

        let scopes = ParseAdapterScopes::from_profile(&profile(vec![bigquery(Some("bq_ds")), sf]));
        let scope = scopes.get(AdapterType::Snowflake).unwrap();
        for key in ["target", "env"] {
            let threads = scope.overlay[key].get_attr("threads").unwrap();
            assert!(threads.is_none(), "{key}: {threads}");
        }
    }

    #[test]
    fn bigquery_scope_on_a_snowflake_default() {
        let scopes =
            ParseAdapterScopes::from_profile(&profile(vec![snowflake(), bigquery(Some("bq_ds"))]));
        let scope = scopes.get(AdapterType::Bigquery).unwrap();
        assert_eq!(scope.adapter_type, AdapterType::Bigquery);
        assert_eq!(
            (scope.database.as_str(), scope.schema.as_str()),
            ("bq-proj", "bq_ds")
        );
        assert_eq!(target_field(scope, "type").as_str(), Some("bigquery"));
        assert_eq!(target_field(scope, "name").as_str(), Some("dev"));

        let base = BTreeMap::new();
        let ctx = scopes.ctx_for(AdapterType::Bigquery, &base).unwrap();
        assert_eq!(ctx["database"].as_str(), Some("bq-proj"));
    }

    #[test]
    fn a_misconfigured_secondary_fails_only_for_itself() {
        let scopes = ParseAdapterScopes::from_profile(&profile(vec![snowflake(), bigquery(None)]));
        let err = scopes.get(AdapterType::Bigquery).unwrap_err();
        assert!(err.to_string().contains("'bigquery'"), "{err}");
        assert!(
            scopes
                .ctx_for(AdapterType::Bigquery, &BTreeMap::new())
                .is_err()
        );
        assert!(scopes.get(AdapterType::Snowflake).is_ok());
    }

    #[test]
    fn declared_is_only_for_declared_non_default_adapters() {
        let scopes =
            ParseAdapterScopes::from_profile(&profile(vec![snowflake(), bigquery(Some("bq_ds"))]));
        for adapter_type in [AdapterType::Postgres, AdapterType::Snowflake] {
            assert!(
                scopes.declared(adapter_type).unwrap().is_none(),
                "{adapter_type}"
            );
        }
        let scope = scopes.declared(AdapterType::Bigquery).unwrap().unwrap();
        assert_eq!(scope.adapter_type, AdapterType::Bigquery);

        let scopes = ParseAdapterScopes::from_profile(&profile(vec![snowflake(), bigquery(None)]));
        let err = scopes.declared(AdapterType::Bigquery).unwrap_err();
        assert!(err.to_string().contains("'bigquery'"), "{err}");
    }

    #[test]
    fn an_undeclared_adapter_gets_the_default_fields() {
        let scopes = ParseAdapterScopes::from_profile(&profile(vec![snowflake()]));
        let scope = scopes.get(AdapterType::Postgres).unwrap();
        assert_eq!(scope.adapter_type, AdapterType::Snowflake);
        assert_eq!(
            (scope.database.as_str(), scope.schema.as_str()),
            ("ANALYTICS", "DBT_X")
        );
        assert!(scope.overlay.is_empty());
    }

    #[test]
    fn a_lake_compute_secondary_keeps_the_default_dialect() {
        // `database`/`schema` as `load_profiles` inherits them from the default connection.
        let lake_compute = DbConfig::LakeCompute(Box::new(LakeComputeConfig {
            path: Some("lake.duckdb".to_string()),
            database: Some("ANALYTICS".to_string()),
            schema: Some("DBT_X".to_string()),
            ..Default::default()
        }));
        let scopes = ParseAdapterScopes::from_profile(&profile(vec![snowflake(), lake_compute]));
        let scope = scopes.get(AdapterType::LakeCompute).unwrap();
        assert_eq!(
            (scope.database.as_str(), scope.schema.as_str()),
            ("ANALYTICS", "DBT_X")
        );
        assert_eq!(target_field(scope, "database").as_str(), Some("ANALYTICS"));
        assert_eq!(target_field(scope, "schema").as_str(), Some("DBT_X"));
        assert_eq!(scope.overlay["database"].as_str(), Some("ANALYTICS"));
        // No dialect: its naming macros resolve through the default's family.
        assert!(!scope.overlay.contains_key(DIALECT));
    }
}
