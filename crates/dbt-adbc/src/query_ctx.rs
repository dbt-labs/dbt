/// Context carrying metadata associated with a query.
#[derive(Clone, Debug, Default)]
pub struct QueryCtx {
    // Model executing this query
    node_unique_id: Option<String>,
    // Unique id this query targets, mirroring the Jinja `TARGET_UNIQUE_ID` key. Equal to
    // `node_unique_id` for ordinary work; see `target_unique_id` for when the two diverge.
    target_unique_id: Option<String>,
    // Execution Phase
    phase: Option<&'static str>,
    // Description (abribrary string) associated with the query
    desc: Option<String>,
    // Whether the query is for metadata fetch (schema hydration)
    metadata: bool,
    // dbt project (package) name issuing the query, for usage attribution.
    project_name: Option<String>,
    // dbt model (node) name issuing the query, for usage attribution.
    model_name: Option<String>,
    // dbt invocation id issuing the query, for usage attribution.
    run_id: Option<String>,
}

impl QueryCtx {
    /// Create a new Query Context with a description.
    pub fn new(description: impl Into<String>) -> Self {
        QueryCtx {
            node_unique_id: None,
            target_unique_id: None,
            phase: None,
            desc: Some(description.into()),
            metadata: false,
            project_name: None,
            model_name: None,
            run_id: None,
        }
    }

    /// Create a new Query Context for metadata purposes.
    pub fn new_metadata() -> Self {
        QueryCtx {
            node_unique_id: None,
            target_unique_id: None,
            phase: None,
            desc: None,
            metadata: true,
            project_name: None,
            model_name: None,
            run_id: None,
        }
    }

    /// Set the unique node id associated with this context.
    ///
    /// Re-assigning the node id will panic in debug builds.
    pub fn with_node_id(mut self, node_unique_id: impl Into<String>) -> Self {
        debug_assert!(
            self.node_unique_id.is_none(),
            "unexpected reassignment of node_unique_id"
        );
        self.node_unique_id = Some(node_unique_id.into());
        self
    }

    /// Set the unique id this query targets, mirroring the Jinja `TARGET_UNIQUE_ID` key.
    ///
    /// Re-assigning the target id will panic in debug builds.
    pub fn with_target_unique_id(mut self, target_unique_id: impl Into<String>) -> Self {
        debug_assert!(
            self.target_unique_id.is_none(),
            "unexpected reassignment of target_unique_id"
        );
        self.target_unique_id = Some(target_unique_id.into());
        self
    }

    pub fn with_desc(mut self, desc: impl Into<String>) -> Self {
        self.set_desc(desc.into());
        self
    }

    pub fn set_desc(&mut self, desc: impl Into<String>) {
        debug_assert!(
            self.desc.is_none(),
            "unexpected reassignment of description"
        );
        self.desc = Some(desc.into());
    }

    pub fn with_phase(mut self, phase: &'static str) -> Self {
        self.phase = Some(phase);
        self
    }

    /// Set the dbt project (package) name issuing this query.
    pub fn with_project_name(mut self, project_name: impl Into<String>) -> Self {
        self.project_name = Some(project_name.into());
        self
    }

    /// Set the dbt model (node) name issuing this query.
    pub fn with_model_name(mut self, model_name: impl Into<String>) -> Self {
        self.model_name = Some(model_name.into());
        self
    }

    /// Set the dbt invocation id issuing this query.
    pub fn with_run_id(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }

    pub fn is_metadata(&self) -> bool {
        self.metadata
    }

    /// Return unique node id associated with this context.
    ///
    /// This is the identity of the node whose execution issued the query, and it is the id
    /// anything keyed on node identity must use: connection pooling, telemetry, and the
    /// recording keys of cross-version record/replay.
    pub fn node_id(&self) -> Option<&String> {
        self.node_unique_id.as_ref()
    }

    /// Return the unique id this query targets, if it was set.
    ///
    /// Mirrors the Jinja `TARGET_UNIQUE_ID` key. This equals [`Self::node_id`] for ordinary
    /// work. It differs when a node materializes an additional relation on its own behalf and
    /// that relation's adapter calls have to be told apart from the node's own — a versioned
    /// model with an explicit `latest_version` also materializes the un-versioned pointer view.
    ///
    /// Only consumers that need to distinguish those two units of work should read this;
    /// see [`Self::node_id`] for everything keyed on node identity.
    pub fn target_unique_id(&self) -> Option<&String> {
        self.target_unique_id.as_ref()
    }

    /// The id to attribute this query to when telling a node's own adapter calls apart from
    /// those of an additional relation it materializes: [`Self::target_unique_id`] when set,
    /// otherwise [`Self::node_id`].
    ///
    /// Returns `None` exactly when [`Self::node_id`] does, so callers that treat a missing node
    /// id as "global context, nothing to attribute this to" keep that behaviour. A target id
    /// refines a node's identity; it never stands in for one that isn't there.
    pub fn target_or_node_id(&self) -> Option<&String> {
        let node_unique_id = self.node_unique_id.as_ref()?;
        Some(self.target_unique_id.as_ref().unwrap_or(node_unique_id))
    }

    /// Returns a clone of the description associated with the
    /// context.
    pub fn desc(&self) -> Option<&String> {
        self.desc.as_ref()
    }

    /// Returns the Execution Phase
    pub fn phase(&self) -> Option<&'static str> {
        self.phase
    }

    /// Returns the dbt project (package) name issuing this query, if set.
    pub fn project_name(&self) -> Option<&String> {
        self.project_name.as_ref()
    }

    /// Returns the dbt model (node) name issuing this query, if set.
    pub fn model_name(&self) -> Option<&String> {
        self.model_name.as_ref()
    }

    /// Returns the dbt invocation id issuing this query, if set.
    pub fn run_id(&self) -> Option<&String> {
        self.run_id.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_desc() {
        let query_ctx = QueryCtx::default().with_desc("this is a really good query");
        assert_eq!(query_ctx.desc().unwrap(), "this is a really good query");
    }

    #[test]
    #[should_panic]
    fn test_desc_twice() {
        QueryCtx::default().with_desc("abc").with_desc("123");
    }

    #[test]
    fn test_unique_id() {
        let query_ctx = QueryCtx::default().with_node_id("123");
        assert_eq!(query_ctx.node_id().unwrap(), "123");
    }

    #[test]
    #[should_panic]
    fn test_unique_id_twice() {
        QueryCtx::default().with_node_id("123").with_node_id("abc");
    }

    #[test]
    fn test_target_unique_id_refines_the_node_id() {
        let query_ctx = QueryCtx::default()
            .with_node_id("model.pkg.m.v1")
            .with_target_unique_id("model.pkg.m.v1__latest_version_pointer");
        assert_eq!(query_ctx.node_id().unwrap(), "model.pkg.m.v1");
        assert_eq!(
            query_ctx.target_or_node_id().unwrap(),
            "model.pkg.m.v1__latest_version_pointer"
        );
    }

    #[test]
    fn test_target_or_node_id_falls_back_to_the_node_id() {
        let query_ctx = QueryCtx::default().with_node_id("model.pkg.m");
        assert_eq!(query_ctx.target_or_node_id().unwrap(), "model.pkg.m");
    }

    /// A target id must never stand in for an absent node id: callers read `None` as "global
    /// context", and render-time contexts can carry `TARGET_UNIQUE_ID` with no node at all.
    #[test]
    fn test_target_or_node_id_is_none_without_a_node_id() {
        let query_ctx = QueryCtx::default().with_target_unique_id("model.pkg.m");
        assert_eq!(query_ctx.target_or_node_id(), None);
    }
}
