use crate::args::ResolveArgs;
use crate::dbt_project_config::ProjectConfigResolver;
use crate::dbt_project_config::RootProjectConfigs;
use crate::dbt_project_config::disallow_plus_prefix_from_flags;
use crate::dbt_project_config::init_project_config;
use crate::resolve::resolve_properties::MinimalPropertiesEntry;
use crate::resolve::resolve_utils::validate_unit_test_compute;
use crate::utils::get_node_fqn;
use crate::utils::get_unique_id;
use crate::validation::check_node_static_analysis;
use dbt_adapter_core::AdapterType;
use dbt_common::CodeLocationWithFile;
use dbt_common::ErrorCode;
use dbt_common::FsResult;
use dbt_common::err;
use dbt_common::error::AbstractLocation;
use dbt_common::fs_err;
use dbt_common::io_args::ComputeArg;
use dbt_common::io_args::StaticAnalysisKind;
use dbt_common::io_args::StaticAnalysisOffReason;

use dbt_common::path::DbtPath;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::phases::parse::build_resolve_model_context;
use dbt_jinja_utils::phases::parse::sql_resource::SqlResource;
use dbt_jinja_utils::serde::into_typed_with_jinja;
use dbt_jinja_utils::utils::dependency_package_name_from_ctx;
use dbt_jinja_utils::utils::render_extract_ref_or_source_expr;
use dbt_schemas::schemas::DbtModel;
use dbt_schemas::schemas::DbtUnitTestAttr;
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use dbt_schemas::schemas::common::DbtChecksum;
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::common::DbtQuoting;
use dbt_schemas::schemas::common::Expect;
use dbt_schemas::schemas::common::Formats;
use dbt_schemas::schemas::common::Given;
use dbt_schemas::schemas::common::NodeDependsOn;
use dbt_schemas::schemas::common::Rows;
use dbt_schemas::schemas::packages::DeprecatedDbtPackageLock;
use dbt_schemas::schemas::project::DbtProject;
use dbt_schemas::schemas::project::ResolvableConfig;
use dbt_schemas::schemas::project::ResolvedConfig;
use dbt_schemas::schemas::project::UnitTestConfig;
use dbt_schemas::schemas::properties::UnitTestProperties;
use dbt_schemas::schemas::ref_and_source::DbtRef;
use dbt_schemas::schemas::ref_and_source::DbtSourceWrapper;
use dbt_schemas::schemas::{CommonAttributes, DbtUnitTest, NodeBaseAttributes};
use dbt_schemas::state::DbtPackage;
use dbt_schemas::state::DbtRuntimeConfig;
use dbt_schemas::state::NodeResolverTracker;
use dbt_schemas::state::ResourcePathKind;
use dbt_yaml::Spanned;
use dbt_yaml::Value as YmlValue;
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn resolve_unit_tests(
    arg: &ResolveArgs,
    unit_test_properties: BTreeMap<String, MinimalPropertiesEntry>,
    package: &DbtPackage,
    root_package: &DbtPackage,
    package_quoting: DbtQuoting,
    root_project_configs: &RootProjectConfigs,
    package_name: &str,
    jinja_env: &JinjaEnv,
    base_ctx: &BTreeMap<String, minijinja::Value>,
    model_properties: &BTreeMap<String, MinimalPropertiesEntry>,
    models: &BTreeMap<String, Arc<DbtModel>>,
    packages: &[DbtPackage],
    adapter_type: AdapterType,
    node_resolver: &dyn NodeResolverTracker,
) -> FsResult<(
    BTreeMap<String, Arc<DbtUnitTest>>,
    BTreeMap<String, Arc<DbtUnitTest>>,
)> {
    let mut unit_tests: BTreeMap<String, Arc<DbtUnitTest>> = BTreeMap::new();
    let mut disabled_unit_tests: BTreeMap<String, Arc<DbtUnitTest>> = BTreeMap::new();
    let dependency_package_name = dependency_package_name_from_ctx(jinja_env, base_ctx);
    let config_resolver = ProjectConfigResolver::build(
        root_project_configs.unit_tests.clone(),
        dependency_package_name.is_some(),
        || {
            init_project_config(
                &package.dbt_project.unit_tests,
                (),
                dependency_package_name,
                disallow_plus_prefix_from_flags(root_package.dbt_project.flags.as_ref()),
                adapter_type,
            )
        },
        adapter_type,
    )?
    .with_resolve_defaults(arg.static_analysis.unwrap_or_default());

    for (unit_test_name, mpe) in unit_test_properties.into_iter() {
        // Capture YAML span of the unit-test `name:` declaration for error reporting.
        // The analyzer uses this as `start_location` so messages anchor at the unit
        // test's entry in the source YAML rather than at line 1 col 1.
        let defined_at = mpe.name_span.is_valid().then(|| {
            CodeLocationWithFile::new(
                mpe.name_span.start.line as u32,
                mpe.name_span.start.column as u32,
                mpe.name_span.start.index as u32,
                mpe.relative_path.clone(),
            )
        });

        let unit_test = into_typed_with_jinja::<UnitTestProperties, _>(
            mpe.schema_value,
            false,
            jinja_env,
            base_ctx,
            &[],
            dependency_package_name,
            true,
        )?;
        // todo: Unit test should have a database and schema,
        //    derived from the underlying model, correct?
        // - if so, we should get it and still store it so that it is available,
        // - but we should not serialize it
        // - for now just use the global ones

        let location = if unit_test.model.has_valid_span() {
            {
                CodeLocationWithFile::new(
                    unit_test.model.span().start.line as u32,
                    unit_test.model.span().start.column as u32,
                    unit_test.model.span().start.index as u32,
                    mpe.relative_path.clone(),
                )
            }
        } else {
            Default::default()
        };

        let model_name = format!("model.{}.{}", package_name, unit_test.model);
        // `tested_node_unique_id` is the unversioned model's unique id when resolvable.
        // Versioned cases below override it with the version-specific id.
        // A unit test runs against one model, so it takes that model's adapter --
        // it has no `+adapter` of its own. `lake_compute` is not inherited: unit tests never
        // take the compute-platform path (see `tasks_for_node`), so an `lake_compute` dialect
        // here would only mis-resolve the `unit` materialization.
        let (database, schema, _, tested_node_unique_id, model_adapter) = match models
            .get(&model_name)
        {
            Some(model) => (
                model.__base_attr__.database.clone(),
                model.__base_attr__.schema.clone(),
                model.__base_attr__.alias.clone(),
                Some(model_name.clone()),
                Some(model.node_adapter()).filter(|adapter| *adapter != AdapterType::LakeCompute),
            ),
            None => (
                String::new(),
                String::new(),
                unit_test.model.to_string(),
                None,
                None,
            ),
        };

        // Create base unit test node
        let base_unique_id = format!(
            "unit_test.{}.{}.{}",
            package_name, unit_test.model, unit_test_name
        );

        let fqn = get_node_fqn(
            package_name,
            mpe.relative_path.to_owned(),
            vec![unit_test.model.to_string(), unit_test_name.to_owned()],
            &package.dbt_project.all_source_paths(),
        );

        let mut properties_config =
            config_resolver.resolve_with_properties(&fqn, unit_test.config.as_ref());
        check_node_static_analysis(
            &properties_config,
            arg.static_analysis,
            &base_unique_id,
            dependency_package_name,
        );
        validate_unit_test_compute(properties_config.compute, &mpe.relative_path)?;
        // Sidecar needs a bound LP. Upgrade baseline to strict like the CLI
        // does for non-remote --compute. Can disable via explicit off.
        if properties_config.compute == Some(ComputeArg::Sidecar)
            && *properties_config.static_analysis != StaticAnalysisKind::Off
        {
            properties_config.static_analysis = StaticAnalysisKind::Strict.into();
        }

        let enabled = properties_config.enabled;

        // todo: generalize given input format, according to https://docs.getdbt.com/docs/build/unit-tests

        let dependent_refs = vec![DbtRef {
            name: unit_test.model.to_string(),
            package: Some(package_name.to_owned()),
            version: None,
            location: Some(location.clone()),
        }];

        // Process unit test given inputs to extract ref nodes
        for given_group in unit_test.given.iter() {
            for g in given_group.iter() {
                let input = &g.input;
                if input.contains("ref") || input.contains("source") || input.as_str().eq("this") {
                    continue;
                } else {
                    return err!(
                        ErrorCode::Unexpected,
                        "Invalid given input: {}",
                        input.as_str()
                    );
                }
            }
        }

        let mut file_map: BTreeMap<String, String> = BTreeMap::new();

        for asset in package.fixture_files.iter() {
            asset.path.file_name().map(|file_name| {
                file_map.insert(
                    file_name.to_string_lossy().to_string(),
                    asset.path.to_string_lossy().to_string(),
                )
            });
        }

        let mut given = Vec::new();
        if let Some(given_entries) = unit_test.given.as_ref() {
            for given_entry in given_entries {
                let full_path: Option<String> = match given_entry.fixture {
                    Some(ref fixture) if given_entry.format == Formats::Csv => {
                        file_map.get(&(fixture.clone() + ".csv")).cloned()
                    }
                    Some(ref fixture) if given_entry.format == Formats::Sql => {
                        file_map.get(&(fixture.clone() + ".sql")).cloned()
                    }
                    _ => given_entry.fixture.clone(),
                };

                let rows = if given_entry.format == Formats::Dict
                    && given_entry.rows.is_none()
                    && given_entry.fixture.is_none()
                {
                    let seed_unique_id = resolve_given_seed(
                        jinja_env,
                        base_ctx,
                        &given_entry.input,
                        adapter_type,
                        &database,
                        &schema,
                        &unit_test.model,
                        &fqn,
                        package_name,
                        &root_package.dbt_project.name,
                        package_quoting,
                        &mpe.relative_path,
                        arg.static_analysis,
                        node_resolver,
                    )?;
                    Some(Rows::List(load_implicit_seed_rows(
                        packages,
                        &seed_unique_id,
                        &given_entry.input,
                    )?))
                } else {
                    given_entry.rows.clone()
                };

                given.push(Given {
                    fixture: full_path,
                    input: given_entry.input.clone(),
                    rows,
                    format: given_entry.format.clone(),
                });
            }
        }

        let expect = {
            let full_path: Option<String> = match unit_test.expect.fixture {
                Some(ref fixture) if unit_test.expect.format == Formats::Csv => {
                    file_map.get(&(fixture.clone() + ".csv")).cloned()
                }
                Some(ref fixture) if unit_test.expect.format == Formats::Sql => {
                    file_map.get(&(fixture.clone() + ".sql")).cloned()
                }
                _ => unit_test.expect.fixture.clone(),
            };

            Expect {
                fixture: full_path,
                rows: unit_test.expect.rows.clone(),
                format: unit_test.expect.format.clone(),
            }
        };

        let static_analysis = properties_config.static_analysis.clone();

        let base_unit_test = DbtUnitTest {
            __common_attr__: CommonAttributes {
                name: unit_test_name.to_owned(),
                package_name: package_name.to_owned(),
                original_file_path: DbtPath::from(&mpe.relative_path),
                path: DbtPath::from(&mpe.relative_path),
                name_span: dbt_common::Span::default(),
                unique_id: base_unique_id.clone(),
                fqn,
                // dbt-core: description is always default ''
                description: Some(unit_test.description.to_owned().unwrap_or_default()),
                patch_path: None,
                checksum: DbtChecksum::default(),
                raw_code: None,
                language: Some("sql".to_string()),
                tags: properties_config
                    .tags
                    .inner()
                    .clone()
                    .map(Into::into)
                    .unwrap_or_default(),
                classifiers: Default::default(),
                meta: properties_config.meta.clone().unwrap_or_default(),
            },
            __base_attr__: NodeBaseAttributes {
                adapter: model_adapter.unwrap_or(adapter_type),
                // This node type has no `+propagate` config; nothing is published.
                propagate: Vec::new(),
                effective_propagation_target: None,
                database: database.to_owned(),
                schema: schema.to_owned(),
                // match dbt-core semantics for unit test alias
                alias: unit_test_name.to_owned(),
                relation_name: None,
                depends_on: NodeDependsOn::default(),
                refs: dependent_refs,
                sources: vec![],
                functions: vec![],
                enabled,
                extended_model: false,
                persist_docs: None,
                quoting: package_quoting.try_into()?,
                quoting_ignore_case: package_quoting.snowflake_ignore_case.unwrap_or(false),
                materialized: DbtMaterialization::Unit,
                static_analysis_off_reason: (*static_analysis == StaticAnalysisKind::Off)
                    .then_some(StaticAnalysisOffReason::ConfiguredOff),
                static_analysis,
                compute: properties_config.compute,
                columns: vec![],
                metrics: vec![],
                // In future: populate unrendered_config for unit tests -- after dbt-core starts
                // comparing unit-test config.
                // For now, it is intentionally left empty to match dbt-core's current behavior:
                // unit_tests.py never populates it, unit tests are compared Structurally/by
                // checksum, and check_configs_modified treats NodeType::UnitTest as a non-trigger.
                unrendered_config: Default::default(),
            },
            __unit_test_attr__: DbtUnitTestAttr {
                model: unit_test.model.to_string(),
                given,
                expect,
                versions: None,
                version: None,
                overrides: unit_test.overrides.clone(),
            },
            tested_node_unique_id: tested_node_unique_id.clone(),
            defined_at,
            deprecated_config: properties_config.into(),
            ..Default::default()
        };
        // Check if this model has versions
        if let Some(version_info) = model_properties
            .get(unit_test.model.as_ref())
            .and_then(|mpe| mpe.version_info.as_ref())
        {
            // Parse version configuration to get the include and exclude lists
            // this include and exclude accepted values are different than for generic tests
            // no 'all' or '*' accepted
            let version_config = unit_test.versions.as_ref().and_then(|v| match v {
                dbt_yaml::Value::Mapping(map, _) => {
                    let include_key = dbt_yaml::Value::string("include".to_string());
                    let exclude_key = dbt_yaml::Value::string("exclude".to_string());
                    Some((
                        map.get(&include_key).and_then(parse_version_numbers_yml),
                        map.get(&exclude_key).and_then(parse_version_numbers_yml),
                    ))
                }
                _ => None,
            });

            // In the main code:
            let versions = version_info
                .all_versions
                .keys()
                .filter(|version| {
                    version_config
                        .as_ref()
                        .map(|(include, exclude)| {
                            should_include_version_for_unit_test(include, exclude, version)
                        })
                        .unwrap_or(true) // No version config means include all versions
                })
                .collect::<Vec<&String>>(); // Explicitly collect into Vec<&String>

            if !enabled {
                disabled_unit_tests.insert(base_unique_id, Arc::new(base_unit_test));
                continue;
            }

            // Create a unit test node for each version
            for version in versions {
                let versioned_model_unique_id = get_unique_id(
                    &unit_test.model,
                    package_name,
                    Some(version.clone()),
                    "model",
                );

                // Look up database/schema from the versioned model
                let (ver_database, ver_schema, ver_adapter) = models
                    .get(&versioned_model_unique_id)
                    .map(|m| {
                        (
                            m.__base_attr__.database.clone(),
                            m.__base_attr__.schema.clone(),
                            Some(m.node_adapter())
                                .filter(|adapter| *adapter != AdapterType::LakeCompute),
                        )
                    })
                    .unwrap_or_default();

                let mut versioned_test = base_unit_test.clone();
                versioned_test.__common_attr__.unique_id = format!("{base_unique_id}.v{version}");
                versioned_test.__unit_test_attr__.version = Some(version.clone().into());
                versioned_test.__base_attr__.database = ver_database;
                versioned_test.__base_attr__.schema = ver_schema;
                versioned_test.__base_attr__.depends_on.nodes =
                    vec![versioned_model_unique_id.clone()];
                versioned_test
                    .__base_attr__
                    .depends_on
                    .nodes_with_ref_location =
                    vec![(versioned_model_unique_id.clone(), location.clone())];
                versioned_test.tested_node_unique_id = Some(versioned_model_unique_id.clone());
                versioned_test.__base_attr__.adapter = ver_adapter.unwrap_or(adapter_type);

                unit_tests.insert(
                    versioned_test.__common_attr__.unique_id.clone(),
                    Arc::new(versioned_test),
                );
            }
        } else {
            // Non-versioned case
            if tested_node_unique_id.is_none() || !enabled {
                disabled_unit_tests.insert(base_unique_id, Arc::new(base_unit_test));
            } else {
                unit_tests.insert(base_unique_id, Arc::new(base_unit_test));
            }
        }
    }
    Ok((unit_tests, disabled_unit_tests))
}

