//! The properties files of one package, read once before any node resolves, and the index of
//! what they define.
//!
//! A properties file is a list of items under each of its top-level keys. Each kind of node owns
//! some keys ([`ResolvableNode::properties_keys`]) and says which names an item there defines
//! ([`ResolvableNode::index_item`]). The index maps each name to where it is defined. It holds no
//! YAML: a kind gets the YAML of its entries from the files, by location, when it resolves.

use crate::args::ResolveArgs;
use crate::resolve::resolvable_node::ResolvableNode;
use crate::resolve::resolve_models::properties::VersionInfo;
use dbt_common::cancellation::CancellationToken;
use dbt_common::io_utils::try_read_yml_to_str;
use dbt_common::tracing::dbt_emit::{emit_strict_parse_error, emit_warn_log_message};
use dbt_common::tracing::span_info::SpanStatusRecorder as _;
use dbt_common::{ErrorCode, FsError, FsResult, create_debug_span, fs_err};
use dbt_flags::FlagDef;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::serde::{
    from_yaml_raw, into_typed_with_error, into_typed_with_jinja, unused_key_error,
};
use dbt_jinja_utils::utils::dependency_package_name_from_ctx;
use dbt_schemas::schemas::serde::{FloatOrString, strip_one_trailing_newline_at_keys};
use dbt_schemas::state::DbtPackage;
use dbt_telemetry::AssetParsed;
use dbt_yaml::{Mapping, Span, Value};
use itertools::Itertools;
use minijinja::Value as MinijinjaValue;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::Instrument;

/// One properties file, as its top-level keys.
#[derive(Debug, Default)]
pub(crate) struct PropertiesFile {
    /// The file, relative to its package's root.
    path: PathBuf,
    /// The items under each top-level key that holds a list. A key without a value has none.
    items: BTreeMap<String, Vec<Value>>,
    /// The top-level keys of the file that are ignored: see [`PropertiesFile::read`].
    ignored_keys: Vec<String>,
}

impl PropertiesFile {
    /// Reads the properties file `input`. An empty file has no keys.
    ///
    /// `list_keys` are the top-level keys that dbt reads a list of items from. One that holds
    /// anything else is an error, and the file is not read. A key that dbt doesn't read is
    /// reported as unused, and the rest of the file is still read. A key in `ignored_keys` is
    /// neither read nor reported here: the file only keeps that it has it.
    pub(crate) fn read(
        input: &str,
        path: &Path,
        absolute_path: &Path,
        dependency_package_name: Option<&str>,
        list_keys: &[&str],
        ignored_keys: &[&str],
    ) -> FsResult<Self> {
        let mut file = PropertiesFile {
            path: path.to_path_buf(),
            ..Default::default()
        };
        let mut root =
            from_yaml_raw::<Value>(input, Some(absolute_path), true, dependency_package_name)?;
        // A tag on the file's mapping (`!tag` before the first key) is not read.
        while let Value::Tagged(tagged, ..) = root {
            root = tagged.value;
        }
        let keys = match root {
            Value::Null(..) => return Ok(file),
            Value::Mapping(keys, ..) => keys,
            value => return Err(not_a_properties_file(value)),
        };
        let mut unused = Vec::new();
        for (key, value) in keys {
            match key.as_str() {
                Some("version") => {
                    typed_key::<Option<FloatOrString>>(key, value)?;
                }
                Some("anchors") => {}
                Some(name) if ignored_keys.contains(&name) => {
                    file.ignored_keys.push(name.to_string());
                }
                Some(name) if list_keys.contains(&name) => {
                    let name = name.to_string();
                    let items = match value {
                        Value::Null(..) => None,
                        Value::Sequence(items, ..) => Some(items),
                        // A tagged list, or what is not a list.
                        value => typed_key(key, value)?,
                    };
                    if let Some(items) = items {
                        if let Some(item) = items.iter().find(|item| !is_entry(item)) {
                            return Err(not_an_entry(&name, item));
                        }
                        file.items.insert(name, items);
                    }
                }
                Some(_) => unused.push(key),
                // A key that is not a string is an error.
                None => {
                    let mut entry = Mapping::new();
                    entry.insert(key, value);
                    return Err(not_a_properties_file(Value::mapping(entry)));
                }
            }
        }
        for key in unused {
            let path = key.as_str().unwrap_or_default();
            emit_strict_parse_error(*unused_key_error(&key, path), dependency_package_name);
        }
        Ok(file)
    }

