use dbt_common::path::DbtPath;
use dbt_schemas::schemas::{
    DbtModel, DbtSeed, DbtTest, InternalDbtNode, InternalDbtNodeAttributes,
    common::DbtMaterialization, config_excluded_keys, macros::DbtMacro,
};
use md5;
use serde::Serialize;
use serde_json::{Value as JsonValue, ser::Formatter, to_string};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
};
use std::{io, io::BufReader, io::Write};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NodeHashError {
    #[error("failed to serialize dbt State node property: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("failed to read seed file: {0}")]
    SeedIOError(#[from] io::Error),
}

#[derive(Debug)]
pub struct NodeStateHashes {
    pub node_hash: String,
    pub node_body_hash: Option<String>,
    pub node_configs_hash: Option<String>,
    pub node_persisted_descriptions_hash: Option<String>,
    pub node_macros_hash: Option<String>,
    pub node_contract_hash: Option<String>,
}

pub fn node_state_hashes<'a>(
    node: &'a dyn InternalDbtNodeAttributes,
    project_root: &DbtPath,
    macro_resolver: impl Fn(&str) -> Option<&'a DbtMacro>,
) -> Result<NodeStateHashes, NodeHashError> {
    let type_erased_node = node.as_any();

    let node_body_hash = type_erased_node.downcast_ref::<DbtSeed>().map_or_else(
        || Ok(node_body_hash(node)),
        |s| seed_node_body_hash(s, project_root).map(Some),
    )?;
    let node_configs_hash = node_configs_hash(node)?;
    let node_persisted_descriptions_hash = node_persisted_docs_hash(node)?;

    let node_macros_hash = node_macros_hash(node, macro_resolver);
    let mut node_contract_hash = None;
    let node_fqn = Some(node.selector_string());

    let node_hash = if let Some(model_node) = type_erased_node.downcast_ref::<DbtModel>() {
        node_contract_hash = Some(model_node_contract_hash(model_node));
        let node_ref_representation_hash = model_node_ref_representation_hash(model_node)?;

        // ref: https://github.com/fivetran/query-cache/blob/4ab87c72f2783c230905c5c75ec19eb76314069e/clients/dbt_state/src/dbt_state/node_hash_calculator.py#L180
        hash_parts(&[
            node_body_hash.as_deref(),
            node_configs_hash.as_deref(),
            node_persisted_descriptions_hash.as_deref(),
            node_macros_hash.as_deref(),
            node_fqn.as_deref(),
            node_contract_hash.as_deref(),
            Some(node_ref_representation_hash.as_str()),
        ])
    } else if type_erased_node.downcast_ref::<DbtSeed>().is_some() {
        // seeds just return the body hash directly or an empty string if it is not populated
        // ref: https://github.com/fivetran/query-cache/blob/4ab87c72f2783c230905c5c75ec19eb76314069e/clients/dbt_state/src/dbt_state/node_hash_calculator.py#L203
        node_body_hash.clone().unwrap_or_else(|| "".to_owned())
    } else if let Some(data_test_node) = type_erased_node.downcast_ref::<DbtTest>()
        && data_test_node.__test_attr__.test_metadata.is_some()
    {
        // __test_attr__.test_metadata being supplied means it's a generic test as opposed to a singular test
        // to match the Python code, we only have a special case for generic tests
        // ref: https://github.com/fivetran/query-cache/blob/4ab87c72f2783c230905c5c75ec19eb76314069e/clients/dbt_state/src/dbt_state/node_hash_calculator.py#L193
        hash_parts(&[node_configs_hash.as_deref(), node_fqn.as_deref()])
    } else {
        // fallback node_hash impl that covers a bunch of bases on the other node types
        // ref: https://github.com/fivetran/query-cache/blob/4ab87c72f2783c230905c5c75ec19eb76314069e/clients/dbt_state/src/dbt_state/node_hash_calculator.py#L43
        hash_parts(&[
            node_body_hash.as_deref(),
            node_configs_hash.as_deref(),
            node_persisted_descriptions_hash.as_deref(),
            node_macros_hash.as_deref(),
            node_fqn.as_deref(),
        ])
    };

    let state_hashes = NodeStateHashes {
        node_hash,
        node_body_hash,
        node_configs_hash,
        node_persisted_descriptions_hash,
        node_macros_hash,
        node_contract_hash,
    };

    Ok(state_hashes)
}

fn node_body_hash(node: &dyn InternalDbtNode) -> Option<String> {
    //ref: https://github.com/fivetran/query-cache/blob/4ab87c72f2783c230905c5c75ec19eb76314069e/clients/dbt_state/src/dbt_state/node_hash_calculator.py#L58
    // Python hashes the raw string via `str()`, so the body must not be JSON-encoded.
    node.common()
        .raw_code
        .as_deref()
        .filter(|rc| !rc.is_empty())
        .map(|rc| hash_parts(&[Some(rc)]))
}

fn node_configs_hash(node: &dyn InternalDbtNode) -> Result<Option<String>, NodeHashError> {
    //ref: https://github.com/fivetran/query-cache/blob/4ab87c72f2783c230905c5c75ec19eb76314069e/clients/dbt_state/src/dbt_state/node_hash_calculator.py#L65
    let keys_to_exclude = config_excluded_keys(node.resource_type());

    let filtered = node
        .base()
        .unrendered_config
        .iter()
        .filter(|(name, _)| !keys_to_exclude.contains(&name.as_str()))
        .collect::<BTreeMap<_, _>>();

    if !filtered.is_empty() {
        hash_json(&filtered).map(Some)
    } else {
        Ok(None)
    }
}

fn node_persisted_docs_hash(node: &dyn InternalDbtNode) -> Result<Option<String>, NodeHashError> {
    // ref: https://github.com/fivetran/query-cache/blob/4ab87c72f2783c230905c5c75ec19eb76314069e/clients/dbt_state/src/dbt_state/node_hash_calculator.py#L76
    // Python builds `parts` incrementally, so a key whose `persist_docs` flag is unset is
    // *absent* rather than `null`, and an empty `parts` yields `None` instead of a hash.
    let base_attr = node.base();
    let Some(persist_docs) = base_attr.persist_docs.as_ref() else {
        return Ok(None);
    };

    let mut parts: BTreeMap<&str, JsonValue> = BTreeMap::new();

    if persist_docs.relation == Some(true) {
        let description = node.common().description.as_deref().unwrap_or("");
        parts.insert("description", JsonValue::from(description));
    }

    if persist_docs.columns == Some(true) {
        let cols = base_attr
            .columns
            .iter()
            .map(|col| {
                let name = &*col.name;
                let description = col.description.as_deref().unwrap_or("");
                (name, description)
            })
            .collect::<BTreeMap<_, _>>();

        parts.insert("columns", serde_json::to_value(cols)?);
    }

    if parts.is_empty() {
        return Ok(None);
    }

    hash_json(&parts).map(Some)
}

