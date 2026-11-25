//! Fields that a build of dbt reads beside a node's own properties.
//!
//! A properties entry is a kind's own fields. A build can read more fields in the same entry: it
//! names a type for them, an [`Extension`], for each kind whose entries have them
//! ([`Extensions`]). A kind can have several, each with its own fields and its own flag
//! ([`ExtensionSet`]).
//!
//! The kind that owns the entry reads the extensions before it reads its own properties
//! ([`ExtensionReader`]): it types and renders an extension's fields as it does its own, and then
//! never sees them, so they are not its unused keys. Once it has built the node, it gives each
//! extension back, with the node ([`Extension::resolved`]).
//!
//! ```yaml
//! models:
//!   - name: orders
//!     description: Three orders.   # the model's
//!     widget_size: 3               # an extension's
//! ```
//!
//! An extension reads fields of the entry, and, if it says so, fields of the entry's `columns`.
//! A field with the same name anywhere else in the entry is not the extension's.

use dbt_common::FsResult;
use dbt_common::io_args::project_flag_lookup;
use dbt_flags::FlagDef;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::serde::into_typed_with_jinja;
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use dbt_yaml::{Mapping, Value};
use minijinja::Value as MinijinjaValue;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::Arc;

use crate::resolve::resolvable_node::ResolveCtx;

/// Fields that are read beside the properties of one kind of node.
pub trait Extension: DeserializeOwned + Debug + Send + Sync + 'static {
    /// The fields of an entry that the extension reads: the keys of its type. A key of an entry
    /// that is not one of these is the kind's, or another extension's.
    const FIELDS: &'static [&'static str];

    /// The fields of each of an entry's `columns` that the extension reads. The extension gets
    /// them under `columns`: one mapping for each column that has one of them, with the column's
    /// `name`.
    const COLUMN_FIELDS: &'static [&'static str] = &[];

    /// The feature flag that turns the extension on. One without a flag is always on.
    ///
    /// While it is off, its fields are not read: they are keys that nothing reads.
    fn flag() -> Option<&'static FlagDef> {
        None
    }

    /// Called by the kind that owns the entry, once it has added `node`, which it built from the
    /// same entry, to `ctx`'s nodes. It is called for an entry that has at least one of
    /// [`Self::FIELDS`], or a column with one of [`Self::COLUMN_FIELDS`], and not for a node that
    /// is disabled.
    ///
    /// The properties file is `node`'s `patch_path`.
    fn resolved(
        self,
        _node: &dyn InternalDbtNodeAttributes,
        _ctx: &mut ResolveCtx<'_>,
    ) -> FsResult<()> {
        Ok(())
    }
}

/// The extension that reads no field.
#[derive(Debug, Default, Deserialize)]
pub struct NoExtension {}

impl Extension for NoExtension {
    const FIELDS: &'static [&'static str] = &[];
}

/// The extensions that are read beside the properties of one kind of node: one [`Extension`], or
/// a tuple of them. Each has its own fields, its own flag and its own
/// [`resolved`](Extension::resolved).
pub trait ExtensionSet: Send + Sync + 'static {
    /// Adds the extensions of the set to `members`.
    #[doc(hidden)]
    fn members(members: &mut Members);
}

impl<X: Extension> ExtensionSet for X {
    fn members(members: &mut Members) {
        members.0.push(Member {
            name: std::any::type_name::<X>(),
            fields: X::FIELDS,
            column_fields: X::COLUMN_FIELDS,
            flag: X::flag(),
            read: read::<X>,
        });
    }
}

macro_rules! extension_set_for_tuple {
    ($($set:ident),+) => {
        impl<$($set: ExtensionSet),+> ExtensionSet for ($($set,)+) {
            fn members(members: &mut Members) {
                $($set::members(members);)+
            }
        }
    };
}

extension_set_for_tuple!(A, B);
extension_set_for_tuple!(A, B, C);
extension_set_for_tuple!(A, B, C, D);

/// The extensions of an [`ExtensionSet`].
#[derive(Default)]
pub struct Members(Vec<Member>);

/// One extension of a set, without its type.
#[derive(Clone, Copy)]
struct Member {
    /// The name of the extension's type, for the error of [`check`].
    name: &'static str,
    fields: &'static [&'static str],
    column_fields: &'static [&'static str],
    flag: Option<&'static FlagDef>,
    /// Types the extension from its fields: [`read`].
    read: ReadFn,
}

