// Option names from the driver's header at v0.1.0-alpha.1.
// https://github.com/adbc-drivers/motherduck/blob/8a35a7277de72f2a930f6b68ea4b300ee32e78a6/src/motherduck/motherduck_database.h#L30-L47

/// Attach one database (`single`) or every database in the account (`workspace`).
pub const ATTACH_MODE: &str = "motherduck.attach_mode";

/// MotherDuck session name, passed through as `--session_name`.
pub const SESSION_NAME: &str = "motherduck.session_name";

/// MotherDuck region.
///
/// Unused at present: `dbt-auth` resolves the host from the region itself and
/// passes the driver a finished URI, so this option is never sent.
pub const REGION: &str = "motherduck.region";

/// The longest time to wait while connecting, in whole seconds.
pub const CONNECT_TIMEOUT: &str = "motherduck.connect_timeout";

/// A PEM file of root certificates to verify the server with, instead of the
/// OS trust store the driver uses by default.
pub const TLS_ROOT_CERT_PATH: &str = "motherduck.tls_root_cert_path";
