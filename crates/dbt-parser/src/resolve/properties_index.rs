//! The properties files of one package, read once before any node resolves, and the index of
//! what they define.
//!
//! A properties file is a list of items under each of its top-level keys. Each kind of node owns
//! some keys ([`ResolvableNode::properties_keys`]) and says which names an item there defines
//! ([`ResolvableNode::index_item`]). The index maps each name to where it is defined. It holds no
//! YAML: a kind gets the YAML of its entries from the files, by location, when it resolves.

use crate::resolve::resolvable_node::ResolvableNode;
use crate::resolve::resolve_properties::MinimalPropertiesEntry;
use dbt_common::tracing::dbt_emit::emit_strict_parse_error;
use dbt_common::{ErrorCode, FsError, FsResult, fs_err};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::serde::{
    from_yaml_raw, into_typed_with_error, into_typed_with_jinja, unused_key_error,
};
use dbt_schemas::schemas::serde::{FloatOrString, strip_one_trailing_newline_at_keys};
use dbt_yaml::{Mapping, Span, Value, Verbatim};
use minijinja::Value as MinijinjaValue;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One properties file, as its top-level keys.
#[derive(Debug, Default)]
pub(crate) struct PropertiesFile {
    /// The file, relative to its package's root.
    path: PathBuf,
    /// The items under each top-level key that holds a list. A key without a value has none.
    items: BTreeMap<String, Vec<Value>>,
    /// What is under `experimental:`, by key.
    pub(crate) experimental: Option<BTreeMap<String, Value>>,
}

