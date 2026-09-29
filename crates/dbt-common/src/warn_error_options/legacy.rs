use serde::{Deserialize, Serialize};
use strum::{AsRefStr, EnumIter, EnumMessage, EnumString};

// TODO: these models should live in dbt-schemas crate. It currently lives in dbt-common because
// EvalArgs is defined here and dbt-common cannot depend on dbt-schemas without creating a cycle.

/// Legacy dbt-core event names supported by Fusion in `warn-error-options` configuration
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    EnumIter,
    EnumString,
    AsRefStr,
)]
pub enum SupportedLegacyWarnError {
    JinjaLogWarning,
    LogTestResult,
    NothingToDo,
    NoNodesSelected,
    NoNodesForSelectionCriteria,
    RunResultWarning,
    RunResultWarningMessage,
    NodeNotFoundOrDisabled,
    NoNodeForYamlKey,
    MacroNotFoundForPatch,
    InvalidConcurrentBatchesConfig,
    MicrobatchModelNoEventTimeInputs,
    InvalidMacroAnnotation,
    DeprecatedModel,
    DeprecatedReference,
    UpcomingReferenceDeprecation,
    SnapshotTimestampWarning,
    PackageRedirectDeprecation,
    DepsUnpinned,
    DepsScrubbedPackageName,
    DepsFoundDuplicatePackage,
    FreshnessConfigProblem,
    WarnStateTargetEqual,
    WEOIncludeExcludeDeprecation,
    UnversionedBreakingChange,
    UnsupportedConstraintMaterialization,
    UnusedResourceConfigPath,
    ConstraintNotEnforced,
    ConstraintNotSupported,
}

