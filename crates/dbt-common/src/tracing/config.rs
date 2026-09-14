use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use super::{
    dbt_init::init_tracing_with_layers,
    fs_error_log::get_log_message,
    layers::{
        file_log_layer::build_file_log_layer_with_background_writer,
        json_compat_layer::{
            build_json_compat_layer, build_json_compat_layer_with_background_writer,
        },
        query_log::build_query_log_layer_with_background_writer,
        tui_layer::{TuiConfig, build_tui_layer},
    },
    middlewares::{
        markdown_log_filter::TelemetryMarkdownLogFilter,
        metric_aggregator::TelemetryMetricAggregator, node_warn_outcome::TelemetryNodeWarnOutcome,
        warn_error_options::TelemetryWarnErrorOptionsMiddleware,
    },
    tracing_feature_handles::{FsTracingConfigProvider, TracingConfigProvider},
};
use crate::collections::HashSet;
use crate::{
    constants::{
        DBT_DEFAULT_LOG_FILE_BACKUP_COUNT, DBT_DEFAULT_LOG_FILE_MAX_BYTES,
        DBT_DEFAULT_LOG_FILE_NAME, DBT_DEFAULT_QUERY_LOG_FILE_NAME, DBT_LOG_DIR_NAME,
        DBT_PROJECT_YML, DBT_TARGET_DIR_NAME,
    },
    io_args::{FsCommand, IoArgs, LogFormat, ShowOptions},
    io_utils::determine_project_dir,
    tracing::middlewares::parse_error_filter::TelemetryParsingErrorFilter,
    warn_error_options::WarnErrorOptions,
};
use dbt_error::{ErrorCode, FsError, FsResult};
use dbt_telemetry::TelemetryEventTypeRegistry;
use dbt_tracing::{
    LogRecordInfo,
    layer::{ConsumerLayer, MiddlewareLayer},
    layers::{
        jsonl_writer::{build_jsonl_layer, build_jsonl_layer_with_background_writer},
        otlp::{OtlpResourceConfig, build_otlp_layer},
        parquet_writer::build_parquet_writer_layer,
    },
    rotating_file_writer::RotatingFileWriter,
    shutdown::TelemetryShutdownItem,
};
use tracing::level_filters::LevelFilter;

/// Configuration for tracing.
///
/// This struct defines where trace data should be written for both debug
/// and production scenarios, and defines metadata necessary for top-level span
/// and trace correlation.
#[derive(Clone, Debug)]
pub struct FsTraceConfig {
    /// Name of the package emitting the telemetry, e.g. `dbt-cli` or `dbt-lsp`
    pub(super) package: &'static str,
    /// User-facing CLI brand name shown in the version banner and JSON log lines.
    pub(super) brand_name: &'static str,
    /// The command being executed, e.g. "run", "compile", "list"
    pub(super) command: FsCommand,
    /// Tracing level filter, which specifies maximum verbosity (inverse
    /// of log level) for tui & jsonl log sinks.
    pub(super) max_log_verbosity: LevelFilter,
    /// Maximum verbosity for the file log sink.
    pub(super) max_file_log_verbosity: LevelFilter,
    /// Fully resolved path for production telemetry output (JSONL format).
    ///
    /// If Some(), enables corresponding output layer.
    pub(super) otel_file_path: Option<PathBuf>,
    /// Fully resolved path for production telemetry output (Parquet format)
    ///
    /// If Some(), enables corresponding output layer.
    pub(super) otel_parquet_file_path: Option<PathBuf>,
    /// Fully resolved path to the directory where log-related files
    /// (e.g. dbt.log, query log) should be written.
    pub(super) log_path: PathBuf,
    /// Optional custom name for the log file. If None, defaults to `dbt.log`.
    pub(super) log_file_name: Option<String>,
    /// Max size in bytes for rotating file logs. `0` means no limit.
    pub(super) log_file_max_bytes: u64,
    /// Invocation ID. Used as trace ID for correlation
    pub(super) invocation_id: uuid::Uuid,
    /// Optional parent span ID for OpenTelemetry trace correlation.
    /// Used as fallback when creating root spans.
    pub(super) parent_span_id: Option<u64>,
    /// If True, traces will be forwarded to OTLP endpoints, if any
    /// are set via OTEL environment variables. See `OTLPExporterLayer::new`
    pub(super) export_to_otlp: bool,
    /// The log format being used
    pub(super) log_format: LogFormat,
    /// File-specific log format (`--log-format-file`), overriding `log_format` for the on-disk log sink only.
    pub(super) file_log_format: Option<LogFormat>,
    /// If True, enables separate query log file output
    pub(super) enable_query_log: bool,
    /// Show options controlling terminal/file output visibility
    pub(super) show_options: HashSet<ShowOptions>,
    /// Whether non-result console output is suppressed.
    pub(super) quiet: bool,
    /// Show all deprecations warnings/errors instead of one per package
    pub(super) show_all_deprecations: bool,
    /// The initial warn-error options loaded from CLI/env before project flags are resolved.
    pub(super) warn_error_options: WarnErrorOptions,
    /// Withholds upgrades of warnings with no dbt-core counterpart; set while replaying.
    pub(super) skip_fusion_only_upgrades: bool,
}

