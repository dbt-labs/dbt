// use serde_repr::{Deserialize_repr, Serialize_repr};
use std::{collections::HashMap, fmt::Display};

use dbt_proc_macros::include_frontend_error_codes;
use int_enum::IntEnum;
use strum_macros::{EnumString, IntoStaticStr};

/// Error codes for the SDF CLI.
///
/// Error codes define the general "semantic type" of a [FsError]. Each error
/// code is a 4-digit number stored as a u16 type.
#[include_frontend_error_codes]
#[repr(u16)]
#[non_exhaustive]
#[derive(Debug, Copy, Clone, Eq, PartialEq, IntEnum, EnumString, IntoStaticStr, Default)]
pub enum ErrorCode {
    // ----------------- Frontend errors [0, 999] -----------------------------
    //
    // This section contains user-facing error codes originating from the
    // frontend. Frontend error codes occupy the range [0, 999]
    //
    // **NOTE**: this section is auto-synced with
    // [dbt_frontend_common::error::ErrorCode] by way of the
    // `include_frontend_error_codes` macro. **DO NOT** manually add any error
    // codes in this range here, add them to [dbt_frontend_common::error::ErrorCode]
    // instead.

    // ----------------- CLI errors [1000, 8999] ------------------------------
    //
    // This section contains user-facing error codes originating from the CLI.
    // CLI error codes occupy the range [1000, 8999]
    //
    // Define all CLI error codes here.
    /// Default catch-all code for when you're too lazy to specify a proper code
    #[default]
    Generic = 1000,
    IoError = 1001,
    EncodingError = 1002,
    FileIoError = 1003,
    CacheError = 1004,
    InvalidConfig = 1005,
    InvalidPath = 1006,
    InvalidArgument = 1007,
    MissingArgument = 1008,
    InferenceFailed = 1009,
    InvalidTable = 1010,
    AuthenticationFailed = 1011,
    MissingClassifiers = 1012,
    SerializationError = 1013,
    RemoteError = 1014,
    ExecutionError = 1015,
    ArrowError = 1016,
    ParquetError = 1017,
    ObjectStoreError = 1018,
    LogicalPlanError = 1019,
    ResourceError = 1020,
    GenericDatafusionError = 1021,
    CyclicDependency = 1022,
    UnsupportedFileFormat = 1023,
    FileNotFound = 1024,
    MissingTable = 1025,
    InvalidType = 1026,
    MergeConflict = 1027,
    MissingSourceLocation = 1028,
    TooManyRows = 1029,
    TableMissingProvider = 1030,
    AmbiguousRenamingSpecification = 1031,
    UndefinedField = 1032,
    DuplicateColumns = 1033,
    MissingWorkspaceFile = 1034,
    InvalidEnvironment = 1035,
    DuplicateEnvironment = 1036,
    UnsupportedWorkspaceEdition = 1037,
    CredentialsError = 1038,
    LintCheckFailed = 1039,
    SubprocessError = 1040,
    FmtError = 1041,
    FunctionDefinitionError = 1042,
    BuildError = 1043,
    UnimplementedFunction = 1044,
    NoTableFoundForPrefix = 1045,

    AmbiguousSourceSchema = 1046,
    UnsupportedLogicalPlanForLocalExecution = 1047,
    DependencyNotFound = 1048,
    UnsupportedFileExtension = 1049,
    SkippedArtifact = 1050,

    // fs db errors
    FailedToCreateDatabase = 1051,
    FailedToRegisterSeedTable = 1052,
    FailedToRegisterExistingTable = 1053,
    FailedToWriteTable = 1054,

    FailedToLookupExistingTable = 1055,

    MissingTargetDirectory = 1056,
    ColumnTypeMismatch = 1058,
    DuplicateConfigKey = 1059,
    UnusedConfigKey = 1060,
    InvalidCsvFormat = 1061,

    /// Error code for when a model tries to reference a disabled ref or source
    DisabledDependency = 1062,

    StaleSource = 1063,

    DisabledModel = 1064,

    PackageParsingCompatibility = 1065,

    AccessDenied = 1066,

    GenericExecFailed = 1067,
    LicenseError = 1068,
    MangledRef = 1069,
    BaselineIntrospectionSyntaxInvalid = 1070,
    JinjaWarn = 1071,

