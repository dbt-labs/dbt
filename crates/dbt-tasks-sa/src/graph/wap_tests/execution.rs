use super::*;
use crate::visitor::wap_test_support::ScriptedSchedule;
use crate::wap::require_passing_audits;
use dbt_tasks_core::visitor::SkipReason;

const MODEL: &str = "model.pkg.orders";
const AUDIT: &str = "test.pkg.orders_not_null";
const SECOND_AUDIT: &str = "test.pkg.orders_nonnegative";
const DOWNSTREAM: &str = "model.pkg.downstream";
const DOWNSTREAM_AUDIT: &str = "test.pkg.downstream_not_null";
const INDEPENDENT: &str = "model.pkg.independent";

struct Fixture {
    graph: DiGraph<Arc<dyn Task>, ()>,
}

impl Fixture {
    fn new(wap: bool, chained: bool) -> Self {
        let mut nodes = Nodes::default();
        let mut deps = BTreeMap::new();
        for (id, upstream) in [
            (MODEL, None),
            (DOWNSTREAM, Some(MODEL)),
            (INDEPENDENT, None),
        ] {
            let mut model = DbtModel::default();
            model.__common_attr__.unique_id = id.to_owned();
            nodes.models.insert(id.to_owned(), Arc::new(model));
            deps.insert(
                id.to_owned(),
                upstream.into_iter().map(str::to_owned).collect(),
            );
        }
        for (id, owner) in [(AUDIT, MODEL), (SECOND_AUDIT, MODEL)]
            .into_iter()
            .chain(chained.then_some((DOWNSTREAM_AUDIT, DOWNSTREAM)))
        {
            let mut test = DbtTest::default();
            test.__common_attr__.unique_id = id.to_owned();
            nodes.tests.insert(id.to_owned(), Arc::new(test));
            deps.insert(id.to_owned(), BTreeSet::from([owner.to_owned()]));
        }
        let schedule = Schedule {
            selected_nodes: deps.keys().cloned().collect(),
            all_selected_nodes: deps.keys().cloned().collect(),
            sorted_nodes: deps.keys().cloned().collect(),
            deps,
            ..Default::default()
        };
        let mut plan = WapPlan::default();
        if wap {
            let mut model = entry(MODEL, AUDIT);
            model.audit_ids.insert(SECOND_AUDIT.to_owned());
            plan.models.insert(MODEL.to_owned(), model);
        }
        if chained {
            plan.models
                .insert(DOWNSTREAM.to_owned(), entry(DOWNSTREAM, DOWNSTREAM_AUDIT));
        }
        let (graph, _) = GraphBuilder::build_phased_task_graph(
            &schedule,
            &PhasedFactory,
            &nodes,
            Execute::Remote,
            PHASES_RENDER_ANALYZE_RUN,
            &StrictBuckets {
                off: true,
                ..Default::default()
            },
            false,
            None,
            &plan,
        )
        .unwrap();
        Self { graph }
    }

    fn index(&self, id: &str, task_type: &str) -> NodeIndex {
        self.graph
            .node_indices()
            .find(|index| {
                let task = &self.graph[*index];
                task.work_node_id() == id && task.task_type() == task_type
            })
            .unwrap_or_else(|| panic!("missing task {id}/{task_type}"))
    }

    /// Complete compilation work while leaving every warehouse task in flight.
    fn start_runs(&self, schedule: &mut ScriptedSchedule<'_>) -> BTreeSet<NodeIndex> {
        let mut running = BTreeSet::new();
        loop {
            let ready = schedule.start_ready();
            if ready.is_empty() {
                return running;
            }
            for index in ready {
                if self.graph[index].task_phase() == Some(TP::Run) {
                    running.insert(index);
                } else {
                    schedule.complete(index, Ok(NodeStatus::Succeeded));
                }
            }
        }
    }

    fn start_stage(&self) -> ScriptedSchedule<'_> {
        let mut schedule = ScriptedSchedule::new(&self.graph);
        assert_eq!(
            self.start_runs(&mut schedule),
            BTreeSet::from([self.index(MODEL, "run"), self.index(INDEPENDENT, "run")])
        );
        schedule.complete(self.index(INDEPENDENT, "run"), Ok(NodeStatus::Succeeded));
        schedule
    }

    fn stage_succeeded(&self, schedule: &mut ScriptedSchedule<'_>) {
        schedule.complete(self.index(MODEL, "run"), Ok(NodeStatus::Succeeded));
        assert_eq!(
            self.start_runs(schedule),
            BTreeSet::from([self.index(AUDIT, "run"), self.index(SECOND_AUDIT, "run")])
        );
    }
}

