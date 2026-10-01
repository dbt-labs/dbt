use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use dbt_common::constants::DBT_PROJECT_YML;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_common::{ErrorCode, FsResult};
use dbt_jinja_utils::serde::value_from_file;
use dbt_schemas::schemas::{InternalDbtNode, Nodes};
use dbt_yaml::Value as YmlValue;

type Fqn = Vec<String>;
type FqnSet = BTreeSet<Fqn>;
type ResourceFqns = BTreeMap<&'static str, FqnSet>;

struct ResourceSection {
    yaml_key: &'static str,
    resource_name: &'static str,
}

const RESOURCE_SECTIONS: &[ResourceSection] = &[
    ResourceSection {
        yaml_key: "models",
        resource_name: "models",
    },
    ResourceSection {
        yaml_key: "seeds",
        resource_name: "seeds",
    },
    ResourceSection {
        yaml_key: "snapshots",
        resource_name: "snapshots",
    },
    ResourceSection {
        yaml_key: "sources",
        resource_name: "sources",
    },
    ResourceSection {
        yaml_key: "data_tests",
        resource_name: "data_tests",
    },
    ResourceSection {
        yaml_key: "tests",
        resource_name: "data_tests",
    },
    ResourceSection {
        yaml_key: "unit_tests",
        resource_name: "unit_tests",
    },
    ResourceSection {
        yaml_key: "metrics",
        resource_name: "metrics",
    },
    ResourceSection {
        yaml_key: "semantic-models",
        resource_name: "semantic_models",
    },
    ResourceSection {
        yaml_key: "saved-queries",
        resource_name: "saved_queries",
    },
    ResourceSection {
        yaml_key: "exposures",
        resource_name: "exposures",
    },
    ResourceSection {
        yaml_key: "functions",
        resource_name: "functions",
    },
];

/// Emits dbt-core compatible warnings for config paths that do not match any resource FQN.
pub(crate) fn check_unused_resource_config_paths(
    root_project_path: &Path,
    nodes: &Nodes,
    disabled_nodes: &Nodes,
    skip_data_tests: bool,
) -> FsResult<()> {
    let dbt_project_yml = value_from_file(&root_project_path.join(DBT_PROJECT_YML), false, None)?;
    let used_fqns = collect_used_fqns(nodes);
    let disabled_fqns = collect_disabled_fqns(disabled_nodes);
    let unused_config_paths = collect_unused_config_paths(
        &dbt_project_yml,
        &used_fqns,
        &disabled_fqns,
        skip_data_tests,
    );

    if unused_config_paths.is_empty() {
        return Ok(());
    }

    let unused_paths = unused_config_paths
        .iter()
        .map(|path| format!("- {path}"))
        .collect::<Vec<_>>()
        .join("\n");
    let message = format!(
        "Configuration paths exist in your dbt_project.yml file which do not apply to any resources.\n\
There are {} unused configuration paths:\n{}",
        unused_config_paths.len(),
        unused_paths,
    );

    emit_warn_log_message(ErrorCode::UnusedResourceConfigPath, message);

    Ok(())
}

fn collect_unused_config_paths(
    dbt_project_yml: &YmlValue,
    used_fqns: &ResourceFqns,
    disabled_fqns: &FqnSet,
    skip_data_tests: bool,
) -> Vec<String> {
    let Some(project_map) = dbt_project_yml.as_mapping() else {
        return Vec::new();
    };

    let mut unused_paths = BTreeSet::new();
    for section in RESOURCE_SECTIONS {
        if skip_data_tests && section.yaml_key == "data_tests" {
            continue;
        }

        let Some(section_value) = project_map.get(section.yaml_key) else {
            continue;
        };

        let config_paths = collect_config_paths(section_value);
        let empty_fqns = FqnSet::new();
        let resource_fqns = used_fqns.get(section.resource_name).unwrap_or(&empty_fqns);

        for config_path in config_paths {
            if !is_config_used(&config_path, resource_fqns, disabled_fqns) {
                unused_paths.insert(format_resource_path(section.resource_name, &config_path));
            }
        }
    }

    unused_paths.into_iter().collect()
}

fn collect_config_paths(value: &YmlValue) -> FqnSet {
    let mut paths = FqnSet::new();
    collect_config_paths_inner(value, Vec::new(), &mut paths);
    paths
}

fn collect_config_paths_inner(value: &YmlValue, path: Fqn, paths: &mut FqnSet) {
    let Some(mapping) = value.as_mapping() else {
        paths.insert(path);
        return;
    };

    for (key, value) in mapping.iter() {
        let Some(key) = key.as_str() else {
            paths.insert(path.clone());
            continue;
        };

        if value.as_mapping().is_some() && !key.starts_with('+') {
            let mut child_path = path.clone();
            child_path.push(key.to_string());
            collect_config_paths_inner(value, child_path, paths);
        } else {
            paths.insert(path.clone());
        }
    }
}

fn is_config_used(config_path: &[String], resource_fqns: &FqnSet, disabled_fqns: &FqnSet) -> bool {
    resource_fqns
        .iter()
        .chain(disabled_fqns.iter())
        .any(|fqn| config_path.len() <= fqn.len() && fqn.starts_with(config_path))
}

fn format_resource_path(resource_name: &str, path: &[String]) -> String {
    if path.is_empty() {
        resource_name.to_string()
    } else {
        format!("{}.{}", resource_name, path.join("."))
    }
}