    // Warn-error-options: dedicated codes for dbt-core legacy event names.
    // Each maps 1:1 to a dbt-core event so that `--warn-error-options {error: [EventName]}`
    // can target individual warning types without affecting unrelated warnings.
    DeprecatedModel = 1085,
    DeprecatedReference = 1072,
    UpcomingReferenceDeprecation = 1073,
    /// A jinja warn() call that was upgraded to an error via --warn-error-options.
    /// The warn message is stored in FsError::context.
    JinjaWarnUpgradedToError = 1074,
    SnapshotTimestampMismatch = 1075,
    PackageRedirectDeprecation = 1076,
    DepsUnpinned = 1077,
    FreshnessConfigInvalid = 1078,
    FreshnessMetadataWarning = 1079,
    PackageVersionMismatch = 1080,
    SeedColumnTypeMismatch = 1081,
    CacheInvalidationWarning = 1082,
    UnexpectedApiResponse = 1083,
    WarnStateTargetEqual = 1084,
    WEOIncludeExcludeDeprecation = 1086,
    NodeNotFoundOrDisabled = 1087,
    PackageUpdateAvailable = 1088,
    NoNodeForYamlKey = 1089,
    MacroPatchNotFound = 1090,
    InvalidConcurrentBatchesConfig = 1091,
    NoNodesForSelectionCriteria = 1092,
    MicrobatchModelNoEventTimeInputs = 1093,
    UnversionedBreakingChange = 1094,
    UnsupportedConstraintMaterialization = 1095,
    HubPackageDeprecated = 1096,
    UnusedResourceConfigPath = 1097,
    DepsScrubbedPackageName = 1098,
    DepsDuplicatePackage = 1099,
    /// A constraint is recognized by the adapter but not enforced at the database level.
    ConstraintNotEnforced = 1109,
    /// A constraint type is not supported by the adapter at all and is dropped from the rendered DDL.
    ConstraintNotSupported = 1110,

    // --------------------------------------------------------------------------------------------
    // CLI args/config [1100–1149]
    InvalidFlag = 1100,
    UnsupportedFlag = 1101,
    MissingProfile = 1102,
    ProfileInvalid = 1103,
    EnvVarMissing = 1104,
    EnvVarInvalid = 1105,
    UnsupportedFusionFeature = 1106,
    UnknownCommand = 1107,
    UnknownCliOption = 1108,

    // Project/manifest/package [1150–1199]
    ManifestLoadFailed = 1150,
    PackageResolutionFailed = 1151,
    PackageDownloadFailed = 1152,
    ProfileLoadFailed = 1153,
    GitError = 1154,
    DuplicateSourceTable = 1155,
    NoTablesInSource = 1156,
    SemanticModelDeprecated = 1157,
    PackageMissingProjectFile = 1158,
    DbtYamlValidationError = 1159,

    // Network/HTTP [1200–1249]
    NetworkError = 1200,
    HttpTimeout = 1201,
    RateLimited = 1202,
    HttpError = 1203,
    DbtPlatformApiError = 1204,

    // Auth/credentials [1250–1279]
    AuthFailed = 1250,
    CredentialMissing = 1251,
    CredentialInvalid = 1252,
    CredentialExpired = 1253,
    PermissionDenied = 1254,

    // Adapter/DB [1300–1399]
    DbConnectionFailed = 1300,
    DbAuthFailed = 1301,
    DbSyntaxInvalid = 1302,
    DbResourceExceeded = 1303,
    DbUnavailable = 1304,
    DbTxnConflict = 1305,
    DbNotFound = 1306,
    DbUnsupportedFeature = 1307,
    DbDriverFailed = 1308,
    ReplayDataInvalid = 1309,
    ReplayDataMissing = 1310,

    // Execution/runtime [1400–1449]
    PlannerFailed = 1400,
    ExecutorFailed = 1401,
    ConcurrencyError = 1402,
    TaskTimeout = 1403,
    TaskCancelled = 1404,
    SqlMismatch = 1405,
    SidecarError = 1406,
    NoResultsToShow = 1407,
    SidecarUnsupportedFeature = 1408,
    /// dbt State service degraded into fail-open: config/init/decision/cache
    /// errors that don't abort the command but indicate degraded behavior.
    StateServiceWarn = 1410,

