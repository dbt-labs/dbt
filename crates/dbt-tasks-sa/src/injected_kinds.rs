//! The registry of what features add to a build: kinds of node, the tasks of their nodes, and
//! tables of the information schema.
//!
//! A kind of node is a [`ResolvableNode`], as dbt's own kinds are. A feature in testing adds
//! one behind its flag. A kind whose nodes are only definitions is added with
//! [`InjectedKinds::with_kind`]. A kind whose nodes run is added with
//! [`InjectedKinds::with_tasks`], with the [`InjectedTasks`] that give the task graph the
//! tasks of its nodes. The tables that the build adds to the information schema are declared
//! with [`InjectedKinds::with_injected_tables`], each behind a flag.

use std::collections::BTreeSet;
use std::sync::Arc;

use dbt_common::io_args::FsCommand;
use dbt_flags::{FlagDef, Stage};
use dbt_index_core::info_schema::injected::{self, InjectedTable};
use dbt_telemetry::NodeType;

pub use dbt_parser::experimental::{
    ExperimentalDiagnostics, ExperimentalError, InjectedNodeDef, KindEntries, KindEntry,
};
pub use dbt_parser::resolve::resolvable_node::{
    IndexCtx, IndexedName, InjectedProperties, InjectedPropertiesByKind, InjectedPropertiesSet,
    LinkCtx, NoInjectedProperties, NoInjectedPropertiesByKind, ResolvableNode, ResolvableNodes,
    ResolveCtx,
};

/// What a feature defines a table that it injects into the information schema with, and
/// writes the table's files with.
pub mod info_schema {
    pub use arrow_schema::SchemaRef;
    pub use dbt_index_core::IndexError;
    pub use dbt_index_core::epoch_layers::{append_epoch, merge_epochs, select_epochs};
    pub use dbt_index_core::info_schema::injected::InjectedTable;
    pub use dbt_index_core::info_schema::spec::Ns;
}

use crate::graph::command_phases;
use crate::injected::InjectedTasks;

/// The kinds of node that features add to a build, and the tasks of their nodes.
#[derive(Clone, Default)]
pub struct InjectedKinds {
    /// The kinds, in the order they were added, which is the order they resolve in.
    kinds: Vec<Arc<dyn ResolvableNode>>,
    /// What gives the task graph the tasks of injected nodes: one source for each kind added
    /// with [`Self::with_tasks`].
    tasks: Vec<Arc<dyn InjectedTasks>>,
    /// The kinds of the nodes that the kinds add.
    types: BTreeSet<&'static str>,
    /// The keys that the kinds read.
    keys: BTreeSet<&'static str>,
    /// dbt's own kinds of node, which the kinds are added to.
    dbt: ResolvableNodes,
    /// The tables that the build adds to the information schema, each behind a flag.
    injected_tables: &'static [&'static dyn InjectedTable],
}

