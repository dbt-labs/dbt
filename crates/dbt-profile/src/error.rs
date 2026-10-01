use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("{}", not_found_message(.searched, .explicit_profiles_dir))]
    NotFound {
        searched: Vec<PathBuf>,
        /// True when `--profiles-dir` was explicitly set, restricting the search.
        explicit_profiles_dir: bool,
    },

    #[error("no profile name specified and no dbt_project.yml found to infer it")]
    NoProfileName,

    #[error("Profile '{}' not found in profiles.yml", profile)]
    ProfileMissing { profile: String, path: PathBuf },

    #[error("no 'outputs' key found in profile '{profile}'")]
    NoOutputs { profile: String },

    #[error("target '{target}' not found in profile '{profile}'")]
    TargetMissing { profile: String, target: String },

    #[error("YAML parse error in {}: {source}", path.display())]
    Yaml {
        path: PathBuf,
        source: dbt_yaml::Error,
    },

    #[error("Jinja render error: {0}")]
    Jinja(#[from] minijinja::Error),

    #[error("missing 'type' field in resolved profile output")]
    NoAdapterType,

    /// Something that names a real adapter but is not the spelling authors are
    /// meant to write -- either a retired name (`alt`) or the internal
    /// `DbConfig` tag (`lakecompute`), which deserializes but is not an
    /// external name. Rejected rather than aliased so there is exactly one
    /// accepted spelling per adapter.
    #[error("adapter type '{written}' is not recognized; write '{expected}' instead")]
    UnacceptedAdapterType { written: String, expected: String },

    // ----------------------------------------------------------------------
    // Adapter-type-keyed targets: `outputs.<target>` as a map of adapter type
    // to a list of connections.
    // ----------------------------------------------------------------------
    #[error(
        "target '{target}' in profile '{profile}' must be either a mapping carrying a `type:` \
         (one connection) or a list of connections"
    )]
    TargetNotConnectionList { profile: String, target: String },

    #[error("target '{target}' in profile '{profile}' declares an empty list of connections")]
    EmptyConnectionList { profile: String, target: String },

    #[error(
        "connection #{} in target '{target}' of profile '{profile}' must be a mapping",
        index + 1
    )]
    ConnectionNotMapping {
        profile: String,
        target: String,
        index: usize,
    },

    #[error(
        "connection #{} in target '{target}' of profile '{profile}' needs a `type:` naming its \
         adapter",
        index + 1
    )]
    ConnectionMissingType {
        profile: String,
        target: String,
        index: usize,
    },

    #[error(
        "connection #{} in target '{target}' of profile '{profile}' has a `name:` that is not a \
         non-empty string; omit it to leave the connection unnamed",
        index + 1
    )]
    ConnectionNameNotString {
        profile: String,
        target: String,
        index: usize,
    },

    #[error(
        "adapter '{adapter}' in target '{target}' of profile '{profile}' declares more than one \
         connection named '{connection}'"
    )]
    DuplicateConnectionName {
        profile: String,
        target: String,
        adapter: String,
        connection: String,
    },

    #[error(
        "connection '{connection}' of adapter '{adapter}' in target '{target}' of profile \
         '{profile}' has a non-boolean `default:`"
    )]
    ConnectionDefaultNotBool {
        profile: String,
        target: String,
        adapter: String,
        connection: String,
    },

    #[error(
        "target '{target}' in profile '{profile}' declares {} adapters ({}) but marks no \
         connection `default: true`; one must be, to say which adapter nodes use by default",
        adapters.len(),
        adapters.join(", ")
    )]
    NoDefaultConnection {
        profile: String,
        target: String,
        adapters: Vec<String>,
    },

    #[error(
        "target '{target}' in profile '{profile}' marks {} connections `default: true` ({}); \
         exactly one may be",
        connections.len(),
        connections.join(", ")
    )]
    MultipleDefaultConnections {
        profile: String,
        target: String,
        connections: Vec<String>,
    },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, ProfileError>;

fn not_found_message(searched: &[PathBuf], explicit_profiles_dir: &bool) -> String {
    if searched.len() == 1 {
        let path = &searched[0];
        let mut msg = format!("No profiles.yml found at `{}`.", path.display());
        if *explicit_profiles_dir {
            msg.push_str(
                "\nTry running without the --profiles-dir flag to check the default locations.",
            );
        }
        msg
    } else {
        format!(
            "no profiles.yml found (searched: {:?}). Run `dbt init` to create one",
            searched
        )
    }
}