fn collect_used_fqns(nodes: &Nodes) -> ResourceFqns {
    let mut used_fqns = ResourceFqns::new();
    insert_resource_fqns(&mut used_fqns, "models", &nodes.models);
    insert_resource_fqns(&mut used_fqns, "seeds", &nodes.seeds);
    insert_resource_fqns(&mut used_fqns, "snapshots", &nodes.snapshots);
    insert_resource_fqns(&mut used_fqns, "sources", &nodes.sources);
    insert_resource_fqns(&mut used_fqns, "data_tests", &nodes.tests);
    insert_resource_fqns(&mut used_fqns, "unit_tests", &nodes.unit_tests);
    insert_resource_fqns(&mut used_fqns, "metrics", &nodes.metrics);
    insert_resource_fqns(&mut used_fqns, "semantic_models", &nodes.semantic_models);
    insert_resource_fqns(&mut used_fqns, "saved_queries", &nodes.saved_queries);
    insert_resource_fqns(&mut used_fqns, "exposures", &nodes.exposures);
    insert_resource_fqns(&mut used_fqns, "functions", &nodes.functions);
    used_fqns
}

fn insert_resource_fqns<T>(
    used_fqns: &mut ResourceFqns,
    resource_name: &'static str,
    nodes: &BTreeMap<String, Arc<T>>,
) where
    T: InternalDbtNode,
{
    let mut fqns = FqnSet::new();
    extend_fqns(&mut fqns, nodes);
    used_fqns.insert(resource_name, fqns);
}

fn collect_disabled_fqns(disabled_nodes: &Nodes) -> FqnSet {
    let mut fqns = FqnSet::new();
    extend_fqns(&mut fqns, &disabled_nodes.models);
    extend_fqns(&mut fqns, &disabled_nodes.seeds);
    extend_fqns(&mut fqns, &disabled_nodes.tests);
    extend_fqns(&mut fqns, &disabled_nodes.unit_tests);
    extend_fqns(&mut fqns, &disabled_nodes.sources);
    extend_fqns(&mut fqns, &disabled_nodes.snapshots);
    extend_fqns(&mut fqns, &disabled_nodes.analyses);
    extend_fqns(&mut fqns, &disabled_nodes.exposures);
    extend_fqns(&mut fqns, &disabled_nodes.semantic_models);
    extend_fqns(&mut fqns, &disabled_nodes.metrics);
    extend_fqns(&mut fqns, &disabled_nodes.saved_queries);
    extend_fqns(&mut fqns, &disabled_nodes.functions);
    fqns
}

fn extend_fqns<T>(fqns: &mut FqnSet, nodes: &BTreeMap<String, Arc<T>>)
where
    T: InternalDbtNode,
{
    for node in nodes.values() {
        fqns.insert(node.common().fqn.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fqn(parts: Vec<&str>) -> Fqn {
        parts.into_iter().map(str::to_string).collect()
    }

    fn fqn_set(fqns: Vec<Vec<&str>>) -> FqnSet {
        fqns.into_iter().map(fqn).collect()
    }

    #[test]
    fn config_walker_records_parent_for_plus_keys() {
        let project_yml: YmlValue = dbt_yaml::from_str(
            r#"
models:
  test:
    marts:
      +materialized: view
"#,
        )
        .unwrap();
        let models = project_yml
            .as_mapping()
            .and_then(|mapping| mapping.get("models"))
            .unwrap();

        assert_eq!(
            collect_config_paths(models),
            fqn_set(vec![vec!["test", "marts"]])
        );
    }

    #[test]
    fn config_walker_records_parent_for_non_mapping_values() {
        let project_yml: YmlValue = dbt_yaml::from_str(
            r#"
models:
  test:
    marts:
      materialized: view
"#,
        )
        .unwrap();
        let models = project_yml
            .as_mapping()
            .and_then(|mapping| mapping.get("models"))
            .unwrap();

        assert_eq!(
            collect_config_paths(models),
            fqn_set(vec![vec!["test", "marts"]])
        );
    }

    #[test]
    fn nested_unused_path_is_detected_while_matching_fqn_is_used() {
        let project_yml: YmlValue = dbt_yaml::from_str(
            r#"
models:
  test:
    used:
      +materialized: view
    unused:
      +materialized: table
"#,
        )
        .unwrap();
        let used_fqns =
            ResourceFqns::from([("models", fqn_set(vec![vec!["test", "used", "my_model"]]))]);

        assert_eq!(
            collect_unused_config_paths(&project_yml, &used_fqns, &FqnSet::new(), false),
            vec!["models.test.unused".to_string()]
        );
    }

    #[test]
    fn disabled_fqn_counts_as_used() {
        let project_yml: YmlValue = dbt_yaml::from_str(
            r#"
models:
  test:
    disabled:
      +enabled: false
"#,
        )
        .unwrap();
        let disabled_fqns = fqn_set(vec![vec!["test", "disabled", "my_model"]]);

        assert_eq!(
            collect_unused_config_paths(&project_yml, &ResourceFqns::new(), &disabled_fqns, false),
            Vec::<String>::new()
        );
    }

    #[test]
    fn tests_alias_reports_as_data_tests() {
        let project_yml: YmlValue = dbt_yaml::from_str(
            r#"
tests:
  test:
    +enabled: true
"#,
        )
        .unwrap();

        assert_eq!(
            collect_unused_config_paths(&project_yml, &ResourceFqns::new(), &FqnSet::new(), false),
            vec!["data_tests.test".to_string()]
        );
    }
}
