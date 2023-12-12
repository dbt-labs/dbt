import logging
import re
from argparse import Namespace
from typing import TypeVar

import pytest

from dbt.adapters.events import types as adapter_types
from dbt.common.events.event_manager_client import ctx_set_event_manager
from dbt.contracts.results import TimingInfo, RunResult, RunStatus
from dbt.common.events import types
from dbt.adapters.events.logging import AdapterLogger
from dbt.common.events.base_types import msg_from_base_event
from dbt.events import types as core_types
from dbt.events.base_types import (
    CoreBaseEvent,
    DebugLevel,
    DynamicLevel,
    ErrorLevel,
    InfoLevel,
    TestLevel,
    WarnLevel,
)
from dbt.common.events.event_manager import TestEventManager, EventManager
from dbt.common.events.functions import msg_to_dict, msg_to_json
from dbt.common.events.helpers import get_json_string_utcnow
from dbt.common.events.types import RunResultError
from dbt.flags import set_from_args
from dbt.task.printer import print_run_result_error

set_from_args(Namespace(WARN_ERROR=False), None)


# takes in a class and finds any subclasses for it
def get_all_subclasses(cls):
    all_subclasses = []
    for subclass in cls.__subclasses__():
        if subclass not in [TestLevel, DebugLevel, WarnLevel, InfoLevel, ErrorLevel, DynamicLevel]:
            all_subclasses.append(subclass)
        all_subclasses.extend(get_all_subclasses(subclass))
    return set(all_subclasses)


class TestAdapterLogger:
    # this interface is documented for adapter maintainers to plug into
    # so we should test that it at the very least doesn't explode.
    def test_basic_adapter_logging_interface(self):
        logger = AdapterLogger("dbt_tests")
        logger.debug("debug message")
        logger.info("info message")
        logger.warning("warning message")
        logger.error("error message")
        logger.exception("exception message")
        logger.critical("exception message")

    # python loggers allow deferring string formatting via this signature:
    def test_formatting(self):
        logger = AdapterLogger("dbt_tests")
        # tests that it doesn't throw
        logger.debug("hello {}", "world")

        # enters lower in the call stack to test that it formats correctly
        event = adapter_types.AdapterEventDebug(
            name="dbt_tests", base_msg="hello {}", args=["world"]
        )
        assert "hello world" in event.message()

        # tests that it doesn't throw
        logger.debug("1 2 {}", "3")

        # enters lower in the call stack to test that it formats correctly
        event = adapter_types.AdapterEventDebug(name="dbt_tests", base_msg="1 2 {}", args=[3])
        assert "1 2 3" in event.message()

        # tests that it doesn't throw
        logger.debug("boop{x}boop")

        # enters lower in the call stack to test that it formats correctly
        # in this case it's that we didn't attempt to replace anything since there
        # were no args passed after the initial message
        event = adapter_types.AdapterEventDebug(name="dbt_tests", base_msg="boop{x}boop", args=[])
        assert "boop{x}boop" in event.message()

        # ensure AdapterLogger and subclasses makes all base_msg members
        # of type string; when someone writes logger.debug(a) where a is
        # any non-string object
        event = adapter_types.AdapterEventDebug(name="dbt_tests", base_msg=[1, 2, 3], args=[3])
        assert isinstance(event.base_msg, str)

        event = types.JinjaLogDebug(msg=[1, 2, 3])
        assert isinstance(event.msg, str)

    def test_set_adapter_dependency_log_level(self):
        logger = AdapterLogger("dbt_tests")
        package_log = logging.getLogger("test_package_log")
        logger.set_adapter_dependency_log_level("test_package_log", "DEBUG")
        package_log.debug("debug message")


class TestEventCodes:

    # checks to see if event codes are duplicated to keep codes singluar and clear.
    # also checks that event codes follow correct namming convention ex. E001
    def test_event_codes(self):
        all_concrete = get_all_subclasses(CoreBaseEvent)
        all_codes = set()

        for event_cls in all_concrete:
            code = event_cls.code(event_cls)
            # must be in the form 1 capital letter, 3 digits
            assert re.match("^[A-Z][0-9]{3}", code)
            # cannot have been used already
            assert (
                code not in all_codes
            ), f"{code} is assigned more than once. Check types.py for duplicates."
            all_codes.add(code)