impl InjectedKinds {
    /// Adds `kind`, a kind of node that has nothing to run: its nodes are definitions, which
    /// other nodes read. `node_kinds` are the kinds of its nodes: the prefixes of their unique
    /// ids, and values `resource_type:` selects.
    ///
    /// It resolves after dbt's own kinds and the kinds added before it.
    ///
    /// # Panics
    ///
    /// If a kind of its nodes or one of its keys isn't a lowercase identifier (or two, for a
    /// key of a section: `section.key`) or names something dbt itself uses, if something
    /// registered before adds nodes of the same kind or reads the same key, or if it has no
    /// flag, or one that is on by default or is a behavior flag.
    pub fn with_kind(mut self, kind: Arc<dyn ResolvableNode>, node_kinds: &[&'static str]) -> Self {
        let name = node_kinds.first().expect("a kind adds nodes of some kind");
        self.register(name, node_kinds, kind.as_ref());
        self.kinds.push(kind);
        self
    }

    /// Adds `feature`: a kind of node, and the tasks that its nodes run as. The kinds of its
    /// nodes are [`InjectedTasks::kinds`].
    ///
    /// It resolves as a kind added with [`Self::with_kind`] does, and is held to the same
    /// rules.
    ///
    /// # Panics
    ///
    /// As [`Self::with_kind`] does, for the kind's keys, its flag and the kinds of its nodes.
    /// Also if the command it adds is one that dbt builds a task graph for, or one that
    /// another feature adds.
    pub fn with_tasks<T>(mut self, feature: Arc<T>) -> Self
    where
        T: ResolvableNode + InjectedTasks + 'static,
    {
        let node_kinds = feature.kinds().to_vec();
        let name = node_kinds.first().expect("a kind adds nodes of some kind");
        self.register(name, &node_kinds, feature.as_ref());
        if let Some(command) = feature.command() {
            assert!(
                command_phases(FsCommand::Extension(command.name), &[]).is_none(),
                "`{name}` adds the command `{}`, a command of dbt",
                command.name
            );
            assert!(
                command_phases(FsCommand::Extension(command.name), &self.tasks).is_none(),
                "the command `{}` is added twice",
                command.name
            );
        }
        self.kinds
            .push(Arc::clone(&feature) as Arc<dyn ResolvableNode>);
        self.tasks.push(feature);
        self
    }

    /// Checks `kind`, which adds nodes of the kinds `node_kinds`, against dbt's own names and
    /// what is registered so far. `name` names it in an error.
    fn register(&mut self, name: &str, node_kinds: &[&'static str], kind: &dyn ResolvableNode) {
        let dbt_keys: BTreeSet<&str> = ResolvableNodes::dbt().properties_keys().collect();
        for node_kind in node_kinds {
            check_name("kind", node_kind);
            assert!(
                self.types.insert(node_kind),
                "kind `{node_kind}` is registered twice"
            );
        }
        check_flag(name, kind.flag());
        for key in kind.properties_keys() {
            // A key of a section, `section.key`, is two names. The section is the top-level
            // key.
            let (top_level, section_key) = match key.split_once('.') {
                Some((section, section_key)) => (section, Some(section_key)),
                None => (*key, None),
            };
            check_name("key", top_level);
            if let Some(section_key) = section_key {
                check_name("key", section_key);
            }
            assert!(
                !dbt_keys.contains(top_level) && !matches!(top_level, "version" | "anchors"),
                "kind `{name}` reads `{key}`, a key dbt reads"
            );
            assert!(
                self.keys.insert(*key),
                "`{key}` is read by more than one kind, or twice by `{name}`"
            );
        }
    }

    /// The same registry, for a build that injects the properties `P` into the entries of
    /// dbt's own kinds of node.
    pub fn with_injected_properties<P: InjectedPropertiesByKind>(mut self) -> Self {
        self.dbt = ResolvableNodes::dbt_with_injected_properties::<P>();
        self
    }

    /// Declares the tables that the build injects into the information schema, each while
    /// its flag is on. They are all of them: a second call replaces the first.
    ///
    /// # Panics
    ///
    /// If a table cannot be published beside the catalog ([`injected::check`]), or if two
    /// have the same name.
    pub fn with_injected_tables(mut self, tables: &'static [&'static dyn InjectedTable]) -> Self {
        let mut names = BTreeSet::new();
        for table in tables {
            if let Err(error) = injected::check(*table) {
                panic!("{error}");
            }
            let name = table.qualified_name();
            assert!(
                names.insert(name.clone()),
                "the injected table `{name}` is declared twice"
            );
        }
        self.injected_tables = tables;
        self
    }

    /// The tables that the build injects into the information schema.
    pub fn injected_tables(&self) -> &'static [&'static dyn InjectedTable] {
        self.injected_tables
    }

    /// What gives the task graph the tasks of injected nodes.
    pub fn injected_tasks(&self) -> &[Arc<dyn InjectedTasks>] {
        &self.tasks
    }