fn node_macros_hash<'a>(
    node: &'a dyn InternalDbtNode,
    macro_resolver: impl Fn(&str) -> Option<&'a DbtMacro>,
) -> Option<String> {
    //ref: https://github.com/fivetran/query-cache/blob/4ab87c72f2783c230905c5c75ec19eb76314069e/clients/dbt_state/src/dbt_state/node_hash_calculator.py#L93
    if node.base().depends_on.macros.is_empty() {
        return None;
    }

    // Python hashes `"".join(macro_sqls)`, i.e. the macro bodies concatenated bare.
    let macro_sqls = resolve_macro_tree(node, macro_resolver)
        .values()
        .map(|m| m.macro_sql.as_str())
        .collect::<String>();

    Some(hash_parts(&[Some(macro_sqls.as_str())]))
}

fn resolve_macro_tree<'a>(
    node: &'a dyn InternalDbtNode,
    macro_resolver: impl Fn(&str) -> Option<&'a DbtMacro>,
) -> BTreeMap<&'a String, &'a DbtMacro> {
    // recursively resolve all the macros of the current node, plus all the
    // macros that *those* macros depend on - etc until there are no more macros
    let mut all_macros = BTreeMap::new();

    let mut queue = node
        .base()
        .depends_on
        .macros
        .iter()
        .collect::<BTreeSet<_>>();

    while let Some(next_id) = queue.pop_first() {
        if let Some(m) = macro_resolver(next_id) {
            all_macros.insert(next_id, m);

            for id in &m.depends_on.macros {
                if !all_macros.contains_key(id) {
                    queue.insert(id);
                }
            }
        }
    }

    all_macros
}

/// Mirrors Python's `ModelNode.build_contract_checksum` (dbt-core/core/dbt/contracts/graph/nodes.py:687-709).
fn build_contract_checksum(node: &DbtModel) -> Option<String> {
    let contract = node.__model_attr__.contract.as_ref()?;

    if let Some(checksum) = &contract.checksum {
        return checksum
            .as_str()
            .map(str::to_owned)
            .or_else(|| to_string(checksum).ok());
    }

    if !contract.enforced {
        return None;
    }

    let mut contract_state = String::new();

    let mut sorted_columns: Vec<_> = node.__base_attr__.columns.iter().collect();
    sorted_columns.sort_by(|a, b| a.name.cmp(&b.name));

    for column in sorted_columns {
        contract_state.push('|');
        contract_state.push_str(&column.name);

        match &column.data_type {
            Some(data_type) => contract_state.push_str(data_type),
            None => contract_state.push_str("None"),
        }
        let constraints_str = to_string(&column.constraints).unwrap_or_else(|_| "[]".to_owned());
        contract_state.push_str(&constraints_str);
    }

    let materialization = &node.__base_attr__.materialized;
    let materialization_enforces_constraints = matches!(
        materialization,
        DbtMaterialization::Table | DbtMaterialization::Incremental
    );

    if materialization_enforces_constraints {
        contract_state.push_str(&materialization.to_string());
        let constraints_str =
            to_string(&node.__model_attr__.constraints).unwrap_or_else(|_| "[]".to_owned());
        contract_state.push_str(&constraints_str);
    }

    let mut hasher = Sha256::new();
    hasher.update(contract_state.as_bytes());
    Some(format!("{:x}", hasher.finalize()))
}

fn model_node_contract_hash(node: &DbtModel) -> String {
    //ref: https://github.com/fivetran/query-cache/blob/4ab87c72f2783c230905c5c75ec19eb76314069e/clients/dbt_state/src/dbt_state/node_hash_calculator.py#L152
    let fallback = "enforced:false";

    let value_to_hash = node
        .__model_attr__
        .contract
        .as_ref()
        .map(|contract| {
            if contract.enforced {
                if let Some(checksum) = build_contract_checksum(node) {
                    format!("enforced:true|checksum:{checksum}")
                } else {
                    fallback.to_owned()
                }
            } else {
                fallback.to_owned()
            }
        })
        .unwrap_or_else(|| fallback.to_owned());

    hash_parts(&[Some(value_to_hash.as_str())])
}

fn model_node_ref_representation_hash(node: &DbtModel) -> Result<String, NodeHashError> {
    //ref: https://github.com/fivetran/query-cache/blob/4ab87c72f2783c230905c5c75ec19eb76314069e/clients/dbt_state/src/dbt_state/node_hash_calculator.py#L170
    // Python gates on `hasattr(node, "latest_version")`, which is always true for a model,
    // so this component is always present - it is NOT skipped for unversioned models.
    // `access` and `deprecation_date` go through `str()`, making an absent value the literal
    // string "None"; `latest_version` is passed through raw, so it keeps its JSON type.
    let mut parts: BTreeMap<&str, JsonValue> = BTreeMap::new();

    parts.insert(
        "latest_version",
        match node.latest_version() {
            Some(v) => serde_json::to_value(v)?,
            None => JsonValue::Null,
        },
    );
    parts.insert(
        "access",
        JsonValue::from(
            node.get_access()
                .map_or_else(|| "None".to_owned(), |a| a.to_string()),
        ),
    );
    parts.insert(
        "deprecation_date",
        JsonValue::from(
            node.__model_attr__
                .deprecation_date
                .as_ref()
                .map(|ts| ts.with_defaults().to_string())
                .unwrap_or_else(|| "None".to_owned()),
        ),
    );

    hash_json(&parts)
}

fn seed_node_body_hash(
    node: &DbtSeed,
    project_root_path: &DbtPath,
) -> Result<String, NodeHashError> {
    // ref: https://github.com/fivetran/query-cache/blob/4ab87c72f2783c230905c5c75ec19eb76314069e/clients/dbt_state/src/dbt_state/node_hash_calculator.py#L203
    let seed_relative_path = &node.common().original_file_path;
    let seed_full_path = project_root_path.join(seed_relative_path);

    let mut reader = BufReader::new(File::open(seed_full_path)?);
    let mut ctx = md5::Context::new();
    io::copy(&mut reader, &mut ctx)?;

    Ok(format!("{:x}", ctx.compute()))
}