sample_values = [
    # N.B. Events instantiated here include the module prefix in order to
    # avoid having the entire list twice in the code.
    # A - pre-project loading
    types.MainReportVersion(version=""),
    types.MainReportArgs(args={}),
    types.MainTrackingUserState(user_state=""),
    types.MergedFromState(num_merged=0, sample=[]),
    types.MissingProfileTarget(profile_name="", target_name=""),
    types.InvalidOptionYAML(option_name="vars"),
    types.LogDbtProjectError(),
    types.LogDbtProfileError(),
    types.StarterProjectPath(dir=""),
    types.ConfigFolderDirectory(dir=""),
    types.NoSampleProfileFound(adapter=""),
    types.ProfileWrittenWithSample(name="", path=""),
    types.ProfileWrittenWithTargetTemplateYAML(name="", path=""),
    types.ProfileWrittenWithProjectTemplateYAML(name="", path=""),
    types.SettingUpProfile(),
    types.InvalidProfileTemplateYAML(),
    types.ProjectNameAlreadyExists(name=""),
    types.ProjectCreated(project_name=""),
    # E - Adapter Events ======================
    core_types.AdapterRegistered(adapter_name="dbt-awesome", adapter_version="1.2.3"),
    # D - Deprecations ======================
    core_types.PackageRedirectDeprecation(old_name="", new_name=""),
    core_types.PackageInstallPathDeprecation(),
    core_types.ConfigSourcePathDeprecation(deprecated_path="", exp_path=""),
    core_types.ConfigDataPathDeprecation(deprecated_path="", exp_path=""),
    adapter_types.AdapterDeprecationWarning(old_name="", new_name=""),
    core_types.MetricAttributesRenamed(metric_name=""),
    core_types.ExposureNameDeprecation(exposure=""),
    core_types.InternalDeprecation(name="", reason="", suggested_action="", version=""),
    core_types.EnvironmentVariableRenamed(old_name="", new_name=""),
    core_types.ConfigLogPathDeprecation(deprecated_path=""),
    core_types.ConfigTargetPathDeprecation(deprecated_path=""),
    adapter_types.CollectFreshnessReturnSignature(),
    # E - DB Adapter ======================
    adapter_types.AdapterEventDebug(),
    adapter_types.AdapterEventInfo(),
    adapter_types.AdapterEventWarning(),
    adapter_types.AdapterEventError(),
    adapter_types.NewConnection(conn_type="", conn_name=""),
    adapter_types.ConnectionReused(conn_name=""),
    adapter_types.ConnectionLeftOpenInCleanup(conn_name=""),
    adapter_types.ConnectionClosedInCleanup(conn_name=""),
    adapter_types.RollbackFailed(conn_name=""),
    adapter_types.ConnectionClosed(conn_name=""),
    adapter_types.ConnectionLeftOpen(conn_name=""),
    adapter_types.Rollback(conn_name=""),
    adapter_types.CacheMiss(conn_name="", database="", schema=""),
    adapter_types.ListRelations(database="", schema=""),
    adapter_types.ConnectionUsed(conn_type="", conn_name=""),
    adapter_types.SQLQuery(conn_name="", sql=""),
    adapter_types.SQLQueryStatus(status="", elapsed=0.1),
    adapter_types.SQLCommit(conn_name=""),
    adapter_types.ColTypeChange(
        orig_type="",
        new_type="",
        table={"database": "", "schema": "", "identifier": ""},
    ),
    adapter_types.SchemaCreation(relation={"database": "", "schema": "", "identifier": ""}),
    adapter_types.SchemaDrop(relation={"database": "", "schema": "", "identifier": ""}),
    adapter_types.CacheAction(
        action="adding_relation",
        ref_key={"database": "", "schema": "", "identifier": ""},
        ref_key_2={"database": "", "schema": "", "identifier": ""},
    ),
    adapter_types.CacheDumpGraph(before_after="before", action="rename", dump=dict()),
    adapter_types.AdapterImportError(exc=""),
    adapter_types.PluginLoadError(exc_info=""),
    adapter_types.NewConnectionOpening(connection_state=""),
    adapter_types.CodeExecution(conn_name="", code_content=""),
    adapter_types.CodeExecutionStatus(status="", elapsed=0.1),
    adapter_types.CatalogGenerationError(exc=""),
    adapter_types.WriteCatalogFailure(num_exceptions=0),
    adapter_types.CatalogWritten(path=""),
    adapter_types.CannotGenerateDocs(),
    adapter_types.BuildingCatalog(),
    adapter_types.DatabaseErrorRunningHook(hook_type=""),
    adapter_types.HooksRunning(num_hooks=0, hook_type=""),
    adapter_types.FinishedRunningStats(stat_line="", execution="", execution_time=0),
    adapter_types.ConstraintNotEnforced(constraint="", adapter=""),
    adapter_types.ConstraintNotSupported(constraint="", adapter=""),
    # I - Project parsing ======================
    types.InputFileDiffError(category="testing", file_id="my_file"),
    types.InvalidValueForField(field_name="test", field_value="test"),
    types.ValidationWarning(resource_type="model", field_name="access", node_name="my_macro"),
    types.ParsePerfInfoPath(path=""),
    types.PartialParsingErrorProcessingFile(file=""),
    types.PartialParsingFile(file_id=""),
    types.PartialParsingError(exc_info={}),
    types.PartialParsingSkipParsing(),
    types.UnableToPartialParse(reason="something went wrong"),
    types.StateCheckVarsHash(vars="testing", target="testing", profile="testing"),
    types.PartialParsingNotEnabled(),
    types.ParsedFileLoadFailed(path="", exc="", exc_info=""),
    types.PartialParsingEnabled(deleted=0, added=0, changed=0),
    types.PartialParsingFile(file_id=""),
    types.InvalidDisabledTargetInTestNode(
        resource_type_title="",
        unique_id="",
        original_file_path="",
        target_kind="",
        target_name="",
        target_package="",
    ),
    types.UnusedResourceConfigPath(unused_config_paths=[]),
    types.SeedIncreased(package_name="", name=""),
    types.SeedExceedsLimitSamePath(package_name="", name=""),
    types.SeedExceedsLimitAndPathChanged(package_name="", name=""),
    types.SeedExceedsLimitChecksumChanged(package_name="", name="", checksum_name=""),
    types.UnusedTables(unused_tables=[]),
    types.WrongResourceSchemaFile(
        patch_name="", resource_type="", file_path="", plural_resource_type=""
    ),
    types.NoNodeForYamlKey(patch_name="", yaml_key="", file_path=""),
    types.MacroNotFoundForPatch(patch_name=""),
    types.NodeNotFoundOrDisabled(
        original_file_path="",
        unique_id="",
        resource_type_title="",
        target_name="",
        target_kind="",
        target_package="",
        disabled="",
    ),
    types.JinjaLogWarning(),
    types.JinjaLogInfo(msg=""),
    types.JinjaLogDebug(msg=""),
    types.UnpinnedRefNewVersionAvailable(
        ref_node_name="", ref_node_package="", ref_node_version="", ref_max_version=""
    ),
    core_types.DeprecatedModel(model_name="", model_version="", deprecation_date=""),
    types.DeprecatedReference(
        model_name="",
        ref_model_name="",
        ref_model_package="",
        ref_model_deprecation_date="",
        ref_model_latest_version="",
    ),
    types.UpcomingReferenceDeprecation(
        model_name="",
        ref_model_name="",
        ref_model_package="",
        ref_model_deprecation_date="",
        ref_model_latest_version="",
    ),
    types.UnsupportedConstraintMaterialization(materialized=""),
    types.ParseInlineNodeError(exc=""),
    types.SemanticValidationFailure(msg=""),
    types.UnversionedBreakingChange(
        breaking_changes=[],
        model_name="",
        model_file_path="",
        contract_enforced_disabled=True,
        columns_removed=[],
        column_type_changes=[],
        enforced_column_constraint_removed=[],
        enforced_model_constraint_removed=[],
        materialization_changed=[],
    ),
    types.WarnStateTargetEqual(state_path=""),
    types.FreshnessConfigProblem(msg=""),
    # M - Deps generation ======================
    types.GitSparseCheckoutSubdirectory(subdir=""),
    types.GitProgressCheckoutRevision(revision=""),
    types.GitProgressUpdatingExistingDependency(dir=""),
    types.GitProgressPullingNewDependency(dir=""),
    types.GitNothingToDo(sha=""),
    types.GitProgressUpdatedCheckoutRange(start_sha="", end_sha=""),
    types.GitProgressCheckedOutAt(end_sha=""),
    types.RegistryProgressGETRequest(url=""),
    types.RegistryProgressGETResponse(url="", resp_code=1234),
    types.SelectorReportInvalidSelector(valid_selectors="", spec_method="", raw_spec=""),
    types.DepsNoPackagesFound(),
    types.DepsStartPackageInstall(package_name=""),
    types.DepsInstallInfo(version_name=""),
    types.DepsUpdateAvailable(version_latest=""),
    types.DepsUpToDate(),
    types.DepsListSubdirectory(subdirectory=""),
    types.DepsNotifyUpdatesAvailable(packages=["my_pkg", "other_pkg"]),
    types.RetryExternalCall(attempt=0, max=0),
    types.RecordRetryException(exc=""),
    types.RegistryIndexProgressGETRequest(url=""),
    types.RegistryIndexProgressGETResponse(url="", resp_code=1234),
    types.RegistryResponseUnexpectedType(response=""),
    types.RegistryResponseMissingTopKeys(response=""),
    types.RegistryResponseMissingNestedKeys(response=""),
    types.RegistryResponseExtraNestedKeys(response=""),
    types.DepsSetDownloadDirectory(path=""),
    types.DepsLockUpdating(lock_filepath=""),
    types.DepsAddPackage(package_name="", version="", packages_filepath=""),
    types.DepsFoundDuplicatePackage(removed_package={}),
    core_types.DepsScrubbedPackageName(package_name=""),
    types.SemanticValidationFailure(msg=""),
    # Q - Node execution ======================
    types.RunningOperationCaughtError(exc=""),
    types.CompileComplete(),
    types.FreshnessCheckComplete(),
    types.SeedHeader(header=""),
    types.SQLRunnerException(exc=""),
    types.LogTestResult(
        name="",
        index=0,
        num_models=0,
        execution_time=0,
        num_failures=0,
    ),
    types.LogStartLine(description="", index=0, total=0),
    types.LogModelResult(
        description="",
        status="",
        index=0,
        total=0,
        execution_time=0,
    ),
    types.LogSnapshotResult(
        status="",
        description="",
        cfg={},
        index=0,
        total=0,
        execution_time=0,
    ),
    types.LogSeedResult(
        status="",
        index=0,
        total=0,
        execution_time=0,
        schema="",
        relation="",
    ),
    types.LogFreshnessResult(
        source_name="",
        table_name="",
        index=0,
        total=0,
        execution_time=0,
    ),
    types.LogCancelLine(conn_name=""),
    types.DefaultSelector(name=""),
    types.NodeStart(),
    types.NodeFinished(),
    types.QueryCancelationUnsupported(type=""),
    types.ConcurrencyLine(num_threads=0, target_name=""),
    types.WritingInjectedSQLForNode(),
    types.NodeCompiling(),
    types.NodeExecuting(),
    types.LogHookStartLine(
        statement="",
        index=0,
        total=0,
    ),
    types.LogHookEndLine(
        statement="",
        status="",
        index=0,
        total=0,
        execution_time=0,
    ),
    types.SkippingDetails(
        resource_type="",
        schema="",
        node_name="",
        index=0,
        total=0,
    ),
    types.NothingToDo(),
    types.RunningOperationUncaughtError(exc=""),
    types.EndRunResult(),
    types.NoNodesSelected(),
    types.DepsUnpinned(revision="", git=""),
    types.NoNodesForSelectionCriteria(spec_raw=""),
    types.CommandCompleted(
        command="",
        success=True,
        elapsed=0.1,
        completed_at=get_json_string_utcnow(),
    ),
    types.ShowNode(node_name="", preview="", is_inline=True, unique_id="model.test.my_model"),
    types.CompiledNode(node_name="", compiled="", is_inline=True, unique_id="model.test.my_model"),
    # W - Node testing ======================
    types.CatchableExceptionOnRun(exc=""),
    types.InternalErrorOnRun(build_path="", exc=""),
    types.GenericExceptionOnRun(build_path="", unique_id="", exc=""),
    types.NodeConnectionReleaseError(node_name="", exc=""),
    types.FoundStats(stat_line=""),
    # Z - misc ======================
    types.MainKeyboardInterrupt(),
    types.MainEncounteredError(exc=""),
    types.MainStackTrace(stack_trace=""),
    types.SystemCouldNotWrite(path="", reason="", exc=""),
    types.SystemExecutingCmd(cmd=[""]),
    types.SystemStdOut(bmsg=str(b"")),
    types.SystemStdErr(bmsg=str(b"")),
    types.SystemReportReturnCode(returncode=0),
    types.TimingInfoCollected(),
    types.LogDebugStackTrace(),
    types.CheckCleanPath(path=""),
    types.ConfirmCleanPath(path=""),
    types.ProtectedCleanPath(path=""),
    types.FinishedCleanPaths(),
    types.OpenCommand(open_cmd="", profiles_dir=""),
    types.RunResultWarning(resource_type="", node_name="", path=""),
    types.RunResultFailure(resource_type="", node_name="", path=""),
    types.StatsLine(stats={"error": 0, "skip": 0, "pass": 0, "warn": 0, "total": 0}),
    types.RunResultError(msg=""),
    types.RunResultErrorNoMessage(status=""),
    types.SQLCompiledPath(path=""),
    types.CheckNodeTestFailure(relation_name=""),
    types.EndOfRunSummary(num_errors=0, num_warnings=0, keyboard_interrupt=False),
    types.LogSkipBecauseError(schema="", relation="", index=0, total=0),
    types.EnsureGitInstalled(),
    types.DepsCreatingLocalSymlink(),
    types.DepsSymlinkNotAvailable(),
    types.DisableTracking(),
    types.SendingEvent(kwargs=""),
    types.SendEventFailure(),
    types.FlushEvents(),
    types.FlushEventsFailure(),
    types.Formatting(),
    types.TrackingInitializeFailure(),
    types.RunResultWarningMessage(),
    types.DebugCmdOut(),
    types.DebugCmdResult(),
    types.ListCmdOut(),
    types.Note(msg="This is a note."),
    types.ResourceReport(),
]