/// Builder for [`FsTraceConfig`].
///
/// Every field except `package` has a default, and paths are resolved by
/// [`FsTraceConfigBuilder::build`]:
/// - `project_dir`: auto-detected if unset, falling back to the current working directory
/// - `target_path`: defaults to `{project_dir}/target`
/// - `log_path`: defaults to `{project_dir}/logs`; resolved against `project_dir` if relative
/// - JSONL trace file: `{log_path}/{otel_file_name}`
/// - Parquet trace file: `{target_path}/private/metadata/{otel_parquet_file_name}`
///   (see [`crate::constants::default_metadata_dir`])
#[derive(Clone, Debug)]
pub struct FsTraceConfigBuilder {
    package: &'static str,
    brand_name: &'static str,
    command: FsCommand,
    project_dir: Option<PathBuf>,
    target_path: Option<PathBuf>,
    log_path: Option<PathBuf>,
    max_log_verbosity: LevelFilter,
    max_file_log_verbosity: LevelFilter,
    otel_file_name: Option<String>,
    otel_parquet_file_name: Option<String>,
    log_file_name: Option<String>,
    log_file_max_bytes: u64,
    invocation_id: uuid::Uuid,
    parent_span_id: Option<u64>,
    export_to_otlp: bool,
    log_format: LogFormat,
    file_log_format: Option<LogFormat>,
    enable_query_log: bool,
    show_options: HashSet<ShowOptions>,
    quiet: bool,
    show_all_deprecations: bool,
    warn_error_options: WarnErrorOptions,
    skip_fusion_only_upgrades: bool,
}

impl FsTraceConfigBuilder {
    /// `package` identifies the package emitting telemetry, e.g. `dbt` or `dbt-lsp`.
    pub fn new(package: &'static str, brand_name: &'static str) -> Self {
        Self {
            package,
            brand_name,
            command: FsCommand::Unset,
            project_dir: None,
            target_path: None,
            log_path: None,
            max_log_verbosity: LevelFilter::INFO,
            max_file_log_verbosity: LevelFilter::DEBUG,
            otel_file_name: None,
            otel_parquet_file_name: None,
            log_file_name: None,
            log_file_max_bytes: DBT_DEFAULT_LOG_FILE_MAX_BYTES,
            invocation_id: uuid::Uuid::now_v7(),
            parent_span_id: None,
            export_to_otlp: false,
            log_format: LogFormat::Default,
            file_log_format: None,
            enable_query_log: false,
            show_options: HashSet::default(),
            quiet: false,
            show_all_deprecations: false,
            warn_error_options: WarnErrorOptions::default(),
            skip_fusion_only_upgrades: false,
        }
    }