/// Legacy dbt-core event names that Fusion will eventually support in `warn-error-options` configuration, but does not yet support. These may have a corresponding fusion-native error code providing similar functionality.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    EnumIter,
    EnumString,
    AsRefStr,
)]
pub enum NotYetSupportedLegacyWarnError {
    AdapterDeprecationWarning,
    AdapterEventDebug,
    AdapterEventError,
    AdapterEventInfo,
    AdapterEventWarning,
    AdapterImportError,
    AdapterRegistered,
    ArgumentsPropertyInGenericTestDeprecation,
    ArtifactUploadError,
    ArtifactUploadSkipped,
    ArtifactUploadSuccess,
    ArtifactWritten,
    BehaviorChangeEvent,
    BuildingCatalog,
    CacheAction,
    CacheDumpGraph,
    CacheMiss,
    CannotGenerateDocs,
    CatalogGenerationError,
    CatalogWritten,
    CatchableExceptionOnRun,
    CheckCleanPath,
    CheckNodeTestFailure,
    CodeExecution,
    CodeExecutionStatus,
    ColTypeChange,
    CollectFreshnessReturnSignature,
    CommandCompleted,
    CompileComplete,
    CompiledNode,
    ConcurrencyLine,
    ConfigFolderDirectory,
    ConfirmCleanPath,
    ConnectionClosed,
    ConnectionClosedInCleanup,
    ConnectionLeftOpen,
    ConnectionLeftOpenInCleanup,
    ConnectionReused,
    ConnectionUsed,
    CustomKeyInObjectDeprecation,
    CustomOutputPathInSourceFreshnessDeprecation,
    DatabaseErrorRunningHook,
    DebugCmdOut,
    DebugCmdResult,
    DebugLevel,
    DefaultSelector,
    DeprecationsSummary,
    DepsAddPackage,
    DepsCreatingLocalSymlink,
    DepsInstallInfo,
    DepsListSubdirectory,
    DepsLockUpdating,
    DepsNoPackagesFound,
    DepsSetDownloadDirectory,
    DepsStartPackageInstall,
    DepsSymlinkNotAvailable,
    DepsUpToDate,
    DepsUpdateAvailable,
    DisableTracking,
    DynamicLevel,
    EndOfRunSummary,
    EndRunResult,
    EnsureGitInstalled,
    EnvironmentVariableRenamed,
    ErrorLevel,
    EventLevel,
    ExposureNameDeprecation,
    FinishedCleanPaths,
    FinishedRunningStats,
    FlushEvents,
    FlushEventsFailure,
    Formatting,
    FoundStats,
    FreshnessCheckComplete,
    GenericExceptionOnRun,
    GetMetaKeyWarning,
    GitNothingToDo,
    GitProgressCheckedOutAt,
    GitProgressCheckoutRevision,
    GitProgressPullingNewDependency,
    GitProgressUpdatedCheckoutRange,
    GitProgressUpdatingExistingDependency,
    GitSparseCheckoutSubdirectory,
    HooksRunning,
    InfoLevel,
    InputFileDiffError,
    InternalErrorOnRun,
    InternalDeprecation,
    InvalidDisabledTargetInTestNode,
    InvalidOptionYAML,
    InvalidProfileTemplateYAML,
    JinjaLogDebug,
    JinjaLogInfo,
    ListCmdOut,
    ListRelations,
    LogBatchResult,
    LogCancelLine,
    LogDbtProfileError,
    LogDbtProjectError,
    LogDebugStackTrace,
    LogFreshnessResult,
    LogFunctionResult,
    LogHookEndLine,
    LogHookStartLine,
    LogModelResult,
    LogNodeNoOpResult,
    LogNodeResult,
    LogSeedResult,
    LogSkipBecauseError,
    LogSnapshotResult,
    LogStartBatch,
    LogStartLine,
    MainEncounteredError,
    MainKeyboardInterrupt,
    MainReportArgs,
    MainReportVersion,
    MainStackTrace,
    MainTrackingUserState,
    MarkSkippedChildren,
    MicrobatchExecutionDebug,
    MissingArgumentsPropertyInGenericTestDeprecation,
    MissingPlusPrefixDeprecation,
    MissingProfileTarget,
    ModelParamUsageDeprecation,
    ModulesItertoolsUsageDeprecation,
    NewConnection,
    NewConnectionOpening,
    NoSampleProfileFound,
    NodeCompiling,
    NodeConnectionReleaseError,
    NodeExecuting,
    NodeFinished,
    NodeStart,
    Note,
    OpenCommand,
    PackageInstallPathDeprecation,
    ParseInlineNodeError,
    ParsePerfInfoPath,
    ParsedFileLoadFailed,
    PartialParsingEnabled,
    PartialParsingError,
    PartialParsingErrorProcessingFile,
    PartialParsingFile,
    PartialParsingNotEnabled,
    PartialParsingSkipParsing,
    PluginLoadError,
    PrintEvent,
    ProfileWrittenWithProjectTemplateYAML,
    ProfileWrittenWithSample,
    ProfileWrittenWithTargetTemplateYAML,
    ProjectCreated,
    ProjectNameAlreadyExists,
    PropertyMovedToConfigDeprecation,
    ProtectedCleanPath,
    QueryCancelationUnsupported,
    RecordReplayIssue,
    RecordRetryException,
    RegistryIndexProgressGETRequest,
    RegistryIndexProgressGETResponse,
    RegistryProgressGETRequest,
    RegistryProgressGETResponse,
    RegistryResponseExtraNestedKeys,
    RegistryResponseMissingNestedKeys,
    RegistryResponseMissingTopKeys,
    RegistryResponseUnexpectedType,
    ResourceReport,
    RetryExternalCall,
    Rollback,
    RollbackFailed,
    RunResultError,
    RunResultErrorNoMessage,
    RunResultFailure,
    RunningOperationCaughtError,
    RunningOperationUncaughtError,
    SQLCommit,
    SQLCompiledPath,
    SQLQuery,
    SQLQueryStatus,
    SQLRunnerException,
    SchemaCreation,
    SchemaDrop,
    SeedExceedsLimitAndPathChanged,
    SeedExceedsLimitChecksumChanged,
    SeedHeader,
    SelectExcludeIgnoredWithSelectorWarning,
    SelectorReportInvalidSelector,
    SendEventFailure,
    SendingEvent,
    SettingUpProfile,
    ShowNode,
    SkippingDetails,
    SourceOverrideDeprecation,
    SpacesInResourceNameDeprecation,
    StarterProjectPath,
    StateCheckVarsHash,
    StatsLine,
    SystemCouldNotWrite,
    SystemExecutingCmd,
    SystemReportReturnCode,
    SystemStdErr,
    SystemStdOut,
    TimingInfoCollected,
    TrackingInitializeFailure,
    TypeCodeNotFound,
    UnableToPartialParse,
    UnexpectedJinjaBlockDeprecation,
    UnpinnedRefNewVersionAvailable,
    WarnLevel,
    WriteCatalogFailure,
    WritingInjectedSQLForNode,
}

