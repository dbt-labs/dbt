use dbt_common::path::DbtPath;
use dbt_schemas::schemas::{
    DbtModel, DbtSeed, DbtTest, InternalDbtNode, InternalDbtNodeAttributes,
    common::DbtMaterialization, config_excluded_keys, macros::DbtMacro,
};
use dbt_schemas::state::{DbtRuntimeConfig, DbtVars};
use md5;
use serde::Serialize;
use serde_json::{Value as JsonValue, ser::Formatter, to_string};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashSet},
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
    runtime_vars: Option<&DbtRuntimeConfig>,
) -> Result<NodeStateHashes, NodeHashError> {
    let type_erased_node = node.as_any();

    let node_body_hash = type_erased_node.downcast_ref::<DbtSeed>().map_or_else(
        || Ok(node_body_hash(node)),
        |s| seed_node_body_hash(s, project_root).map(Some),
    )?;
    let node_configs_hash = node_configs_hash(node, runtime_vars, &macro_resolver)?;
    let node_persisted_descriptions_hash = node_persisted_docs_hash(node)?;

    let node_macros_hash = node_macros_hash(node, &macro_resolver);
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

/// Reserved key under which the resolved `var()` / `env_var()` values are folded into the
/// configs hash.
const RUNTIME_VARS_KEY: &str = "__runtime_vars__";

type RuntimeVarValues = BTreeMap<&'static str, BTreeMap<String, JsonValue>>;

fn project_var_value(value: &DbtVars) -> dbt_yaml::Value {
    match value {
        DbtVars::Null => dbt_yaml::Value::null(),
        DbtVars::Value(value) => value.clone(),
        DbtVars::Vars(values) => dbt_yaml::Value::mapping(
            values
                .iter()
                .map(|(key, value)| {
                    (
                        dbt_yaml::Value::string(key.clone()),
                        project_var_value(value),
                    )
                })
                .collect(),
        ),
    }
}

// JSON collapses timestamps into strings, non-finite numbers into null, and
// non-string map keys into strings. Keep a separate typed representation for
// those values; ordinary JSON-compatible values retain their existing hash bytes.
fn requires_yaml_types(value: &dbt_yaml::Value) -> bool {
    use dbt_yaml::Value;
    match value {
        Value::Timestamp(..) | Value::Tagged(..) => true,
        Value::Number(number, _) => number.as_f64().is_some_and(|value| !value.is_finite()),
        Value::Sequence(values, _) => values.iter().any(requires_yaml_types),
        Value::Mapping(values, _) => values
            .iter()
            .any(|(key, value)| !key.is_string() || requires_yaml_types(value)),
        _ => false,
    }
}

fn typed_yaml_value(value: &dbt_yaml::Value) -> JsonValue {
    use dbt_yaml::Value;
    use serde_json::json;
    match value {
        Value::Null(_) => json!(["null"]),
        Value::Bool(value, _) => json!(["bool", value]),
        Value::String(value, _) => json!(["string", value]),
        Value::Number(value, _) => json!(["number", value.to_string()]),
        Value::Timestamp(value, _) => json!(["timestamp", value.to_string()]),
        Value::Tagged(value, _) => {
            json!(["tag", value.tag.to_string(), typed_yaml_value(&value.value)])
        }
        Value::Sequence(values, _) => json!([
            "sequence",
            values.iter().map(typed_yaml_value).collect::<Vec<_>>()
        ]),
        Value::Mapping(values, _) => {
            let mut entries = values
                .iter()
                .map(|(key, value)| (typed_yaml_value(key), typed_yaml_value(value)))
                .collect::<Vec<_>>();
            entries.sort_by_cached_key(|(key, _)| key.to_string());
            json!(["mapping", entries])
        }
    }
}

fn collect_config_strings<'a>(value: &'a dbt_yaml::Value, sources: &mut Vec<&'a str>) {
    if let Some(source) = value.as_str() {
        sources.push(source);
    } else if let Some(sequence) = value.as_sequence() {
        for item in sequence {
            collect_config_strings(item, sources);
        }
    } else if let Some(mapping) = value.as_mapping() {
        for (_, item) in mapping.iter() {
            collect_config_strings(item, sources);
        }
    }
}

/// Use the renderer's test-argument rules so nested calls inside `ref` and `source`
/// expressions are discovered as well.
fn as_template_source(source: &str) -> Cow<'_, str> {
    if dbt_jinja_utils::jinja_arg_format::is_renderable_test_arg(source) {
        Cow::Owned(format!("{{{{ {source} }}}}"))
    } else {
        Cow::Borrowed(source)
    }
}