#[allow(clippy::too_many_arguments)]
fn resolve_given_seed(
    jinja_env: &JinjaEnv,
    base_ctx: &BTreeMap<String, minijinja::Value>,
    input: &Spanned<String>,
    adapter_type: AdapterType,
    database: &str,
    schema: &str,
    model_name: &str,
    fqn: &[String],
    package_name: &str,
    root_project_name: &str,
    package_quoting: DbtQuoting,
    relative_path: &std::path::Path,
    global_static_analysis: Option<StaticAnalysisKind>,
    node_resolver: &dyn NodeResolverTracker,
) -> FsResult<String> {
    let sql_resources: Arc<Mutex<Vec<SqlResource<UnitTestConfig>>>> =
        Arc::new(Mutex::new(Vec::new()));
    let mut resolve_model_context = base_ctx.clone();
    resolve_model_context.extend(build_resolve_model_context(
        &UnitTestConfig::default(),
        false,
        adapter_type,
        database,
        schema,
        model_name,
        fqn.to_vec(),
        package_name,
        root_project_name,
        package_quoting,
        Arc::new(DbtRuntimeConfig::default()),
        Arc::new(DbtRuntimeConfig::default()),
        sql_resources.clone(),
        Arc::new(AtomicBool::new(false)),
        relative_path,
        &PathBuf::new(),
        global_static_analysis,
    ));

    match render_extract_ref_or_source_expr(
        jinja_env,
        &resolve_model_context,
        sql_resources,
        input,
    )? {
        SqlResource::Ref((name, package, version, _)) => {
            let (unique_id, _, _, _) = node_resolver.lookup_ref(
                &package,
                &name,
                &version.as_ref().map(ToString::to_string),
                &Some(package_name.to_string()),
            )?;
            if unique_id.starts_with("seed.") {
                Ok(unique_id)
            } else {
                err!(
                    ErrorCode::InvalidConfig,
                    "Unit test input '{}' must reference a seed when rows and fixture are omitted",
                    input.as_str()
                )
            }
        }
        _ => err!(
            ErrorCode::InvalidConfig,
            "Unit test input '{}' must reference a seed when rows and fixture are omitted",
            input.as_str()
        ),
    }
}