    pub fn from_io_args(package: &'static str, brand_name: &'static str, io_args: &IoArgs) -> Self {
        Self::new(package, brand_name)
            .with_log_path(io_args.log_path.as_ref())
            .with_max_log_verbosity(io_args.max_log_verbosity())
            .with_max_file_log_verbosity(io_args.max_file_log_verbosity())
            .with_otel_file_name(io_args.otel_file_name.as_deref())
            .with_otel_parquet_file_name(io_args.otel_parquet_file_name())
            .with_invocation_id(io_args.invocation_id)
            .with_parent_span_id(io_args.otel_parent_span_id)
            .with_export_to_otlp(io_args.export_to_otlp)
            .with_log_format(io_args.log_format)
            .with_file_log_format(io_args.log_format_file)
            .with_show_options(io_args.show.clone())
            .with_show_all_deprecations(io_args.show_all_deprecations)
            .with_log_file_max_bytes(io_args.log_file_max_bytes)
    }

    /// The user-facing CLI brand name shown in the version banner and JSON log lines.
    pub fn with_brand_name(mut self, brand_name: &'static str) -> Self {
        self.brand_name = brand_name;
        self
    }

    /// The command being executed, e.g. "run", "compile", "list".
    pub fn with_command(mut self, command: FsCommand) -> Self {
        self.command = command;
        self
    }

    /// The dbt project directory. If unset, it is auto-detected using
    /// `dbt_project.yml` as a marker, falling back to the current working directory.
    pub fn with_project_dir(mut self, project_dir: Option<&PathBuf>) -> Self {
        self.project_dir = project_dir.cloned();
        self
    }

    /// The target directory, used for the Parquet trace output. Defaults to
    /// `{project_dir}/target`.
    pub fn with_target_path(mut self, target_path: Option<&PathBuf>) -> Self {
        self.target_path = target_path.cloned();
        self
    }

    /// The directory for log-related files. Defaults to `{project_dir}/logs`, and
    /// is resolved relative to the project directory if relative.
    pub fn with_log_path(mut self, log_path: Option<&PathBuf>) -> Self {
        self.log_path = log_path.cloned();
        self
    }

    /// Maximum verbosity (inverse of log level) for the tui & jsonl log sinks.
    pub fn with_max_log_verbosity(mut self, max_log_verbosity: LevelFilter) -> Self {
        self.max_log_verbosity = max_log_verbosity;
        self
    }

    /// Maximum verbosity for the file log sink.
    pub fn with_max_file_log_verbosity(mut self, max_file_log_verbosity: LevelFilter) -> Self {
        self.max_file_log_verbosity = max_file_log_verbosity;
        self
    }

    /// File name of the JSONL trace output, written to `{log_path}`. If unset,
    /// the JSONL output layer is disabled.
    pub fn with_otel_file_name(mut self, otel_file_name: Option<&str>) -> Self {
        self.otel_file_name = otel_file_name.map(str::to_string);
        self
    }

    /// File name of the Parquet trace output, written to `{target_path}/private/metadata`.
    /// If unset, the Parquet output layer is disabled.
    pub fn with_otel_parquet_file_name(mut self, otel_parquet_file_name: Option<&str>) -> Self {
        self.otel_parquet_file_name = otel_parquet_file_name.map(str::to_string);
        self
    }

    /// Custom name for the log file written to `{log_path}`. Defaults to `dbt.log`.
    pub fn with_log_file_name(mut self, log_file_name: Option<&str>) -> Self {
        self.log_file_name = log_file_name.map(str::to_string);
        self
    }

    /// Max size in bytes for rotating file logs. `0` means no limit.
    pub fn with_log_file_max_bytes(mut self, log_file_max_bytes: u64) -> Self {
        self.log_file_max_bytes = log_file_max_bytes;
        self
    }