    /// Every feature flag this binary knows: dbt's own, and the flags that turn on the kinds
    /// it adds, which need not be in `dbt_flags::ALL`.
    pub fn known_flags(&self) -> Vec<&'static FlagDef> {
        let mut known: Vec<&'static FlagDef> = dbt_flags::ALL.to_vec();
        for flag in self.kinds.iter().filter_map(|kind| kind.flag()) {
            if !known.iter().any(|known| known.name == flag.name) {
                known.push(flag);
            }
        }
        known
    }

    /// The name and description of every beta feature this binary knows, for `--help`.
    pub fn beta_features(&self) -> Vec<(&'static str, &'static str)> {
        self.known_flags()
            .into_iter()
            .filter(|flag| flag.stage == Stage::Beta)
            .map(|flag| (flag.name, flag.description))
            .collect()
    }

    /// The kinds of node a project resolves with: dbt's own, then the kinds added, in the
    /// order they were added.
    pub fn resolvable_nodes(&self) -> ResolvableNodes {
        self.kinds
            .iter()
            .cloned()
            .fold(self.dbt.clone(), |nodes, kind| nodes.with(kind))
    }
}

/// Checks that `name`, a kind of node or a key, is a lowercase identifier that dbt doesn't
/// use.
fn check_name(what: &str, name: &str) {
    assert!(
        is_identifier(name),
        "{what} `{name}` is not a lowercase identifier"
    );
    assert!(!is_dbt_name(name), "{what} `{name}` is a name dbt uses");
}

fn check_flag(kind: &str, flag: Option<&FlagDef>) {
    let Some(flag) = flag else {
        panic!("kind `{kind}` must have a feature flag");
    };
    assert!(
        matches!(flag.stage, Stage::Experimental | Stage::Beta),
        "the flag `{}` of kind `{kind}` must be an experimental or beta flag",
        flag.name
    );
    assert!(
        !flag.default,
        "the flag `{}` of kind `{kind}` must be off by default",
        flag.name
    );
}