#[test]
fn wap_publication_waits_for_every_audit_completion() {
    let fixture = Fixture::new(true, false);
    let mut schedule = fixture.start_stage();
    fixture.stage_succeeded(&mut schedule);

    schedule.complete(fixture.index(AUDIT, "run"), Ok(NodeStatus::TestPassed));
    assert!(
        schedule.start_ready().is_empty(),
        "second audit is still running"
    );
    schedule.complete(
        fixture.index(SECOND_AUDIT, "run"),
        Ok(NodeStatus::TestPassed),
    );
    let publication = fixture.index(MODEL, "wap_publish_run");
    assert_eq!(schedule.start_ready(), vec![publication]);
    assert!(
        schedule.start_ready().is_empty(),
        "publication is still running"
    );

    let result = require_passing_audits([
        (AUDIT, Some(NodeStatus::TestPassed)),
        (SECOND_AUDIT, Some(NodeStatus::TestPassed)),
    ])
    .map(|()| NodeStatus::Succeeded);
    schedule.complete(publication, result);
    assert_eq!(
        fixture.start_runs(&mut schedule),
        BTreeSet::from([fixture.index(DOWNSTREAM, "run")])
    );
    schedule.complete(fixture.index(DOWNSTREAM, "run"), Ok(NodeStatus::Succeeded));
    assert!(schedule.start_ready().is_empty());
    schedule.assert_finished();
}

#[test]
fn wap_failed_transformation_skips_audits_publication_and_downstream() {
    for result in [
        Ok(NodeStatus::Errored),
        Err(fs_err!(ErrorCode::ExecutionError, "transformation failed")),
    ] {
        let fixture = Fixture::new(true, false);
        let mut schedule = fixture.start_stage();
        schedule.complete(fixture.index(MODEL, "run"), result);

        assert!(schedule.start_ready().is_empty());
        for id in [AUDIT, SECOND_AUDIT, DOWNSTREAM] {
            for phase in ["render", "run"] {
                assert_eq!(
                    schedule.skip_reason(fixture.index(id, phase)),
                    Some(&SkipReason::FailedUpstream(MODEL.to_owned()))
                );
            }
        }
        assert_eq!(
            schedule.skip_reason(fixture.index(MODEL, "wap_publish_run")),
            Some(&SkipReason::FailedPhase)
        );
        assert!(
            schedule
                .skip_reason(fixture.index(INDEPENDENT, "run"))
                .is_none()
        );
        schedule.assert_finished();
    }
}

#[test]
fn wap_audit_failure_keeps_publication_blocked_after_other_audits_finish() {
    let fixture = Fixture::new(true, false);
    let mut schedule = fixture.start_stage();
    fixture.stage_succeeded(&mut schedule);

    schedule.complete(fixture.index(AUDIT, "run"), Ok(NodeStatus::Errored));
    assert!(schedule.start_ready().is_empty());
    schedule.complete(
        fixture.index(SECOND_AUDIT, "run"),
        Ok(NodeStatus::TestPassed),
    );

    assert!(schedule.start_ready().is_empty());
    for index in [
        fixture.index(MODEL, "wap_publish_run"),
        fixture.index(DOWNSTREAM, "render"),
        fixture.index(DOWNSTREAM, "run"),
    ] {
        assert_eq!(
            schedule.skip_reason(index),
            Some(&SkipReason::FailedUpstream(AUDIT.to_owned()))
        );
    }
    schedule.assert_finished();
}