/// Types an extension from its fields, the Jinja environment and context of the entry's
/// properties, and the entry's package, unless it is the root package: [`read`].
type ReadFn = fn(
    Value,
    &JinjaEnv,
    &BTreeMap<String, MinijinjaValue>,
    Option<&str>,
) -> FsResult<Box<dyn ReadExtension>>;

/// The [`ExtensionSet`] of each kind of node whose entries can have one.
pub trait Extensions: Send + Sync + 'static {
    /// Read in the entries under `models:`. A model with versions has an entry for each version.
    type Model: ExtensionSet;
    /// Read in the entries under `seeds:`.
    type Seed: ExtensionSet;
    /// Read in the entries under `snapshots:`.
    type Snapshot: ExtensionSet;
    /// Read in the tables of the entries under `sources:`.
    type SourceTable: ExtensionSet;
}

/// No extensions: every entry is only its kind's own fields.
#[derive(Debug, Default)]
pub struct NoExtensions;

impl Extensions for NoExtensions {
    type Model = NoExtension;
    type Seed = NoExtension;
    type Snapshot = NoExtension;
    type SourceTable = NoExtension;
}

/// Checks the extension sets of `E`.
///
/// # Panics
///
/// If two extensions of one kind read the same field.
pub(crate) fn check<E: Extensions>() {
    check_set::<E::Model>("models");
    check_set::<E::Seed>("seeds");
    check_set::<E::Snapshot>("snapshots");
    check_set::<E::SourceTable>("sources");
}

fn check_set<S: ExtensionSet>(kind: &str) {
    let mut members = Members::default();
    S::members(&mut members);
    let mut readers: BTreeMap<&str, &str> = BTreeMap::new();
    let mut column_readers: BTreeMap<&str, &str> = BTreeMap::new();
    for member in &members.0 {
        for field in member.fields {
            if let Some(other) = readers.insert(field, member.name) {
                panic!(
                    "`{field}` in the entries of `{kind}` is read by two extensions: `{other}` \
                     and `{}`",
                    member.name
                );
            }
        }
        for field in member.column_fields {
            if let Some(other) = column_readers.insert(field, member.name) {
                panic!(
                    "`{field}` in the columns of `{kind}` is read by two extensions: `{other}` \
                     and `{}`",
                    member.name
                );
            }
        }
    }
}

/// An extension that was read in an entry, to give back with the entry's node.
trait ReadExtension: Send {
    /// [`Extension::resolved`].
    fn resolved(
        self: Box<Self>,
        node: &dyn InternalDbtNodeAttributes,
        ctx: &mut ResolveCtx<'_>,
    ) -> FsResult<()>;
}

impl<X: Extension> ReadExtension for X {
    fn resolved(
        self: Box<Self>,
        node: &dyn InternalDbtNodeAttributes,
        ctx: &mut ResolveCtx<'_>,
    ) -> FsResult<()> {
        Extension::resolved(*self, node, ctx)
    }
}

/// The extensions that were read in one entry. There is at least one.
pub struct ReadExtensions(Vec<Box<dyn ReadExtension>>);

/// Reads the extensions of one kind of node in its entries: the ones of the kind's set that are
/// on.
pub struct ExtensionReader {
    members: Vec<Member>,
}

impl ExtensionReader {
    /// The extensions of the set `S` that are on, reading the root project's `flags:`.
    pub(crate) fn of<S: ExtensionSet>(ctx: &ResolveCtx<'_>) -> Self {
        let project_flags = project_flag_lookup(ctx.dbt_state.root_project().flags.as_ref());
        Self::new::<S>(|flag| flag.resolve_with_project(&project_flags).value)
    }