/// Mirrors Python `_calculate_hash(*args)`:
/// `md5("".join(str(x) for x in args))`.
///
/// `None` entries are skipped, mirroring the `if p is not None` filter Python applies
/// when composing parts. An empty slice hashes `""`, as Python's empty join does.
/// ref: https://github.com/fivetran/query-cache/blob/4ab87c72f2783c230905c5c75ec19eb76314069e/clients/dbt_state/src/dbt_state/node_hash_calculator.py#L29
fn hash_parts(parts: &[Option<&str>]) -> String {
    let joined: String = parts.iter().flatten().copied().collect();
    format!("{:x}", md5::compute(joined))
}

/// Mirrors Python `_calculate_hash(json.dumps(value, sort_keys=True))`.
///
/// Key sorting must come from the value itself (use `BTreeMap`, not a struct, wherever
/// Python relies on `sort_keys=True` — a struct serializes in declaration order).
fn hash_json<T>(value: &T) -> Result<String, NodeHashError>
where
    T: Serialize + ?Sized,
{
    Ok(format!("{:x}", md5::compute(python_json(value)?)))
}

fn python_json<T>(value: &T) -> Result<String, NodeHashError>
where
    T: Serialize + ?Sized,
{
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, PythonJsonFormatter);
    value.serialize(&mut ser)?;
    // `serde_json` only ever writes UTF-8, and the formatter below emits ASCII.
    Ok(String::from_utf8(buf).expect("serde_json emits valid UTF-8"))
}

/// Makes `serde_json` match `json.dumps`'s defaults, which differ in two ways that both
/// change the bytes fed to md5: separators (`", "` / `": "` vs serde's compact `,` / `:`)
/// and `ensure_ascii=True` (`\uXXXX` escapes vs serde's raw UTF-8).
struct PythonJsonFormatter;

impl Formatter for PythonJsonFormatter {
    fn begin_array_value<W>(&mut self, writer: &mut W, first: bool) -> io::Result<()>
    where
        W: ?Sized + Write,
    {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }

    fn begin_object_key<W>(&mut self, writer: &mut W, first: bool) -> io::Result<()>
    where
        W: ?Sized + Write,
    {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }

    fn begin_object_value<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + Write,
    {
        writer.write_all(b": ")
    }