    // Serialization [1450–1460]
    JsonInvalid = 1450,
    YamlInvalid = 1451,
    // --------------------------------------------------------------------------------------------
    // Jinja
    MacroUnsupportedValueType = 1500,
    JinjaError = 1501,
    MacroSyntaxInvalid = 1502,
    MacroVarNotFound = 1503,
    InvalidSeedValue = 1504,
    MacroUseIllegal = 1505,
    /// Emitted when `validate_macro_args` is enabled and a YAML-documented
    /// macro argument name or type does not match the Jinja macro definition.
    ValidateMacroArgs = 1506,
    JinjaTypeCheckFailed = 1507,
    JinjaTopLevelReturn = 1508,
    /// Emitted when a `{% snapshot %}`/`{% docs %}` block name is followed by
    /// extra characters that dbt-core's regex-based name extractor silently
    /// discards (e.g. `{% snapshot foo.sql %}`). Deprecation warning only.
    MalformedBlockName = 1509,

    // --------------------------------------------------------------------------------------------
    // Local execution
    SelectorError = 1600,
    NoNodesSelected = 1601,
    InvalidColumnSelector = 1602,
    /// An externally supplied node set replaced the computed selection.
    SelectionOverrideActive = 1603,
    /// An externally supplied node set was available but not applicable to this command.
    SelectionOverrideSkipped = 1604,
    /// The nodes actually reported by the run differ from the externally supplied node set.
    SelectionOverrideDivergence = 1605,

    // --------------------------------------------------------------------------------------------
    // Checks
    //
    // One code per outcome rather than a shared one: `warn_error_options` targets codes, so
    // collapsing these onto `Generic` would mean upgrading or silencing a check outcome also
    // upgrades or silences every unrelated `Generic` warning in the run.
    /// A check found violations at `error` severity. Logged as an error.
    CheckFailed = 1650,
    /// A check found violations at `warn` severity.
    CheckWarned = 1651,
    /// A check had nothing in scope to evaluate.
    CheckSkipped = 1652,
    /// A check could not be evaluated (e.g. its query errored).
    CheckEvaluationFailed = 1653,
    /// The metadata a check reads is not on disk, so checks were skipped and the build
    /// proceeded. Skipped rather than failed because this is infrastructure, not a verdict on the
    /// project; `warn_error_options` can promote it where the gate must be mandatory.
    ///
    /// It was `CheckIndexUnavailable` while checks read `target/private/index`. They read the
    /// parse metadata epochs directly now, so only the name changed -- the number is the same,
    /// and an existing `warn_error_options` entry keeps working.
    CheckMetadataUnavailable = 1654,
    /// No longer emitted. The number is retained rather than reused so an old
    /// `warn_error_options` entry stays inert instead of silently promoting something else.
    ///
    /// It reported the index being turned off as the reason the gate did not run, which was true
    /// while checks read the index. They read the parse metadata now, so nothing about the index
    /// decides whether a check runs -- `--skip-checks` does, and asking to skip is not warned
    /// about. Reachable only through `--no-write-index`, which is hidden, so in practice this
    /// code could only be met by a programmatic caller.
    CheckIndexDisabled = 1655,

    // --------------------------------------------------------------------------------------------
    // Information schema
    /// `target/info_schema/` is missing, unreadable, or will not open.
    ///
    /// Distinct from `InvalidArgument`, which this case used to report: `--info <view>` is a
    /// well-formed request against state that has not been produced, so calling it a bad
    /// argument sent people to check their flags, and made a missing artifact
    /// indistinguishable from a typo'd option in anything that reads codes rather than prose.
    /// The query itself keeps `InvalidArgument` -- an unknown view or a syntax error really is
    /// the argument.
    InfoSchemaUnavailable = 1656,
    /// `--generate-info-schema` was asked for and the write failed, so `target/info_schema/`
    /// is missing or holds the previous run's tables. A warning rather than an error: the
    /// artifact is opt-in and nothing downstream reads it, so a failed write must not fail a
    /// build that otherwise succeeded. Its own code so a project that depends on the artifact
    /// -- a report, a scheduled query -- can promote just this with `warn_error_options`
    /// instead of every `Generic` warning in the run.
    InfoSchemaWriteFailed = 1657,
    /// The information schema was written from a `parse`, so the columns only `compile`,
    /// `run` or `build` fill are absent: compiled code, column types, column-level lineage
    /// and runtime results. Reports a successful write, not a failure -- and separated from
    /// `InfoSchemaWriteFailed` for that reason, since the two used one code and read
    /// identically in a log.
    InfoSchemaIncomplete = 1658,