    /// The extensions of the set `S` whose flag `is_on`, or that have none.
    fn new<S: ExtensionSet>(is_on: impl Fn(&'static FlagDef) -> bool) -> Self {
        let mut members = Members::default();
        S::members(&mut members);
        members.0.retain(|member| {
            !(member.fields.is_empty() && member.column_fields.is_empty())
                && member.flag.is_none_or(&is_on)
        });
        ExtensionReader { members: members.0 }
    }

    /// Reads the extensions in `entry`: takes the fields of each out of the entry, and types
    /// them, with Jinja rendered as for the entry's own properties. None if the entry has none
    /// of their fields.
    ///
    /// The kind then types what is left as its own properties, exactly as it does without an
    /// extension.
    pub(crate) fn read(
        &self,
        entry: &mut Value,
        jinja_env: &JinjaEnv,
        base_ctx: &BTreeMap<String, MinijinjaValue>,
        dependency_package_name: Option<&str>,
    ) -> FsResult<Option<ReadExtensions>> {
        let mut read = Vec::new();
        for member in &self.members {
            if let Some(fields) = take_fields(entry, member.fields, member.column_fields) {
                read.push((member.read)(
                    fields,
                    jinja_env,
                    base_ctx,
                    dependency_package_name,
                )?);
            }
        }
        Ok((!read.is_empty()).then_some(ReadExtensions(read)))
    }
}

/// The extension `X`, from `fields`, the fields of its own that an entry has: typed, with Jinja
/// rendered as for the entry's own properties.
///
/// A kind's properties and an extension are not typed in one pass, as one struct that flattens
/// both: `dbt_yaml` then gives a key that the kind does not read to the extension wherever it is
/// written, so `policy` on a column would be read as the entry's.
fn read<X: Extension>(
    fields: Value,
    jinja_env: &JinjaEnv,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
    dependency_package_name: Option<&str>,
) -> FsResult<Box<dyn ReadExtension>> {
    let extension: X = typed(fields, jinja_env, base_ctx, dependency_package_name)?;
    Ok(Box::new(extension))
}

/// `fields` as the extension `X`.
fn typed<X: Extension>(
    fields: Value,
    jinja_env: &JinjaEnv,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
    dependency_package_name: Option<&str>,
) -> FsResult<X> {
    into_typed_with_jinja(
        fields,
        false,
        jinja_env,
        base_ctx,
        &[],
        dependency_package_name,
        true,
    )
}

/// Takes `fields` out of the mapping `entry`, with their keys, as a mapping that is written
/// where `entry` is. The other keys keep their order. None if `entry` has none of them.
///
/// `column_fields` are taken out of each mapping under the entry's `columns`, if they are a
/// list. The columns that have one are given under `columns`, each with its `name`.
fn take_fields(entry: &mut Value, fields: &[&str], column_fields: &[&str]) -> Option<Value> {
    let span = entry.span().clone();
    let entry = entry.as_mapping_mut()?;
    let mut taken = take_keys(entry, fields);
    let columns: Vec<Value> = entry
        .get_mut(COLUMNS)
        .and_then(Value::as_sequence_mut)
        .into_iter()
        .flatten()
        .filter_map(|column| {
            let span = column.span().clone();
            let column = column.as_mapping_mut()?;
            let fields = take_keys(column, column_fields);
            if fields.is_empty() {
                return None;
            }
            let mut named = Mapping::new();
            if let Some(name) = column.get(NAME) {
                named.insert(Value::string(NAME.to_string()), name.clone());
            }
            named.extend(fields);
            Some(Value::mapping(named).with_span(span))
        })
        .collect();
    if !columns.is_empty() {
        taken.insert(Value::string(COLUMNS.to_string()), Value::sequence(columns));
    }
    (!taken.is_empty()).then(|| Value::mapping(taken).with_span(span))
}

const COLUMNS: &str = "columns";
const NAME: &str = "name";

/// Takes `keys` out of `mapping`, with their values. The other keys keep their order.
fn take_keys(mapping: &mut Mapping, keys: &[&str]) -> Mapping {
    let mut taken = Mapping::new();
    for key in keys {
        if let Some((key, value)) = mapping.shift_remove_entry(*key) {
            taken.insert(key, value);
        }
    }
    taken
}

/// Gives the extensions of each entry back, with the entry's node: see [`Extension::resolved`].
/// `node` finds an enabled node of `ctx` by its unique id.
pub(crate) fn give_back<N: InternalDbtNodeAttributes>(
    ctx: &mut ResolveCtx<'_>,
    extensions: Vec<(String, ReadExtensions)>,
    node: impl Fn(&ResolveCtx<'_>, &str) -> Option<Arc<N>>,
) -> FsResult<()> {
    for (unique_id, extensions) in extensions {
        if let Some(node) = node(ctx, &unique_id) {
            for extension in extensions.0 {
                extension.resolved(node.as_ref(), ctx)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_flags::{Remote, Stage};
    use dbt_yaml::{Spanned, Verbatim};

    /// An extension for the tests: a tag, a label, and a note that is not rendered.
    #[derive(Debug, Deserialize)]
    struct Tagged {
        tag: Option<Spanned<String>>,
        label: Option<String>,
        note: Verbatim<Option<String>>,
    }

    impl Extension for Tagged {
        const FIELDS: &'static [&'static str] = &["tag", "label", "note"];
    }

    static SIZES: FlagDef = FlagDef {
        name: "test_sizes",
        description: "Sizes, for the tests of extensions.",
        stage: Stage::Beta,
        default: false,
        remote: Remote::Never,
        aliases: &[],
        legacy_env: &[],
        tracking: None,
        remove_by: None,
        docs_url: None,
    };

    /// Another extension, behind a flag.
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Sized {
        size: Option<u64>,
    }

    impl Extension for Sized {
        const FIELDS: &'static [&'static str] = &["size"];

        fn flag() -> Option<&'static FlagDef> {
            Some(&SIZES)
        }
    }

    /// An extension that reads a field that [`Tagged`] reads.
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Labelled {
        label: Option<String>,
    }

    impl Extension for Labelled {
        const FIELDS: &'static [&'static str] = &["label"];
    }

    fn yaml(input: &str) -> Value {
        dbt_yaml::from_str(input).expect("valid YAML")
    }

    /// `fields` as the extension `X`.
    fn typed_as<X: Extension>(fields: &str) -> FsResult<X> {
        let jinja_env = JinjaEnv::new(minijinja::Environment::new());
        typed(yaml(fields), &jinja_env, &BTreeMap::new(), None)
    }

    /// How many extensions of the set `S` are read in `entry`, with the flags that `is_on`.
    fn read_in<S: ExtensionSet>(
        entry: &mut Value,
        is_on: impl Fn(&'static FlagDef) -> bool,
    ) -> FsResult<usize> {
        let jinja_env = JinjaEnv::new(minijinja::Environment::new());
        let read =
            ExtensionReader::new::<S>(is_on).read(entry, &jinja_env, &BTreeMap::new(), None)?;
        Ok(read.map_or(0, |read| read.0.len()))
    }

    #[test]
    fn an_extension_is_typed_as_the_entrys_own_properties_are() -> FsResult<()> {
        let tagged: Tagged = typed_as(
            "tag: gold
label: \"{{ 'go' ~ 'ld' }}\"
note: \"{{ as written }}\"
",
        )?;
        // Rendered. `note` is `Verbatim`: it is not.
        assert_eq!(tagged.label.as_deref(), Some("gold"));
        assert_eq!(tagged.note.as_deref(), Some("{{ as written }}"));
        // A field keeps where it is written.
        let tag = tagged.tag.expect("the fields have a tag");
        assert_eq!(tag.as_str(), "gold");
        assert_eq!(tag.span().start.line, 1);

        // `note` is a string.
        assert!(typed_as::<Tagged>("note: [1, 2]\n").is_err());
        Ok(())
    }

    #[test]
    fn an_extension_is_read_out_of_an_entry() -> FsResult<()> {
        let mut entry = yaml(
            "name: orders
tag: gold
description: Three orders.
note: n
columns:
  - {name: id}
",
        );
        assert_eq!(read_in::<Tagged>(&mut entry, |_| true)?, 1);
        // The kind reads what is left, in the order it was written.
        assert_eq!(
            entry,
            yaml("{name: orders, description: Three orders., columns: [{name: id}]}")
        );
        let keys: Vec<&str> = entry
            .as_mapping()
            .expect("a mapping")
            .keys()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(keys, ["name", "description", "columns"]);
        Ok(())
    }

    #[test]
    fn what_is_read_keeps_where_the_entry_is_written() {
        let mut entry = yaml("models:\n  - name: orders\n    tag: gold\n");
        let entry = &mut entry["models"][0];
        let taken = take_fields(entry, Tagged::FIELDS, &[]).expect("the entry has a tag");
        // The entry starts on the second line, and its tag is on the third.
        assert_eq!(taken.span().start.line, 2);
        assert_eq!(taken["tag"].span().start.line, 3);
    }

    #[test]
    fn an_extension_reads_fields_of_the_entry_only() -> FsResult<()> {
        // A column's `tag` and a config's `note` are theirs.
        let input = "name: orders
columns:
  - {name: id, tag: a column's}
config:
  note: a config's
";
        let mut entry = yaml(input);
        assert_eq!(read_in::<Tagged>(&mut entry, |_| true)?, 0);
        assert_eq!(entry, yaml(input));
        Ok(())
    }

    #[test]
    fn an_entry_without_the_fields_has_no_extension() -> FsResult<()> {
        let mut entry = yaml("name: orders\ndescription: Three orders.\n");
        assert_eq!(read_in::<Tagged>(&mut entry, |_| true)?, 0);
        // Nor does an entry that is not a mapping, which its kind reports.
        assert_eq!(read_in::<Tagged>(&mut yaml("orders"), |_| true)?, 0);
        // `NoExtension` reads nothing.
        let mut entry = yaml("name: orders\ntag: gold\n");
        assert_eq!(read_in::<NoExtension>(&mut entry, |_| true)?, 0);
        assert_eq!(entry, yaml("name: orders\ntag: gold\n"));
        Ok(())
    }

    #[test]
    fn each_extension_of_a_set_reads_its_own_fields() -> FsResult<()> {
        let input = "name: orders\ntag: gold\nsize: 3\n";

        let mut entry = yaml(input);
        assert_eq!(read_in::<(Tagged, Sized)>(&mut entry, |_| true)?, 2);
        assert_eq!(entry, yaml("name: orders\n"));

        // An entry with the fields of one has that one.
        let mut entry = yaml("name: orders\nsize: 3\n");
        assert_eq!(read_in::<(Tagged, Sized)>(&mut entry, |_| true)?, 1);
        assert_eq!(entry, yaml("name: orders\n"));

        // A set can hold sets.
        let mut entry = yaml(input);
        assert_eq!(
            read_in::<(NoExtension, (Tagged, Sized))>(&mut entry, |_| true)?,
            2
        );
        Ok(())
    }

    #[test]
    fn an_extension_that_is_off_leaves_its_fields_in_the_entry() -> FsResult<()> {
        let mut entry = yaml("name: orders\ntag: gold\nsize: [not, a, size]\n");
        // `Sized` is behind `test_sizes`. `Tagged` has no flag.
        let read = read_in::<(Tagged, Sized)>(&mut entry, |flag| flag.name != "test_sizes")?;
        assert_eq!(read, 1);
        // `size` is the kind's to report, and it is not typed.
        assert_eq!(entry, yaml("name: orders\nsize: [not, a, size]\n"));
        Ok(())
    }

    /// An extension that reads a field of columns, and gets them under `columns`.
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Units {
        columns: Vec<ColumnUnit>,
    }

    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct ColumnUnit {
        name: String,
        unit: String,
    }

    impl Extension for Units {
        const FIELDS: &'static [&'static str] = &[];
        const COLUMN_FIELDS: &'static [&'static str] = &["unit"];
    }

    #[test]
    fn an_extension_reads_the_fields_of_columns_that_it_names() -> FsResult<()> {
        let mut entry = yaml(
            "name: orders
unit: an entry's
columns:
  - {name: id, description: d}
  - {name: amount, unit: usd, description: d}
",
        );
        let taken = take_fields(&mut entry.clone(), Units::FIELDS, Units::COLUMN_FIELDS)
            .expect("a column has a unit");
        assert_eq!(taken, yaml("columns: [{name: amount, unit: usd}]"));
        // The column is on the fifth line.
        assert_eq!(taken["columns"][0].span().start.line, 5);

        assert_eq!(read_in::<Units>(&mut entry, |_| true)?, 1);
        // The entry's own `unit` is not a column's, and the columns keep their other fields.
        assert_eq!(
            entry,
            yaml(
                "{name: orders, unit: an entry's, columns: [{name: id, description: d}, {name: amount, description: d}]}"
            )
        );

        // Columns that a Jinja expression gives are not written in the entry.
        let input = "name: orders\ncolumns: \"{{ var('columns') }}\"\n";
        let mut entry = yaml(input);
        assert_eq!(read_in::<Units>(&mut entry, |_| true)?, 0);
        assert_eq!(entry, yaml(input));
        Ok(())
    }

    #[test]
    #[should_panic(expected = "`unit` in the columns of `models` is read by two extensions")]
    fn two_extensions_of_a_kind_do_not_read_the_same_field_of_columns() {
        check_set::<(Units, (Tagged, Units))>("models");
    }

    #[test]
    #[should_panic(expected = "`label` in the entries of `models` is read by two extensions")]
    fn two_extensions_of_a_kind_do_not_read_the_same_field() {
        check_set::<(Tagged, Labelled)>("models");
    }

    #[test]
    fn extensions_of_different_kinds_can_read_the_same_field() {
        struct Both;
        impl Extensions for Both {
            type Model = (Tagged, Sized);
            type Seed = Labelled;
            type Snapshot = NoExtension;
            type SourceTable = Tagged;
        }
        check::<Both>();
    }
}