    /// Invocation ID, used as trace ID for correlation.
    pub fn with_invocation_id(mut self, invocation_id: uuid::Uuid) -> Self {
        self.invocation_id = invocation_id;
        self
    }

    /// Parent span ID for OpenTelemetry trace correlation, used as fallback when
    /// creating root spans.
    pub fn with_parent_span_id(mut self, parent_span_id: Option<u64>) -> Self {
        self.parent_span_id = parent_span_id;
        self
    }

    /// If true, traces are forwarded to the OTLP endpoints set via OTEL
    /// environment variables.
    pub fn with_export_to_otlp(mut self, export_to_otlp: bool) -> Self {
        self.export_to_otlp = export_to_otlp;
        self
    }

    /// The log format to use.
    pub fn with_log_format(mut self, log_format: LogFormat) -> Self {
        self.log_format = log_format;
        self
    }

    /// The file-specific log format (`--log-format-file`), overriding
    /// [`Self::with_log_format`] for the on-disk log sink only. When `None`, the
    /// on-disk sink falls back to `log_format`.
    pub fn with_file_log_format(mut self, file_log_format: Option<LogFormat>) -> Self {
        self.file_log_format = file_log_format;
        self
    }

    /// If true, a separate query log file is written.
    pub fn with_query_log_enabled(mut self, enable_query_log: bool) -> Self {
        self.enable_query_log = enable_query_log;
        self
    }

    /// Show options controlling terminal/file output visibility.
    pub fn with_show_options(mut self, show_options: HashSet<ShowOptions>) -> Self {
        self.show_options = show_options;
        self
    }

    /// Whether non-result console output is suppressed.
    pub fn with_quiet(mut self, quiet: bool) -> Self {
        self.quiet = quiet;
        self
    }

    /// If true, show all deprecation warnings/errors instead of one per package.
    pub fn with_show_all_deprecations(mut self, show_all_deprecations: bool) -> Self {
        self.show_all_deprecations = show_all_deprecations;
        self
    }

    /// The initial warn-error options from CLI/env, before project flags are resolved.
    pub fn with_warn_error_options(mut self, warn_error_options: WarnErrorOptions) -> Self {
        self.warn_error_options = warn_error_options;
        self
    }

    /// Withholds upgrades of warnings with no dbt-core counterpart; set while replaying.
    pub fn with_skip_fusion_only_upgrades(mut self, skip_fusion_only_upgrades: bool) -> Self {
        self.skip_fusion_only_upgrades = skip_fusion_only_upgrades;
        self
    }

    /// Resolves all paths and returns the configuration. This never fails: it
    /// falls back to the current working directory if no project directory can
    /// be determined.
    pub fn build(self) -> FsTraceConfig {
        let (in_dir, out_dir) =
            calculate_trace_dirs(self.project_dir.as_ref(), self.target_path.as_ref());

        // Resolve log directory path (base directory for auxiliary log files)
        let log_dir_path = self.log_path.map_or_else(
            || in_dir.join(DBT_LOG_DIR_NAME),
            |log_path| {
                if log_path.is_relative() {
                    in_dir.join(log_path)
                } else {
                    log_path
                }
            },
        );

        FsTraceConfig {
            package: self.package,
            command: self.command,
            max_log_verbosity: self.max_log_verbosity,
            max_file_log_verbosity: self.max_file_log_verbosity,
            otel_file_path: self
                .otel_file_name
                .map(|file_name| log_dir_path.join(file_name)),
            otel_parquet_file_path: self
                .otel_parquet_file_name
                .map(|file_name| crate::constants::default_metadata_dir(out_dir).join(file_name)),
            log_path: log_dir_path,
            log_file_name: self.log_file_name,
            log_file_max_bytes: self.log_file_max_bytes,
            invocation_id: self.invocation_id,
            parent_span_id: self.parent_span_id,
            export_to_otlp: self.export_to_otlp,
            log_format: self.log_format,
            file_log_format: self.file_log_format,
            enable_query_log: self.enable_query_log,
            show_options: self.show_options,
            quiet: self.quiet,
            show_all_deprecations: self.show_all_deprecations,
            warn_error_options: self.warn_error_options,
            skip_fusion_only_upgrades: self.skip_fusion_only_upgrades,
            brand_name: self.brand_name,
        }
    }
}