    // --------------------------------------------------------------------------------------------
    // Project metadata index
    //
    // `target/private/index/` is not a user API and is not documented as one, but its *warnings*
    // are user-visible and no longer opt-in: `--write-index` is hidden, and the index is implied
    // on for `build`, `run` and `check` unless `--no-write-index` says otherwise. So a failure
    // here reaches everyone, and "promote it in CI" is a reasonable thing to want without also
    // promoting every `Generic` warning in the run.
    /// The index could not be written, so features that read it have nothing current to read.
    /// Checks are no longer among them: they read the parse metadata epochs, so a failed index
    /// write leaves the gate running normally.
    ///
    /// The index's *parse-tier* advisory deliberately has no code and stays on `Generic`: it
    /// fires only for `parse --write-index`, and `--write-index` is undocumented, so there is
    /// nobody to target it. A code is worth adding when someone can act on it.
    IndexWriteFailed = 1659,

    // --------------------------------------------------------------------------------------------
    // CLI errors
    NoLongerSupportedOption = 1700,
    NotYetSupportedOption = 1701,
    DeprecatedOption = 1702,
    DeprecatedStaticAnalysisValue = 1703,
    NotSupportedWarnErrorOption = 1704,
    /// No longer emitted. Retained so an existing `warn_error_options` entry naming it
    /// keeps resolving instead of failing as an unknown code.
    DocsGenerateWarning = 1705,

    // --------------------------------------------------------------------------------------------
    // Agent skills
    /// `ai_provider` names a harness dbt has no destination directory for.
    UnknownAiProvider = 1800,
    /// Skills were found but `ai_provider` is unset, so nothing was installed.
    AiProviderUnset = 1801,
    /// A `SKILL.md` is missing required frontmatter or its `name` doesn't match its directory.
    InvalidSkill = 1802,
    /// Two or more enabled skills resolve to the same install name, so they would
    /// occupy the same directory. dbt fails rather than pick a winner; the user
    /// disables all but one (or a maintainer renames), as with duplicate model names.
    SkillNameCollision = 1803,
    /// No longer emitted. Retained so an existing `warn_error_options` entry naming it
    /// keeps resolving, and so the discriminant is not reused: dbt now recognizes its
    /// own installs by their recorded metadata and reclaims them silently, rather than
    /// warning that a copy was modified.
    SkillModifiedByUser = 1804,
    /// A skill directory dbt wanted to install into is already occupied.
    SkillDestinationOccupied = 1805,
    /// A `skill-paths` entry resolves outside the project that declared it.
    SkillPathEscapesProject = 1806,

    // --------------------------------------------------------------------------------------------
    // Local execution
    SessionError = 2000,
    UnsupportedLocalExecutionDialect = 2001,

    // --------------------------------------------------------------------------------------------
    // Error processing an .slt file
    SltParse = 3000,
    SltLimits = 3001,
    SltConfig = 3002,
    SltDatabaseError = 3003,

    // --------------------------------------------------------------------------------------------
    // Lineage
    InvalidLineageSchema = 3500,

    InvalidDialect = 8998,
    RuntimeError = 8999,
    InvalidUserInput = 8997,
    InvalidOptions = 8996,
    OperationCanceled = 8995,