fn load_implicit_seed_rows(
    packages: &[DbtPackage],
    seed_unique_id: &str,
    input: &Spanned<String>,
) -> FsResult<Vec<BTreeMap<String, YmlValue>>> {
    let (package, seed_asset) = packages
        .iter()
        .find_map(|package| {
            package.seed_files.iter().find_map(|asset| {
                let seed_name = asset.path.file_stem()?.to_str()?;
                (asset
                    .path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("csv"))
                    && seed_unique_id == format!("seed.{}.{}", package.dbt_project.name, seed_name))
                .then_some((package, asset))
            })
        })
        .ok_or_else(|| {
            fs_err!(
                code => ErrorCode::InvalidConfig,
                loc => input.span().clone(),
                "Unable to find seed '{}' for unit tests",
                seed_unique_id
            )
        })?;

    let data = match package
        .embedded_file_contents
        .as_ref()
        .and_then(|contents| contents.get(&DbtPath::from(&seed_asset.path)))
    {
        Some(content) => content.as_bytes().to_vec(),
        None => {
            let path = seed_asset.base_path.join(&seed_asset.path);
            std::fs::read(&path).map_err(|error| {
                fs_err!(
                    code => ErrorCode::IoError,
                    loc => path,
                    "Failed to read implicit unit test seed: {}",
                    error
                )
            })?
        }
    };

    parse_implicit_seed_rows(&data).map_err(|error| {
        fs_err!(
            code => ErrorCode::InvalidConfig,
            loc => input.span().clone(),
            "Failed to read seed '{}' for unit tests: {}",
            seed_unique_id,
            error
        )
    })
}