class TestEventJSONSerialization:

    # attempts to test that every event is serializable to json.
    # event types that take `Any` are not possible to test in this way since some will serialize
    # just fine and others won't.
    def test_all_serializable(self):
        all_non_abstract_events = set(
            get_all_subclasses(CoreBaseEvent),
        )
        all_event_values_list = list(map(lambda x: x.__class__, sample_values))
        diff = all_non_abstract_events.difference(set(all_event_values_list))
        assert (
            not diff
        ), f"{diff}test is missing concrete values in `sample_values`. Please add the values for the aforementioned event classes"

        # make sure everything in the list is a value not a type
        for event in sample_values:
            assert type(event) != type

        # if we have everything we need to test, try to serialize everything
        count = 0
        for event in sample_values:
            msg = msg_from_base_event(event)
            print(f"--- msg: {msg.info.name}")
            # Serialize to dictionary
            try:
                msg_to_dict(msg)
            except Exception as e:
                raise Exception(
                    f"{event} can not be converted to a dict. Originating exception: {e}"
                )
            # Serialize to json
            try:
                msg_to_json(msg)
            except Exception as e:
                raise Exception(f"{event} is not serializable to json. Originating exception: {e}")
            # Serialize to binary
            try:
                msg.SerializeToString()
            except Exception as e:
                raise Exception(
                    f"{event} is not serializable to binary protobuf. Originating exception: {e}"
                )
            count += 1
        print(f"--- Found {count} events")