    // -----------------  ---------------------
    // CLI Internal errors [9000, 9899]
    // Everything below this line is an internal error. They will be presented
    // as bugs if surfaced to the user.
    NotSupported = 9000,
    Unknown = 9001,
    Unexpected = 9002,
    NotImplemented = 9003,
    InvalidTableNameInCLI = 9004,
    CoalesceHasOnlyNulls = 9005,
    CacheStale = 9010,
    NoFilesChanged = 9011,
    // ExitRepl is not really an error, but a special error code that is used to
    // signal the repl to exit gracefully:
    ExitRepl = 9006,
    /// Not really an error: signals that main() should exit with a specific
    /// i32 status code carried in `WrappedError::ExitCode`.
    ExitWithStatus = 9007,
    // ----------------- Internal errors from frontend [9900, 9999] -----------
    // This section contains the internal error codes from the frontend.
    //
    // **NOTE**: this section is auto-synced with
    // [dbt_frontend_common::error::ErrorCode] by way of the
    // `include_frontend_error_codes` macro. **DO NOT** manually add any error
    // codes in this range here, add them to [dbt_frontend_common::error::ErrorCode]
    // instead.
}
impl std::hash::Hash for ErrorCode {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (*self as u16).hash(state)
    }
}

impl Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:04}", *self as u16)
    }
}

impl ErrorCode {
    pub fn name(self) -> &'static str {
        self.into()
    }

    pub fn name_and_code(self) -> String {
        format!("{} (dbt{self})", self.name())
    }

    pub fn is_bug(&self) -> bool {
        (*self as u16) >= (Self::NotSupported as u16)
    }

    pub fn is_frontend(&self) -> bool {
        (*self as u16) < (Self::Generic as u16)
    }

    /// Returns true if this code represents a database-level error.
    ///
    /// Used to distinguish adapter/database errors from other Jinja execution
    /// errors so they can be formatted in a user-friendly way (similar to
    /// dbt-core's "Database Error in model X" format).
    pub fn is_database_error(&self) -> bool {
        matches!(
            self,
            ErrorCode::DbConnectionFailed
                | ErrorCode::DbAuthFailed
                | ErrorCode::DbSyntaxInvalid
                | ErrorCode::DbResourceExceeded
                | ErrorCode::DbUnavailable
                | ErrorCode::DbTxnConflict
                | ErrorCode::DbNotFound
                | ErrorCode::DbUnsupportedFeature
                | ErrorCode::DbDriverFailed
                | ErrorCode::ExecutorFailed
        )
    }
}

impl From<dbt_frontend_common::error::ErrorCode> for ErrorCode {
    fn from(code: dbt_frontend_common::error::ErrorCode) -> Self {
        let frontend_code = code as u16;
        if frontend_code < dbt_frontend_common::error::ErrorCode::NotSupported as u16 {
            Self::try_from(frontend_code).expect("invalid cli error code: {frontend_code}")
        } else {
            // Internal errors map to the 9k range:
            Self::try_from(frontend_code + 9000).expect("invalid cli error code: {frontend_code}")
        }
    }
}
/// General warning handling. Warnings are controlled via -w from the CLI.
///
/// Warnings can be set and unset. They are usually passed as part of EvalArg.
///
/// A warning is active if its key in the Warnings hashmap is defined.
/// The value of the key can be used to provide additional info, for instance
/// for the warning capitalization_identifier:upper, use the error code for
/// capitalization_identifier as key and the string "upper" as value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warnings {
    // todo: better representation, but good enough for now...
    pub values: HashMap<ErrorCode, String>,
}

impl Warnings {
    /// Creates an empty Warning instance.
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
        }
    }

    /// Adds an error code to the warnings.
    pub fn with_error_code(mut self, code: ErrorCode) -> Self {
        self.values.insert(code, String::new());
        self
    }

    /// Adds an error code to the warnings with a specified value.
    pub fn with_error_code_and_value(mut self, code: ErrorCode, value: String) -> Self {
        self.values.insert(code, value);
        self
    }

    /// Checks if the warnings is turned on.
    pub fn contains(&self, code: &ErrorCode) -> bool {
        self.values.contains_key(code)
    }

    /// Checks if there are no warnings.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Returns an iterator over the error codes and their corresponding values in the warnings.
    pub fn iter(&self) -> impl Iterator<Item = (&ErrorCode, &String)> {
        self.values.iter()
    }
}

impl Default for Warnings {
    /// Creates a new Warnings instance with an empty hashmap.
    fn default() -> Self {
        Warnings::new()
    }
}

#[cfg(test)]
mod tests {
    use super::ErrorCode;

    #[test]
    fn state_service_warn_renders_name_and_code() {
        assert_eq!(
            ErrorCode::StateServiceWarn.name_and_code(),
            "StateServiceWarn (dbt1410)"
        );
    }
}