    /// Whether the file has a list under `key`, even an empty one.
    pub(crate) fn has_items(&self, key: &str) -> bool {
        self.items.contains_key(key)
    }

    /// Takes the item at `location` out of the file, leaving null in its place.
    fn take_item(&mut self, location: &PropertiesLocation) -> Value {
        self.items
            .get_mut(location.key)
            .and_then(|items| items.get_mut(location.item))
            .map(std::mem::take)
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct PropertiesEntry {
    pub name: String,
    pub name_span: Span,
    pub relative_path: PathBuf,
    pub schema_value: Value,
    pub table_value: Option<Value>,
    pub version_info: Option<VersionInfo>,
    pub duplicate_paths: Vec<PathBuf>,
}

/// Where an item is in a package's properties files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PropertiesLocation {
    /// The file, by its position among the package's properties files, in the order they are
    /// read.
    pub(crate) file: usize,
    /// The top-level key the item is under.
    pub(crate) key: &'static str,
    /// The item's position in the list under the key.
    pub(crate) item: usize,
    /// The position of the name among the names that the item defines: the table of a source,
    /// the version of a model.
    pub(crate) part: usize,
}

/// A name found under a key, with the item that is its entry.
#[derive(Debug)]
pub(crate) struct IndexedEntry {
    pub(crate) name: String,
    /// Where the name is written, for a kind that gives its nodes that location.
    pub(crate) name_span: Span,
    /// The item that is the name's entry.
    pub(crate) location: PropertiesLocation,
    /// The files of the other items that define the name, in the order they were read.
    pub(crate) duplicate_paths: Vec<PathBuf>,
}

/// A name that an item of a properties file defines, as its kind indexes it.
#[derive(Debug)]
pub struct IndexedName {
    /// The key the name is found under: the item's top-level key, unless its kind reads the
    /// items of several keys as one set of entries.
    pub key: &'static str,
    /// The name.
    pub name: String,
    /// Where the name is written in its file.
    pub name_span: Span,
}

/// Where one name is defined.
#[derive(Debug)]
struct Definitions {
    name_span: Span,
    /// The items that define the name, in the order they were read. There is at least one.
    locations: Vec<PropertiesLocation>,
}

/// Where each name that a package's properties files define is defined, by the key the name is
/// found under and then by the name.
#[derive(Debug, Default)]
pub(crate) struct PropertiesIndex {
    entries: BTreeMap<&'static str, BTreeMap<String, Definitions>>,
}

impl PropertiesIndex {
    /// Records that the item at `location` defines `name`.
    fn insert(&mut self, name: IndexedName, location: PropertiesLocation) {
        self.entries
            .entry(name.key)
            .or_default()
            .entry(name.name)
            .or_insert_with(|| Definitions {
                name_span: name.name_span,
                locations: Vec::new(),
            })
            .locations
            .push(location);
    }
}

/// What indexing the items of one properties file reads.
#[derive(Clone, Copy)]
pub struct IndexCtx<'a> {
    pub(crate) jinja_env: &'a Arc<JinjaEnv>,
    /// The Jinja context of the package's properties files.
    pub(crate) base_ctx: &'a Arc<BTreeMap<String, MinijinjaValue>>,
    /// The package of the file, unless it is the root package.
    pub(crate) dependency_package_name: Option<&'a str>,
    /// The file, relative to its package's root.
    pub(crate) properties_path: &'a Path,
}

impl<'a> IndexCtx<'a> {
    /// What indexing the properties files of one package reads. [`PackageProperties::index_file`]
    /// names the file.
    pub(crate) fn new(
        jinja_env: &'a Arc<JinjaEnv>,
        base_ctx: &'a Arc<BTreeMap<String, MinijinjaValue>>,
    ) -> Self {
        IndexCtx {
            jinja_env,
            base_ctx,
            dependency_package_name: dependency_package_name_from_ctx(jinja_env, base_ctx),
            properties_path: Path::new(""),
        }
    }

    /// The one name that `item` defines, found under `key`: its `name`, checked for forbidden
    /// characters.
    pub fn named(&self, key: &'static str, item: &Value) -> FsResult<Vec<IndexedName>> {
        let (name, name_span) = self.name(item)?;
        Ok(vec![IndexedName {
            key,
            name: validate_resource_name(&name)?,
            name_span,
        }])
    }

