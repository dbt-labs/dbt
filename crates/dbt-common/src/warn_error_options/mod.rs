use std::collections::{BTreeMap, BTreeSet};

use crate::{ErrorCode, collections::HashMap};
use dbt_yaml::{Value, Verbatim};
use serde::{Deserialize, Serialize, Serializer};
use strum::{AsRefStr, EnumMessage, IntoEnumIterator};

mod legacy;

pub use legacy::{
    NotAWarningInDbtCoreLegacyWarnError, NotYetSupportedLegacyWarnError, SupportedLegacyWarnError,
    WillNotSupportLegacyWarnError,
};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, AsRefStr,
)]
pub enum LegacyWarnErrorGroupValue {
    #[serde(rename = "all", alias = "*")]
    All,
    #[serde(rename = "Deprecations")]
    Deprecations,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum FusionWarnErrorGroupValue {
    StaticAnalysis,
    PackageParsingCompatibilityErrors,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(untagged)]
pub enum WarnErrorOptionValue {
    FusionCode(u16),
    LegacyGroup(LegacyWarnErrorGroupValue),
    FusionGroup(FusionWarnErrorGroupValue),
    SupportedLegacy(SupportedLegacyWarnError),
    NotYetSupportedLegacy(NotYetSupportedLegacyWarnError),
    WillNotSupportLegacy(WillNotSupportLegacyWarnError),
    Unsupported(String),
}

impl Serialize for WarnErrorOptionValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::FusionCode(code) => match ErrorCode::try_from(*code) {
                Ok(code) => format!("{code:?}").serialize(serializer),
                Err(_) => code.serialize(serializer),
            },
            Self::LegacyGroup(value) => value.serialize(serializer),
            Self::FusionGroup(value) => value.serialize(serializer),
            Self::SupportedLegacy(value) => value.serialize(serializer),
            Self::NotYetSupportedLegacy(value) => value.serialize(serializer),
            Self::WillNotSupportLegacy(value) => value.serialize(serializer),
            Self::Unsupported(value) => value.serialize(serializer),
        }
    }
}

