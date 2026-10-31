use dbt_tasks_sa::plugin::Plugins;

/// The plugins that add their own kinds of node to a project.
#[derive(Default)]
pub struct PluginFeature {
    pub plugins: Plugins,
}