    /// The `name` of `item`, with Jinja rendered, and where it is written. Only the name is
    /// read and rendered: nothing else of the item is typed or copied.
    pub fn name(&self, item: &Value) -> FsResult<(String, Span)> {
        let name = name_value(item)?;
        let name_span = name.span().clone();
        let name = render_name(
            name,
            self.jinja_env,
            self.base_ctx,
            self.dependency_package_name,
        )?;
        Ok((name, name_span))
    }

    /// `item` as a `T`, with Jinja rendered.
    pub(crate) fn typed<T: DeserializeOwned>(&self, item: &Value) -> FsResult<T> {
        into_typed_with_jinja(
            name_stripped(item),
            false,
            self.jinja_env,
            self.base_ctx.as_ref(),
            &[],
            self.dependency_package_name,
            true,
        )
    }
}

/// `item` as a `T`, with Jinja rendered, without reporting what [`IndexCtx::typed`] reported when
/// the item was indexed.
pub(crate) fn typed_again<T: DeserializeOwned>(
    item: &Value,
    jinja_env: &JinjaEnv,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
) -> FsResult<T> {
    into_typed_with_jinja(
        name_stripped(item),
        false,
        jinja_env,
        base_ctx,
        &[],
        dependency_package_name_from_ctx(jinja_env, base_ctx),
        false,
    )
}

/// The value of `name` in `item`, with its trailing newline dropped, before Jinja runs:
/// dbt-core strips the template source, so an expression's own newline survives.
pub(crate) fn name_value(item: &Value) -> FsResult<Value> {
    let Some(name) = item.get("name") else {
        return Err(fs_err!(
            code => ErrorCode::SerializationError,
            loc => item.span().clone(),
            "YAML error: missing field `name`"
        ));
    };
    Ok(
        match name.as_str().and_then(|name| name.strip_suffix('\n')) {
            Some(stripped) => Value::string(stripped.to_string()).with_span(name.span().clone()),
            None => name.clone(),
        },
    )
}

/// `name`, the value of an item's `name`, as a string, with Jinja rendered.
pub(crate) fn render_name(
    name: Value,
    jinja_env: &JinjaEnv,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
    dependency_package_name: Option<&str>,
) -> FsResult<String> {
    into_typed_with_jinja(
        name,
        false,
        jinja_env,
        base_ctx,
        &[],
        dependency_package_name,
        true,
    )
}

/// Clone of a schema-yaml entry with `name`'s trailing newline dropped, before Jinja runs.
pub(crate) fn name_stripped(value: &Value) -> Value {
    let mut value = value.clone();
    strip_one_trailing_newline_at_keys(&mut value, &["name"]);
    value
}

pub(crate) fn validate_resource_name(name: &str) -> FsResult<String> {
    // Check for the space character for now. This can be extended anytime we deprecate
    // more of special characters like !@#%$":'
    if name.chars().any(|c| matches!(c, ' ')) {
        let err = fs_err!(
            ErrorCode::DbtYamlValidationError,
            "Resource name '{}' contains forbidden characters",
            name
        );
        Err(err)
    } else {
        Ok(name.to_string())
    }
}

/// The properties files of one package, and the index of what they define.
#[derive(Debug, Default)]
pub(crate) struct PackageProperties {
    /// The files, in the order they were read.
    files: Vec<PropertiesFile>,
    index: PropertiesIndex,
}

impl PackageProperties {
    /// Adds `file`, and indexes the items under the keys of each of `kinds`, kind by kind. The
    /// first item that a kind cannot index is the error.
    pub(crate) async fn index_file(
        &mut self,
        kinds: &[&dyn ResolvableNode],
        file: PropertiesFile,
        ctx: &IndexCtx<'_>,
    ) -> FsResult<()> {
        let file_id = self.files.len();
        self.files.push(file);
        let file = &self.files[file_id];
        let ctx = &IndexCtx {
            properties_path: &file.path,
            ..*ctx
        };
        for kind in kinds {
            for &key in kind.properties_keys() {
                let Some(items) = file.items.get(key) else {
                    continue;
                };
                for (item_id, item) in items.iter().enumerate() {
                    let names = kind.index_item(key, item, ctx).await?;
                    for (part, name) in names.into_iter().enumerate() {
                        let location = PropertiesLocation {
                            file: file_id,
                            key,
                            item: item_id,
                            part,
                        };
                        self.index.insert(name, location);
                    }
                }
            }
        }
        Ok(())
    }

