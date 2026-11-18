// Option key constants for the Trino ADBC driver (`adbc_driver_trino`).
//
// Like the Athena driver, the Trino driver uses a plain `trino.*` prefix.

/// ADBC option key for the user to run queries as. The driver sends it as the `X-Trino-User`
/// header, overriding the authenticated user (Trino checks the impersonation rules).
pub const IMPERSONATION_USER: &str = "trino.impersonation_user";

/// ADBC option key prefix for extra HTTP headers. Valid on both database and statement options:
/// on the database it applies to every request, on a statement only to that statement.
/// The header name is appended, e.g. `trino.http_header.X-Foo`.
pub const HTTP_HEADER_PREFIX: &str = "trino.http_header.";