/// Finds a package's config anywhere in the dependency tree (each package lists only its direct
/// dependencies), checking direct entries first.
fn find_package_config<'c>(
    cfg: &'c DbtRuntimeConfig,
    package: &str,
) -> Option<&'c DbtRuntimeConfig> {
    fn walk<'c>(
        cfg: &'c DbtRuntimeConfig,
        package: &str,
        visited: &mut HashSet<&'c str>,
    ) -> Option<&'c DbtRuntimeConfig> {
        if let Some(found) = cfg.dependencies.get(package) {
            return Some(found);
        }
        for (name, dep) in &cfg.dependencies {
            if visited.insert(name.as_str())
                && let Some(found) = walk(dep, package, visited)
            {
                return Some(found);
            }
        }
        None
    }
    walk(cfg, package, &mut HashSet::new()).map(|c| c as &DbtRuntimeConfig)
}

/// Parses `src` and records the literal names of its `var()` / `env_var()` calls. A name is
/// pushed onto `worklist` (as `(is_var, name)`) only the first time it is inserted into its set,
/// which also stops cycles.
#[derive(Default)]
struct RuntimeVarReferences {
    var_names: BTreeSet<String>,
    env_var_names: BTreeSet<String>,
    worklist: Vec<(bool, String)>,
    dynamic_calls: HashSet<String>,
}

impl RuntimeVarReferences {
    fn add(&mut self, is_var: bool, name: String) {
        let names = if is_var {
            &mut self.var_names
        } else {
            &mut self.env_var_names
        };
        if names.insert(name.clone()) {
            self.worklist.push((is_var, name));
        }
    }

    fn scan(&mut self, src: &str) {
        use minijinja::compiler::meta::find_runtime_value_calls;
        use minijinja::compiler::parser::parse;
        use minijinja::machinery::WhitespaceConfig;
        use minijinja::syntax::SyntaxConfig;

        let Ok(ast) = parse(
            src,
            "<node>",
            SyntaxConfig::default(),
            WhitespaceConfig::default(),
        ) else {
            return;
        };
        let calls = find_runtime_value_calls(&ast, &["var", "env_var"]);
        self.dynamic_calls.extend(calls.dynamic_calls);
        for call in calls.calls {
            let Some(name) = call.args.into_iter().next() else {
                continue;
            };
            self.add(call.name == "var", name);
        }
    }
}

trait RuntimeEnvironment {
    fn get(&self, name: &str) -> Option<String>;
    fn names(&self) -> Vec<String>;
}

struct ProcessEnvironment;
impl RuntimeEnvironment for ProcessEnvironment {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
    fn names(&self) -> Vec<String> {
        std::env::vars_os()
            .filter_map(|(name, _)| name.into_string().ok())
            .collect()
    }
}

impl RuntimeEnvironment for BTreeMap<String, String> {
    fn get(&self, name: &str) -> Option<String> {
        BTreeMap::get(self, name).cloned()
    }
    fn names(&self) -> Vec<String> {
        self.keys().cloned().collect()
    }
}