    /// Takes the names found under `key`, in the order of the names. The first item that defines
    /// a name is its entry. The files of the others are its duplicates.
    ///
    /// A kind whose entries are more than an item each builds them from these, with
    /// [`Self::item`] and [`Self::take_item`].
    pub(crate) fn take_indexed(&mut self, key: &'static str) -> Vec<IndexedEntry> {
        self.take_indexed_at(key, |locations| locations.split_first())
    }

    /// Takes the entries found under `key`, by name, with their YAML. The first item that
    /// defines a name is its entry. The files of the others are the entry's duplicates.
    pub(crate) fn take_entries(&mut self, key: &'static str) -> BTreeMap<String, PropertiesEntry> {
        let indexed = self.take_indexed(key);
        self.entries(indexed)
    }

    /// Takes the entries found under `key`, by name, with their YAML. The last item that defines
    /// a name is its entry, and it has no duplicates.
    pub(crate) fn take_last_entries(
        &mut self,
        key: &'static str,
    ) -> BTreeMap<String, PropertiesEntry> {
        let indexed = self.take_indexed_at(key, |locations| {
            locations.last().map(|last| (last, Default::default()))
        });
        self.entries(indexed)
    }

    /// Whether a file of the package has a list under the top-level key `key`, even an empty
    /// one.
    pub(crate) fn has_key(&self, key: &str) -> bool {
        self.files.iter().any(|file| file.has_items(key))
    }

    /// The file of the item at `location`, relative to its package's root.
    pub(crate) fn path(&self, location: &PropertiesLocation) -> &Path {
        &self.files[location.file].path
    }

    /// The item at `location`. Null once it is taken.
    pub(crate) fn item(&self, location: &PropertiesLocation) -> &Value {
        static NULL: Value = Value::null();
        self.files[location.file]
            .items
            .get(location.key)
            .and_then(|items| items.get(location.item))
            .unwrap_or(&NULL)
    }

    /// The item at `location`, to change in place.
    ///
    /// # Panics
    /// If no item is at `location`. A location from the index has one.
    pub(crate) fn item_mut(&mut self, location: &PropertiesLocation) -> &mut Value {
        self.files[location.file]
            .items
            .get_mut(location.key)
            .and_then(|items| items.get_mut(location.item))
            .expect("an indexed location has an item")
    }

    /// Takes the item at `location` out of its file, leaving null in its place.
    pub(crate) fn take_item(&mut self, location: &PropertiesLocation) -> Value {
        self.files[location.file].take_item(location)
    }

    /// Takes the names found under `key`. `split` gives the item that is a name's entry and the
    /// items that are its duplicates.
    fn take_indexed_at(
        &mut self,
        key: &'static str,
        split: impl Fn(&[PropertiesLocation]) -> Option<(&PropertiesLocation, &[PropertiesLocation])>,
    ) -> Vec<IndexedEntry> {
        let files = &self.files;
        self.index
            .entries
            .remove(key)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(name, definitions)| {
                let (entry, duplicates) = split(&definitions.locations)?;
                Some(IndexedEntry {
                    name,
                    name_span: definitions.name_span,
                    location: *entry,
                    duplicate_paths: duplicates
                        .iter()
                        .map(|duplicate| files[duplicate.file].path.clone())
                        .collect(),
                })
            })
            .collect()
    }

    /// The entries of `indexed`, by name: each is its item, taken out of its file.
    fn entries(&mut self, indexed: Vec<IndexedEntry>) -> BTreeMap<String, PropertiesEntry> {
        indexed
            .into_iter()
            .map(|indexed| {
                let entry = PropertiesEntry {
                    name: indexed.name.clone(),
                    name_span: indexed.name_span,
                    relative_path: self.path(&indexed.location).to_path_buf(),
                    schema_value: self.take_item(&indexed.location),
                    table_value: None,
                    version_info: None,
                    duplicate_paths: indexed.duplicate_paths,
                };
                (indexed.name, entry)
            })
            .collect()
    }
}

