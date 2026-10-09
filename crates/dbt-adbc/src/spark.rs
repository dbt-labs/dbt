//! ADBC connection options for Spark

pub const HOST: &str = "spark.host";
pub const PORT: &str = "spark.port";

pub const TRANSPORT_API: &str = "spark.api";
pub mod transport_api {
    pub const THRIFT_BINARY: &str = "thrift+binary";
    pub const THRIFT_HTTP: &str = "thrift+http";
    pub const LIVY: &str = "livy";
    pub const CONNECT: &str = "connect";
}

pub const AUTH_TYPE: &str = "spark.auth_type";
pub mod auth_type {
    // Thrift
    pub const NOSASL: &str = "nosasl";
    pub const PLAIN: &str = "plain";
    pub const LDAP: &str = "ldap";
    pub const KERBEROS: &str = "kerberos";

    // Livy
    pub const BASIC: &str = "basic";
    pub const AWS_SIGV4: &str = "aws_sigv4";
    pub const AZURE_TOKEN: &str = "azure_token";

    // Spark Connect
    pub const NONE: &str = "none";
    pub const TOKEN: &str = "token";
}

pub const USERNAME: &str = "username";
pub const PASSWORD: &str = "password";

pub const SESSION_CONFIG_PREFIX: &str = "spark.opt.";

pub const KERBEROS_SERVICE_NAME: &str = "spark.kerberos.service_name";

pub mod livy {
    pub const SESSION_KIND: &str = "spark.livy.session_kind";
    pub mod session_kind {
        pub const SQL: &str = "sql";
        pub const SPARK: &str = "spark";
        pub const PYSPARK: &str = "pyspark";
    }

    pub const SESSION_TTL: &str = "spark.livy.session_ttl";

    /// Path appended to the host to form the Livy endpoint base URL
    /// (e.g. Microsoft Fabric's `/v1/workspaces/{id}/lakehouses/{id}/livyapi/versions/2023-12-01`).
    pub const BASE_URL: &str = "spark.livy.base_url";

    /// Microsoft Entra ID options for [`super::auth_type::AZURE_TOKEN`].
    /// Credential parameters ride in the standard username/password options,
    /// following the MSSQL driver's fedauth conventions: a service principal
    /// is `username = "<client id>@<tenant id>"` with the client secret as
    /// the password; a user-assigned managed identity's client ID is the
    /// username.
    pub mod azure {
        pub const CREDENTIAL: &str = "spark.livy.azure.credential";
        /// Credential kind names match the MSSQL driver's `fedauth` values.
        pub mod credential {
            pub const DEFAULT: &str = "ActiveDirectoryDefault";
            pub const AZ_CLI: &str = "ActiveDirectoryAzCli";
            pub const SERVICE_PRINCIPAL: &str = "ActiveDirectoryServicePrincipal";
            pub const ENVIRONMENT: &str = "ActiveDirectoryEnvironment";
            pub const MANAGED_IDENTITY: &str = "ActiveDirectoryManagedIdentity";
        }

        pub const TOKEN_SCOPE: &str = "spark.livy.azure.token_scope";
    }

    pub mod aws {
        pub const REGION: &str = "spark.livy.aws.region";
        pub const EMR_SERVERLESS_EXECUTION_ROLE_ARN: &str =
            "spark.livy.aws.emr_serverless.execution_role_arn";
    }
}