/// Helper function to calculate in_dir and out_dir for tracing configuration.
/// This implements the same logic as `execute_setup_and_all_phases` but without canonicalization.
/// Unlike the project setup logic, this function never fails - it falls back to using the current
/// working directory if no project directory can be determined.
fn calculate_trace_dirs(
    project_dir: Option<&PathBuf>,
    target_path: Option<&PathBuf>,
) -> (PathBuf, PathBuf) {
    let in_dir = project_dir.cloned().unwrap_or_else(|| {
        // If no project directory is provided, try to determine it
        // Fallback to empty path if not found
        determine_project_dir(&[], DBT_PROJECT_YML).unwrap_or_else(|_| PathBuf::new())
    });

    // If no target path is provided, determine the output directory
    let out_dir = target_path
        .cloned()
        .unwrap_or_else(|| in_dir.join(DBT_TARGET_DIR_NAME));

    (in_dir, out_dir)
}

fn dbt_log_preprocessor_hook(record: &LogRecordInfo) -> Cow<'_, LogRecordInfo> {
    if get_log_message(&record.attributes).is_none() {
        return Cow::Borrowed(record);
    }

    match console::strip_ansi_codes(record.body.as_str()) {
        Cow::Owned(stripped) => Cow::Owned(LogRecordInfo {
            body: stripped,
            ..record.clone()
        }),
        Cow::Borrowed(_) => Cow::Borrowed(record),
    }
}

/// Builds the middleware pipeline shared by dbt tracing configurations.
pub fn build_shared_middleware_layers(
    show_all_deprecations: bool,
    config_provider: Arc<dyn TracingConfigProvider>,
    skip_fusion_only_upgrades: bool,
) -> Vec<MiddlewareLayer> {
    let warn_error_options_middleware =
        TelemetryWarnErrorOptionsMiddleware::new(config_provider, skip_fusion_only_upgrades);

    vec![
        Box::new(TelemetryMarkdownLogFilter),
        Box::new(TelemetryParsingErrorFilter::new(show_all_deprecations)),
        Box::new(warn_error_options_middleware),
        Box::new(TelemetryNodeWarnOutcome),
        Box::new(TelemetryMetricAggregator),
    ]
}

/// Builds a JSONL file consumer with dbt log preprocessing.
pub fn build_jsonl_file_consumer(
    file_path: &std::path::Path,
    max_verbosity: LevelFilter,
) -> FsResult<(ConsumerLayer, TelemetryShutdownItem)> {
    if let Some(log_dir) = file_path.parent() {
        crate::stdfs::create_dir_all(log_dir)?;
    }

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file_path)
        .map_err(|error| {
            fs_err!(
                ErrorCode::IoError,
                "Failed to open telemetry jsonl file for append: {}",
                error
            )
        })?;

    Ok(build_jsonl_layer_with_background_writer(
        file,
        max_verbosity,
        Some(dbt_log_preprocessor_hook),
    ))
}