/// Reads the properties files of `package`, and indexes the items under the keys of each of
/// `kinds`, kind by kind.
///
/// A file that cannot be read is reported, and the others are still read. An item that its kind
/// cannot index is the error.
///
/// `off_keys` are the top-level keys of the kinds whose feature flag is off, with the flag. What
/// is under them is ignored, with one warning for each key the package has.
pub(crate) async fn index_properties(
    kinds: &[&dyn ResolvableNode],
    arg: &ResolveArgs,
    package: &DbtPackage,
    root_package_name: &str,
    jinja_env: &Arc<JinjaEnv>,
    base_ctx: &Arc<BTreeMap<String, MinijinjaValue>>,
    token: &CancellationToken,
    off_keys: &BTreeMap<&'static str, &'static FlagDef>,
) -> FsResult<PackageProperties> {
    let mut properties = PackageProperties::default();
    let list_keys: Vec<&str> = kinds
        .iter()
        .flat_map(|kind| kind.properties_keys())
        .copied()
        .collect();
    let index_cx = IndexCtx::new(jinja_env, base_ctx);

    let ignored_keys: Vec<&str> = off_keys.keys().copied().collect();
    // The keys of kinds that are off, in any of the package's files.
    let mut ignored_keys_found: BTreeSet<String> = BTreeSet::new();

    for dbt_asset in package.dbt_properties.iter().dedup() {
        token.check_cancellation()?;
        let absolute_path = dbt_asset.base_path.join(&dbt_asset.path);
        let display_path = dbt_asset.to_display_path(&arg.io.in_dir);
        let asset_name = dbt_asset
            .path
            .file_stem()
            .expect("File name can't be empty")
            .to_string_lossy();
        let span = create_debug_span(AssetParsed::new_with_phase_from_context(
            package.dbt_project.name.clone(),
            asset_name.to_string(),
            dbt_asset.path.display().to_string(),
            display_path.display().to_string(),
            None,
        ));

        let dependency_package_name = if package.dbt_project.name != root_package_name {
            Some(package.dbt_project.name.as_str())
        } else {
            None
        };

        async {
            let input = try_read_yml_to_str(&absolute_path)?;

            let result = match PropertiesFile::read(
                &input,
                &dbt_asset.path,
                &absolute_path,
                dependency_package_name,
                &list_keys,
                &ignored_keys,
            ) {
                Ok(mut file) => {
                    ignored_keys_found.extend(std::mem::take(&mut file.ignored_keys));
                    properties.index_file(kinds, file, &index_cx).await?;
                    Ok(())
                }
                Err(e) => Err(e),
            };

            // Record both success and failure statuses to the span, but continue processing
            // regardless of outcome.
            let _ = result.as_ref().record_status(&span);
            if let Err(e) = result {
                emit_strict_parse_error(*e, dependency_package_name);
            }
            Ok::<(), Box<FsError>>(())
        }
        .instrument(span.clone())
        .await?;
    }
    for key in ignored_keys_found {
        if let Some(flag) = off_keys.get(key.as_str()) {
            emit_warn_log_message(
                ErrorCode::ExperimentalNotEnabled,
                format!(
                    "Ignoring `{key}` in package '{}': the `{}` feature flag is off",
                    package.dbt_project.name, flag.name
                ),
            );
        }
    }
    Ok(properties)
}

/// The value of the top-level key `key` as a `T`. The error names the key, as typing the whole
/// file does.
fn typed_key<T: DeserializeOwned>(key: Value, value: Value) -> FsResult<T> {
    let mut entry = Mapping::new();
    entry.insert(key, value);
    let typed: BTreeMap<String, T> =
        into_typed_with_error(Value::mapping(entry), false, None, None)?;
    Ok(typed
        .into_values()
        .next()
        .expect("a mapping with one key is typed as one value"))
}

/// Whether `item`, in the list of a top-level key, can be an entry: a mapping.
///
/// A string is not an entry, although Jinja can render one expression to a mapping. dbt Core
/// checks each entry before it renders anything. A string can still give the value of a key in
/// an entry, such as `columns: "{{ var('columns') }}"`.
fn is_entry(item: &Value) -> bool {
    matches!(item, Value::Mapping(..) | Value::Tagged(..))
}