#[test]
fn wap_warned_audit_fails_publication_gate_and_blocks_downstream() {
    let fixture = Fixture::new(true, false);
    let mut schedule = fixture.start_stage();
    fixture.stage_succeeded(&mut schedule);
    schedule.complete(fixture.index(AUDIT, "run"), Ok(NodeStatus::TestWarned));
    schedule.complete(
        fixture.index(SECOND_AUDIT, "run"),
        Ok(NodeStatus::TestPassed),
    );
    let publication = fixture.index(MODEL, "wap_publish_run");
    assert_eq!(schedule.start_ready(), vec![publication]);

    // WARN is intentionally nonfatal to the visitor. The real WAP gate must
    // still turn it into a publication error before consumers can run.
    let result = require_passing_audits([
        (AUDIT, Some(NodeStatus::TestWarned)),
        (SECOND_AUDIT, Some(NodeStatus::TestPassed)),
    ])
    .map(|()| NodeStatus::Succeeded);
    assert!(result.is_err());
    schedule.complete(publication, result);

    assert!(schedule.start_ready().is_empty());
    assert_eq!(
        schedule.skip_reason(fixture.index(DOWNSTREAM, "render")),
        Some(&SkipReason::FailedUpstream(MODEL.to_owned()))
    );
    schedule.assert_finished();
}

#[test]
fn wap_chain_starts_next_candidate_only_after_upstream_publication() {
    let fixture = Fixture::new(true, true);
    let mut schedule = fixture.start_stage();
    fixture.stage_succeeded(&mut schedule);
    for id in [AUDIT, SECOND_AUDIT] {
        schedule.complete(fixture.index(id, "run"), Ok(NodeStatus::TestPassed));
    }
    let first_publication = fixture.index(MODEL, "wap_publish_run");
    assert_eq!(schedule.start_ready(), vec![first_publication]);
    schedule.complete(first_publication, Ok(NodeStatus::Succeeded));
    assert_eq!(
        fixture.start_runs(&mut schedule),
        BTreeSet::from([fixture.index(DOWNSTREAM, "run")])
    );
    schedule.complete(fixture.index(DOWNSTREAM, "run"), Ok(NodeStatus::Succeeded));
    assert_eq!(
        fixture.start_runs(&mut schedule),
        BTreeSet::from([fixture.index(DOWNSTREAM_AUDIT, "run")])
    );
    schedule.complete(
        fixture.index(DOWNSTREAM_AUDIT, "run"),
        Ok(NodeStatus::TestPassed),
    );
    let second_publication = fixture.index(DOWNSTREAM, "wap_publish_run");
    assert_eq!(schedule.start_ready(), vec![second_publication]);
    schedule.complete(second_publication, Ok(NodeStatus::Succeeded));
    assert!(schedule.start_ready().is_empty());
    schedule.assert_finished();
}

#[test]
fn wap_failure_skips_entire_downstream_wap_cycle() {
    let fixture = Fixture::new(true, true);
    let mut schedule = fixture.start_stage();
    fixture.stage_succeeded(&mut schedule);
    schedule.complete(fixture.index(AUDIT, "run"), Ok(NodeStatus::Errored));
    schedule.complete(
        fixture.index(SECOND_AUDIT, "run"),
        Ok(NodeStatus::TestPassed),
    );

    assert!(schedule.start_ready().is_empty());
    for (id, phase) in [
        (DOWNSTREAM, "render"),
        (DOWNSTREAM, "run"),
        (DOWNSTREAM_AUDIT, "render"),
        (DOWNSTREAM_AUDIT, "run"),
        (DOWNSTREAM, "wap_publish_run"),
    ] {
        assert_eq!(
            schedule.skip_reason(fixture.index(id, phase)),
            Some(&SkipReason::FailedUpstream(AUDIT.to_owned()))
        );
    }
    schedule.assert_finished();
}

#[test]
fn wap_disabled_preserves_normal_warning_behavior_without_publication_task() {
    let fixture = Fixture::new(false, false);
    assert!(
        !fixture
            .graph
            .node_weights()
            .any(|task| task.task_type() == "wap_publish_run")
    );
    let mut schedule = fixture.start_stage();
    fixture.stage_succeeded(&mut schedule);
    schedule.complete(fixture.index(AUDIT, "run"), Ok(NodeStatus::TestWarned));
    schedule.complete(
        fixture.index(SECOND_AUDIT, "run"),
        Ok(NodeStatus::TestPassed),
    );

    assert_eq!(
        fixture.start_runs(&mut schedule),
        BTreeSet::from([fixture.index(DOWNSTREAM, "run")])
    );
    schedule.complete(fixture.index(DOWNSTREAM, "run"), Ok(NodeStatus::Succeeded));
    assert!(schedule.start_ready().is_empty());
    schedule.assert_finished();
}