/// Builds an unstructured rotating file-log consumer.
#[allow(clippy::too_many_arguments)]
pub fn build_file_log_consumer(
    max_file_log_verbosity: LevelFilter,
    log_path: &std::path::Path,
    log_file_name: Option<&str>,
    log_file_max_bytes: u64,
    log_format: LogFormat,
    invocation_id: uuid::Uuid,
    command: FsCommand,
    command_name: &'static str,
) -> FsResult<(
    Option<ConsumerLayer>,
    Vec<TelemetryShutdownItem>,
    Option<PathBuf>,
)> {
    if max_file_log_verbosity == LevelFilter::OFF {
        return Ok((None, Vec::new(), None));
    }

    crate::stdfs::create_dir_all(log_path)?;
    let log_file_name = log_file_name.unwrap_or(DBT_DEFAULT_LOG_FILE_NAME);
    let file_log_path = log_path.join(log_file_name);
    let file = RotatingFileWriter::new(
        &file_log_path,
        log_file_max_bytes,
        DBT_DEFAULT_LOG_FILE_BACKUP_COUNT,
    )
    .map_err(|error| {
        fs_err!(
            ErrorCode::IoError,
            "Failed to open log file for append: {}",
            error
        )
    })?;

    let Some((consumer, shutdown_item)) = (match log_format {
        LogFormat::Default | LogFormat::Text => Some(build_file_log_layer_with_background_writer(
            file,
            max_file_log_verbosity,
        )),
        LogFormat::Json => Some(build_json_compat_layer_with_background_writer(
            file,
            max_file_log_verbosity,
            invocation_id,
            command,
            command_name,
        )),
        LogFormat::Otel => None,
    }) else {
        return Ok((None, Vec::new(), Some(file_log_path)));
    };

    Ok((Some(consumer), vec![shutdown_item], Some(file_log_path)))
}

pub struct FsTraceLayers {
    middleware_layers: Vec<MiddlewareLayer>,
    consumer_layers: Vec<ConsumerLayer>,
    shutdown_items: Vec<TelemetryShutdownItem>,
}

impl FsTraceLayers {
    pub fn into_parts(
        self,
    ) -> (
        Vec<MiddlewareLayer>,
        Vec<ConsumerLayer>,
        Vec<TelemetryShutdownItem>,
    ) {
        (
            self.middleware_layers,
            self.consumer_layers,
            self.shutdown_items,
        )
    }
}

impl FsTraceConfig {
    pub fn create_config_provider(&self) -> Arc<dyn TracingConfigProvider> {
        Arc::new(FsTracingConfigProvider::from_warn_error_options(
            self.warn_error_options.clone(),
        ))
    }

    fn tui_config(&self) -> TuiConfig {
        TuiConfig {
            show_options: self.show_options.clone(),
            command: self.command,
            quiet: self.quiet,
        }
    }

    /// Initializes tracing with the consumers configured for this CLI invocation.
    pub fn init(
        self,
        config_provider: Arc<dyn TracingConfigProvider>,
    ) -> FsResult<dbt_tracing::init::TelemetryHandle> {
        let package = self.package;
        let fallback_trace_id = self.invocation_id.as_u128();
        let fallback_parent_span_id = self.parent_span_id;
        let max_log_verbosity = std::cmp::max(self.max_log_verbosity, self.max_file_log_verbosity);
        let (middlewares, consumer_layers, shutdown_items) =
            self.build_layers(config_provider)?.into_parts();

        let telemetry_handle = init_tracing_with_layers(
            package,
            fallback_trace_id,
            fallback_parent_span_id,
            max_log_verbosity,
            middlewares,
            consumer_layers,
            shutdown_items,
        )?;
        Ok(telemetry_handle)
    }