impl PropertiesFile {
    /// Reads the properties file `input`. An empty file has no keys.
    ///
    /// `list_keys` are the top-level keys that dbt reads a list of items from. One that holds
    /// anything else is an error, and the file is not read. A key that dbt doesn't read is
    /// reported as unused, and the rest of the file is still read.
    pub(crate) fn read(
        input: &str,
        path: &Path,
        absolute_path: &Path,
        dependency_package_name: Option<&str>,
        list_keys: &[&str],
    ) -> FsResult<Self> {
        let mut file = PropertiesFile {
            path: path.to_path_buf(),
            ..Default::default()
        };
        let keys = match from_yaml_raw::<Value>(
            input,
            Some(absolute_path),
            true,
            dependency_package_name,
        )? {
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
                Some("experimental") => file.experimental = typed_key(key, value)?,
                Some(name) if list_keys.contains(&name) => {
                    let name = name.to_string();
                    let items = match value {
                        Value::Null(..) => None,
                        Value::Sequence(items, ..) => Some(items),
                        // A tagged list, or what is not a list.
                        value => typed_key(key, value)?,
                    };
                    if let Some(items) = items {
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

    /// Takes the list under `key` out of the file, if it has one.
    pub(crate) fn take_items(&mut self, key: &str) -> Option<Vec<Value>> {
        self.items.remove(key)
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
}

/// A name that an item of a properties file defines, as its kind indexes it.
#[derive(Debug)]
pub(crate) struct IndexedName {
    /// The key the name is found under: the item's top-level key, unless its kind reads the
    /// items of several keys as one set of entries.
    pub(crate) key: &'static str,
    pub(crate) name: String,
    /// Where the name is written, for a kind that gives its nodes that location.
    pub(crate) name_span: Span,
}

impl IndexedName {
    /// A name found under `key`, without where it is written.
    pub(crate) fn new(key: &'static str, name: String) -> Self {
        IndexedName {
            key,
            name,
            name_span: Span::default(),
        }
    }
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
pub(crate) struct IndexCx<'a> {
    pub(crate) jinja_env: &'a Arc<JinjaEnv>,
    /// The Jinja context of the package's properties files.
    pub(crate) base_ctx: &'a Arc<BTreeMap<String, MinijinjaValue>>,
    /// The package of the file, unless it is the root package.
    pub(crate) dependency_package_name: Option<&'a str>,
}

impl IndexCx<'_> {
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
    /// first item that a kind cannot index is the error. Gives the file's position among the
    /// package's files.
    pub(crate) async fn index_file(
        &mut self,
        kinds: &[&dyn ResolvableNode],
        file: PropertiesFile,
        cx: &IndexCx<'_>,
    ) -> FsResult<usize> {
        let file_id = self.files.len();
        self.files.push(file);
        let file = &self.files[file_id];
        for kind in kinds {
            for &key in kind.properties_keys() {
                let Some(items) = file.items.get(key) else {
                    continue;
                };
                for (item_id, item) in items.iter().enumerate() {
                    let location = PropertiesLocation {
                        file: file_id,
                        key,
                        item: item_id,
                    };
                    for name in kind.index_item(key, item, cx).await? {
                        self.index.insert(name, location);
                    }
                }
            }
        }
        Ok(file_id)
    }

    /// Adds `value`, the list under `experimental: <key>:` in the file `file_id`, to the file as
    /// the items of `key`, and indexes each item by its `name`.
    ///
    /// The keys of experimentals are under `experimental:`, not top-level keys, so no kind of
    /// node owns them. This reads them until they are. An experimental's key is never a key of a
    /// kind of node.
    pub(crate) fn index_experimental_items(
        &mut self,
        file_id: usize,
        key: &'static str,
        value: Value,
        cx: &IndexCx<'_>,
    ) -> FsResult<()> {
        /// An entry under `experimental:`, read only for its name.
        #[derive(Deserialize)]
        struct MinimalExperimentalEntryValue {
            name: String,
            __additional_properties__: Verbatim<BTreeMap<String, Value>>,
        }

        let file = &mut self.files[file_id];
        let items = match value {
            Value::Null(..) => return Ok(()),
            Value::Sequence(items, ..) => items,
            _ => {
                return Err(fs_err!(
                    code => ErrorCode::InvalidConfig,
                    loc => file.path.clone(),
                    "`experimental: {}` must be a list",
                    key
                ));
            }
        };
        for (item_id, item) in items.iter().enumerate() {
            let entry: MinimalExperimentalEntryValue = cx.typed(item)?;
            let location = PropertiesLocation {
                file: file_id,
                key,
                item: item_id,
            };
            self.index.insert(
                IndexedName::new(key, validate_resource_name(&entry.name)?),
                location,
            );
        }
        file.items.insert(key.to_string(), items);
        Ok(())
    }

    /// Takes the entries found under `key`, by name, with their YAML. The first item that
    /// defines a name is its entry. The files of the others are the entry's duplicates.
    pub(crate) fn take_entries(
        &mut self,
        key: &'static str,
    ) -> BTreeMap<String, MinimalPropertiesEntry> {
        self.take_entries_at(key, |locations| locations.split_first())
    }

    /// Takes the entries found under `key`, by name, with their YAML. The last item that defines
    /// a name is its entry, and it has no duplicates.
    pub(crate) fn take_last_entries(
        &mut self,
        key: &'static str,
    ) -> BTreeMap<String, MinimalPropertiesEntry> {
        self.take_entries_at(key, |locations| {
            locations.last().map(|last| (last, Default::default()))
        })
    }

    /// Takes the entries found under `key`. `split` gives the item that is a name's entry and the
    /// items that are its duplicates.
    fn take_entries_at(
        &mut self,
        key: &'static str,
        split: impl Fn(&[PropertiesLocation]) -> Option<(&PropertiesLocation, &[PropertiesLocation])>,
    ) -> BTreeMap<String, MinimalPropertiesEntry> {
        let files = &mut self.files;
        self.index
            .entries
            .remove(key)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(name, definitions)| {
                let (entry, duplicates) = split(&definitions.locations)?;
                let entry = MinimalPropertiesEntry {
                    name: name.clone(),
                    name_span: definitions.name_span,
                    relative_path: files[entry.file].path.clone(),
                    schema_value: files[entry.file].take_item(entry),
                    table_value: None,
                    version_info: None,
                    duplicate_paths: duplicates
                        .iter()
                        .map(|duplicate| files[duplicate.file].path.clone())
                        .collect(),
                };
                Some((name, entry))
            })
            .collect()
    }
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
            _cx: &IndexCx<'_>,
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
            _cx: &mut crate::resolve::resolvable_node::ResolveCx<'_>,
        ) -> FsResult<()> {
            Ok(())
        }
    }

    /// The properties of a package with `files`, each a path and its content, indexed by
    /// [`Widgets`].
    async fn index(files: &[(&str, &str)]) -> FsResult<PackageProperties> {
        let jinja_env = Arc::new(JinjaEnv::new(minijinja::Environment::new()));
        let base_ctx = Arc::new(BTreeMap::new());
        let cx = IndexCx {
            jinja_env: &jinja_env,
            base_ctx: &base_ctx,
            dependency_package_name: None,
        };
        let mut properties = PackageProperties::default();
        for (path, input) in files {
            let file = PropertiesFile::read(
                input,
                Path::new(path),
                &Path::new("/project").join(path),
                None,
                &["widgets", "gadgets"],
            )?;
            properties.index_file(&[&Widgets], file, &cx).await?;
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

    #[dbt_runtime::test]
    async fn the_items_under_experimental_are_entries_of_their_key() -> FsResult<()> {
        let jinja_env = Arc::new(JinjaEnv::new(minijinja::Environment::new()));
        let base_ctx = Arc::new(BTreeMap::new());
        let cx = IndexCx {
            jinja_env: &jinja_env,
            base_ctx: &base_ctx,
            dependency_package_name: None,
        };
        let mut properties = PackageProperties::default();
        for path in ["a.yml", "b.yml"] {
            let mut file = PropertiesFile::read(
                "experimental:\n  widget:\n    - name: one\n  part:\n  gadget: 1\n",
                Path::new(path),
                &Path::new("/project").join(path),
                None,
                &[],
            )?;
            let mut experimental = file.experimental.take().unwrap_or_default();
            let file_id = properties.index_file(&[], file, &cx).await?;
            let widgets = experimental.remove("widget").unwrap();
            properties.index_experimental_items(file_id, "widget", widgets, &cx)?;
            // A key without a value has no items.
            let parts = experimental.remove("part").unwrap();
            properties.index_experimental_items(file_id, "part", parts, &cx)?;
            let gadgets = experimental.remove("gadget").unwrap();
            let error = properties
                .index_experimental_items(file_id, "gadget", gadgets, &cx)
                .expect_err("the key holds no list");
            assert_eq!(error.context, "`experimental: gadget` must be a list");
        }
        let widgets = properties.take_entries("widget");
        assert_eq!(widgets["one"].relative_path, Path::new("a.yml"));
        assert_eq!(widgets["one"].duplicate_paths, [PathBuf::from("b.yml")]);
        assert!(widgets["one"].schema_value.is_mapping());
        assert!(properties.take_entries("part").is_empty());
        Ok(())
    }

    #[test]
    fn a_file_is_read_as_the_lists_under_its_keys() {
        let mut file = read("version: 2\nmodels:\n  - name: a\n  - name: b\nseeds: []\n").unwrap();
        assert_eq!(file.take_items("models").map(|items| items.len()), Some(2));
        assert!(file.take_items("models").is_none());
        // An empty list is a list; a key without a value is not.
        assert!(file.has_items("seeds"));
        assert!(!read("seeds:\n").unwrap().has_items("seeds"));
    }

    #[test]
    fn an_empty_file_has_no_keys() {
        for input in ["", "# only a comment\n"] {
            let file = read(input).unwrap();
            assert!(!file.has_items("models"));
            assert!(file.experimental.is_none());
        }
    }

    #[test]
    fn a_tagged_list_is_a_list() {
        assert!(read("models: !tag [1]\n").unwrap().has_items("models"));
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
        assert_eq!(
            error("experimental: [1]\n"),
            "YAML error: experimental: invalid type: sequence, expected a map"
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
    fn a_key_that_is_not_read_is_not_an_error() {
        let file = read("unknown: 1\nmodels: []\n").unwrap();
        assert!(file.has_items("models"));
        assert!(!file.has_items("unknown"));
    }
}