fn is_identifier(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Whether dbt itself uses `name` as a resource type, a unique id prefix, or a value
/// `resource_type:` gives its own meaning.
fn is_dbt_name(name: &str) -> bool {
    matches!(name, "group" | "doc" | "relation")
        || NodeType::from_str_name(&format!("NODE_TYPE_{}", name.to_ascii_uppercase())).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::injected::{InjectedCommand, InjectedRun, StartCtx, TP};
    use async_trait::async_trait;
    use dbt_common::FsResult;
    use dbt_flags::Remote;

    static TEST_FLAG: FlagDef = flag(false, Stage::Beta);
    static ON_BY_DEFAULT: FlagDef = flag(true, Stage::Beta);
    static BEHAVIOR: FlagDef = flag(false, Stage::Behavior);

    const fn flag(default: bool, stage: Stage) -> FlagDef {
        FlagDef {
            name: "test_injected_kinds",
            description: "Kinds for testing injected kinds.",
            stage,
            default,
            remote: Remote::Never,
            aliases: &[],
            legacy_env: &[],
            tracking: None,
            remove_by: None,
            docs_url: None,
        }
    }

    const WIDGET: &str = "widget";
    const GADGET: &str = "gadget";
    const GIZMO: &str = "gizmo";

    /// The one key of a kind whose nodes are of the kind `kind`: the kind itself.
    fn own_key(kind: &str) -> &'static [&'static str] {
        match kind {
            WIDGET => &[WIDGET],
            GADGET => &[GADGET],
            GIZMO => &[GIZMO],
            _ => &[],
        }
    }

    /// A kind with nothing to run, which reads `keys`, with a flag or without.
    struct WithKeys {
        keys: &'static [&'static str],
        flag: Option<&'static FlagDef>,
    }

    #[async_trait]
    impl ResolvableNode for WithKeys {
        fn flag(&self) -> Option<&'static FlagDef> {
            self.flag
        }
        fn properties_keys(&self) -> &'static [&'static str] {
            self.keys
        }
        async fn resolve(&self, _ctx: &mut ResolveCtx<'_>) -> FsResult<()> {
            Ok(())
        }
    }

    /// A registry with one kind, of `widget` nodes, which reads `keys`.
    fn reads(keys: &'static [&'static str]) -> InjectedKinds {
        with_flag(keys, Some(&TEST_FLAG))
    }

    /// A registry with one kind, of `widget` nodes, which reads `keys` and has the flag `flag`.
    fn with_flag(keys: &'static [&'static str], flag: Option<&'static FlagDef>) -> InjectedKinds {
        InjectedKinds::default().with_kind(Arc::new(WithKeys { keys, flag }), &[WIDGET])
    }

    /// A kind of node of the kind `kinds`, with tasks for its nodes, which reads the keys
    /// `keys` and may add a command.
    struct WithTasks {
        kinds: [&'static str; 1],
        keys: &'static [&'static str],
        command: Option<&'static str>,
    }

    impl WithTasks {
        fn new(kind: &'static str) -> Self {
            Self {
                kinds: [kind],
                keys: own_key(kind),
                command: None,
            }
        }
    }

    #[async_trait]
    impl ResolvableNode for WithTasks {
        fn flag(&self) -> Option<&'static FlagDef> {
            Some(&TEST_FLAG)
        }
        fn properties_keys(&self) -> &'static [&'static str] {
            self.keys
        }
        async fn resolve(&self, _ctx: &mut ResolveCtx<'_>) -> FsResult<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl InjectedTasks for WithTasks {
        fn kinds(&self) -> &[&'static str] {
            &self.kinds
        }
        fn command(&self) -> Option<InjectedCommand> {
            self.command.map(InjectedCommand::runs)
        }
        async fn start(&self, _cx: &StartCtx<'_>) -> FsResult<Option<Arc<dyn InjectedRun>>> {
            Ok(None)
        }
    }

    /// A registry of kinds with tasks, one for each of `kinds`.
    fn with_tasks(kinds: &[&'static str]) -> InjectedKinds {
        kinds
            .iter()
            .fold(InjectedKinds::default(), |registry, kind| {
                registry.with_tasks(Arc::new(WithTasks::new(kind)))
            })
    }

    /// A project resolves with dbt's kinds of node, then the kinds added, in the order they
    /// were added.
    #[test]
    fn added_kinds_resolve_after_dbts_kinds() {
        let dbt_keys: Vec<&str> = ResolvableNodes::dbt().properties_keys().collect();
        let keys: Vec<&str> = with_tasks(&[WIDGET, GADGET])
            .resolvable_nodes()
            .properties_keys()
            .collect();
        assert_eq!(keys[..dbt_keys.len()], dbt_keys);
        assert_eq!(keys[dbt_keys.len()..], [WIDGET, GADGET]);
    }

    /// A kind that has nothing to run resolves in the order it was added, its flag is known,
    /// and the task graph gets no tasks from it.
    #[test]
    fn a_kind_with_nothing_to_run_gives_the_graph_no_tasks() {
        let dbt_keys = ResolvableNodes::dbt().properties_keys().count();
        let registry = with_tasks(&[WIDGET]).with_kind(
            Arc::new(WithKeys {
                keys: &[GIZMO, "workshop.parts"],
                flag: Some(&TEST_FLAG),
            }),
            &[GADGET, GIZMO],
        );
        let keys: Vec<&str> = registry.resolvable_nodes().properties_keys().collect();
        assert_eq!(keys[dbt_keys..], [WIDGET, GIZMO, "workshop.parts"]);
        assert!(
            registry
                .known_flags()
                .iter()
                .any(|flag| flag.name == TEST_FLAG.name)
        );
        assert_eq!(registry.injected_tasks().len(), 1);
    }

    /// A kind added with its tasks resolves in the order it was added, its flag is known, and
    /// the task graph gets its tasks and the phases of its command.
    #[test]
    fn a_kind_with_tasks_resolves_and_gives_the_graph_its_tasks() {
        let dbt_keys = ResolvableNodes::dbt().properties_keys().count();
        let registry = InjectedKinds::default()
            .with_tasks(Arc::new(WithTasks {
                command: Some("gadgets"),
                ..WithTasks::new(GADGET)
            }))
            .with_kind(
                Arc::new(WithKeys {
                    keys: &[GIZMO],
                    flag: Some(&TEST_FLAG),
                }),
                &[GIZMO],
            );
        let keys: Vec<&str> = registry.resolvable_nodes().properties_keys().collect();
        assert_eq!(keys[dbt_keys..], [GADGET, GIZMO]);
        assert!(
            registry
                .known_flags()
                .iter()
                .any(|flag| flag.name == TEST_FLAG.name)
        );
        let tasks = registry.injected_tasks();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].kinds(), [GADGET]);
        assert_eq!(
            command_phases(FsCommand::Extension("gadgets"), tasks),
            Some(&[TP::Run][..])
        );
    }

    #[test]
    #[should_panic(expected = "kind `widget` is registered twice")]
    fn a_kind_adds_nodes_of_kinds_that_nothing_else_adds() {
        with_tasks(&[WIDGET]).with_kind(
            Arc::new(WithKeys {
                keys: &[GIZMO],
                flag: Some(&TEST_FLAG),
            }),
            &[GADGET, WIDGET],
        );
    }

    /// A kind has one source of tasks.
    #[test]
    #[should_panic(expected = "kind `widget` is registered twice")]
    fn a_kind_has_one_source_of_tasks() {
        with_tasks(&[WIDGET]).with_tasks(Arc::new(WithTasks {
            keys: own_key(GIZMO),
            ..WithTasks::new(WIDGET)
        }));
    }

    #[test]
    #[should_panic(expected = "`widget` adds the command `lineage`, a command of dbt")]
    fn a_features_command_is_not_a_command_of_dbt() {
        InjectedKinds::default().with_tasks(Arc::new(WithTasks {
            command: Some("lineage"),
            ..WithTasks::new(WIDGET)
        }));
    }

    #[test]
    #[should_panic(expected = "the command `widgets` is added twice")]
    fn a_command_is_added_by_one_feature() {
        InjectedKinds::default()
            .with_tasks(Arc::new(WithTasks {
                command: Some("widgets"),
                ..WithTasks::new(WIDGET)
            }))
            .with_tasks(Arc::new(WithTasks {
                command: Some("widgets"),
                ..WithTasks::new(GADGET)
            }));
    }

    #[test]
    fn a_kind_reads_its_keys() {
        reads(&[WIDGET, GIZMO]);
    }

    #[test]
    #[should_panic(expected = "`gadget` is read by more than one kind")]
    fn a_key_is_read_by_one_kind() {
        with_tasks(&[GADGET]).with_kind(
            Arc::new(WithKeys {
                keys: &[GADGET],
                flag: Some(&TEST_FLAG),
            }),
            &[WIDGET],
        );
    }

    #[test]
    #[should_panic(expected = "kind `widget` reads `models`, a key dbt reads")]
    fn a_key_cannot_be_a_key_dbt_reads() {
        reads(&["models"]);
    }

    #[test]
    fn a_key_can_be_a_key_of_a_section() {
        let keys: Vec<&str> = reads(&["workshop.parts"])
            .resolvable_nodes()
            .properties_keys()
            .collect();
        assert_eq!(keys.last(), Some(&"workshop.parts"));
    }

    #[test]
    #[should_panic(expected = "kind `widget` reads `models.parts`, a key dbt reads")]
    fn a_section_cannot_be_a_key_dbt_reads() {
        reads(&["models.parts"]);
    }

    #[test]
    #[should_panic(expected = "key `Parts` is not a lowercase identifier")]
    fn a_key_of_a_section_is_a_lowercase_identifier() {
        reads(&["workshop.Parts"]);
    }

    #[test]
    #[should_panic(expected = "key `parts.bolts` is not a lowercase identifier")]
    fn a_section_has_no_sections() {
        reads(&["workshop.parts.bolts"]);
    }

    #[test]
    #[should_panic(expected = "kind `widget` must have a feature flag")]
    fn a_kind_has_a_flag() {
        with_flag(&[WIDGET], None);
    }

    #[test]
    #[should_panic(expected = "must be off by default")]
    fn a_kinds_flag_is_off_by_default() {
        with_flag(&[WIDGET], Some(&ON_BY_DEFAULT));
    }

    #[test]
    #[should_panic(expected = "must be an experimental or beta flag")]
    fn a_kinds_flag_is_not_a_behavior_flag() {
        with_flag(&[WIDGET], Some(&BEHAVIOR));
    }

    /// The flag of a kind is a known feature flag, so `--beta-features` turns it on without
    /// an "Unknown beta feature" warning.
    #[test]
    fn the_flags_of_kinds_are_known() {
        let snapshot = dbt_flags::FlagSnapshot::builder()
            .cli(["test_injected_kinds"])
            .build();
        let without = InjectedKinds::default();
        assert_eq!(
            snapshot.warnings(&without.known_flags()),
            ["Unknown beta feature 'test_injected_kinds' in --beta-features; it has no effect"]
        );
        let with = with_tasks(&[WIDGET]);
        assert!(snapshot.warnings(&with.known_flags()).is_empty());
    }

    /// A table of one column, which a feature injects.
    struct TestTable(&'static str);

    impl InjectedTable for TestTable {
        fn flag(&self) -> &'static FlagDef {
            &TEST_FLAG
        }
        fn ns(&self) -> info_schema::Ns {
            info_schema::Ns::DbtRt
        }
        fn name(&self) -> &'static str {
            self.0
        }
        fn schema(&self) -> arrow_schema::SchemaRef {
            use arrow_schema::{DataType, Field, Schema};
            Arc::new(Schema::new(vec![Field::new(
                "widget",
                DataType::Utf8,
                true,
            )]))
        }
        fn select(
            &self,
            _metadata_dir: &std::path::Path,
        ) -> Result<Option<String>, info_schema::IndexError> {
            Ok(None)
        }
    }

    static WIDGET_RUNS: TestTable = TestTable("widget_runs");
    static GADGET_RUNS: TestTable = TestTable("gadget_runs");
    static RUN_RESULTS: TestTable = TestTable("run_results");
    static INJECTED_TABLES: [&dyn InjectedTable; 2] = [&WIDGET_RUNS, &GADGET_RUNS];
    static CLASHES_WITH_THE_CATALOG: [&dyn InjectedTable; 1] = [&RUN_RESULTS];
    static SAME_NAME: [&dyn InjectedTable; 2] = [&WIDGET_RUNS, &WIDGET_RUNS];

    /// A build injects no table into the information schema unless it declares one.
    #[test]
    fn the_injected_tables_are_the_declared_ones() {
        assert!(InjectedKinds::default().injected_tables().is_empty());
        let registry = InjectedKinds::default().with_injected_tables(&INJECTED_TABLES);
        let names: Vec<String> = registry
            .injected_tables()
            .iter()
            .map(|table| table.qualified_name())
            .collect();
        assert_eq!(names, ["dbt_rt.widget_runs", "dbt_rt.gadget_runs"]);
    }

    #[test]
    #[should_panic(expected = "the information schema has a table of that name")]
    fn an_injected_table_is_not_a_table_of_the_catalog() {
        InjectedKinds::default().with_injected_tables(&CLASHES_WITH_THE_CATALOG);
    }

    #[test]
    #[should_panic(expected = "the injected table `dbt_rt.widget_runs` is declared twice")]
    fn an_injected_table_is_declared_once() {
        InjectedKinds::default().with_injected_tables(&SAME_NAME);
    }

    #[test]
    fn a_kind_of_node_cannot_be_a_name_dbt_uses() {
        for name in [
            "unit_test",
            "model",
            "experimental",
            "group",
            "doc",
            "relation",
            "Widget",
        ] {
            let result = std::panic::catch_unwind(|| {
                InjectedKinds::default().with_kind(
                    Arc::new(WithKeys {
                        keys: &[],
                        flag: Some(&TEST_FLAG),
                    }),
                    &[name],
                )
            });
            assert!(result.is_err(), "{name} should be rejected");
        }
    }
}