    /// Builds the configured tracing layers and corresponding shutdown items.
    /// This method handles all path creation and file opening as needed.
    /// If no layers are configured, returns an empty layer and no shutdown items.
    pub fn build_layers(
        &self,
        config_provider: Arc<dyn TracingConfigProvider>,
    ) -> FsResult<FsTraceLayers> {
        let mut shutdown_items = Vec::new();
        let mut consumer_layers = Vec::new();
        let middleware_layers = build_shared_middleware_layers(
            self.show_all_deprecations,
            Arc::clone(&config_provider),
            self.skip_fusion_only_upgrades,
        );

        // Create jsonl writer layer if file path provided
        if let Some(file_path) = &self.otel_file_path {
            let (layer, handle) =
                build_jsonl_file_consumer(file_path, self.max_file_log_verbosity)?;

            // Keep a handle for shutdown
            shutdown_items.push(handle);

            // Create layer and apply user specified filtering
            consumer_layers.push(layer)
        };

        // Create parquet writer layer if file path provided
        if let Some(file_path) = &self.otel_parquet_file_path {
            // Create the file and initialize the Parquet layer
            let file_dir = file_path.parent().ok_or_else(|| {
                fs_err!(
                    ErrorCode::IoError,
                    "Failed to get parent directory for file path"
                )
            })?;

            crate::stdfs::create_dir_all(file_dir)?;

            let file = std::fs::File::create(file_path)
                .map_err(|e| fs_err!(ErrorCode::IoError, "Failed to create parquet file: {}", e))?;

            let (parquet_layer, writer_handle) =
                build_parquet_writer_layer::<_, TelemetryEventTypeRegistry>(file)
                    .map_err(FsError::from)?;

            // Keep a handle for shutdown
            shutdown_items.push(writer_handle);

            // Create layer. User specified filtering is not applied here
            consumer_layers.push(parquet_layer)
        };

        // Create console layer based on log format.
        match self.log_format {
            LogFormat::Default | LogFormat::Text => {
                // Create layer and apply user specified filtering
                consumer_layers.push(build_tui_layer(
                    self.max_log_verbosity,
                    self.log_format,
                    self.tui_config(),
                ))
            }
            LogFormat::Json => {
                // Create layer and apply user specified filtering
                consumer_layers.push(build_json_compat_layer(
                    std::io::stdout(),
                    self.max_log_verbosity,
                    self.invocation_id,
                    self.command,
                    self.brand_name,
                ))
            }
            LogFormat::Otel => {
                // Create jsonl writer layer on stdout if log format is OTEL
                // No shutdown logic as we flushing to stdout as we write anyway
                consumer_layers.push(build_jsonl_layer(
                    std::io::stdout(),
                    self.max_log_verbosity,
                    Some(dbt_log_preprocessor_hook),
                ));
            }
        };

        // If any of the file logs are enabled - create the log directory
        if self.enable_query_log || self.max_file_log_verbosity != LevelFilter::OFF {
            // Ensure log directory exists
            crate::stdfs::create_dir_all(&self.log_path)?;
        }

        // File sink honours `--log-format-file` when set, otherwise uses `log_format`
        let (file_log_layer, mut file_log_shutdown_items, file_log_path) = build_file_log_consumer(
            self.max_file_log_verbosity,
            &self.log_path,
            self.log_file_name.as_deref(),
            self.log_file_max_bytes,
            self.file_log_format.unwrap_or(self.log_format),
            self.invocation_id,
            self.command,
            self.brand_name,
        )?;
        if let Some(file_log_layer) = file_log_layer {
            consumer_layers.push(file_log_layer);
        }
        shutdown_items.append(&mut file_log_shutdown_items);

        config_provider.set_file_log_path(file_log_path);

        // Create query log writer layer (always enabled; internal-only event sink)
        if self.enable_query_log {
            let file_path = self.log_path.join(DBT_DEFAULT_QUERY_LOG_FILE_NAME);
            // Keep query_log.sql scoped to the current invocation.
            let file = crate::stdfs::File::create(&file_path)
                .map_err(|e| fs_err!(ErrorCode::IoError, "Failed to open query log file: {}", e))?;

            let (layer, handle) = build_query_log_layer_with_background_writer(file);
            shutdown_items.push(handle);
            consumer_layers.push(layer)
        };

        // Create OTLP layer - if enabled and endpoint is set via env vars
        if self.export_to_otlp
            && let Some((otlp_layer, mut handles)) = build_otlp_layer(
                OtlpResourceConfig::new(self.brand_name, env!("CARGO_PKG_VERSION")),
                Some(dbt_log_preprocessor_hook),
            )
        {
            shutdown_items.append(&mut handles);
            consumer_layers.push(otlp_layer)
        };

        Ok(FsTraceLayers {
            middleware_layers,
            consumer_layers,
            shutdown_items,
        })
    }
}