/// Statically collects the literal names of `var()` / `env_var()` calls in the node's raw code,
/// in every string value of its unrendered config (nested mappings and sequences included) and in
/// the macros it depends on (and, for generic tests, in the `test_metadata` kwargs), and resolves their values: vars from
/// the CLI vars over the node package's vars (undefined vars are omitted), env vars from the
/// process env (unset becomes `null`). Resolved string values containing Jinja are scanned
/// recursively for further calls. Returns `None` if no call is found.
fn node_runtime_vars<'a>(
    node: &'a dyn InternalDbtNode,
    cfg: &DbtRuntimeConfig,
    macro_resolver: impl Fn(&str) -> Option<&'a DbtMacro>,
    env: &impl RuntimeEnvironment,
) -> Option<RuntimeVarValues> {
    let mut sources: Vec<Cow<str>> = Vec::new();
    if let Some(raw_code) = node
        .common()
        .raw_code
        .as_deref()
        .filter(|rc| !rc.is_empty())
    {
        sources.push(Cow::Borrowed(raw_code));
    }
    let mut config_strings = Vec::new();
    for value in node.base().unrendered_config.values() {
        collect_config_strings(value, &mut config_strings);
    }
    sources.extend(config_strings.into_iter().map(Cow::Borrowed));
    // The authored arguments of a generic test live only in its kwargs, not in the synthesized raw code.
    if let Some(metadata) = node
        .as_any()
        .downcast_ref::<DbtTest>()
        .and_then(|t| t.__test_attr__.test_metadata.as_ref())
    {
        let mut kwarg_strings = Vec::new();
        for value in metadata.kwargs.values() {
            collect_config_strings(value, &mut kwarg_strings);
        }
        sources.extend(kwarg_strings.into_iter().map(as_template_source));
    }
    sources.extend(
        resolve_macro_tree(node, macro_resolver)
            .values()
            .map(|m| Cow::Borrowed(m.macro_sql.as_str())),
    );

    let mut references = RuntimeVarReferences::default();
    for src in sources {
        references.scan(&src);
    }
    if references.worklist.is_empty() && references.dynamic_calls.is_empty() {
        return None;
    }

    let pkg_vars = find_package_config(cfg, &node.common().package_name).map(|c| &c.inner.vars);
    let mut vars = BTreeMap::new();
    let mut var_types = BTreeMap::new();
    let mut env_vars = BTreeMap::new();
    let mut expanded_vars = false;
    let mut expanded_env = false;
    loop {
        // A computed name can select any visible value. Include that namespace
        // rather than silently losing its dependencies. Literal calls stay precise.
        if !expanded_vars && references.dynamic_calls.contains("var") {
            expanded_vars = true;
            for name in cfg
                .inner
                .cli_vars
                .keys()
                .chain(pkg_vars.into_iter().flat_map(|vars| vars.keys()))
            {
                references.add(true, name.clone());
            }
        }
        if !expanded_env && references.dynamic_calls.contains("env_var") {
            expanded_env = true;
            for name in env.names() {
                references.add(false, name);
            }
        }
        let Some((is_var, name)) = references.worklist.pop() else {
            break;
        };
        let mut nested: Option<String> = None;
        if is_var {
            let value = cfg
                .inner
                .cli_vars
                .get(&name)
                .map(Cow::Borrowed)
                .or_else(|| {
                    pkg_vars
                        .and_then(|vars| vars.get(&name))
                        .map(|value| Cow::Owned(project_var_value(value)))
                });
            if let Some(value) = value {
                if let Some(source) = value.as_str() {
                    nested = Some(source.to_string());
                }
                let json = serde_json::to_value(value.as_ref());
                if requires_yaml_types(&value) || json.is_err() {
                    var_types.insert(name.clone(), typed_yaml_value(&value));
                }
                vars.insert(name, json.unwrap_or(JsonValue::Null));
            }
        } else {
            let value = env.get(&name);
            nested = value.clone();
            env_vars.insert(name, value.map_or(JsonValue::Null, JsonValue::String));
        }
        if let Some(nested_source) =
            nested.filter(|s| ["{{", "{%", "{#"].iter().any(|t| s.contains(t)))
        {
            references.scan(&nested_source);
        }
    }

    let mut result = BTreeMap::from([("vars", vars), ("env_vars", env_vars)]);
    if !var_types.is_empty() {
        result.insert("var_types", var_types);
    }
    Some(result)
}

fn node_configs_hash<'a>(
    node: &'a dyn InternalDbtNode,
    runtime_vars: Option<&DbtRuntimeConfig>,
    macro_resolver: impl Fn(&str) -> Option<&'a DbtMacro>,
) -> Result<Option<String>, NodeHashError> {
    node_configs_hash_with_env(node, runtime_vars, macro_resolver, &ProcessEnvironment)
}