impl WarnErrorOptionValue {
    pub fn all() -> Self {
        Self::LegacyGroup(LegacyWarnErrorGroupValue::All)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarnErrorOptions {
    #[serde(default)]
    pub error: Vec<WarnErrorOptionValue>,
    #[serde(default)]
    pub warn: Vec<WarnErrorOptionValue>,
    #[serde(default)]
    pub silence: Vec<WarnErrorOptionValue>,
    #[serde(default, skip_serializing)]
    pub __ignored__: Verbatim<HashMap<String, Value>>,
    /// True if the deprecated `include` key was used instead of `error`.
    #[serde(skip)]
    pub used_deprecated_include: bool,
    /// True if the deprecated `exclude` key was used instead of `silence`.
    #[serde(skip)]
    pub used_deprecated_exclude: bool,
}

/// Decisions order in their precedence order, from lowest to highest:
/// - Upgrade the warning to an error
/// - Retain the warning as is
/// - Silence the warning entirely
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WarnErrorDecision {
    UpgradeToError,
    Retain,
    Silence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCtx {
    /// Error was emitted while processing a dependency package.
    DependencyPackage,
    /// Error was emitted while processing the root project.
    ProjectRoot,
    /// Caller does not have enough context to determine package ownership.
    Unknown,
}

impl ErrorCtx {
    /// Builds context from the package name field used by structured log messages.
    pub fn from_dependency_package_name(package_name: Option<impl AsRef<str>>) -> Self {
        if package_name.is_some() {
            Self::DependencyPackage
        } else {
            Self::ProjectRoot
        }
    }
}

// Parsing related implementations. For resolving logic see separate block below
impl WarnErrorOptions {
    pub fn from_cli_mapping(mapping: BTreeMap<String, Value>) -> Self {
        Self::from_key_value_pairs(mapping.iter().map(|(key, value)| (key.as_str(), value)))
    }

    pub fn from_yaml_value(value: &Value) -> Self {
        let Value::Mapping(mapping, _) = value else {
            return Self::default();
        };

        Self::from_key_value_pairs(
            mapping
                .iter()
                .filter_map(|(key, value)| key.as_str().map(|key| (key, value))),
        )
    }

    pub fn add_all_to_error(&mut self) {
        self.extend_error([WarnErrorOptionValue::all()]);
    }

    /// Validates fully resolved warn-error options and returns the user-facing warning/error text.
    ///
    /// Branches are intentionally collapsed into one imperative pass so resolution can emit
    /// stable warning lines and at most one error block after precedence has already been applied:
    /// - legacy values we will never support contribute one line each next as they have individual messages
    /// - recognized-but-not-yet-supported values are aggregated into one final warning line.
    /// - bogus values or unknown Fusion numeric codes become a single error block.
    pub fn validation_messages(&self) -> (Vec<String>, Option<String>) {
        let mut invalid_fusion_codes = BTreeSet::new();
        let mut invalid_raw_values = BTreeSet::new();
        let mut will_not_support = BTreeMap::new();
        let mut not_yet_supported = BTreeSet::new();

        for value in self
            .error
            .iter()
            .chain(self.warn.iter())
            .chain(self.silence.iter())
        {
            match value {
                // Numeric values are either a supported Fusion code or entirely invalid if the code does not exist.
                WarnErrorOptionValue::FusionCode(code) => match ErrorCode::try_from(*code) {
                    Ok(_) => {}
                    Err(_) => {
                        invalid_fusion_codes.insert(*code);
                    }
                },
                WarnErrorOptionValue::LegacyGroup(
                    LegacyWarnErrorGroupValue::All | LegacyWarnErrorGroupValue::Deprecations,
                ) => {}
                WarnErrorOptionValue::FusionGroup(
                    FusionWarnErrorGroupValue::StaticAnalysis
                    | FusionWarnErrorGroupValue::PackageParsingCompatibilityErrors,
                ) => {}
                WarnErrorOptionValue::SupportedLegacy(_) => {}
                WarnErrorOptionValue::NotYetSupportedLegacy(legacy) => {
                    not_yet_supported.insert(legacy.as_ref());
                }
                // These legacy values are known and intentionally unsupported, so keep each reason
                // as its own line and sort by the legacy event name for stable output.
                WarnErrorOptionValue::WillNotSupportLegacy(legacy) => {
                    will_not_support.insert(
                        legacy.as_ref(),
                        legacy.get_message().expect(
                            "will-not-support legacy variants must have messages. ensured via test",
                        ),
                    );
                }
                // Text values that are neither known dbt-core events nor recognized groups are
                // invalid user input and should hard-error instead of silently proceeding.
                WarnErrorOptionValue::Unsupported(raw) => {
                    invalid_raw_values.insert(raw);
                }
            }
        }

        let mut warning_lines = will_not_support
            .into_iter()
            .map(|(name, reason)| {
                format!(
                    "warn_error_options value `{name}` will not be supported in Fusion: {reason} Please remove from cli argument or config."
                )
            })
            .collect::<Vec<_>>();

        const MAX_CODES_IN_MSG: usize = 50;

        if let Some(warn_line) = match not_yet_supported.len() {
            0 => None,
            1 => Some(format!(
                "warn_error_options value `{}` is recognized, but Fusion does not support it yet.",
                not_yet_supported.iter().next().unwrap()
            )),
            len if len <= MAX_CODES_IN_MSG => {
                let values = not_yet_supported
                    .into_iter()
                    .map(|value| format!("`{value}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!(
                    "warn_error_options values {values} are recognized, but Fusion does not support them yet."
                ))
            }
            // Too many, show at most MAX_CODES_IN_MSG and N others
            len => {
                let values = not_yet_supported
                    .into_iter()
                    .take(MAX_CODES_IN_MSG)
                    .map(|value| format!("`{value}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!(
                    "warn_error_options values {values} and {} others are recognized, but Fusion does not support them yet.",
                    len - MAX_CODES_IN_MSG
                ))
            }
        } {
            warning_lines.push(warn_line);
        }

        let mut error_lines = Vec::new();

        // Parsed numbers that are not valid Fusion error codes use a dedicated error message
        if let Some(error_line) = match invalid_fusion_codes.len() {
            0 => None,
            1 => Some(format!(
                "warn_error_options value `{}` is not a known Fusion error code.",
                invalid_fusion_codes.iter().next().unwrap()
            )),
            len if len <= MAX_CODES_IN_MSG => {
                let values = invalid_fusion_codes
                    .into_iter()
                    .map(|value| format!("`{value}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!(
                    "warn_error_options values {values} are not known Fusion error codes."
                ))
            }
            // Too many, show at most MAX_CODES_IN_MSG and N others
            len => {
                let values = invalid_fusion_codes
                    .into_iter()
                    .take(MAX_CODES_IN_MSG)
                    .map(|value| format!("`{value}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!(
                    "warn_error_options values {values} and {} others are not known Fusion error codes.",
                    len - MAX_CODES_IN_MSG
                ))
            }
        } {
            error_lines.push(error_line);
        }

        // All other unknown values, including large numbers that are out of u16 bounds
        if let Some(error_line) = match invalid_raw_values.len() {
            0 => None,
            1 => Some(format!(
                "warn_error_options value `{}` is invalid because it is not a known Fusion error name, dbt-core event name, or supported warn-error group.",
                invalid_raw_values.iter().next().unwrap()
            )),
            len if len <= MAX_CODES_IN_MSG => {
                let values = invalid_raw_values
                    .into_iter()
                    .map(|value| format!("`{value}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!(
                    "warn_error_options values {values} are invalid because they are not known Fusion error names, dbt-core event names, or supported warn-error groups."
                ))
            }
            // Too many, show at most MAX_CODES_IN_MSG and N others
            len => {
                let values = invalid_raw_values
                    .into_iter()
                    .take(MAX_CODES_IN_MSG)
                    .map(|value| format!("`{value}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!(
                    "warn_error_options values {values} and {} others are invalid because they are not known Fusion error names, dbt-core event names, or supported warn-error groups.",
                    len - MAX_CODES_IN_MSG
                ))
            }
        } {
            error_lines.push(error_line);
        }

        (
            warning_lines,
            (!error_lines.is_empty()).then(|| error_lines.join("\n")),
        )
    }

    pub fn deprecated_keys_message(&self) -> Option<String> {
        if !self.used_deprecated_include && !self.used_deprecated_exclude {
            return None;
        }

        let include_msg = if self.used_deprecated_include {
            "use 'error' instead of 'include'. "
        } else {
            ""
        };
        let exclude_msg = if self.used_deprecated_exclude {
            "use 'warn' instead of 'exclude'. "
        } else {
            ""
        };

        Some(format!(
            "Found deprecated keys in warn_error_options: {include_msg}{exclude_msg}"
        ))
    }

    fn extend_error(&mut self, values: impl IntoIterator<Item = WarnErrorOptionValue>) {
        extend_unique(&mut self.error, values);
    }

    fn extend_warn(&mut self, values: impl IntoIterator<Item = WarnErrorOptionValue>) {
        extend_unique(&mut self.warn, values);
    }

    fn extend_silence(&mut self, values: impl IntoIterator<Item = WarnErrorOptionValue>) {
        extend_unique(&mut self.silence, values);
    }

    fn from_key_value_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a Value)>) -> Self {
        // TODO: we currently allow `warn` to be present even when `All` or `Deprecations` are not
        // present in `error` which is not matching dbt-core which errors. But resolution logic
        // is still honoring precedence correctly
        let mut warn_error_options = Self::default();

        for (key, value) in pairs {
            match key.to_ascii_lowercase().as_str() {
                "include" => {
                    warn_error_options.used_deprecated_include = true;
                    warn_error_options.extend_error(parse_warn_error_option_values(value));
                }
                "error" => {
                    warn_error_options.extend_error(parse_warn_error_option_values(value));
                }
                "warn" => {
                    warn_error_options.extend_warn(parse_warn_error_option_values(value));
                }
                "exclude" => {
                    warn_error_options.used_deprecated_exclude = true;
                    warn_error_options.extend_silence(parse_warn_error_option_values(value));
                }
                "silence" => {
                    warn_error_options.extend_silence(parse_warn_error_option_values(value));
                }
                _ => {}
            }
        }

        warn_error_options
    }
}

pub fn parse_warn_error_options(value: &str) -> Result<WarnErrorOptions, String> {
    crate::io_args::check_key_value_cli_arg(value).map(WarnErrorOptions::from_cli_mapping)
}

pub fn resolve_warn_error_options(
    from_cli: Option<bool>,
    from_cli_or_env: Option<&WarnErrorOptions>,
    project_flags: Option<&Value>,
) -> (bool, WarnErrorOptions) {
    let warn_error = from_cli.unwrap_or_else(|| {
        project_flags
            .and_then(project_flags_warn_error)
            .unwrap_or_default()
    });
    let mut warn_error_options = from_cli_or_env.cloned().unwrap_or_else(|| {
        project_flags
            .and_then(project_flags_warn_error_options)
            .unwrap_or_default()
    });
    if warn_error {
        warn_error_options.add_all_to_error();
    }
    (warn_error, warn_error_options)
}

fn project_flags_warn_error(flags: &Value) -> Option<bool> {
    project_flags_get_value(flags, "warn_error").and_then(Value::as_bool)
}

fn project_flags_warn_error_options(flags: &Value) -> Option<WarnErrorOptions> {
    project_flags_get_value(flags, "warn_error_options").map(WarnErrorOptions::from_yaml_value)
}

pub fn project_flags_get_value<'a>(flags: &'a Value, key: &str) -> Option<&'a Value> {
    let Value::Mapping(mapping, _) = flags else {
        return None;
    };

    mapping.iter().find_map(|(candidate, value)| {
        candidate
            .as_str()
            .filter(|candidate| candidate.eq_ignore_ascii_case(key))
            .map(|_| value)
    })
}