fn parse_implicit_seed_rows(data: &[u8]) -> FsResult<Vec<BTreeMap<String, YmlValue>>> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(data);
    let headers = reader
        .headers()
        .map_err(|error| {
            fs_err!(
                ErrorCode::InvalidConfig,
                "Failed to read headers: {}",
                error
            )
        })?
        .clone();
    let mut rows = Vec::new();

    for result in reader.records() {
        let record = result.map_err(|error| {
            fs_err!(ErrorCode::InvalidConfig, "Failed to read record: {}", error)
        })?;
        if record.len() > headers.len() {
            return err!(
                ErrorCode::InvalidConfig,
                "CSV row has {} fields, but the header has {}",
                record.len(),
                headers.len()
            );
        }

        rows.push(
            headers
                .iter()
                .enumerate()
                .map(|(index, header)| {
                    let value = match record.get(index) {
                        Some(value) => YmlValue::string(value.to_string()),
                        None => YmlValue::null(),
                    };
                    (header.to_string(), value)
                })
                .collect(),
        );
    }

    Ok(rows)
}

fn parse_version_numbers_yml(value: &dbt_yaml::Value) -> Option<Vec<String>> {
    match value {
        dbt_yaml::Value::Sequence(arr, _) => Some(
            arr.iter()
                .filter_map(|v| match v {
                    dbt_yaml::Value::Number(n, _) => Some(n.to_string()),
                    dbt_yaml::Value::String(s, _) => s.parse::<i64>().ok().map(|n| n.to_string()),
                    _ => None,
                })
                .collect(),
        ),
        dbt_yaml::Value::String(s, _) => s.parse::<i64>().ok().map(|n| vec![n.to_string()]),
        _ => None,
    }
}

fn should_include_version_for_unit_test(
    include: &Option<Vec<String>>,
    exclude: &Option<Vec<String>>,
    version: &str,
) -> bool {
    // If there's an include list, version must be in it
    let meets_include = include
        .as_ref()
        .map(|inc| inc.contains(&version.to_string()))
        .unwrap_or(true); // No include list means include all

    // If there's an exclude list, version must not be in it
    let meets_exclude = !exclude
        .as_ref()
        .map(|exc| exc.contains(&version.to_string()))
        .unwrap_or(false); // No exclude list means exclude none

    meets_include && meets_exclude
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_implicit_seed_rows_preserves_strings_and_trailing_nulls() {
        let rows = parse_implicit_seed_rows(b"id,name,note\n1,alpha,\n2,beta\n")
            .expect("implicit seed rows should use DictReader-compatible parsing");

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get("id"), Some(&YmlValue::string("1".into())));
        assert_eq!(rows[0].get("name"), Some(&YmlValue::string("alpha".into())));
        assert_eq!(rows[0].get("note"), Some(&YmlValue::string("".into())));
        assert_eq!(rows[1].get("note"), Some(&YmlValue::null()));
    }
}
