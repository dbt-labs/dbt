use dbt_tasks_sa::experimental::Experimentals;

/// The experimentals that add their own kinds of node to a project.
#[derive(Default)]
pub struct ExperimentalFeature {
    pub experimentals: Experimentals,
}