fn extend_unique(
    target: &mut Vec<WarnErrorOptionValue>,
    values: impl IntoIterator<Item = WarnErrorOptionValue>,
) {
    for value in values {
        if !target.contains(&value) {
            target.push(value);
        }
    }
}

fn parse_warn_error_option_values(value: &Value) -> Vec<WarnErrorOptionValue> {
    match value {
        Value::Sequence(values, _) => values
            .iter()
            .filter_map(parse_warn_error_option_value)
            .collect(),
        _ => parse_warn_error_option_value(value).into_iter().collect(),
    }
}

fn parse_warn_error_option_value(value: &Value) -> Option<WarnErrorOptionValue> {
    // Fusion errors are configured by name, not by numeric dbt code.
    if let Some(code) = value.as_i64() {
        return Some(WarnErrorOptionValue::Unsupported(code.to_string()));
    }

    if let Some(code) = value.as_u64() {
        return Some(WarnErrorOptionValue::Unsupported(code.to_string()));
    }

    let raw = value.as_str()?;
    if raw == "*" || raw.eq_ignore_ascii_case("all") {
        return Some(WarnErrorOptionValue::all());
    }

    if raw.eq_ignore_ascii_case("Deprecations") {
        return Some(WarnErrorOptionValue::LegacyGroup(
            LegacyWarnErrorGroupValue::Deprecations,
        ));
    }

    if raw == "StaticAnalysis" {
        return Some(WarnErrorOptionValue::FusionGroup(
            FusionWarnErrorGroupValue::StaticAnalysis,
        ));
    }

    if raw == "PackageParsingCompatibility" {
        return Some(WarnErrorOptionValue::FusionGroup(
            FusionWarnErrorGroupValue::PackageParsingCompatibilityErrors,
        ));
    }

    if let Ok(code) = raw.parse::<ErrorCode>() {
        return Some(WarnErrorOptionValue::FusionCode(code as u16));
    }

    if let Ok(legacy) = raw.try_into() {
        return Some(WarnErrorOptionValue::SupportedLegacy(legacy));
    }

    if let Ok(legacy) = raw.try_into() {
        return Some(WarnErrorOptionValue::WillNotSupportLegacy(legacy));
    }

    if raw.parse::<NotAWarningInDbtCoreLegacyWarnError>().is_ok() {
        return None;
    }

    if let Ok(legacy) = raw.try_into() {
        return Some(WarnErrorOptionValue::NotYetSupportedLegacy(legacy));
    }

    Some(WarnErrorOptionValue::Unsupported(raw.to_string()))
}

impl WarnErrorOptions {
    pub fn decision_for_error_code(&self, error_code: ErrorCode) -> WarnErrorDecision {
        self.decision_for_error_code_with_context(error_code, ErrorCtx::Unknown)
    }

    pub fn decision_for_error_code_with_context(
        &self,
        error_code: ErrorCode,
        error_ctx: ErrorCtx,
    ) -> WarnErrorDecision {
        // dbt-core precedence logic is as follows:
        // 1. named event > Deprecations > "all" / "*"
        // 2. silence > warn > error

        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        enum MatchType {
            NotMatched,
            All, // least specific match, matches all error codes
            Group,
            NamedEvent,
        }

        let matches = |value: &WarnErrorOptionValue| match value {
            WarnErrorOptionValue::LegacyGroup(LegacyWarnErrorGroupValue::All) => MatchType::All,
            WarnErrorOptionValue::LegacyGroup(LegacyWarnErrorGroupValue::Deprecations)
                if matches_deprecations_group(error_code) =>
            {
                MatchType::Group
            }
            WarnErrorOptionValue::FusionGroup(FusionWarnErrorGroupValue::StaticAnalysis)
                if error_code.is_frontend() =>
            {
                MatchType::Group
            }
            WarnErrorOptionValue::FusionGroup(
                FusionWarnErrorGroupValue::PackageParsingCompatibilityErrors,
            ) if Self::matches_package_parsing_compatibility(error_code, error_ctx) => {
                MatchType::Group
            }
            WarnErrorOptionValue::FusionCode(code) if *code == error_code as u16 => {
                MatchType::NamedEvent
            }
            WarnErrorOptionValue::SupportedLegacy(legacy)
                if matches_legacy_error_code(*legacy, error_code) =>
            {
                MatchType::NamedEvent
            }
            _ => MatchType::NotMatched,
        };

        // Keep the canary warning-only unless the code is selected explicitly.
        let implicit_warn_match = if error_code == ErrorCode::DeprecatedConfigKey {
            MatchType::Group
        } else {
            MatchType::NotMatched
        };

        let max_matches_and_verdict = [
            (
                self.silence
                    .iter()
                    .map(matches)
                    .max()
                    .unwrap_or(MatchType::NotMatched),
                WarnErrorDecision::Silence,
            ),
            (
                self.warn
                    .iter()
                    .map(matches)
                    .max()
                    .unwrap_or(MatchType::NotMatched)
                    .max(implicit_warn_match),
                WarnErrorDecision::Retain,
            ),
            (
                self.error
                    .iter()
                    .map(matches)
                    .max()
                    .unwrap_or(MatchType::NotMatched),
                WarnErrorDecision::UpgradeToError,
            ),
        ];

        max_matches_and_verdict
            .into_iter()
            // Do not consider options that do not match the error code at all
            .filter(|(match_type, _verdict)| *match_type != MatchType::NotMatched)
            // Order first by most specific match type, then by verdict precedence (silence > warn > error)
            .max()
            // If one exist, then this is the verdict we return
            .map(|(_match_type, verdict)| verdict)
            // default to retaining the warning if no matches at all, including "all"
            .unwrap_or(WarnErrorDecision::Retain)
    }

