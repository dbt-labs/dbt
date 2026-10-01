use dbt_clap_core::CommonArgs;

/// Configuration for the ANTLR parser.
pub trait AntlrParserConfig: Send + Sync {
    /// The ANTLR parser configuration is a set of globals initialized by this trait's
    /// implementation.
    fn apply_configuration(&self, common_args: &CommonArgs);
}

pub struct AntlrParserFeature {
    pub config: Box<dyn AntlrParserConfig>,
}

impl AntlrParserFeature {
    pub fn new(config: Box<dyn AntlrParserConfig>) -> Self {
        Self { config }
    }
}

impl Default for AntlrParserFeature {
    fn default() -> Self {
        Self {
            config: Box::new(NoOpAntlrParserConfig),
        }
    }
}

struct NoOpAntlrParserConfig;

impl AntlrParserConfig for NoOpAntlrParserConfig {
    fn apply_configuration(&self, _common_args: &CommonArgs) {
        // No-op
    }
}