/// The error for `item`, in the list of the top-level key `key`, which is not a mapping.
fn not_an_entry(key: &str, item: &Value) -> Box<FsError> {
    let found = match item {
        Value::Null(..) => "is empty",
        Value::Bool(..) => "is a boolean",
        Value::Number(..) => "is a number",
        Value::String(..) => "is a string",
        Value::Sequence(..) => "is a list",
        Value::Mapping(..) | Value::Tagged(..) => "is not one",
    };
    let hint = match item.as_str() {
        Some(text) if text.contains("{{") || text.contains("{%") => {
            " Jinja can give the value of a key in an entry, but not a whole entry."
        }
        _ => "",
    };
    fs_err!(
        code => ErrorCode::SerializationError,
        loc => item.span().clone(),
        "YAML error: {key}: each entry must be a mapping, but this entry {found}.{hint}"
    )
}

/// The error for `value`, which is not the mapping with string keys that a properties file is.
fn not_a_properties_file(value: Value) -> Box<FsError> {
    /// The name is part of the error's message.
    #[derive(Deserialize)]
    struct DbtPropertiesFileValues {}

    match into_typed_with_error::<DbtPropertiesFileValues>(value, false, None, None) {
        Err(error) => error,
        Ok(_) => fs_err!(
            ErrorCode::SerializationError,
            "YAML error: a properties file must be a mapping with string keys"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(input: &str) -> FsResult<PropertiesFile> {
        PropertiesFile::read(
            input,
            Path::new("models/schema.yml"),
            Path::new("/project/models/schema.yml"),
            None,
            &["models", "seeds"],
            &["evals"],
        )
    }

    fn error(input: &str) -> String {
        read(input).expect_err("the file is an error").context
    }

    /// A kind that owns `widgets` and `gadgets`, and finds the names of both under `widgets`.
    struct Widgets;

    #[async_trait::async_trait]
    impl ResolvableNode for Widgets {
        fn properties_keys(&self) -> &'static [&'static str] {
            &["widgets", "gadgets"]
        }

        async fn index_item(
            &self,
            _key: &'static str,
            item: &Value,
            _ctx: &IndexCtx<'_>,
        ) -> FsResult<Vec<IndexedName>> {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| fs_err!(ErrorCode::InvalidConfig, "a widget without a name"))?;
            Ok(vec![IndexedName {
                key: "widgets",
                name: name.to_string(),
                name_span: item.span().clone(),
            }])
        }

        async fn resolve(
            &self,
            _ctx: &mut crate::resolve::resolvable_node::ResolveCtx<'_>,
        ) -> FsResult<()> {
            Ok(())
        }
    }

    /// The properties of a package with `files`, each a path and its content, indexed by
    /// [`Widgets`].
    async fn index(files: &[(&str, &str)]) -> FsResult<PackageProperties> {
        let jinja_env = Arc::new(JinjaEnv::new(minijinja::Environment::new()));
        let base_ctx = Arc::new(BTreeMap::new());
        let ctx = IndexCtx::new(&jinja_env, &base_ctx);
        let mut properties = PackageProperties::default();
        for (path, input) in files {
            let file = PropertiesFile::read(
                input,
                Path::new(path),
                &Path::new("/project").join(path),
                None,
                &["widgets", "gadgets"],
                &[],
            )?;
            properties.index_file(&[&Widgets], file, &ctx).await?;
        }
        Ok(properties)
    }

    #[dbt_runtime::test]
    async fn an_entry_is_the_first_item_that_defines_its_name() -> FsResult<()> {
        let mut properties = index(&[
            (
                "a.yml",
                "widgets:\n  - name: one\n    size: 1\n  - name: two\n",
            ),
            ("b.yml", "widgets:\n  - name: one\n    size: 2\n"),
            ("c.yml", "gadgets:\n  - name: one\n    size: 3\n"),
        ])
        .await?;
        let entries = properties.take_entries("widgets");
        assert_eq!(entries.keys().collect::<Vec<_>>(), ["one", "two"]);
        let one = &entries["one"];
        assert_eq!(one.name, "one");
        assert_eq!(one.relative_path, Path::new("a.yml"));
        assert_eq!(
            one.schema_value.get("size").and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            one.duplicate_paths,
            [PathBuf::from("b.yml"), PathBuf::from("c.yml")]
        );
        assert!(one.name_span.is_valid());
        assert!(entries["two"].duplicate_paths.is_empty());
        // The entries are taken once.
        assert!(properties.take_entries("widgets").is_empty());
        Ok(())
    }

    #[dbt_runtime::test]
    async fn the_last_item_that_defines_a_name_can_be_its_entry() -> FsResult<()> {
        let mut properties = index(&[
            ("a.yml", "widgets:\n  - name: one\n    size: 1\n"),
            ("b.yml", "gadgets:\n  - name: one\n    size: 2\n"),
        ])
        .await?;
        let entries = properties.take_last_entries("widgets");
        let one = &entries["one"];
        assert_eq!(one.relative_path, Path::new("b.yml"));
        assert_eq!(
            one.schema_value.get("size").and_then(Value::as_u64),
            Some(2)
        );
        assert!(one.duplicate_paths.is_empty());
        Ok(())
    }

    #[dbt_runtime::test]
    async fn the_items_of_a_file_are_indexed_key_by_key() -> FsResult<()> {
        // `gadgets` is written first, but `widgets` is the kind's first key.
        let mut properties = index(&[(
            "a.yml",
            "gadgets:\n  - name: one\n    size: 2\nwidgets:\n  - name: one\n    size: 1\n",
        )])
        .await?;
        let entries = properties.take_entries("widgets");
        assert_eq!(
            entries["one"]
                .schema_value
                .get("size")
                .and_then(Value::as_u64),
            Some(1)
        );
        Ok(())
    }

    #[dbt_runtime::test]
    async fn an_item_that_its_kind_cannot_index_is_an_error() {
        let error = index(&[("a.yml", "widgets:\n  - size: 1\n")])
            .await
            .expect_err("the item has no name");
        assert_eq!(error.context, "a widget without a name");
    }

    #[dbt_runtime::test]
    async fn a_key_that_no_kind_owns_has_no_entries() -> FsResult<()> {
        let mut properties = index(&[("a.yml", "widgets:\n  - name: one\n")]).await?;
        assert!(properties.take_entries("gadgets").is_empty());
        assert!(properties.take_entries("models").is_empty());
        Ok(())
    }

    /// The `name` of the YAML `item`, as [`IndexCtx::name`] gives it.
    fn name_of(item: &str) -> FsResult<(String, Span)> {
        let jinja_env = Arc::new(JinjaEnv::new(minijinja::Environment::new()));
        let base_ctx = Arc::new(BTreeMap::new());
        let item: Value = dbt_yaml::from_str(item).unwrap();
        IndexCtx::new(&jinja_env, &base_ctx).name(&item)
    }

    #[dbt_runtime::test]
    async fn only_the_name_of_an_item_is_read() -> FsResult<()> {
        // What else the item has is not typed or rendered: `versions` is not a list here, and
        // `description` is not valid Jinja.
        let (name, span) = name_of("description: \"{{ nope( }}\"\nversions: 3\nname: orders\n")?;
        assert_eq!(name, "orders");
        // Where the name is written: line 3, after `name: `.
        assert_eq!((span.start.line, span.start.column), (3, 7));
        Ok(())
    }

    #[dbt_runtime::test]
    async fn the_name_of_an_item_is_rendered() -> FsResult<()> {
        assert_eq!(name_of("name: \"{{ 'ord' ~ 'ers' }}\"\n")?.0, "orders");
        // A block scalar's own trailing newline is dropped before Jinja runs.
        assert_eq!(name_of("name: |\n  orders\n")?.0, "orders");
        Ok(())
    }

    #[dbt_runtime::test]
    async fn an_item_without_a_name_is_an_error() {
        let error = name_of("description: no name\n").expect_err("the item has no name");
        assert_eq!(error.code, ErrorCode::SerializationError);
        assert_eq!(error.context, "YAML error: missing field `name`");
        // What is not a mapping has no name either.
        assert!(name_of("orders\n").is_err());
    }

    #[dbt_runtime::test]
    async fn a_package_has_a_key_that_one_of_its_files_has() -> FsResult<()> {
        let properties = index(&[
            ("a.yml", "widgets:\n  - name: one\n"),
            // An empty list is a list; a key without a value is not.
            ("b.yml", "gadgets: []\nwidgets:\n"),
        ])
        .await?;
        assert!(properties.has_key("widgets"));
        assert!(properties.has_key("gadgets"));
        assert!(!properties.has_key("models"));
        assert!(!index(&[("a.yml", "widgets:\n")]).await?.has_key("widgets"));
        Ok(())
    }

    #[test]
    fn a_file_is_read_as_the_lists_under_its_keys() {
        let file = read("version: 2\nmodels:\n  - name: a\n  - name: b\nseeds: []\n").unwrap();
        assert_eq!(file.items.get("models").map(Vec::len), Some(2));
        // An empty list is a list; a key without a value is not.
        assert!(file.has_items("seeds"));
        assert!(!read("seeds:\n").unwrap().has_items("seeds"));
    }

    #[test]
    fn an_empty_file_has_no_keys() {
        for input in ["", "# only a comment\n"] {
            let file = read(input).unwrap();
            assert!(!file.has_items("models"));
            assert!(file.ignored_keys.is_empty());
        }
    }

    #[test]
    fn a_tagged_list_is_a_list() {
        let file = read("models: !tag [{name: a}]\n").unwrap();
        assert!(file.has_items("models"));
    }

    #[test]
    fn a_tag_on_the_file_or_on_an_entry_is_not_read() {
        for input in [
            "!tag\nmodels:\n  - name: a\n",
            "--- !tag\nmodels:\n  - name: a\n",
            "models:\n  - !tag\n    name: a\n",
        ] {
            let file = read(input).unwrap();
            let models = file.items.get("models").expect("the file has models");
            assert_eq!(models.len(), 1, "{input}");
            assert_eq!(models[0].get("name").and_then(Value::as_str), Some("a"));
        }
        // What the tag is on must still be a mapping.
        assert_eq!(
            error("!tag [a]\n"),
            "YAML error: invalid type: sequence, expected struct DbtPropertiesFileValues"
        );
    }

    #[test]
    fn an_entry_that_is_not_a_mapping_is_an_error() {
        let error_of = |entry: &str| error(&format!("models:\n  - {entry}\n"));
        // One Jinja expression is not rendered to an entry.
        assert_eq!(
            error_of("\"{{ var('customers') }}\""),
            "YAML error: models: each entry must be a mapping, but this entry is a string. \
             Jinja can give the value of a key in an entry, but not a whole entry."
        );
        assert_eq!(
            error_of("customers"),
            "YAML error: models: each entry must be a mapping, but this entry is a string."
        );
        assert_eq!(
            error_of(""),
            "YAML error: models: each entry must be a mapping, but this entry is empty."
        );
        assert_eq!(
            error_of("1"),
            "YAML error: models: each entry must be a mapping, but this entry is a number."
        );
        assert_eq!(
            error_of("[a]"),
            "YAML error: models: each entry must be a mapping, but this entry is a list."
        );
        // The key of the entry is named.
        assert_eq!(
            error("models:\n  - name: a\nseeds:\n  - true\n"),
            "YAML error: seeds: each entry must be a mapping, but this entry is a boolean."
        );
    }

    #[test]
    fn a_string_in_an_entry_is_not_an_error() {
        let file = read(
            "models:\n  - name: a\n    columns: \"{{ var('columns') }}\"\n    config: \"{{ var('config') }}\"\n",
        )
        .unwrap();
        assert_eq!(file.items.get("models").map(Vec::len), Some(1));
    }

    #[test]
    fn a_key_of_the_wrong_type_is_an_error_that_names_it() {
        assert_eq!(
            error("models: {}\n"),
            "YAML error: models: invalid type: map, expected a sequence"
        );
        assert_eq!(
            error("version: [1]\n"),
            "YAML error: version: data did not match any variant of untagged enum FloatOrString"
        );
    }

    #[test]
    fn what_is_not_a_mapping_with_string_keys_is_an_error() {
        assert_eq!(
            error("- a\n- b\n"),
            "YAML error: invalid type: sequence, expected struct DbtPropertiesFileValues"
        );
        assert_eq!(
            error("1: 2\n"),
            "YAML error: invalid type: integer `1`, expected field identifier"
        );
    }

    #[test]
    fn an_ignored_key_is_kept_and_not_read() {
        // Whatever it holds: it is not typed.
        let file = read("evals: 1\nmodels: []\n").unwrap();
        assert_eq!(file.ignored_keys, ["evals"]);
        assert!(!file.has_items("evals"));
        assert!(file.has_items("models"));
    }

    #[test]
    fn a_key_that_is_not_read_is_not_an_error() {
        let file = read("unknown: 1\nmodels: []\n").unwrap();
        assert!(file.has_items("models"));
        assert!(!file.has_items("unknown"));
    }
}
