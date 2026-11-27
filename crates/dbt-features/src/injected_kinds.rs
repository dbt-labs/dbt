use dbt_tasks_sa::injected_kinds::InjectedKinds;

/// The kinds of node that features add to a project, with what else they add.
#[derive(Default)]
pub struct InjectedKindsFeature {
    pub kinds: InjectedKinds,
}