    fn matches_package_parsing_compatibility(error_code: ErrorCode, error_ctx: ErrorCtx) -> bool {
        error_code == ErrorCode::PackageParsingCompatibility
            || matches!(error_ctx, ErrorCtx::DependencyPackage)
                && matches!(
                    error_code,
                    ErrorCode::DuplicateConfigKey
                        | ErrorCode::UnusedConfigKey
                        | ErrorCode::SerializationError
                        | ErrorCode::DbtYamlValidationError
                        | ErrorCode::JinjaError
                        | ErrorCode::MacroSyntaxInvalid
                        | ErrorCode::InvalidConfig
                        | ErrorCode::ExecutionError
                        | ErrorCode::YamlInvalid
                )
    }

    pub fn decision_for_supported_legacy(
        &self,
        legacy: SupportedLegacyWarnError,
    ) -> WarnErrorDecision {
        // 0 = no match, 1 = via "all", 2 = by name
        let specificity = |list: &[WarnErrorOptionValue]| {
            list.iter().fold(0u8, |best, v| match v {
                WarnErrorOptionValue::SupportedLegacy(c) if *c == legacy => 2,
                WarnErrorOptionValue::LegacyGroup(LegacyWarnErrorGroupValue::All) => best.max(1),
                _ => best,
            })
        };

        // Most-specific match wins; on ties, verdict ord breaks ties (silence > warn > error)
        match [
            (specificity(&self.silence), WarnErrorDecision::Silence),
            (specificity(&self.warn), WarnErrorDecision::Retain),
            (specificity(&self.error), WarnErrorDecision::UpgradeToError),
        ]
        .into_iter()
        .max()
        {
            Some((s, v)) if s > 0 => v,
            _ => WarnErrorDecision::Retain,
        }
    }
}

fn matches_deprecations_group(error_code: ErrorCode) -> bool {
    matches!(
        error_code,
        ErrorCode::PackageRedirectDeprecation
            | ErrorCode::WEOIncludeExcludeDeprecation
            | ErrorCode::MalformedBlockName
    )
}

/// Returns true if no dbt-core warning maps to this error code.
///
/// Derived from the legacy mapping rather than a hand-kept list, so a newly
/// added code counts as Fusion-only until it is wired to a dbt-core event.
pub fn is_fusion_only_warning(error_code: ErrorCode) -> bool {
    !matches_deprecations_group(error_code)
        && !SupportedLegacyWarnError::iter()
            .any(|legacy| matches_legacy_error_code(legacy, error_code))
}

fn matches_legacy_error_code(legacy: SupportedLegacyWarnError, error_code: ErrorCode) -> bool {
    match legacy {
        SupportedLegacyWarnError::JinjaLogWarning => error_code == ErrorCode::JinjaWarn,
        // This one is matched against node event, not error code
        SupportedLegacyWarnError::LogTestResult => false,
        SupportedLegacyWarnError::NothingToDo | SupportedLegacyWarnError::NoNodesSelected => {
            error_code == ErrorCode::NoNodesSelected
        }
        SupportedLegacyWarnError::NoNodesForSelectionCriteria => {
            error_code == ErrorCode::NoNodesForSelectionCriteria
        }
        SupportedLegacyWarnError::RunResultWarning
        | SupportedLegacyWarnError::RunResultWarningMessage => false,
        SupportedLegacyWarnError::NodeNotFoundOrDisabled => {
            error_code == ErrorCode::NodeNotFoundOrDisabled
        }
        SupportedLegacyWarnError::NoNodeForYamlKey => error_code == ErrorCode::NoNodeForYamlKey,
        SupportedLegacyWarnError::MacroNotFoundForPatch => {
            error_code == ErrorCode::MacroPatchNotFound
        }
        SupportedLegacyWarnError::InvalidConcurrentBatchesConfig => {
            error_code == ErrorCode::InvalidConcurrentBatchesConfig
        }
        SupportedLegacyWarnError::MicrobatchModelNoEventTimeInputs => {
            error_code == ErrorCode::MicrobatchModelNoEventTimeInputs
        }
        SupportedLegacyWarnError::InvalidMacroAnnotation => {
            error_code == ErrorCode::ValidateMacroArgs
        }
        SupportedLegacyWarnError::DeprecatedModel => error_code == ErrorCode::DeprecatedModel,
        SupportedLegacyWarnError::DeprecatedReference => {
            error_code == ErrorCode::DeprecatedReference
        }
        SupportedLegacyWarnError::UpcomingReferenceDeprecation => {
            error_code == ErrorCode::UpcomingReferenceDeprecation
        }
        SupportedLegacyWarnError::SnapshotTimestampWarning => {
            error_code == ErrorCode::SnapshotTimestampMismatch
        }
        SupportedLegacyWarnError::PackageRedirectDeprecation => {
            error_code == ErrorCode::PackageRedirectDeprecation
        }
        SupportedLegacyWarnError::DepsUnpinned => error_code == ErrorCode::DepsUnpinned,
        SupportedLegacyWarnError::DepsScrubbedPackageName => {
            error_code == ErrorCode::DepsScrubbedPackageName
        }
        SupportedLegacyWarnError::DepsFoundDuplicatePackage => {
            error_code == ErrorCode::DepsDuplicatePackage
        }
        SupportedLegacyWarnError::FreshnessConfigProblem => {
            error_code == ErrorCode::FreshnessConfigInvalid
        }
        SupportedLegacyWarnError::WarnStateTargetEqual => {
            error_code == ErrorCode::WarnStateTargetEqual
        }
        SupportedLegacyWarnError::WEOIncludeExcludeDeprecation => {
            error_code == ErrorCode::WEOIncludeExcludeDeprecation
        }
        SupportedLegacyWarnError::UnversionedBreakingChange => {
            error_code == ErrorCode::UnversionedBreakingChange
        }
        SupportedLegacyWarnError::UnsupportedConstraintMaterialization => {
            error_code == ErrorCode::UnsupportedConstraintMaterialization
        }
        SupportedLegacyWarnError::UnusedResourceConfigPath => {
            error_code == ErrorCode::UnusedResourceConfigPath
        }
        SupportedLegacyWarnError::ConstraintNotEnforced => {
            error_code == ErrorCode::ConstraintNotEnforced
        }
        SupportedLegacyWarnError::ConstraintNotSupported => {
            error_code == ErrorCode::ConstraintNotSupported
        }
    }
}