/// Legacy dbt-core event names that are valid event names but are not warnings in dbt Core.
/// dbt Core silently ignores these in `warn_error_options`, so Fusion does the same.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    EnumIter,
    EnumString,
    AsRefStr,
)]
pub enum NotAWarningInDbtCoreLegacyWarnError {
    DepsNotifyUpdatesAvailable,
}

/// Legacy dbt-core event names that Fusion will not support in `warn-error-options` configuration, either because they are no longer relevant or can't be implemented in a way that provides value to users.
///
/// NOTE: each variant in this enum should have a message describing why it will not be supported, matching
/// public documentation.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    EnumIter,
    EnumMessage,
    EnumString,
    AsRefStr,
)]
pub enum WillNotSupportLegacyWarnError {
    #[strum(
        message = "Fusion only supports the newer behavior-change flag, where this case is a hard error."
    )]
    MicrobatchMacroOutsideOfBatchesDeprecation,
    #[strum(
        message = "This warning comes from partial parsing in dbt Core, which Fusion does not support."
    )]
    SeedExceedsLimitSamePath,
    #[strum(
        message = "This warning comes from partial parsing in dbt Core, which Fusion does not support."
    )]
    SeedIncreased,
    #[strum(
        message = "Fusion only supports the newer behavior-change flag, where this case is a hard error."
    )]
    GenerateSchemaNameNullValueDeprecation,
    #[strum(
        message = "Fusion already implements the new semantic layer spec, so this legacy warning no longer applies."
    )]
    GenericSemanticLayerDeprecation,
    #[strum(
        message = "Fusion already implements the new semantic layer spec, so this legacy warning no longer applies."
    )]
    MFCumulativeTypeParamsDeprecation,
    #[strum(
        message = "Fusion already implements the new semantic layer spec, so this legacy warning no longer applies."
    )]
    MFTimespineWithoutYamlConfigurationDeprecation,
    #[strum(
        message = "Fusion already implements the new semantic layer spec, so this legacy warning no longer applies."
    )]
    MetricAttributesRenamed,
    #[strum(
        message = "Fusion already implements the new semantic layer spec, so this legacy warning no longer applies."
    )]
    TimeDimensionsRequireGranularityDeprecation,
    #[strum(
        message = "Fusion already uses the newer source freshness behavior, so this legacy warning does not apply."
    )]
    SourceFreshnessProjectHooksNotRun,
    #[strum(message = "Fusion does not support semantic models, so this warning does not apply.")]
    SemanticValidationFailure,
    #[strum(
        message = "Fusion already validates allowed YAML keys strictly, so this warning would be redundant."
    )]
    ValidationWarning,
    #[strum(
        message = "Fusion already enforces this as a hard YAML parsing or schema validation error, so this warn_error_options entry has no effect."
    )]
    CustomKeyInConfigDeprecation,
    #[strum(
        message = "Fusion already enforces this as a hard YAML parsing or schema validation error, so this warn_error_options entry has no effect."
    )]
    CustomTopLevelKeyDeprecation,
    #[strum(
        message = "Fusion already enforces this as a hard YAML parsing or schema validation error, so this warn_error_options entry has no effect."
    )]
    DuplicateNameDistinctNodeTypesDeprecation,
    #[strum(
        message = "Fusion already enforces this as a hard YAML parsing or schema validation error, so this warn_error_options entry has no effect."
    )]
    DuplicateYAMLKeysDeprecation,
    #[strum(
        message = "Fusion already enforces this as a hard YAML parsing or schema validation error, so this warn_error_options entry has no effect."
    )]
    GenericJSONSchemaValidationDeprecation,
    #[strum(
        message = "Fusion already enforces this as a hard YAML parsing or schema validation error, so this warn_error_options entry has no effect."
    )]
    InvalidValueForField,
    #[strum(
        message = "Fusion already enforces this as a hard YAML parsing or schema validation error, so this warn_error_options entry has no effect."
    )]
    ResourceNamesWithSpacesDeprecation,
    #[strum(
        message = "Fusion already enforces the latest behavior, which prevents packages from overriding built-in materializations."
    )]
    PackageMaterializationOverrideDeprecation,
    #[strum(
        message = "Fusion does not surface this warning by default, which matches current dbt Core behavior."
    )]
    TestsConfigDeprecation,
    #[strum(
        message = "Fusion already errors on this configuration, which matches newer dbt Core behavior."
    )]
    ProjectFlagsMovedDeprecation,
    #[strum(message = "This is now fully deprecated in Fusion.")]
    ConfigSourcePathDeprecation,
    #[strum(message = "This is now fully deprecated in Fusion.")]
    ConfigLogPathDeprecation,
    #[strum(message = "This is now fully deprecated in Fusion.")]
    ConfigTargetPathDeprecation,
    #[strum(message = "This is now fully deprecated in Fusion.")]
    ConfigDataPathDeprecation,
    #[strum(
        message = "Fusion reserves the DBT_ENGINE_ prefix and rejects unknown environment variables that use it."
    )]
    EnvironmentVariableNamespaceDeprecation,
    #[strum(
        message = "Fusion does not allow source overrides, so packages must disable a source explicitly instead."
    )]
    UnusedTables,
    #[strum(message = "Fusion reports this case under NoNodeForYamlKey instead.")]
    WrongResourceSchemaFile,
    #[strum(
        message = "Fusion does not warn on this case; set the `require_ref_searches_node_package_before_root` flag to control the resolution order instead."
    )]
    PackageNodeDependsOnRootProjectNode,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use strum::IntoEnumIterator;

    #[test]
    fn enums_are_not_overlapping() {
        let groups = [
            (
                "SupportedLegacyWarnError",
                SupportedLegacyWarnError::iter()
                    .map(|variant| variant.as_ref().to_string())
                    .collect::<Vec<_>>(),
            ),
            (
                "NotYetSupportedLegacyWarnError",
                NotYetSupportedLegacyWarnError::iter()
                    .map(|variant| variant.as_ref().to_string())
                    .collect::<Vec<_>>(),
            ),
            (
                "WillNotSupportLegacyWarnError",
                WillNotSupportLegacyWarnError::iter()
                    .map(|variant| variant.as_ref().to_string())
                    .collect::<Vec<_>>(),
            ),
            (
                "NotAWarningInDbtCoreLegacyWarnError",
                NotAWarningInDbtCoreLegacyWarnError::iter()
                    .map(|variant| variant.as_ref().to_string())
                    .collect::<Vec<_>>(),
            ),
        ];

        let mut seen = BTreeMap::new();
        for (group_name, variants) in groups {
            for variant in variants {
                let previous_group = seen.insert(variant.clone(), group_name);
                assert!(
                    previous_group.is_none(),
                    "Variant `{variant}` of `{group_name}` should not also be present in `{}`",
                    previous_group.unwrap()
                );
            }
        }
    }

    #[test]
    fn will_not_support_legacy_all_have_messages() {
        for variant in WillNotSupportLegacyWarnError::iter() {
            assert!(
                !variant.get_message().unwrap_or("").is_empty(),
                "Variant `{}` of `WillNotSupportLegacyWarnError` must have a non-empty message describing why it will not be supported",
                variant.as_ref()
            );
        }
    }
}