fn node_configs_hash_with_env<'a>(
    node: &'a dyn InternalDbtNode,
    runtime_vars: Option<&DbtRuntimeConfig>,
    macro_resolver: impl Fn(&str) -> Option<&'a DbtMacro>,
    env: &impl RuntimeEnvironment,
) -> Result<Option<String>, NodeHashError> {
    //ref: https://github.com/fivetran/query-cache/blob/4ab87c72f2783c230905c5c75ec19eb76314069e/clients/dbt_state/src/dbt_state/node_hash_calculator.py#L65
    let keys_to_exclude = config_excluded_keys(node.resource_type());

    let filtered = node
        .base()
        .unrendered_config
        .iter()
        .filter(|(name, _)| !keys_to_exclude.contains(&name.as_str()))
        .collect::<BTreeMap<_, _>>();

    let runtime = runtime_vars.and_then(|cfg| node_runtime_vars(node, cfg, macro_resolver, env));

    match runtime {
        // Without runtime vars the bytes must stay identical to the pre-existing hash.
        None if filtered.is_empty() => Ok(None),
        None => hash_json(&filtered).map(Some),
        Some(mut runtime) => {
            let mut with_runtime = filtered
                .into_iter()
                .map(|(k, v)| Ok((k.clone(), serde_json::to_value(v)?)))
                .collect::<Result<BTreeMap<String, JsonValue>, serde_json::Error>>()?;
            if let Some(config_value) = with_runtime.get(RUNTIME_VARS_KEY) {
                runtime.insert(
                    "unrendered_config",
                    BTreeMap::from([(RUNTIME_VARS_KEY.to_string(), config_value.clone())]),
                );
            }
            with_runtime.insert(RUNTIME_VARS_KEY.to_string(), serde_json::to_value(runtime)?);
            hash_json(&with_runtime).map(Some)
        }
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
            assert!(node_configs_hash(&node, None, |_| None).unwrap().is_some());
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
                node_configs_hash(&with_excluded, None, |_| None).unwrap(),
                node_configs_hash(&without_excluded, None, |_| None).unwrap()
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
                node_configs_hash(&with_excluded, None, |_| None).unwrap(),
                node_configs_hash(&without_excluded, None, |_| None).unwrap()
            );
        }

        #[test]
        fn node_configs_hash_none_when_unrendered_config_empty() {
            let node = test_node(&["project", "models", "test_model"]);
            assert_eq!(node_configs_hash(&node, None, |_| None).unwrap(), None);
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
            assert_eq!(node_configs_hash(&node, None, |_| None).unwrap(), None);
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

            let base_hash =
                node_state_hashes(&node, &dummy_project_root(), resolver(&macros_map), None)
                    .unwrap()
                    .node_hash;

            node.__common_attr__.raw_code = Some("SELECT 2".to_string());
            let changed_hash =
                node_state_hashes(&node, &dummy_project_root(), resolver(&macros_map), None)
                    .unwrap()
                    .node_hash;

            assert_ne!(base_hash, changed_hash);
        }
    }

    #[test]
    fn test_node_state_hashes_always_returns_a_valid_digest() {
        let node = test_node(&["project", "models", "test_model"]);
        let macros_map: BTreeMap<String, DbtMacro> = BTreeMap::new();
        let node_hash =
            node_state_hashes(&node, &dummy_project_root(), resolver(&macros_map), None)
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
            node_state_hashes(&node_a, &dummy_project_root(), resolver(&macros_map), None)
                .unwrap()
                .node_hash,
            node_state_hashes(&node_b, &dummy_project_root(), resolver(&macros_map), None)
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
        let base_hash =
            node_state_hashes(&base, &dummy_project_root(), resolver(&macros_map), None)
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
            None,
        )
        .unwrap()
        .node_hash;
        assert_ne!(base_hash, different_contract_hash);

        base.__model_attr__.latest_version = Some(StringOrInteger::Integer(3));
        let different_version_hash =
            node_state_hashes(&base, &dummy_project_root(), resolver(&macros_map), None)
                .unwrap()
                .node_hash;
        assert_ne!(base_hash, different_version_hash);
    }

    #[test]
    fn model_node_state_hashes_always_returns_a_valid_digest() {
        let node = model_node(&["project", "models", "test_model"]);
        let macros_map: BTreeMap<String, DbtMacro> = BTreeMap::new();
        let node_hash =
            node_state_hashes(&node, &dummy_project_root(), resolver(&macros_map), None)
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
            node_state_hashes(&node_a, &dummy_project_root(), resolver(&macros_map), None)
                .unwrap()
                .node_hash,
            node_state_hashes(&node_b, &dummy_project_root(), resolver(&macros_map), None)
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
            node_state_hashes(&a, &dummy_project_root(), resolver(&macros_map), None)
                .unwrap()
                .node_hash,
            node_state_hashes(&b, &dummy_project_root(), resolver(&macros_map), None)
                .unwrap()
                .node_hash
        );
    }

    #[test]
    fn generic_test_node_state_hashes_node_hash_always_returns_a_valid_digest() {
        let node = generic_test_node(&["project", "models", "test_model"]);
        let macros_map: BTreeMap<String, DbtMacro> = BTreeMap::new();
        let node_hash =
            node_state_hashes(&node, &dummy_project_root(), resolver(&macros_map), None)
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
            node_state_hashes(&a, &dummy_project_root(), resolver(&macros_map), None)
                .unwrap()
                .node_hash,
            node_state_hashes(&b, &dummy_project_root(), resolver(&macros_map), None)
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
            node_state_hashes(&a, &dummy_project_root(), resolver(&macros_map), None)
                .unwrap()
                .node_hash,
            node_state_hashes(&b, &dummy_project_root(), resolver(&macros_map), None)
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
        let hashes = node_state_hashes(&seed, &project_root, resolver(&macros_map), None).unwrap();

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

/// `var()` / `env_var()` values folded into the configs hash under `compare_unrendered_code`.
#[cfg(test)]
mod runtime_vars {
    use super::*;
    use dbt_schemas::schemas::macros::MacroDependsOn;
    use dbt_schemas::state::DbtVars;
    use dbt_yaml::Value as YmlValue;
    use std::sync::Arc;

    fn model(raw_code: &str) -> DbtModel {
        let mut m = DbtModel::default();
        m.__common_attr__.package_name = "pkg".to_string();
        m.__common_attr__.raw_code = Some(raw_code.to_string());
        m.__base_attr__.unrendered_config = BTreeMap::from([(
            "materialized".to_string(),
            YmlValue::string("table".to_string()),
        )]);
        m
    }

    fn config(cli: &[(&str, bool)], project: &[(&str, bool)]) -> DbtRuntimeConfig {
        let mut pkg = DbtRuntimeConfig::default();
        for (k, v) in project {
            pkg.inner
                .vars
                .insert(k.to_string(), DbtVars::Value(YmlValue::bool(*v)));
        }
        let mut cfg = DbtRuntimeConfig::default();
        for (k, v) in cli {
            cfg.inner.cli_vars.insert(k.to_string(), YmlValue::bool(*v));
        }
        cfg.dependencies.insert("pkg".to_string(), Arc::new(pkg));
        cfg
    }

    fn hash(node: &DbtModel, cfg: Option<&DbtRuntimeConfig>, env: &[(&str, &str)]) -> String {
        node_configs_hash_with_env(
            node,
            cfg,
            |_| None,
            &env.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
        )
        .unwrap()
        .unwrap()
    }

    const DRY_RUN: &str = "select {{ var('dry_run', false) }}";

    fn generic_test_with_kwarg(value: YmlValue) -> DbtTest {
        let mut t = DbtTest::default();
        t.__common_attr__.package_name = "pkg".to_string();
        t.__base_attr__.unrendered_config =
            BTreeMap::from([("severity".to_string(), YmlValue::string("warn".to_string()))]);
        t.__test_attr__.test_metadata = Some(dbt_schemas::schemas::TestMetadata {
            name: "accepted_values".to_string(),
            kwargs: BTreeMap::from([("values".to_string(), value)]),
            namespace: None,
        });
        t
    }

    fn test_hash(node: &DbtTest, cfg: Option<&DbtRuntimeConfig>, env: &[(&str, &str)]) -> String {
        node_configs_hash_with_env(
            node,
            cfg,
            |_| None,
            &env.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
        )
        .unwrap()
        .unwrap()
    }

    fn config_with_vars(project: &[(&str, &str)]) -> DbtRuntimeConfig {
        let mut pkg = DbtRuntimeConfig::default();
        for (k, v) in project {
            pkg.inner.vars.insert(
                k.to_string(),
                DbtVars::Value(YmlValue::string(v.to_string())),
            );
        }
        let mut cfg = DbtRuntimeConfig::default();
        cfg.dependencies.insert("pkg".to_string(), Arc::new(pkg));
        cfg
    }

    #[test]
    fn env_var_inside_var_value_is_folded_in() {
        let node = model("select {{ var('query') }}");
        let cfg = config_with_vars(&[("query", "{{ env_var('MODE') }}")]);
        assert_ne!(
            hash(&node, Some(&cfg), &[("MODE", "a")]),
            hash(&node, Some(&cfg), &[("MODE", "b")])
        );
        assert_eq!(
            hash(&node, None, &[("MODE", "a")]),
            hash(&node, None, &[("MODE", "b")])
        );
    }

    #[test]
    fn chained_var_values_are_followed() {
        let node = model("select {{ var('a') }}");
        let hash_with = |b: &str| {
            let cfg = config_with_vars(&[("a", "{{ var('b') }}"), ("b", b)]);
            hash(&node, Some(&cfg), &[])
        };
        assert_ne!(hash_with("x"), hash_with("y"));
    }

    #[test]
    fn cyclic_var_values_terminate() {
        let node = model("select {{ var('a') }}");
        let cfg = config_with_vars(&[("a", "{{ var('b') }}"), ("b", "{{ var('a') }}")]);
        assert_eq!(hash(&node, Some(&cfg), &[]), hash(&node, Some(&cfg), &[]));
    }

    #[test]
    fn var_inside_env_var_value_is_followed() {
        let node = model("select {{ env_var('MODE') }}");
        let hash_with = |x: &str| {
            let cfg = config_with_vars(&[("x", x)]);
            hash(&node, Some(&cfg), &[("MODE", "{{ var('x') }}")])
        };
        assert_ne!(hash_with("x1"), hash_with("x2"));
    }

    #[test]
    fn transitive_package_project_var_changes_hash() {
        let build = |value: bool| {
            let mut b = DbtRuntimeConfig::default();
            b.inner
                .vars
                .insert("dry_run".to_string(), DbtVars::Value(YmlValue::bool(value)));
            let mut a = DbtRuntimeConfig::default();
            a.dependencies.insert("b".to_string(), Arc::new(b));
            let mut cfg = DbtRuntimeConfig::default();
            cfg.dependencies.insert("a".to_string(), Arc::new(a));
            cfg
        };
        let mut node = model(DRY_RUN);
        node.__common_attr__.package_name = "b".to_string();
        let on = hash(&node, Some(&build(true)), &[]);
        assert_ne!(on, hash(&node, Some(&build(false)), &[]));
        assert_ne!(on, hash(&node, None, &[]));
    }

    #[test]
    fn generic_test_kwargs_var_forms_are_picked_up() {
        let on = config(&[("dry_run", true)], &[]);
        let off = config(&[("dry_run", false)], &[]);
        for kwarg in [
            "{{ var('dry_run') }}",
            "var('dry_run')",
            "{{ var('dry_run') }}-bar",
        ] {
            let node = generic_test_with_kwarg(YmlValue::string(kwarg.to_string()));
            assert_ne!(
                test_hash(&node, Some(&on), &[]),
                test_hash(&node, Some(&off), &[]),
                "{kwarg}"
            );
        }
        let nested = dbt_yaml::from_str::<YmlValue>(r#"["{{ var('dry_run') }}", "lit"]"#).unwrap();
        let node = generic_test_with_kwarg(nested);
        assert_ne!(
            test_hash(&node, Some(&on), &[]),
            test_hash(&node, Some(&off), &[])
        );
    }

    #[test]
    fn generic_test_kwargs_bare_env_var_is_picked_up() {
        let node = generic_test_with_kwarg(YmlValue::string("env_var('MY_ENV')".to_string()));
        let cfg = config(&[], &[]);
        assert_ne!(
            test_hash(&node, Some(&cfg), &[("MY_ENV", "1")]),
            test_hash(&node, Some(&cfg), &[("MY_ENV", "2")])
        );
    }

    #[test]
    fn generic_test_kwargs_find_vars_inside_bare_resolvers() {
        let a = config_with_vars(&[("parent", "parent_a")]);
        let b = config_with_vars(&[("parent", "parent_b")]);
        for argument in [
            "ref(var('parent'))",
            "source(var('parent'), 'table')",
            "doc(var('parent'))",
            "get_where_subquery(ref(var('parent')))",
        ] {
            let node = generic_test_with_kwarg(YmlValue::string(argument.to_string()));
            assert_ne!(
                test_hash(&node, Some(&a), &[]),
                test_hash(&node, Some(&b), &[]),
                "{argument}"
            );
        }
    }

    #[test]
    fn nested_default_strings_and_autoescape_affect_hashes() {
        let cfg = config(&[], &[]);
        for code in [
            r#"select '{{ var('query', "{{ env_var('MODE') }}") }}'"#,
            r#"select '{{ env_var('QUERY', "{{ env_var('MODE') }}") }}'"#,
        ] {
            let node = model(code);
            assert_ne!(
                hash(&node, Some(&cfg), &[("MODE", "a")]),
                hash(&node, Some(&cfg), &[("MODE", "b")]),
                "{code}"
            );
        }
        let node =
            model("{% autoescape var('escape', false) %}select '{{ '<x>' }}'{% endautoescape %}");
        assert_ne!(
            hash(&node, Some(&config(&[("escape", true)], &[])), &[]),
            hash(&node, Some(&config(&[("escape", false)], &[])), &[]),
        );
    }

    #[test]
    fn generic_test_kwargs_without_runtime_calls_keep_hash() {
        let node = generic_test_with_kwarg(YmlValue::string("plain".to_string()));
        let cfg = config(&[("dry_run", true)], &[]);
        assert_eq!(
            test_hash(&node, Some(&cfg), &[]),
            test_hash(&node, None, &[])
        );
    }

    #[test]
    fn no_var_usage_matches_hash_without_runtime_vars() {
        let cfg = config(&[], &[("unused", true)]);
        for source in [
            "select 1",
            "select '{{ context.target.name }}'",
            "select '{{ context.get('target').name }}'",
        ] {
            let node = model(source);
            assert_eq!(hash(&node, Some(&cfg), &[]), hash(&node, None, &[]));
        }
    }

    #[test]
    fn flag_off_ignores_var_values_and_flag_on_does_not() {
        let node = model(DRY_RUN);
        let on = config(&[("dry_run", true)], &[]);
        let off = config(&[("dry_run", false)], &[]);
        assert_eq!(hash(&node, None, &[]), hash(&node, None, &[]));
        assert_ne!(hash(&node, Some(&on), &[]), hash(&node, Some(&off), &[]));
        assert_ne!(hash(&node, Some(&on), &[]), hash(&node, None, &[]));
    }

    #[test]
    fn cli_and_project_var_with_same_value_hash_equal_and_unset_differs() {
        let node = model(DRY_RUN);
        let cli = config(&[("dry_run", true)], &[]);
        let project = config(&[], &[("dry_run", true)]);
        let unset = config(&[], &[]);
        assert_eq!(
            hash(&node, Some(&cli), &[]),
            hash(&node, Some(&project), &[])
        );
        assert_ne!(hash(&node, Some(&cli), &[]), hash(&node, Some(&unset), &[]));
    }

    #[test]
    fn cli_var_wins_over_project_var() {
        let node = model(DRY_RUN);
        let both = config(&[("dry_run", true)], &[("dry_run", false)]);
        let cli_only = config(&[("dry_run", true)], &[]);
        assert_eq!(
            hash(&node, Some(&both), &[]),
            hash(&node, Some(&cli_only), &[])
        );
    }

    #[test]
    fn unused_var_does_not_affect_hash() {
        let node = model(DRY_RUN);
        let a = config(&[("dry_run", true), ("other", true)], &[]);
        let b = config(&[("dry_run", true), ("other", false)], &[]);
        assert_eq!(hash(&node, Some(&a), &[]), hash(&node, Some(&b), &[]));
    }

    #[test]
    fn computed_names_include_visible_values_and_nested_dependencies() {
        for code in [
            "{% set key='dry_run' %}select {{ var(key) }}",
            "{% macro m(key) %}{{ var(key) }}{% endmacro %}{{ m('dry_run') }}",
            "{% set lookup=var %}select {{ lookup('dry_run') }}",
            "select {{ context['var']('dry_run') }}",
            "select {{ context.get('var')('dry_run') }}",
        ] {
            let node = model(code);
            assert_ne!(
                hash(&node, Some(&config(&[("dry_run", true)], &[])), &[]),
                hash(&node, Some(&config(&[("dry_run", false)], &[])), &[]),
                "{code}",
            );
            assert_ne!(
                hash(&node, Some(&config(&[], &[("dry_run", true)])), &[]),
                hash(&node, Some(&config(&[], &[])), &[]),
                "{code}",
            );
        }
        let node = model("{{ var.has_var('dry_run') }}");
        assert_ne!(
            hash(&node, Some(&config(&[("dry_run", true)], &[])), &[]),
            hash(&node, Some(&config(&[], &[])), &[])
        );
        let node = model("{% set key='MODE' %}select '{{ env_var(key) }}'");
        let cfg = config(&[], &[]);
        assert_ne!(
            hash(&node, Some(&cfg), &[("MODE", "a")]),
            hash(&node, Some(&cfg), &[("MODE", "b")])
        );
        assert_eq!(
            hash(&node, Some(&cfg), &[("MODE", "a"), ("OTHER", "b")]),
            hash(&node, Some(&cfg), &[("OTHER", "b"), ("MODE", "a")])
        );

        let cfg = config_with_vars(&[
            ("query", "{% set name='key' %}{{ var(name) }}"),
            ("key", "{{ env_var('MODE') }}"),
        ]);
        let node = model("select {{ var('query') }}");
        assert_ne!(
            hash(&node, Some(&cfg), &[("MODE", "a")]),
            hash(&node, Some(&cfg), &[("MODE", "b")])
        );
    }

    #[test]
    fn yaml_types_are_preserved_in_cli_and_project_var_hashes() {
        let node = model("select {{ var('value') }}");
        let with_yaml = |source: &str, cli: bool| {
            let value = dbt_yaml::from_str::<YmlValue>(source).unwrap();
            let mut cfg = config(&[], &[]);
            if cli {
                cfg.inner.cli_vars.insert("value".into(), value);
            } else {
                Arc::make_mut(cfg.dependencies.get_mut("pkg").unwrap())
                    .inner
                    .vars
                    .insert(
                        "value".into(),
                        dbt_yaml::from_str::<DbtVars>(source).unwrap(),
                    );
            }
            hash(&node, Some(&cfg), &[])
        };
        for (typed, plain) in [
            ("2026-08-03", "'2026-08-03'"),
            ("[2026-08-03]", "['2026-08-03']"),
            ("{date: 2026-08-03}", "{date: '2026-08-03'}"),
            (".nan", "null"),
            (".inf", "-.inf"),
            ("{1: x}", "{'1': x}"),
            ("!first x", "!second x"),
        ] {
            for cli in [false, true] {
                assert_ne!(
                    with_yaml(typed, cli),
                    with_yaml(plain, cli),
                    "{typed} vs {plain}, cli={cli}"
                );
            }
            assert_eq!(with_yaml(typed, false), with_yaml(typed, true), "{typed}");
        }
        assert_eq!(
            with_yaml("{a: 2026-08-03, b: 1}", true),
            with_yaml("{b: 1, a: 2026-08-03}", true)
        );
    }

    #[test]
    fn env_var_values_are_folded_in() {
        let node = model(r#"select '{{ env_var("MY_ENV", "x") }}'"#);
        let cfg = config(&[], &[]);
        assert_ne!(
            hash(&node, Some(&cfg), &[("MY_ENV", "1")]),
            hash(&node, Some(&cfg), &[("MY_ENV", "2")])
        );
        assert_eq!(
            hash(&node, Some(&cfg), &[("MY_ENV", "1")]),
            hash(&node, Some(&cfg), &[("MY_ENV", "1"), ("UNRELATED", "z")])
        );
        assert_ne!(
            hash(&node, Some(&cfg), &[]),
            hash(&node, Some(&cfg), &[("MY_ENV", "1")])
        );
    }

    #[test]
    fn var_in_unrendered_config_string_is_picked_up() {
        let mut node = model("select 1");
        node.__base_attr__.unrendered_config.insert(
            "schema".to_string(),
            YmlValue::string("{{ var('dry_run') }}".to_string()),
        );
        let on = config(&[("dry_run", true)], &[]);
        let off = config(&[("dry_run", false)], &[]);
        assert_ne!(hash(&node, Some(&on), &[]), hash(&node, Some(&off), &[]));
    }

    #[test]
    fn var_and_env_var_in_nested_config_values_are_picked_up() {
        let nested = r#"
            meta:
              owner: "{{ var('dry_run') }}"
              tags:
                - plain
                - "{{ env_var('MY_ENV') }}"
        "#;
        let mut node = model("select 1");
        node.__base_attr__.unrendered_config.insert(
            "post_hook".to_string(),
            dbt_yaml::from_str::<YmlValue>(nested).unwrap(),
        );
        let on = config(&[("dry_run", true)], &[]);
        let off = config(&[("dry_run", false)], &[]);

        // A var nested in a mapping value.
        assert_ne!(hash(&node, Some(&on), &[]), hash(&node, Some(&off), &[]));
        // An env_var nested in a sequence inside a mapping.
        assert_ne!(
            hash(&node, Some(&on), &[("MY_ENV", "1")]),
            hash(&node, Some(&on), &[("MY_ENV", "2")])
        );
        // Flag off: nested usages do not affect the hash.
        assert_eq!(hash(&node, None, &[]), hash(&node, None, &[]));
    }

    #[test]
    fn unparseable_sources_without_config_give_none() {
        let cfg = config(&[("dry_run", true)], &[]);
        let mut node = model("select {{ var(");
        node.__base_attr__.unrendered_config.clear();
        assert_eq!(
            node_configs_hash(&node, Some(&cfg), |_| None).unwrap(),
            None
        );
    }

    #[test]
    fn runtime_metadata_does_not_hide_an_authored_config_key() {
        let mut node = model(DRY_RUN);
        let cfg = config(&[("dry_run", true)], &[]);
        node.__base_attr__
            .unrendered_config
            .insert(RUNTIME_VARS_KEY.into(), YmlValue::string("first".into()));
        let first = hash(&node, Some(&cfg), &[]);
        node.__base_attr__
            .unrendered_config
            .insert(RUNTIME_VARS_KEY.into(), YmlValue::string("second".into()));
        assert_ne!(first, hash(&node, Some(&cfg), &[]));
    }

    #[test]
    fn env_var_in_transitive_dependent_macro_is_folded_in() {
        let mut node = model("select {{ outer_macro() }}");
        node.__base_attr__.depends_on.macros = vec!["macro.pkg.outer_macro".to_string()];
        let macros = BTreeMap::from([
            (
                "macro.pkg.outer_macro".to_string(),
                DbtMacro {
                    macro_sql: "{% macro outer_macro() %}{{ inner_macro() }}{% endmacro %}"
                        .to_string(),
                    depends_on: MacroDependsOn {
                        macros: vec!["macro.pkg.inner_macro".to_string()],
                    },
                    ..Default::default()
                },
            ),
            (
                "macro.pkg.inner_macro".to_string(),
                DbtMacro {
                    macro_sql: "{% macro inner_macro() %}{{ env_var('MODE') }}{% endmacro %}"
                        .to_string(),
                    ..Default::default()
                },
            ),
        ]);
        let cfg = config(&[], &[]);
        let hash_with = |cfg: Option<&DbtRuntimeConfig>, mode: &str| {
            node_configs_hash_with_env(
                &node,
                cfg,
                |id| macros.get(id),
                &BTreeMap::from([("MODE".to_string(), mode.to_string())]),
            )
            .unwrap()
            .unwrap()
        };

        assert_ne!(hash_with(Some(&cfg), "a"), hash_with(Some(&cfg), "b"));
        // Flag off: the hash is unaffected by the env value.
        assert_eq!(hash_with(None, "a"), hash_with(None, "b"));
    }

    #[test]
    fn node_hash_changes_with_runtime_vars() {
        let node = model(DRY_RUN);
        let macros: BTreeMap<String, DbtMacro> = BTreeMap::new();
        let node_hash = |cfg: Option<&DbtRuntimeConfig>| {
            node_state_hashes(&node, &"hash-tests".into(), |id| macros.get(id), cfg)
                .unwrap()
                .node_hash
        };
        let on = config(&[("dry_run", true)], &[]);
        let off = config(&[("dry_run", false)], &[]);
        assert_ne!(node_hash(Some(&on)), node_hash(Some(&off)));
        assert_eq!(node_hash(None), node_hash(None));
    }

    /// Absolute digest computed with
    /// `json.dumps({"materialized": "table", "__runtime_vars__": {"vars": {"dry_run": True},
    /// "env_vars": {}}}, sort_keys=True)`.
    #[test]
    fn matches_python_digest() {
        let node = model(DRY_RUN);
        let cfg = config(&[("dry_run", true)], &[]);
        assert_eq!(
            hash(&node, Some(&cfg), &[]),
            "eb8e0eeb22a136d2d6297170941f1fe0"
        );
    }
}