// Tests here are only for things not covered with e2e dbt-cli tests
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorCode;

    /// Guards the derivation in [`is_fusion_only_warning`]: every legacy warning
    /// wired to an error code must count as having a dbt-core counterpart.
    #[test]
    fn mapped_legacy_warnings_are_not_fusion_only() {
        for legacy in SupportedLegacyWarnError::iter() {
            for code in [
                ErrorCode::JinjaWarn,
                ErrorCode::NoNodesSelected,
                ErrorCode::DeprecatedModel,
                ErrorCode::PackageRedirectDeprecation,
                ErrorCode::UnusedResourceConfigPath,
                ErrorCode::ConstraintNotSupported,
            ] {
                if matches_legacy_error_code(legacy, code) {
                    assert!(
                        !is_fusion_only_warning(code),
                        "{code:?} maps to legacy warning {legacy:?} and must not be Fusion-only"
                    );
                }
            }
        }
    }

    #[test]
    fn deprecations_group_codes_are_not_fusion_only() {
        for code in [
            ErrorCode::PackageRedirectDeprecation,
            ErrorCode::WEOIncludeExcludeDeprecation,
            ErrorCode::MalformedBlockName,
        ] {
            assert!(
                !is_fusion_only_warning(code),
                "{code:?} is a dbt-core deprecation"
            );
        }
    }

    #[test]
    fn codes_without_a_legacy_mapping_are_fusion_only() {
        for code in [
            ErrorCode::PackageParsingCompatibility,
            ErrorCode::UnusedConfigKey,
        ] {
            assert!(
                is_fusion_only_warning(code),
                "{code:?} has no dbt-core counterpart"
            );
        }
    }

    #[test]
    fn deprecated_config_key_canary_is_not_escalated_by_warn_error() {
        assert!(
            is_fusion_only_warning(ErrorCode::DeprecatedConfigKey),
            "DeprecatedConfigKey must stay Fusion-only while it is a canary"
        );

        let mut blanket_warn_error = WarnErrorOptions::default();
        blanket_warn_error.add_all_to_error();
        assert_eq!(
            blanket_warn_error.decision_for_error_code(ErrorCode::DeprecatedConfigKey),
            WarnErrorDecision::Retain,
            "blanket --warn-error must not turn the canary into an error"
        );

        let silenced_by_name = WarnErrorOptions {
            silence: vec![WarnErrorOptionValue::FusionCode(
                ErrorCode::DeprecatedConfigKey as u16,
            )],
            ..Default::default()
        };
        assert_eq!(
            silenced_by_name.decision_for_error_code(ErrorCode::DeprecatedConfigKey),
            WarnErrorDecision::Silence,
        );

        let escalated_by_name = WarnErrorOptions {
            error: vec![WarnErrorOptionValue::FusionCode(
                ErrorCode::DeprecatedConfigKey as u16,
            )],
            ..Default::default()
        };
        assert_eq!(
            escalated_by_name.decision_for_error_code(ErrorCode::DeprecatedConfigKey),
            WarnErrorDecision::UpgradeToError,
        );
    }

    #[test]
    fn error_decision_honors_precedence() {
        let options = WarnErrorOptions {
            error: vec![
                WarnErrorOptionValue::FusionCode(ErrorCode::Generic as u16),
                WarnErrorOptionValue::FusionCode(ErrorCode::NoNodesSelected as u16),
                WarnErrorOptionValue::FusionCode(ErrorCode::IoError as u16),
                WarnErrorOptionValue::LegacyGroup(LegacyWarnErrorGroupValue::All),
            ],
            warn: vec![
                // This one in silence, so silence should win
                WarnErrorOptionValue::FusionCode(ErrorCode::NoNodesSelected as u16),
                // This one should win over error because it's a specific match,
                // neither All, nor direct code should override it
                WarnErrorOptionValue::FusionCode(ErrorCode::IoError as u16),
            ],
            silence: vec![WarnErrorOptionValue::FusionCode(
                ErrorCode::NoNodesSelected as u16,
            )],
            ..Default::default()
        };

        assert_eq!(
            options.decision_for_error_code(ErrorCode::NoNodesSelected),
            WarnErrorDecision::Silence,
            "Specific code match in silence should take precedence over all matches in warn and error, including specific matches"
        );

        assert_eq!(
            options.decision_for_error_code(ErrorCode::JsonInvalid),
            WarnErrorDecision::UpgradeToError,
            "All group in error should take precedence over all match in warn and all in silence"
        );

        assert_eq!(
            options.decision_for_error_code(ErrorCode::IoError),
            WarnErrorDecision::Retain,
            "Specific code match in warn should take precedence over all matches in error"
        );

        let deprecations_options = WarnErrorOptions {
            error: vec![WarnErrorOptionValue::LegacyGroup(
                LegacyWarnErrorGroupValue::All,
            )],
            warn: vec![WarnErrorOptionValue::LegacyGroup(
                LegacyWarnErrorGroupValue::Deprecations,
            )],
            silence: vec![WarnErrorOptionValue::FusionCode(
                ErrorCode::WEOIncludeExcludeDeprecation as u16,
            )],
            ..Default::default()
        };

        assert_eq!(
            deprecations_options.decision_for_error_code(ErrorCode::WEOIncludeExcludeDeprecation),
            WarnErrorDecision::Silence,
            "Named event silence should beat Deprecations warn and All error"
        );
    }

    #[test]
    fn malformed_block_name_is_a_deprecation() {
        let options = WarnErrorOptions {
            error: vec![WarnErrorOptionValue::LegacyGroup(
                LegacyWarnErrorGroupValue::Deprecations,
            )],
            ..Default::default()
        };

        assert_eq!(
            options.decision_for_error_code(ErrorCode::MalformedBlockName),
            WarnErrorDecision::UpgradeToError,
            "MalformedBlockName should be escalated by the Deprecations group"
        );
    }

    #[test]
    fn legacy_decision_match_precedence() {
        use SupportedLegacyWarnError::LogTestResult;

        let options = WarnErrorOptions {
            error: vec![WarnErrorOptionValue::SupportedLegacy(LogTestResult)],
            ..Default::default()
        };
        assert_eq!(
            options.decision_for_supported_legacy(LogTestResult),
            WarnErrorDecision::UpgradeToError,
        );

        let options = WarnErrorOptions {
            error: vec![WarnErrorOptionValue::all()],
            ..Default::default()
        };
        assert_eq!(
            options.decision_for_supported_legacy(LogTestResult),
            WarnErrorDecision::UpgradeToError,
        );

        let options = WarnErrorOptions {
            error: vec![WarnErrorOptionValue::all()],
            warn: vec![WarnErrorOptionValue::SupportedLegacy(LogTestResult)],
            ..Default::default()
        };
        assert_eq!(
            options.decision_for_supported_legacy(LogTestResult),
            WarnErrorDecision::Retain,
        );

        assert_eq!(
            WarnErrorOptions::default().decision_for_supported_legacy(LogTestResult),
            WarnErrorDecision::Retain,
        );
    }

    #[test]
    fn parses_legacy_cli_shape_case_insensitively_and_deduplicates() {
        let parsed = parse_warn_error_options(
            "{InClUdE: [all, '*', ALL, BogusWarningClass, BogusWarningClass, 17, DepsDuplicatePackage, 1099], ExClUdE: [foo, foo]}",
        )
        .unwrap();

        assert_eq!(
            parsed,
            WarnErrorOptions {
                error: vec![
                    WarnErrorOptionValue::all(),
                    WarnErrorOptionValue::Unsupported("BogusWarningClass".to_string()),
                    WarnErrorOptionValue::Unsupported("17".to_string()),
                    WarnErrorOptionValue::FusionCode(ErrorCode::DepsDuplicatePackage as u16),
                    WarnErrorOptionValue::Unsupported("1099".to_string()),
                ],
                warn: vec![],
                silence: vec![WarnErrorOptionValue::Unsupported("foo".to_string())],
                used_deprecated_include: true,
                used_deprecated_exclude: true,
                ..Default::default()
            }
        );
    }

    #[test]
    fn parses_v2_shape_and_ignores_unknown_keys() {
        let parsed = WarnErrorOptions::from_yaml_value(
            &dbt_yaml::from_str(
                "{error: [Generic, IoError, all], WARN: NoNodesForSelectionCriteria, silence: [x, x], bogus: [2]}",
            )
            .unwrap(),
        );

        assert_eq!(
            parsed,
            WarnErrorOptions {
                error: vec![
                    WarnErrorOptionValue::FusionCode(ErrorCode::Generic as u16),
                    WarnErrorOptionValue::FusionCode(ErrorCode::IoError as u16),
                    WarnErrorOptionValue::all()
                ],
                warn: vec![WarnErrorOptionValue::FusionCode(
                    ErrorCode::NoNodesForSelectionCriteria as u16,
                )],
                silence: vec![WarnErrorOptionValue::Unsupported("x".to_string())],
                ..Default::default()
            }
        );
    }

    #[test]
    fn not_a_warning_in_dbt_core_legacy_names_parse_and_validate_silently() {
        use strum::IntoEnumIterator;

        for variant in NotAWarningInDbtCoreLegacyWarnError::iter() {
            let name = variant.as_ref();
            let parsed = parse_warn_error_options(&format!(
                "{{error: [{name}], warn: [{name}], silence: [{name}]}}"
            ))
            .unwrap();

            assert_eq!(parsed.error, Vec::new());
            assert_eq!(parsed.warn, Vec::new());
            assert_eq!(parsed.silence, Vec::new());
            assert_eq!(parsed.validation_messages(), (Vec::new(), None));
        }
    }

    #[test]
    fn new_legacy_names_parse_and_resolve() {
        let cases: &[(&str, SupportedLegacyWarnError, ErrorCode)] = &[
            (
                "NodeNotFoundOrDisabled",
                SupportedLegacyWarnError::NodeNotFoundOrDisabled,
                ErrorCode::NodeNotFoundOrDisabled,
            ),
            (
                "NoNodeForYamlKey",
                SupportedLegacyWarnError::NoNodeForYamlKey,
                ErrorCode::NoNodeForYamlKey,
            ),
            (
                "MacroNotFoundForPatch",
                SupportedLegacyWarnError::MacroNotFoundForPatch,
                ErrorCode::MacroPatchNotFound,
            ),
            (
                "InvalidConcurrentBatchesConfig",
                SupportedLegacyWarnError::InvalidConcurrentBatchesConfig,
                ErrorCode::InvalidConcurrentBatchesConfig,
            ),
            (
                "InvalidMacroAnnotation",
                SupportedLegacyWarnError::InvalidMacroAnnotation,
                ErrorCode::ValidateMacroArgs,
            ),
            (
                "DeprecatedModel",
                SupportedLegacyWarnError::DeprecatedModel,
                ErrorCode::DeprecatedModel,
            ),
            (
                "DeprecatedReference",
                SupportedLegacyWarnError::DeprecatedReference,
                ErrorCode::DeprecatedReference,
            ),
            (
                "UpcomingReferenceDeprecation",
                SupportedLegacyWarnError::UpcomingReferenceDeprecation,
                ErrorCode::UpcomingReferenceDeprecation,
            ),
            (
                "JinjaLogWarning",
                SupportedLegacyWarnError::JinjaLogWarning,
                ErrorCode::JinjaWarn,
            ),
            (
                "SnapshotTimestampWarning",
                SupportedLegacyWarnError::SnapshotTimestampWarning,
                ErrorCode::SnapshotTimestampMismatch,
            ),
            (
                "PackageRedirectDeprecation",
                SupportedLegacyWarnError::PackageRedirectDeprecation,
                ErrorCode::PackageRedirectDeprecation,
            ),
            (
                "DepsUnpinned",
                SupportedLegacyWarnError::DepsUnpinned,
                ErrorCode::DepsUnpinned,
            ),
            (
                "DepsScrubbedPackageName",
                SupportedLegacyWarnError::DepsScrubbedPackageName,
                ErrorCode::DepsScrubbedPackageName,
            ),
            (
                "DepsFoundDuplicatePackage",
                SupportedLegacyWarnError::DepsFoundDuplicatePackage,
                ErrorCode::DepsDuplicatePackage,
            ),
            (
                "FreshnessConfigProblem",
                SupportedLegacyWarnError::FreshnessConfigProblem,
                ErrorCode::FreshnessConfigInvalid,
            ),
            (
                "WarnStateTargetEqual",
                SupportedLegacyWarnError::WarnStateTargetEqual,
                ErrorCode::WarnStateTargetEqual,
            ),
            (
                "NoNodesForSelectionCriteria",
                SupportedLegacyWarnError::NoNodesForSelectionCriteria,
                ErrorCode::NoNodesForSelectionCriteria,
            ),
            (
                "UnversionedBreakingChange",
                SupportedLegacyWarnError::UnversionedBreakingChange,
                ErrorCode::UnversionedBreakingChange,
            ),
            (
                "UnsupportedConstraintMaterialization",
                SupportedLegacyWarnError::UnsupportedConstraintMaterialization,
                ErrorCode::UnsupportedConstraintMaterialization,
            ),
            (
                "UnusedResourceConfigPath",
                SupportedLegacyWarnError::UnusedResourceConfigPath,
                ErrorCode::UnusedResourceConfigPath,
            ),
            // Tested separately below.
        ];

        for (name, expected_legacy, expected_code) in cases {
            // Exact Fusion names parse as codes; legacy-only aliases still parse as supported legacy.
            let parsed = parse_warn_error_options(&format!("{{error: [{name}]}}")).unwrap();
            let expected_value = name
                .parse::<ErrorCode>()
                .map(|code| WarnErrorOptionValue::FusionCode(code as u16))
                .unwrap_or(WarnErrorOptionValue::SupportedLegacy(*expected_legacy));
            assert_eq!(
                parsed.error,
                vec![expected_value],
                "{name} should parse as a supported warn-error option",
            );

            // Verify the legacy name maps to the expected ErrorCode
            assert!(
                matches_legacy_error_code(*expected_legacy, *expected_code),
                "{name} should match {expected_code:?}",
            );

            // Verify decision_for_error_code upgrades when in error list
            assert_eq!(
                parsed.decision_for_error_code(*expected_code),
                WarnErrorDecision::UpgradeToError,
                "{name} in error list should upgrade {expected_code:?}",
            );
        }

        assert!(matches_legacy_error_code(
            SupportedLegacyWarnError::JinjaLogWarning,
            ErrorCode::JinjaWarn,
        ));

        assert!(matches_legacy_error_code(
            SupportedLegacyWarnError::WEOIncludeExcludeDeprecation,
            ErrorCode::WEOIncludeExcludeDeprecation,
        ));
        let parsed = parse_warn_error_options("{error: [WEOIncludeExcludeDeprecation]}").unwrap();
        assert_eq!(
            parsed.decision_for_error_code(ErrorCode::WEOIncludeExcludeDeprecation),
            WarnErrorDecision::UpgradeToError,
            "WEOIncludeExcludeDeprecation in error list should upgrade",
        );
    }

    #[test]
    fn package_parsing_compatibility_group_parse_and_resolve() {
        let parsed = parse_warn_error_options(
            "{silence: [PackageParsingCompatibility], error: [DuplicateConfigKey]}",
        )
        .unwrap();

        assert_eq!(
            parsed.silence,
            vec![WarnErrorOptionValue::FusionGroup(
                FusionWarnErrorGroupValue::PackageParsingCompatibilityErrors,
            )],
        );
        assert_eq!(
            parsed.decision_for_error_code(ErrorCode::DuplicateConfigKey),
            WarnErrorDecision::UpgradeToError,
            "specific visible code should match by fusion error name",
        );
        assert_eq!(
            parsed.decision_for_error_code(ErrorCode::PackageParsingCompatibility),
            WarnErrorDecision::Silence,
            "package compatibility group should match the aggregate code",
        );

        let aggregate = parse_warn_error_options(
            "{error: [PackageParsingCompatibility], silence: [DuplicateConfigKey]}",
        )
        .unwrap();
        assert_eq!(
            aggregate.decision_for_error_code(ErrorCode::PackageParsingCompatibility),
            WarnErrorDecision::UpgradeToError,
            "aggregate package compatibility code should match the group",
        );
        assert_eq!(
            aggregate.decision_for_error_code(ErrorCode::DuplicateConfigKey),
            WarnErrorDecision::Silence,
            "ordinary code matching should still match explicit Fusion error names",
        );

        let group_only =
            parse_warn_error_options("{error: [PackageParsingCompatibility]}").unwrap();
        assert_eq!(
            group_only.decision_for_error_code(ErrorCode::DuplicateConfigKey),
            WarnErrorDecision::Retain,
            "context-free matching should not treat individual package parse codes as group members",
        );
        assert_eq!(
            group_only.decision_for_error_code_with_context(
                ErrorCode::DuplicateConfigKey,
                ErrorCtx::DependencyPackage,
            ),
            WarnErrorDecision::UpgradeToError,
            "package compatibility group should match visible individual package parse codes",
        );
    }

    #[test]
    fn all_supported_legacy_names_parse_correctly() {
        use strum::IntoEnumIterator;
        for variant in SupportedLegacyWarnError::iter() {
            let name = format!("{variant:?}");
            let parsed = parse_warn_error_options(&format!("{{error: [{name}]}}")).unwrap();
            assert!(
                parsed
                    .error
                    .iter()
                    .all(|v| !matches!(v, WarnErrorOptionValue::Unsupported(_))),
                "{name} should not parse as Unsupported",
            );
        }
    }

    #[test]
    fn fusion_error_names_do_not_unintentionally_overlap_legacy_names() {
        use std::collections::BTreeMap;
        use strum::IntoEnumIterator;

        let expected_overlaps = BTreeMap::from([
            (
                "ConstraintNotEnforced".to_string(),
                ErrorCode::ConstraintNotEnforced,
            ),
            (
                "ConstraintNotSupported".to_string(),
                ErrorCode::ConstraintNotSupported,
            ),
            ("DeprecatedModel".to_string(), ErrorCode::DeprecatedModel),
            (
                "DeprecatedReference".to_string(),
                ErrorCode::DeprecatedReference,
            ),
            (
                "DepsScrubbedPackageName".to_string(),
                ErrorCode::DepsScrubbedPackageName,
            ),
            ("DepsUnpinned".to_string(), ErrorCode::DepsUnpinned),
            (
                "InvalidConcurrentBatchesConfig".to_string(),
                ErrorCode::InvalidConcurrentBatchesConfig,
            ),
            (
                "MicrobatchModelNoEventTimeInputs".to_string(),
                ErrorCode::MicrobatchModelNoEventTimeInputs,
            ),
            (
                "NodeNotFoundOrDisabled".to_string(),
                ErrorCode::NodeNotFoundOrDisabled,
            ),
            ("NoNodeForYamlKey".to_string(), ErrorCode::NoNodeForYamlKey),
            (
                "NoNodesForSelectionCriteria".to_string(),
                ErrorCode::NoNodesForSelectionCriteria,
            ),
            ("NoNodesSelected".to_string(), ErrorCode::NoNodesSelected),
            (
                "PackageRedirectDeprecation".to_string(),
                ErrorCode::PackageRedirectDeprecation,
            ),
            (
                "UnversionedBreakingChange".to_string(),
                ErrorCode::UnversionedBreakingChange,
            ),
            (
                "UnsupportedConstraintMaterialization".to_string(),
                ErrorCode::UnsupportedConstraintMaterialization,
            ),
            (
                "UnusedResourceConfigPath".to_string(),
                ErrorCode::UnusedResourceConfigPath,
            ),
            (
                "UpcomingReferenceDeprecation".to_string(),
                ErrorCode::UpcomingReferenceDeprecation,
            ),
            (
                "WarnStateTargetEqual".to_string(),
                ErrorCode::WarnStateTargetEqual,
            ),
            (
                "WEOIncludeExcludeDeprecation".to_string(),
                ErrorCode::WEOIncludeExcludeDeprecation,
            ),
        ]);

        let mut actual_overlaps = BTreeMap::new();

        for name in SupportedLegacyWarnError::iter()
            .map(|variant| variant.as_ref().to_string())
            .chain(
                NotYetSupportedLegacyWarnError::iter().map(|variant| variant.as_ref().to_string()),
            )
            .chain(
                WillNotSupportLegacyWarnError::iter().map(|variant| variant.as_ref().to_string()),
            )
            .chain(
                NotAWarningInDbtCoreLegacyWarnError::iter()
                    .map(|variant| variant.as_ref().to_string()),
            )
        {
            if let Ok(code) = name.parse::<ErrorCode>() {
                actual_overlaps.insert(name, code);
            }
        }

        assert_eq!(
            actual_overlaps, expected_overlaps,
            concat!(
                "Fusion ErrorCode variant names must not unintentionally overlap legacy ",
                "dbt-core warn_error_options names.\n\n",
                "This usually means a new ErrorCode variant in ",
                "`fs/sa/crates/dbt-error/src/codes.rs` or a new frontend error code variant in ",
                "`fs/sa/crates/dbt-frontend-common/src/error/codes.rs` reused a name that ",
                "dbt-core already reserves for warn/error options. Prefer a distinct Fusion ",
                "name unless the new code has exactly the same semantic meaning as the legacy ",
                "dbt-core name.\n\n",
                "If the overlap is intentional, update the legacy-to-Fusion mapping in ",
                "`matches_legacy_error_code` in ",
                "`fs/sa/crates/dbt-common/src/warn_error_options/mod.rs`, then add the ",
                "intentional overlap to `expected_overlaps` in this test."
            ),
        );
    }

    #[test]
    fn serializes_values_canonically() {
        let options = WarnErrorOptions {
            error: vec![
                WarnErrorOptionValue::all(),
                WarnErrorOptionValue::FusionCode(ErrorCode::SelectorError as u16),
                WarnErrorOptionValue::FusionCode(17),
            ],
            warn: vec![WarnErrorOptionValue::FusionCode(
                ErrorCode::NoNodesForSelectionCriteria as u16,
            )],
            silence: vec![WarnErrorOptionValue::SupportedLegacy(
                SupportedLegacyWarnError::LogTestResult,
            )],
            ..Default::default()
        };

        assert_eq!(
            serde_json::to_value(&options).unwrap(),
            serde_json::json!({
                "error": ["all", "SelectorError", 17],
                "warn": ["NoNodesForSelectionCriteria"],
                "silence": ["LogTestResult"],
            })
        );
    }
}