    fn write_string_fragment<W>(&mut self, writer: &mut W, fragment: &str) -> io::Result<()>
    where
        W: ?Sized + Write,
    {
        for c in fragment.chars() {
            if c.is_ascii() {
                writer.write_all(c.encode_utf8(&mut [0u8; 4]).as_bytes())?;
            } else {
                // Python escapes astral characters as UTF-16 surrogate pairs.
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    write!(writer, "\\u{unit:04x}")?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_common::path::DbtPath;
    use dbt_schemas::schemas::{
        CommonAttributes, DbtSeedAttr, TestMetadata,
        common::{Access, DbtContract, PersistDocsConfig},
        dbt_column::DbtColumn,
        macros::MacroDependsOn,
        serde::StringOrInteger,
    };
    use dbt_yaml::Value as YmlValue;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn test_node(fqn: &[&str]) -> DbtTest {
        let mut t = DbtTest::default();
        t.__common_attr__.fqn = fqn.iter().map(|s| s.to_string()).collect();
        t
    }

    /// A `DbtTest` with `test_metadata` set represents a generic (schema) test,
    /// as opposed to a singular test (which hits the same fallback path as
    /// snapshots - see `test_node` above).
    fn generic_test_node(fqn: &[&str]) -> DbtTest {
        let mut t = test_node(fqn);
        t.__test_attr__.test_metadata = Some(TestMetadata {
            name: "unique".to_string(),
            kwargs: BTreeMap::default(),
            namespace: None,
        });
        t
    }

    fn model_node(fqn: &[&str]) -> DbtModel {
        let mut m = DbtModel::default();
        m.__common_attr__.fqn = fqn.iter().map(|s| s.to_string()).collect();
        m
    }

    fn resolver<'a>(
        macros: &'a BTreeMap<String, DbtMacro>,
    ) -> impl Fn(&str) -> Option<&'a DbtMacro> {
        move |id| macros.get(id)
    }

    fn dummy_project_root() -> DbtPath {
        "hash-tests".into()
    }

    mod body_hash {
        use super::*;

        #[test]
        fn node_body_hash_present_when_raw_code_set() {
            let mut node = test_node(&["project", "models", "test_model"]);
            node.__common_attr__.raw_code = Some("SELECT * FROM table".to_string());
            assert!(node_body_hash(&node).is_some());
        }

        #[test]
        fn node_body_hash_none_when_raw_code_absent() {
            let node = test_node(&["project", "models", "test_model"]);
            assert_eq!(node_body_hash(&node), None);
        }

        #[test]
        fn node_body_hash_none_when_raw_code_empty() {
            let mut node = test_node(&["project", "models", "test_model"]);
            node.__common_attr__.raw_code = Some(String::new());
            assert_eq!(node_body_hash(&node), None);
        }

        #[test]
        fn node_body_hash_differs_for_different_raw_code() {
            let mut a = test_node(&["project", "models", "test_model"]);
            a.__common_attr__.raw_code = Some("SELECT * FROM table1".to_string());
            let mut b = test_node(&["project", "models", "test_model"]);
            b.__common_attr__.raw_code = Some("SELECT * FROM table2".to_string());
            assert_ne!(node_body_hash(&a), node_body_hash(&b));
        }

        #[test]
        fn seed_node_body_hash_matches_md5_hex() {
            let tmpdir = TempDir::new().unwrap();
            let project_root: DbtPath = tmpdir.path().into();
            let seed_file_path = project_root.join("seed_data.csv");

            let seed_bytes = b"id,name\n1,Ada\n2,Grace\n";
            fs::write(&seed_file_path, seed_bytes).unwrap();

            let expected_hash = "e146991e1c07585745c5a65f06a517e9";

            let common_attr = CommonAttributes {
                original_file_path: seed_file_path.get_relative_path(&project_root).unwrap(),
                ..Default::default()
            };

            let seed = DbtSeed {
                __common_attr__: common_attr,
                ..Default::default()
            };

            assert_eq!(
                seed_node_body_hash(&seed, &project_root).unwrap(),
                expected_hash
            );
        }
    }

    mod configs_hash {
        use super::*;

        #[test]
        fn node_configs_hash_present_when_unrendered_config_set() {
            let mut node = test_node(&["project", "models", "test_model"]);
            node.__base_attr__.unrendered_config = BTreeMap::from([
                (
                    "materialized".to_string(),
                    YmlValue::string("table".to_string()),
                ),
                ("enabled".to_string(), YmlValue::bool(true)),
            ]);
            assert!(node_configs_hash(&node).unwrap().is_some());
        }

        #[test]
        fn node_configs_hash_excludes_alias_schema_database_tags_group_for_models() {
            let mut with_excluded = model_node(&["project", "models", "test_model"]);
            with_excluded.__base_attr__.unrendered_config = BTreeMap::from([
                (
                    "materialized".to_string(),
                    YmlValue::string("table".to_string()),
                ),
                (
                    "alias".to_string(),
                    YmlValue::string("my_alias".to_string()),
                ),
                (
                    "schema".to_string(),
                    YmlValue::string("my_schema".to_string()),
                ),
                (
                    "database".to_string(),
                    YmlValue::string("my_db".to_string()),
                ),
                (
                    "tags".to_string(),
                    dbt_yaml::from_str(r#"["tag1"]"#).unwrap(),
                ),
                (
                    "group".to_string(),
                    YmlValue::string("my_group".to_string()),
                ),
                ("enabled".to_string(), YmlValue::bool(true)),
            ]);

            let mut without_excluded = model_node(&["project", "models", "test_model"]);
            without_excluded.__base_attr__.unrendered_config = BTreeMap::from([
                (
                    "materialized".to_string(),
                    YmlValue::string("table".to_string()),
                ),
                ("enabled".to_string(), YmlValue::bool(true)),
            ]);

            assert_eq!(
                node_configs_hash(&with_excluded).unwrap(),
                node_configs_hash(&without_excluded).unwrap()
            );
        }

        #[test]
        fn node_configs_hash_excludes_specific_keys_for_generic_tests_too() {
            let mut with_excluded = generic_test_node(&["project", "models", "test1"]);
            with_excluded.__base_attr__.unrendered_config = BTreeMap::from([
                (
                    "severity".to_string(),
                    YmlValue::string("error".to_string()),
                ),
                (
                    "alias".to_string(),
                    YmlValue::string("my_alias".to_string()),
                ),
            ]);

            let mut without_excluded = generic_test_node(&["project", "models", "test1"]);
            without_excluded.__base_attr__.unrendered_config = BTreeMap::from([(
                "severity".to_string(),
                YmlValue::string("error".to_string()),
            )]);

            assert_eq!(
                node_configs_hash(&with_excluded).unwrap(),
                node_configs_hash(&without_excluded).unwrap()
            );
        }

        #[test]
        fn node_configs_hash_none_when_unrendered_config_empty() {
            let node = test_node(&["project", "models", "test_model"]);
            assert_eq!(node_configs_hash(&node).unwrap(), None);
        }

        #[test]
        fn node_configs_hash_none_when_all_keys_excluded() {
            let mut node = model_node(&["project", "models", "test_model"]);
            node.__base_attr__.unrendered_config = BTreeMap::from([
                (
                    "alias".to_string(),
                    YmlValue::string("my_alias".to_string()),
                ),
                (
                    "schema".to_string(),
                    YmlValue::string("my_schema".to_string()),
                ),
                (
                    "database".to_string(),
                    YmlValue::string("my_db".to_string()),
                ),
                (
                    "tags".to_string(),
                    dbt_yaml::from_str(r#"["tag1"]"#).unwrap(),
                ),
            ]);
            assert_eq!(node_configs_hash(&node).unwrap(), None);
        }
    }

    mod persisted_docs_hash {
        use super::*;

        #[test]
        fn node_persisted_docs_hash_none_when_no_persist_docs() {
            let node = test_node(&["project", "models", "test_model"]);
            assert_eq!(node_persisted_docs_hash(&node).unwrap(), None);
        }

        #[test]
        fn node_persisted_docs_hash_none_when_persist_docs_empty() {
            let mut node = test_node(&["project", "models", "test_model"]);
            node.__base_attr__.persist_docs = Some(PersistDocsConfig::default());
            assert_eq!(node_persisted_docs_hash(&node).unwrap(), None);
        }

        #[test]
        fn node_persisted_docs_hash_relation_persisted_varies_with_description() {
            let mut with_desc = test_node(&["project", "models", "test_model"]);
            with_desc.__base_attr__.persist_docs = Some(PersistDocsConfig {
                relation: Some(true),
                columns: None,
            });
            with_desc.__common_attr__.description = Some("My model description".to_string());

            let mut without_desc = test_node(&["project", "models", "test_model"]);
            without_desc.__base_attr__.persist_docs = Some(PersistDocsConfig {
                relation: Some(true),
                columns: None,
            });

            let with_hash = node_persisted_docs_hash(&with_desc).unwrap();
            let without_hash = node_persisted_docs_hash(&without_desc).unwrap();
            assert!(with_hash.is_some());
            assert!(without_hash.is_some());
            assert_ne!(with_hash, without_hash);
        }

        #[test]
        fn node_persisted_docs_hash_columns_persisted_varies_with_column_descriptions() {
            let col = |name: &str, desc: Option<&str>| {
                Arc::new(DbtColumn {
                    name: name.to_string(),
                    description: desc.map(str::to_string),
                    ..Default::default()
                })
            };

            let mut with_desc = test_node(&["project", "models", "test_model"]);
            with_desc.__base_attr__.persist_docs = Some(PersistDocsConfig {
                relation: None,
                columns: Some(true),
            });
            with_desc.__base_attr__.columns = vec![col("col1", Some("Column 1 description"))];

            let mut none_desc = test_node(&["project", "models", "test_model"]);
            none_desc.__base_attr__.persist_docs = Some(PersistDocsConfig {
                relation: None,
                columns: Some(true),
            });
            none_desc.__base_attr__.columns = vec![col("col1", None)];

            assert_ne!(
                node_persisted_docs_hash(&with_desc).unwrap(),
                node_persisted_docs_hash(&none_desc).unwrap()
            );
        }

        #[test]
        fn node_persisted_docs_hash_combines_relation_and_columns() {
            let mut node = test_node(&["project", "models", "test_model"]);
            node.__base_attr__.persist_docs = Some(PersistDocsConfig {
                relation: Some(true),
                columns: Some(true),
            });
            node.__common_attr__.description = Some("Model description".to_string());
            node.__base_attr__.columns = vec![Arc::new(DbtColumn {
                name: "col1".to_string(),
                description: Some("Column 1".to_string()),
                ..Default::default()
            })];

            assert!(node_persisted_docs_hash(&node).unwrap().is_some());
        }

        #[test]
        fn node_persisted_docs_hash_some_when_columns_persisted_but_no_columns_present() {
            let mut node = test_node(&["project", "models", "test_model"]);
            node.__base_attr__.persist_docs = Some(PersistDocsConfig {
                relation: None,
                columns: Some(true),
            });
            assert!(node_persisted_docs_hash(&node).unwrap().is_some());
        }
    }

    mod macros_hash {
        use super::*;

        #[test]
        fn node_macros_hash_none_when_no_macros_referenced() {
            let node = test_node(&["project", "models", "test_model"]);
            let macros_map: BTreeMap<String, DbtMacro> = BTreeMap::new();
            assert_eq!(node_macros_hash(&node, resolver(&macros_map)), None);
        }

        #[test]
        fn node_macros_hash_present_for_direct_macro() {
            let mut node = test_node(&["project", "models", "test_model"]);
            node.__base_attr__.depends_on.macros = vec!["macro.project.test1".to_string()];

            let macros_map = BTreeMap::from([(
                "macro.project.test1".to_string(),
                DbtMacro {
                    macro_sql: "{% macro test1() %} SELECT 1 {% endmacro %}".to_string(),
                    ..Default::default()
                },
            )]);

            assert!(node_macros_hash(&node, resolver(&macros_map)).is_some());
        }

        #[test]
        fn node_macros_hash_combines_multiple_direct_macros() {
            let macros_map = BTreeMap::from([
                (
                    "macro1".to_string(),
                    DbtMacro {
                        macro_sql: "SQL1".to_string(),
                        ..Default::default()
                    },
                ),
                (
                    "macro2".to_string(),
                    DbtMacro {
                        macro_sql: "SQL2".to_string(),
                        ..Default::default()
                    },
                ),
            ]);

            let mut with_two = test_node(&["project", "models", "test_model"]);
            with_two.__base_attr__.depends_on.macros =
                vec!["macro1".to_string(), "macro2".to_string()];
            let two_hash = node_macros_hash(&with_two, resolver(&macros_map));

            let mut with_one = test_node(&["project", "models", "test_model"]);
            with_one.__base_attr__.depends_on.macros = vec!["macro1".to_string()];
            let one_hash = node_macros_hash(&with_one, resolver(&macros_map));

            assert_ne!(two_hash, one_hash);
        }

        #[test]
        fn node_macros_hash_does_not_include_upstream_node_macros() {
            // node_macros_hash only ever looks at this node's own depends_on.macros, never
            // depends_on.nodes, so an upstream node's macros can never leak in.
            let mut node = test_node(&["project", "models", "test_model"]);
            let mut upstream_node = test_node(&["project", "models", "upstream"]);
            upstream_node
                .base_mut()
                .depends_on
                .macros
                .push("macro_from_upstream".to_owned());

            node.base_mut()
                .depends_on
                .nodes
                .push(upstream_node.unique_id());

            assert!(!node.base().depends_on.nodes.is_empty());
            assert!(node.base().depends_on.macros.is_empty());

            let macros_map = BTreeMap::from([(
                "macro_from_upstream".to_string(),
                DbtMacro {
                    macro_sql: "SQL_FROM_UPSTREAM".to_string(),
                    ..Default::default()
                },
            )]);
            assert_eq!(node_macros_hash(&node, resolver(&macros_map)), None);
        }

        #[test]
        fn node_macros_hash_includes_transitive_macro_dependencies() {
            let mut node = test_node(&["project", "models", "test_model"]);
            node.__base_attr__.depends_on.macros = vec!["macro1".to_string()];

            let macros_with_dep = BTreeMap::from([
                (
                    "macro1".to_string(),
                    DbtMacro {
                        macro_sql: "SQL1".to_string(),
                        depends_on: MacroDependsOn {
                            macros: vec!["macro2".to_string()],
                        },
                        ..Default::default()
                    },
                ),
                (
                    "macro2".to_string(),
                    DbtMacro {
                        macro_sql: "SQL2".to_string(),
                        ..Default::default()
                    },
                ),
            ]);
            let with_dep_hash = node_macros_hash(&node, resolver(&macros_with_dep));

            let macros_without_dep = BTreeMap::from([(
                "macro1".to_string(),
                DbtMacro {
                    macro_sql: "SQL1".to_string(),
                    ..Default::default()
                },
            )]);
            let without_dep_hash = node_macros_hash(&node, resolver(&macros_without_dep));

            assert_ne!(with_dep_hash, without_dep_hash);
        }

        #[test]
        fn node_macros_hash_some_when_macro_reference_is_unresolvable() {
            let mut node = test_node(&["project", "models", "test_model"]);
            node.__base_attr__.depends_on.macros = vec!["missing_macro".to_string()];
            let macros_map: BTreeMap<String, DbtMacro> = BTreeMap::new();
            assert!(node_macros_hash(&node, resolver(&macros_map)).is_some());
        }

        #[test]
        fn node_macros_hash_is_deterministic_regardless_of_insertion_order() {
            let mut node = test_node(&["project", "models", "test_model"]);
            node.__base_attr__.depends_on.macros = (0..10).map(|i| format!("macro_{i}")).collect();

            let macros_map: BTreeMap<String, DbtMacro> = (0..10)
                .map(|i| {
                    (
                        format!("macro_{i}"),
                        DbtMacro {
                            macro_sql: format!("SQL_{i}"),
                            ..Default::default()
                        },
                    )
                })
                .collect();

            let first = node_macros_hash(&node, resolver(&macros_map));
            for _ in 0..9 {
                let repeat = node_macros_hash(&node, resolver(&macros_map));
                assert_eq!(first, repeat);
            }
        }
    }

    mod contract_hash {
        use super::*;

        #[test]
        fn model_node_contract_hash_enforced_with_checksum_differs_from_not_enforced() {
            let mut enforced = model_node(&["project", "models", "test_model"]);
            enforced.__model_attr__.contract = Some(DbtContract {
                enforced: true,
                checksum: Some(YmlValue::string("abc123".to_string())),
                ..Default::default()
            });

            let mut not_enforced = model_node(&["project", "models", "test_model"]);
            not_enforced.__model_attr__.contract = Some(DbtContract {
                enforced: false,
                ..Default::default()
            });

            assert_ne!(
                model_node_contract_hash(&enforced),
                model_node_contract_hash(&not_enforced)
            );
        }

        #[test]
        fn model_node_contract_hash_enforced_without_checksum_computes_checksum() {
            // When enforced=true and no checksum is provided, build_contract_checksum computes one
            // This differs from enforced=false which always returns the fallback hash
            let mut enforced_no_checksum = model_node(&["project", "models", "test_model"]);
            enforced_no_checksum.__model_attr__.contract = Some(DbtContract {
                enforced: true,
                checksum: None,
                ..Default::default()
            });

            let mut not_enforced_with_checksum = model_node(&["project", "models", "test_model"]);
            not_enforced_with_checksum.__model_attr__.contract = Some(DbtContract {
                enforced: false,
                checksum: Some(YmlValue::string("abc123".to_string())),
                ..Default::default()
            });

            // Enforced without checksum now computes a checksum, so it differs from not_enforced
            assert_ne!(
                model_node_contract_hash(&enforced_no_checksum),
                model_node_contract_hash(&not_enforced_with_checksum)
            );
        }

        #[test]
        fn build_contract_checksum_returns_existing_checksum_if_present() {
            let mut node = model_node(&["project", "models", "test_model"]);
            node.__model_attr__.contract = Some(DbtContract {
                enforced: true,
                checksum: Some(YmlValue::string("existing_checksum".to_string())),
                ..Default::default()
            });

            assert_eq!(
                build_contract_checksum(&node),
                Some("existing_checksum".to_string())
            );
        }

        #[test]
        fn build_contract_checksum_returns_none_if_not_enforced() {
            let mut node = model_node(&["project", "models", "test_model"]);
            node.__model_attr__.contract = Some(DbtContract {
                enforced: false,
                checksum: None,
                ..Default::default()
            });

            assert_eq!(build_contract_checksum(&node), None);
        }

        #[test]
        fn build_contract_checksum_computes_from_columns_when_enforced() {
            let mut node = model_node(&["project", "models", "test_model"]);
            node.__model_attr__.contract = Some(DbtContract {
                enforced: true,
                checksum: None,
                ..Default::default()
            });
            node.__base_attr__.columns = vec![Arc::new(DbtColumn {
                name: "id".to_string(),
                data_type: Some("integer".to_string()),
                constraints: vec![],
                ..Default::default()
            })];

            let checksum = build_contract_checksum(&node);
            assert!(checksum.is_some());
            // The checksum should be a 64-character hex string (SHA256)
            assert_eq!(checksum.as_ref().unwrap().len(), 64);
        }

        #[test]
        fn build_contract_checksum_is_deterministic() {
            let mut node1 = model_node(&["project", "models", "test_model"]);
            node1.__model_attr__.contract = Some(DbtContract {
                enforced: true,
                checksum: None,
                ..Default::default()
            });
            node1.__base_attr__.columns = vec![
                Arc::new(DbtColumn {
                    name: "id".to_string(),
                    data_type: Some("integer".to_string()),
                    constraints: vec![],
                    ..Default::default()
                }),
                Arc::new(DbtColumn {
                    name: "name".to_string(),
                    data_type: Some("varchar".to_string()),
                    constraints: vec![],
                    ..Default::default()
                }),
            ];

            let mut node2 = model_node(&["project", "models", "test_model"]);
            node2.__model_attr__.contract = Some(DbtContract {
                enforced: true,
                checksum: None,
                ..Default::default()
            });
            // Columns in different order should produce same checksum (sorted by name)
            node2.__base_attr__.columns = vec![
                Arc::new(DbtColumn {
                    name: "name".to_string(),
                    data_type: Some("varchar".to_string()),
                    constraints: vec![],
                    ..Default::default()
                }),
                Arc::new(DbtColumn {
                    name: "id".to_string(),
                    data_type: Some("integer".to_string()),
                    constraints: vec![],
                    ..Default::default()
                }),
            ];

            assert_eq!(
                build_contract_checksum(&node1),
                build_contract_checksum(&node2)
            );
        }

        #[test]
        fn build_contract_checksum_differs_for_different_columns() {
            let mut node1 = model_node(&["project", "models", "test_model"]);
            node1.__model_attr__.contract = Some(DbtContract {
                enforced: true,
                checksum: None,
                ..Default::default()
            });
            node1.__base_attr__.columns = vec![Arc::new(DbtColumn {
                name: "id".to_string(),
                data_type: Some("integer".to_string()),
                constraints: vec![],
                ..Default::default()
            })];

            let mut node2 = model_node(&["project", "models", "test_model"]);
            node2.__model_attr__.contract = Some(DbtContract {
                enforced: true,
                checksum: None,
                ..Default::default()
            });
            node2.__base_attr__.columns = vec![Arc::new(DbtColumn {
                name: "id".to_string(),
                data_type: Some("bigint".to_string()), // Different data type
                constraints: vec![],
                ..Default::default()
            })];

            assert_ne!(
                build_contract_checksum(&node1),
                build_contract_checksum(&node2)
            );
        }

        #[test]
        fn model_node_contract_hash_no_contract_matches_not_enforced() {
            let no_contract = model_node(&["project", "models", "test_model"]);

            let mut not_enforced = model_node(&["project", "models", "test_model"]);
            not_enforced.__model_attr__.contract = Some(DbtContract {
                enforced: false,
                ..Default::default()
            });

            assert_eq!(
                model_node_contract_hash(&no_contract),
                model_node_contract_hash(&not_enforced)
            );
        }
    }

    mod node_ref_representation_hash {
        use super::*;

        #[test]
        fn model_ref_repr_hash_present_with_model_attributes() {
            let mut node = model_node(&["project", "models", "test_model"]);
            node.__model_attr__.latest_version = Some(StringOrInteger::Integer(2));
            node.__model_attr__.access = Access::Public;
            node.__model_attr__.deprecation_date = dbt_yaml::Timestamp::parse("2025-12-31");

            assert_eq!(model_node_ref_representation_hash(&node).unwrap().len(), 32);
        }

        #[test]
        fn model_ref_repr_hash_differs_across_versions() {
            let mut v1 = model_node(&["project", "models", "test_model"]);
            v1.__model_attr__.latest_version = Some(StringOrInteger::Integer(1));
            v1.__model_attr__.access = Access::Public;

            let mut v2 = model_node(&["project", "models", "test_model"]);
            v2.__model_attr__.latest_version = Some(StringOrInteger::Integer(2));
            v2.__model_attr__.access = Access::Public;

            assert_ne!(
                model_node_ref_representation_hash(&v1).unwrap(),
                model_node_ref_representation_hash(&v2).unwrap()
            );
        }

        #[test]
        fn node_state_hashes_changes_when_a_component_changes() {
            let mut node = test_node(&["project", "models", "test_model"]);
            node.__common_attr__.raw_code = Some("SELECT 1".to_string());
            node.__base_attr__.unrendered_config = BTreeMap::from([(
                "materialized".to_string(),
                YmlValue::string("table".to_string()),
            )]);
            node.__base_attr__.depends_on.macros = vec!["macro1".to_string()];

            let macros_map = BTreeMap::from([(
                "macro1".to_string(),
                DbtMacro {
                    macro_sql: "MACRO_SQL".to_string(),
                    ..Default::default()
                },
            )]);

            let base_hash = node_state_hashes(&node, &dummy_project_root(), resolver(&macros_map))
                .unwrap()
                .node_hash;

            node.__common_attr__.raw_code = Some("SELECT 2".to_string());
            let changed_hash =
                node_state_hashes(&node, &dummy_project_root(), resolver(&macros_map))
                    .unwrap()
                    .node_hash;

            assert_ne!(base_hash, changed_hash);
        }
    }

    #[test]
    fn test_node_state_hashes_always_returns_a_valid_digest() {
        let node = test_node(&["project", "models", "test_model"]);
        let macros_map: BTreeMap<String, DbtMacro> = BTreeMap::new();
        let node_hash = node_state_hashes(&node, &dummy_project_root(), resolver(&macros_map))
            .unwrap()
            .node_hash;
        assert_eq!(node_hash.len(), 32);
    }

    #[test]
    fn test_node_state_hashes_different_fqns_produce_different_hashes() {
        let node_a = test_node(&["project", "models", "model1"]);
        let node_b = test_node(&["project", "models", "model2"]);
        let macros_map: BTreeMap<String, DbtMacro> = BTreeMap::new();
        assert_ne!(
            node_state_hashes(&node_a, &dummy_project_root(), resolver(&macros_map))
                .unwrap()
                .node_hash,
            node_state_hashes(&node_b, &dummy_project_root(), resolver(&macros_map))
                .unwrap()
                .node_hash
        );
    }

    #[test]
    fn model_node_state_hashes_includes_contract_and_ref_representation() {
        let mut base = model_node(&["project", "models", "test_model"]);
        base.__common_attr__.raw_code = Some("SELECT 1".to_string());
        base.__model_attr__.latest_version = Some(StringOrInteger::Integer(2));
        base.__model_attr__.access = Access::Public;

        let macros_map: BTreeMap<String, DbtMacro> = BTreeMap::new();
        let base_hash = node_state_hashes(&base, &dummy_project_root(), resolver(&macros_map))
            .unwrap()
            .node_hash;

        let mut different_contract = base.clone();
        different_contract.__model_attr__.contract = Some(DbtContract {
            enforced: true,
            checksum: Some(YmlValue::string("abc123".to_string())),
            ..Default::default()
        });
        let different_contract_hash = node_state_hashes(
            &different_contract,
            &dummy_project_root(),
            resolver(&macros_map),
        )
        .unwrap()
        .node_hash;
        assert_ne!(base_hash, different_contract_hash);

        base.__model_attr__.latest_version = Some(StringOrInteger::Integer(3));
        let different_version_hash =
            node_state_hashes(&base, &dummy_project_root(), resolver(&macros_map))
                .unwrap()
                .node_hash;
        assert_ne!(base_hash, different_version_hash);
    }

    #[test]
    fn model_node_state_hashes_always_returns_a_valid_digest() {
        let node = model_node(&["project", "models", "test_model"]);
        let macros_map: BTreeMap<String, DbtMacro> = BTreeMap::new();
        let node_hash = node_state_hashes(&node, &dummy_project_root(), resolver(&macros_map))
            .unwrap()
            .node_hash;
        assert_eq!(node_hash.len(), 32);
    }

    #[test]
    fn model_node_state_hashes_different_fqns_produce_different_hashes() {
        let node_a = model_node(&["project", "models", "model1"]);
        let node_b = model_node(&["project", "models", "model2"]);
        let macros_map: BTreeMap<String, DbtMacro> = BTreeMap::new();
        assert_ne!(
            node_state_hashes(&node_a, &dummy_project_root(), resolver(&macros_map))
                .unwrap()
                .node_hash,
            node_state_hashes(&node_b, &dummy_project_root(), resolver(&macros_map))
                .unwrap()
                .node_hash
        );
    }

    #[test]
    fn generic_test_node_state_hashes_node_hash_ignores_raw_code_and_macros() {
        let macros_map = BTreeMap::from([(
            "macro1".to_string(),
            DbtMacro {
                macro_sql: "MACRO_SQL".to_string(),
                ..Default::default()
            },
        )]);

        let mut a = generic_test_node(&["project", "models", "test_model"]);
        a.__common_attr__.raw_code = Some("SELECT 1".to_string());
        a.__base_attr__.unrendered_config = BTreeMap::from([(
            "severity".to_string(),
            YmlValue::string("error".to_string()),
        )]);
        a.__base_attr__.depends_on.macros = vec!["macro1".to_string()];

        let mut b = generic_test_node(&["project", "models", "test_model"]);
        b.__common_attr__.raw_code = Some("SELECT 2".to_string());
        b.__base_attr__.unrendered_config = BTreeMap::from([(
            "severity".to_string(),
            YmlValue::string("error".to_string()),
        )]);
        b.__base_attr__.depends_on.macros = vec![];

        assert_eq!(
            node_state_hashes(&a, &dummy_project_root(), resolver(&macros_map))
                .unwrap()
                .node_hash,
            node_state_hashes(&b, &dummy_project_root(), resolver(&macros_map))
                .unwrap()
                .node_hash
        );
    }

    #[test]
    fn generic_test_node_state_hashes_node_hash_always_returns_a_valid_digest() {
        let node = generic_test_node(&["project", "models", "test_model"]);
        let macros_map: BTreeMap<String, DbtMacro> = BTreeMap::new();
        let node_hash = node_state_hashes(&node, &dummy_project_root(), resolver(&macros_map))
            .unwrap()
            .node_hash;
        assert_eq!(node_hash.len(), 32);
    }

    #[test]
    fn generic_test_node_state_hashes_different_configs_produce_different_hashes() {
        let mut a = generic_test_node(&["project", "models", "test1"]);
        a.__base_attr__.unrendered_config = BTreeMap::from([(
            "severity".to_string(),
            YmlValue::string("error".to_string()),
        )]);

        let mut b = generic_test_node(&["project", "models", "test1"]);
        b.__base_attr__.unrendered_config =
            BTreeMap::from([("severity".to_string(), YmlValue::string("warn".to_string()))]);

        let macros_map: BTreeMap<String, DbtMacro> = BTreeMap::new();
        assert_ne!(
            node_state_hashes(&a, &dummy_project_root(), resolver(&macros_map))
                .unwrap()
                .node_hash,
            node_state_hashes(&b, &dummy_project_root(), resolver(&macros_map))
                .unwrap()
                .node_hash
        );
    }

    #[test]
    fn generic_test_node_state_hashes_different_fqns_produce_different_hashes() {
        let a = generic_test_node(&["project", "models", "test1"]);
        let b = generic_test_node(&["project", "models", "test2"]);
        let macros_map: BTreeMap<String, DbtMacro> = BTreeMap::new();
        assert_ne!(
            node_state_hashes(&a, &dummy_project_root(), resolver(&macros_map))
                .unwrap()
                .node_hash,
            node_state_hashes(&b, &dummy_project_root(), resolver(&macros_map))
                .unwrap()
                .node_hash
        );
    }

    #[test]
    fn seed_node_state_hashes_node_hash_equals_body_hash() {
        let tmp = TempDir::new().unwrap();
        let project_root: DbtPath = tmp.path().into();
        let seed_file_path = project_root.join("test_seed.csv");
        fs::write(&seed_file_path, b"id,name\n1,Alice\n2,Bob\n").unwrap();

        let seed = DbtSeed {
            __seed_attr__: DbtSeedAttr {
                root_path: Some(project_root.to_path_buf()),
                ..Default::default()
            },
            __common_attr__: CommonAttributes {
                original_file_path: seed_file_path.get_relative_path(&project_root).unwrap(),
                fqn: vec![
                    "project".to_string(),
                    "seeds".to_string(),
                    "test_seed".to_string(),
                ],
                ..Default::default()
            },
            ..Default::default()
        };

        let macros_map: BTreeMap<String, DbtMacro> = BTreeMap::new();
        let hashes = node_state_hashes(&seed, &project_root, resolver(&macros_map)).unwrap();

        assert_eq!(hashes.node_hash, hashes.node_body_hash.unwrap());
    }
}

/// Parity tests against digests generated by the real Python implementation
/// (`node_hash_calculator.py` @ blob `54af5cb3`). These are *absolute* assertions: unlike
/// the relative tests above, they fail if the encoding drifts from Python at all.
#[cfg(test)]
mod python_parity {
    use super::*;

    #[test]
    fn hash_parts_matches_python_calculate_hash() {
        // md5("SELECT 1"), not md5("\"SELECT 1\"")
        assert_eq!(
            hash_parts(&[Some("SELECT 1")]),
            "b1698e52a0f16203489454196a0c6307"
        );
        // `None` entries are skipped, mirroring Python's `if p is not None`
        assert_eq!(
            hash_parts(&[Some("alpha"), None, Some("gamma")]),
            "e789ea650d1653e1bb3d9dd9bf7d4343"
        );
        // an empty join hashes ""
        assert_eq!(hash_parts(&[]), "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn hash_json_uses_python_dumps_separators() {
        let cfg: BTreeMap<&str, JsonValue> = BTreeMap::from([
            ("materialized", JsonValue::from("table")),
            ("enabled", JsonValue::from(true)),
        ]);
        assert_eq!(
            python_json(&cfg).unwrap(),
            r#"{"enabled": true, "materialized": "table"}"#
        );
        assert_eq!(hash_json(&cfg).unwrap(), "eba87d5f0d3687e809ad8a92d18836b0");
    }

    #[test]
    fn hash_json_escapes_non_ascii_like_ensure_ascii() {
        let cfg: BTreeMap<&str, &str> = BTreeMap::from([("desc", "café — naïve")]);
        assert_eq!(
            python_json(&cfg).unwrap(),
            r#"{"desc": "caf\u00e9 \u2014 na\u00efve"}"#
        );
        assert_eq!(hash_json(&cfg).unwrap(), "73c3bee704059be2e9c22398a0f37dbc");
    }

    #[test]
    fn hash_json_escapes_astral_chars_as_surrogate_pairs() {
        let cfg: BTreeMap<&str, &str> = BTreeMap::from([("emoji", "ok 😀")]);
        assert_eq!(
            python_json(&cfg).unwrap(),
            r#"{"emoji": "ok \ud83d\ude00"}"#
        );
        assert_eq!(hash_json(&cfg).unwrap(), "578b1abd690250b685d42106abc2a9f1");
    }

    #[test]
    fn contract_strings_match_python() {
        // f"enforced:true|checksum:{checksum}" - lowercase literal, checksum unquoted
        assert_eq!(
            hash_parts(&[Some("enforced:true|checksum:abc123")]),
            "891e7c8be2eed79961a4a84c64022605"
        );
        assert_eq!(
            hash_parts(&[Some("enforced:false")]),
            "298a88779c630d09cfb26e926f3af91b"
        );
    }

    #[test]
    fn macros_hash_concatenates_bare() {
        assert_eq!(
            hash_parts(&[Some("SQL1SQL2")]),
            "0d672178832f67ee8e6353fa7eb9a5fe"
        );
    }

    #[test]
    fn persisted_docs_shapes_match_python() {
        // Only the set key is present, and `columns` sorts before `description`.
        let cols: BTreeMap<&str, &str> = BTreeMap::from([("id", "the id"), ("name", "the name")]);
        let only_columns: BTreeMap<&str, JsonValue> =
            BTreeMap::from([("columns", serde_json::to_value(&cols).unwrap())]);
        assert_eq!(
            hash_json(&only_columns).unwrap(),
            "43a13119e7e34227d32da6168013e48d"
        );

        let only_description: BTreeMap<&str, JsonValue> =
            BTreeMap::from([("description", JsonValue::from("a model"))]);
        assert_eq!(
            hash_json(&only_description).unwrap(),
            "f540b8920872d148eaffa61ee958df95"
        );
    }

    #[test]
    fn ref_representation_defaults_match_python() {
        // `access`/`deprecation_date` go through `str()`, so an absent value is the literal
        // "None"; `latest_version` is passed through raw and stays JSON `null`.
        let parts: BTreeMap<&str, JsonValue> = BTreeMap::from([
            ("latest_version", JsonValue::Null),
            ("access", JsonValue::from("protected")),
            ("deprecation_date", JsonValue::from("None")),
        ]);
        assert_eq!(
            python_json(&parts).unwrap(),
            r#"{"access": "protected", "deprecation_date": "None", "latest_version": null}"#
        );
        assert_eq!(
            hash_json(&parts).unwrap(),
            "dd915b2a6c70d306ecf120a98e2b99ba"
        );
    }
}
