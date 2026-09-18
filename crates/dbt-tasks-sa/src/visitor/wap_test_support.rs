//! Drive production scheduling decisions with explicitly ordered task completions.
//!
//! These tests replace warehouse work, not dependency readiness or failure propagation.

use super::*;

pub(crate) struct ScriptedSchedule<'a> {
    graph: &'a DiGraph<Arc<dyn Task>, ()>,
    strategy: VisitStrategy,
    indegree: Vec<i32>,
    dependents: Vec<HashSet<NodeIndex>>,
    pending: Vec<NodeIndex>,
    waiting: HashMap<NodeIndex, tracing::Span>,
    skip_set: SkipSet,
}

impl<'a> ScriptedSchedule<'a> {
    pub(crate) fn new(graph: &'a DiGraph<Arc<dyn Task>, ()>) -> Self {
        let strategy = VisitStrategy::Parallel;
        let (indegree, dependents) = get_indegree_and_dependents(graph);
        Self {
            graph,
            strategy,
            indegree,
            dependents,
            pending: strategy.new_pending_nodes(graph).unwrap(),
            waiting: HashMap::new(),
            skip_set: SkipSet::new(),
        }
    }

    pub(crate) fn start_ready(&mut self) -> Vec<NodeIndex> {
        let mut started = Vec::new();
        while let Some(index) = self
            .strategy
            .try_pop_pending_node(&self.waiting, &mut self.pending)
        {
            if self.skip_set.skip.contains_key(&index) {
                self.strategy.release_dependents(
                    index,
                    &self.dependents,
                    &mut self.indegree,
                    &mut self.pending,
                );
            } else {
                assert!(self.waiting.insert(index, tracing::Span::none()).is_none());
                started.push(index);
            }
        }
        started
    }

    pub(crate) fn complete(&mut self, index: NodeIndex, result: FsResult<NodeStatus>) {
        assert!(
            self.waiting.remove(&index).is_some(),
            "task was not running"
        );
        self.skip_set
            .handle_task_result(result, index, &self.dependents, self.graph, false, false);
        self.strategy.release_dependents(
            index,
            &self.dependents,
            &mut self.indegree,
            &mut self.pending,
        );
    }

    pub(crate) fn skip_reason(&self, index: NodeIndex) -> Option<&SkipReason> {
        self.skip_set.skip.get(&index)
    }

    pub(crate) fn assert_finished(&self) {
        assert!(self.pending.is_empty());
        assert!(self.waiting.is_empty());
        assert!(
            self.strategy
                .get_incomplete_tasks(self.graph, &self.indegree)
                .is_empty()
        );
    }
}