T = TypeVar("T")


def test_date_serialization():
    ti = TimingInfo("test")
    ti.begin()
    ti.end()
    ti_dict = ti.to_dict()
    assert ti_dict["started_at"].endswith("Z")
    assert ti_dict["completed_at"].endswith("Z")


def test_bad_serialization():
    """Tests that bad serialization enters the proper exception handling

    When pytest is in use the exception handling of `BaseEvent` raises an
    exception. When pytest isn't present, it fires a Note event. Thus to test
    that bad serializations are properly handled, the best we can do is test
    that the exception handling path is used.
    """

    with pytest.raises(Exception) as excinfo:
        types.Note(param_event_doesnt_have="This should break")

    assert (
        str(excinfo.value)
        == "[Note]: Unable to parse dict {'param_event_doesnt_have': 'This should break'}"
    )


def test_single_run_error():

    try:
        # Add a recording event manager to the context, so we can test events.
        event_mgr = TestEventManager()
        ctx_set_event_manager(event_mgr)

        error_result = RunResult(
            status=RunStatus.Error,
            timing=[],
            thread_id="",
            execution_time=0.0,
            node=None,
            adapter_response=dict(),
            message="oh no!",
            failures=[],
        )

        print_run_result_error(error_result)
        events = [e for e in event_mgr.event_history if isinstance(e[0], RunResultError)]

        assert len(events) == 1
        assert events[0][0].msg == "oh no!"

    finally:
        # Set an empty event manager unconditionally on exit. This is an early
        # attempt at unit testing events, and we need to think about how it
        # could be done in a thread safe way in the long run.
        ctx_set_event_manager(EventManager())
