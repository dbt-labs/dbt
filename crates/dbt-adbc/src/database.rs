//! ADBC Database
//!
//!

use crate::driver_manager::ManagedDatabase as ManagedAdbcDatabase;
use adbc_core::{
    Database as _, Optionable,
    error::{Error, Result, Status},
    options::{AdbcVersion, InfoCode, OptionConnection, OptionDatabase, OptionValue},
};
use arrow_array::{
    Array,
    cast::AsArray,
    types::{Int64Type, UInt32Type},
};
use serde::Deserialize;
use siphasher::sip128::{Hasher128, SipHasher24};
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use std::{collections::HashSet, ffi::c_int};
use std::{
    hash::{Hash, Hasher},
    ops::Deref,
};
use tracy_client::span;

use crate::{Backend, Connection, connection::AdbcConnection, snowflake, str_from_sqlstate};

mod builder;
pub use builder::*;

/// ADBC Database.
///
/// dyn-compatible trait covering functionality from the adbc_core::{Database, Optionable} traits.
pub trait Database: Send + Sync + DatabaseInfo {
    // adbc_core::Database<Box<dyn Connection>> functions -----------------------
    fn new_connection(&self) -> Result<Box<dyn Connection>>;
    fn new_connection_with_opts(
        &self,
        opts: Vec<(OptionConnection, OptionValue)>,
    ) -> Result<Box<dyn Connection>>;

    // adbc_core::Optionable<Option = OptionDatabase> functions -----------------
    fn set_option(&mut self, key: OptionDatabase, value: OptionValue) -> Result<()>;
    fn get_option_string(&self, key: OptionDatabase) -> Result<String>;
    fn get_option_bytes(&self, key: OptionDatabase) -> Result<Vec<u8>>;
    fn get_option_int(&self, key: OptionDatabase) -> Result<i64>;
    fn get_option_double(&self, key: OptionDatabase) -> Result<f64>;

    /// Returns the [`AdbcVersion`] reported by the driver.
    fn adbc_version(&mut self) -> Result<AdbcVersion> {
        self.new_connection()?
            .get_info(Some(HashSet::from_iter([InfoCode::DriverAdbcVersion])))?
            .next()
            .ok_or_else(|| Error::with_message_and_status("failed to get info", Status::Internal))?
            .map_err(Into::into)
            .and_then(|record_batch| {
                assert_eq!(
                    record_batch.column(0).as_primitive::<UInt32Type>().value(0),
                    u32::from(&InfoCode::DriverAdbcVersion)
                );
                AdbcVersion::try_from(
                    record_batch
                        .column(1)
                        .as_union()
                        .value(0)
                        .as_primitive::<Int64Type>()
                        .value(0) as c_int,
                )
            })
    }

    fn clone_box(&self) -> Box<dyn Database>;
}

pub trait DatabaseInfo {
    fn get_info(&mut self, info_code: InfoCode) -> Result<Arc<dyn Array>>;

    /// Returns the name of the vendor.
    fn vendor_name(&mut self) -> Result<String> {
        self.get_info(InfoCode::VendorName)
            .map(|array| array.as_string::<i32>().value(0).to_owned())
    }

    /// Returns the version of the vendor.
    fn vendor_version(&mut self) -> Result<String> {
        self.get_info(InfoCode::VendorVersion)
            .map(|array| array.as_string::<i32>().value(0).to_owned())
    }

    /// Returns the Arrow version of the vendor.
    fn vendor_arrow_version(&mut self) -> Result<String> {
        self.get_info(InfoCode::VendorArrowVersion)
            .map(|array| array.as_string::<i32>().value(0).to_owned())
    }

    /// Returns true if SQL queries are supported.
    fn vendor_sql(&mut self) -> Result<bool> {
        self.get_info(InfoCode::VendorSql)
            .map(|array| array.as_boolean().value(0))
    }

    /// Returns true if Substrait queries are supported.
    fn vendor_substrait(&mut self) -> Result<bool> {
        self.get_info(InfoCode::VendorSubstrait)
            .map(|array| array.as_boolean().value(0))
    }

    /// Returns the name of the wrapped driver.
    fn driver_name(&mut self) -> Result<String> {
        self.get_info(InfoCode::DriverName)
            .map(|array| array.as_string::<i32>().value(0).to_owned())
    }

    /// Returns the version of the wrapped driver.
    fn driver_version(&mut self) -> Result<String> {
        self.get_info(InfoCode::DriverVersion)
            .map(|array| array.as_string::<i32>().value(0).to_owned())
    }

    /// Returns the Arrow version of the wrapped driver.
    fn driver_arrow_version(&mut self) -> Result<String> {
        self.get_info(InfoCode::DriverArrowVersion)
            .map(|array| array.as_string::<i32>().value(0).to_owned())
    }
}

impl Clone for Box<dyn Database> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[derive(Deserialize, Debug)]
struct RefreshResponse {
    access_token: String,
}

const REFRESH_TOKEN_REQ_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
struct TokenRefresher {
    http_agent: ureq::Agent,
    token_request_url: String,
    encoded_creds: String,
    refresh_token: String,
}

impl TokenRefresher {
    pub fn new(
        client_id: String,
        client_secret: String,
        account: String,
        refresh_token: String,
    ) -> Self {
        use base64::Engine as _;
        use base64::engine::general_purpose;
        const BASE64_ENGINE: &general_purpose::GeneralPurpose = &general_purpose::STANDARD;

        let http_config = ureq::Agent::config_builder()
            .timeout_global(Some(REFRESH_TOKEN_REQ_TIMEOUT))
            .build();
        let http_agent = ureq::Agent::new_with_config(http_config);

        let token_request_url = format!(
            "https://{}.snowflakecomputing.com/oauth/token-request",
            &account
        );

        let encoded_creds = BASE64_ENGINE.encode(format!("{client_id}:{client_secret}"));

        TokenRefresher {
            http_agent,
            token_request_url,
            encoded_creds,
            refresh_token,
        }
    }

    pub fn for_database(backend: Backend, database: &ManagedAdbcDatabase) -> Result<Option<Self>> {
        if backend != Backend::Snowflake {
            return Ok(None);
        }
        let client_id =
            database.get_option_string(OptionDatabase::Other(snowflake::CLIENT_ID.to_string()))?;
        let client_secret = database
            .get_option_string(OptionDatabase::Other(snowflake::CLIENT_SECRET.to_string()))?;
        let account =
            database.get_option_string(OptionDatabase::Other(snowflake::ACCOUNT.to_string()))?;
        let refresh_token = database
            .get_option_string(OptionDatabase::Other(snowflake::REFRESH_TOKEN.to_string()))?;

        // Not an auth configuration that needs refreshing
        if client_id.is_empty()
            || client_secret.is_empty()
            || account.is_empty()
            || refresh_token.is_empty()
        {
            return Ok(None);
        }

        let refresher = Self::new(client_id, client_secret, account, refresh_token);
        Ok(Some(refresher))
    }

    fn refreshed_auth_token(&self) -> Result<String> {
        let _span = span!("refreshed_auth_token");
        use http::header::AUTHORIZATION;
        let result = self
            .http_agent
            .post(&self.token_request_url)
            .header(AUTHORIZATION, format!("Basic {}", self.encoded_creds))
            .send_form([
                ("grant_type", "refresh_token"),
                ("refresh_token", self.refresh_token.as_str()),
            ]);

        let resp = result
            .map_err(|e| {
                Error::with_message_and_status(
                    format!(
                        "Failed to make request to {}: {}",
                        self.token_request_url, e
                    ),
                    Status::IO,
                )
            })?
            .into_body()
            .read_json::<RefreshResponse>()
            .map_err(|e| {
                Error::with_message_and_status(
                    format!("Failed parse payload of auth token request: {e}"),
                    Status::InvalidData,
                )
            })?;

        Ok(resp.access_token)
    }
}

struct InnerAdbcDatabase {
    pub(crate) backend: Backend,
    /// Readers-writer lock to protect the database state.
    ///
    /// Unlike [std::sync::RwLock], this lock is fair to writers and won't block them
    /// indefinitely if there is a steady flow of readers acquiring the lock.
    pub(crate) managed_database: parking_lot::RwLock<ManagedAdbcDatabase>,
    token_refresher: Option<TokenRefresher>,
}

impl InnerAdbcDatabase {
    fn new_with_refresher(
        backend: Backend,
        managed_database: ManagedAdbcDatabase,
        token_refresher: Option<TokenRefresher>,
    ) -> Self {
        InnerAdbcDatabase {
            backend,
            managed_database: parking_lot::RwLock::new(managed_database),
            token_refresher,
        }
    }

    fn new_connection_with_opts_impl(
        &self,
        conn_opts: Vec<(OptionConnection, OptionValue)>,
    ) -> Result<Box<dyn Connection>> {
        let conn_opts_for_retry = conn_opts.clone();
        self.try_new_connection_with_opts_once(conn_opts)
            .or_else(|e| {
                // Snowflake might return a connection error when the cached ID token is invalid.
                //
                //     390195 (08004): The provided ID Token is invalid.
                //
                // Trying again consistently is enough to make the issue go away.
                if self.backend == Backend::Snowflake
                    && e.vendor_code == 390195
                    && str_from_sqlstate(&e.sqlstate) == "08004"
                {
                    self.try_new_connection_with_opts_once(conn_opts_for_retry)
                } else {
                    Err(e)
                }
            })
    }

    /// Called before a connection is being opened with new_connection() or
    /// new_connection_with_opts().
    ///
    /// Based on the warehouse backend and auth options, this function may:
    /// 1) add database options to be overriden right before a new connection is created
    /// 2) add connection-specific options to be used by the new connection
    ///
    /// NOTE: this function must not try to acquire any locks on the database.
    /// See `new_connection_with_opts()` to see how it's used.
    fn will_open_connection(
        &self,
        db_opts: &mut Vec<(OptionDatabase, OptionValue)>,
        _conn_opts: &mut Vec<(OptionConnection, OptionValue)>,
    ) -> Result<()> {
        self.token_refresher
            .as_ref()
            .map(|refresher| {
                let auth_token = refresher.refreshed_auth_token()?;
                if self.backend == Backend::Snowflake {
                    let pair = (
                        OptionDatabase::Other(snowflake::AUTH_TOKEN.to_string()),
                        OptionValue::String(auth_token),
                    );
                    db_opts.push(pair);
                }
                Ok(())
            })
            .unwrap_or(Ok(()))
    }

    fn try_new_connection_with_opts_once(
        &self,
        mut conn_opts: Vec<(OptionConnection, OptionValue)>,
    ) -> Result<Box<dyn Connection>> {
        let _span = span!("try_new_connection_with_opts_once");
        let mut db_opts = Vec::<(OptionDatabase, OptionValue)>::new();

        // Start the connection creation process with a READ lock to prevent any other thread to
        // set_option() on the database while we're in the process of creating a connection.
        let managed_database = {
            let _span = span!("RwLock::read");
            self.managed_database.read()
        };
        let () = self.will_open_connection(&mut db_opts, &mut conn_opts)?; // MUST not lock!

        let new_conn = |managed_database: &ManagedAdbcDatabase| -> Result<AdbcConnection> {
            #[cfg(feature = "adbc-fuzzying")]
            {
                use rand::Rng as _;
                let mut rng = rand::rng();
                let x: f32 = rng.random();
                if x < 0.25 {
                    let e = Error::with_message_and_status(
                        "simulated connection error",
                        Status::Internal,
                    );
                    return Err(e);
                }
            }
            let conn_res = if conn_opts.is_empty() {
                managed_database.new_connection()
            } else {
                managed_database.new_connection_with_opts(conn_opts)
            };
            Ok(AdbcConnection(self.backend, conn_res?, 0))
        };

        let conn = if db_opts.is_empty() {
            new_conn(managed_database.deref())
        } else {
            drop(managed_database); // drop the read lock before acquiring a write lock
            let mut managed_database_write = {
                let _span = span!("RwLock::write");
                self.managed_database.write()
            };
            for (key, value) in db_opts.into_iter() {
                managed_database_write.set_option(key, value)?;
            }
            new_conn(managed_database_write.deref())
        }?;
        Ok(Box::new(conn))
    }
}

/// ADBC Database.
///
/// Databases hold state shared by multiple connections. Generally, this means common
/// configuration and caches.
pub(crate) struct AdbcDatabase {
    inner: Arc<InnerAdbcDatabase>,
}

impl AdbcDatabase {
    pub fn new(backend: Backend, managed_database: ManagedAdbcDatabase) -> Self {
        let token_refresher = TokenRefresher::for_database(backend, &managed_database)
            .ok()
            .flatten();
        let inner =
            InnerAdbcDatabase::new_with_refresher(backend, managed_database, token_refresher);
        Self {
            inner: Arc::new(inner),
        }
    }
}

impl DatabaseInfo for AdbcDatabase {
    fn get_info(&mut self, info_code: InfoCode) -> Result<Arc<dyn Array>> {
        let conn = self.new_connection()?;
        let _rlock = self.inner.managed_database.read();
        let mut record_batch_reader = conn.get_info(Some(HashSet::from_iter([info_code])))?;
        record_batch_reader
            .next()
            .ok_or_else(|| Error::with_message_and_status("failed to get info", Status::Internal))?
            .map_err(Into::into)
            .and_then(|record_batch| {
                if InfoCode::try_from(record_batch.column(0).as_primitive::<UInt32Type>().value(0))?
                    == info_code
                {
                    Ok(record_batch.column(1).as_union().value(0))
                } else {
                    Err(Error::with_message_and_status(
                        "invalid get info reply",
                        Status::Internal,
                    ))
                }
            })
    }
}

impl Database for AdbcDatabase {
    fn new_connection(&self) -> Result<Box<dyn Connection>> {
        let opts = Vec::new();
        self.new_connection_with_opts(opts)
    }

    fn new_connection_with_opts(
        &self,
        conn_opts: Vec<(OptionConnection, OptionValue)>,
    ) -> Result<Box<dyn Connection>> {
        self.inner.new_connection_with_opts_impl(conn_opts)
    }

    fn set_option(&mut self, key: OptionDatabase, value: OptionValue) -> Result<()> {
        let mut managed_database = self.inner.managed_database.write();
        managed_database.set_option(key, value)
    }

    fn get_option_string(&self, key: OptionDatabase) -> Result<String> {
        let managed_database = self.inner.managed_database.read();
        managed_database.get_option_string(key)
    }

    fn get_option_bytes(&self, key: OptionDatabase) -> Result<Vec<u8>> {
        let managed_database = self.inner.managed_database.read();
        managed_database.get_option_bytes(key)
    }

    fn get_option_int(&self, key: OptionDatabase) -> Result<i64> {
        let managed_database = self.inner.managed_database.read();
        managed_database.get_option_int(key)
    }

    fn get_option_double(&self, key: OptionDatabase) -> Result<f64> {
        let managed_database = self.inner.managed_database.read();
        managed_database.get_option_double(key)
    }

    fn clone_box(&self) -> Box<dyn Database> {
        let adbc_database = AdbcDatabase {
            inner: self.inner.clone(),
        };
        Box::new(adbc_database)
    }
}

#[derive(Debug, Copy, Clone, Ord, Eq, PartialOrd, PartialEq)]
pub struct Fingerprint {
    h1: u64,
    h2: u64,
}

impl Fingerprint {
    /// A 64-bit projection of the fingerprint, suitable as a connection-pool
    /// reuse key: two connection configurations with the same fingerprint share
    /// this value.
    pub fn as_u64(&self) -> u64 {
        self.h1
    }
}

impl Hash for Fingerprint {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // the fingerprint itself is already a hash, so we can use a truncated
        // version of it to produce a 64-bit hash of the fingerprint
        state.write_u64(self.h1)
    }
}

/// The key used to fingerprint the database configuration.
static HASHER_KEY: LazyLock<[u8; 16]> = LazyLock::new(|| {
    let mut key = [0u8; 16];
    getrandom::getrandom(&mut key).unwrap();
    key
});

#[inline]
pub(crate) fn fingerprint_config<'a>(
    opts: impl Iterator<Item = &'a (OptionDatabase, OptionValue)>,
) -> Fingerprint {
    let mut hasher = SipHasher24::new_with_key(&HASHER_KEY);
    for (name, value) in opts {
        match name {
            OptionDatabase::Uri => hasher.write_u64(1),
            OptionDatabase::Username => hasher.write_u64(2),
            OptionDatabase::Password => hasher.write_u64(3),
            OptionDatabase::Other(name) => {
                hasher.write_u64(4);
                hasher.write_u64(name.len() as u64);
                hasher.write(name.as_bytes());
            }
            _ => {
                let bytes = name.as_ref().as_bytes();
                hasher.write_u64(5);
                hasher.write_u64(bytes.len() as u64);
                hasher.write(bytes);
            }
        }

        match value {
            OptionValue::String(s) => {
                hasher.write_u64(1);
                hasher.write_u64(s.len() as u64);
                hasher.write(s.as_bytes());
            }
            OptionValue::Bytes(b) => {
                hasher.write_u64(2);
                hasher.write_u64(b.len() as u64);
                hasher.write(b);
            }
            OptionValue::Int(i) => {
                hasher.write_u64(3);
                hasher.write_u64(*i as u64);
            }
            OptionValue::Double(d) => {
                hasher.write_u64(4);
                hasher.write_u64(d.to_bits());
            }
            _ => panic!("unexpected OptionValue variant"),
        }
    }
    let hash = hasher.finish128();
    Fingerprint {
        h1: hash.h1,
        h2: hash.h2,
    }
}

#[cfg(test)]
mod tests {
    use crate::database::{Fingerprint, fingerprint_config};
    use crate::{Backend, database};
    use std::collections::HashSet;

    #[test]
    fn config_fingerprinting() {
        const BACKEND: Backend = Backend::Snowflake;
        let config1 = {
            let mut builder = database::Builder::new(BACKEND);
            builder
                .with_username("user")
                .with_password("password")
                .with_parse_uri("https://snowflakecomputing.com")
                .unwrap()
                .with_named_option("option", "value")
                .unwrap();
            builder.into_iter().collect::<Vec<_>>()
        };
        let config2 = {
            let mut builder = database::Builder::new(BACKEND);
            builder
                .with_username("user")
                .with_password("password")
                .with_parse_uri("https://snowflakecomputing.com")
                .unwrap()
                .with_named_option("option=value", "")
                .unwrap();
            builder.into_iter().collect::<Vec<_>>()
        };
        let config3 = {
            let mut builder = database::Builder::new(BACKEND);
            builder
                .with_username("user")
                .with_password("password")
                .with_parse_uri("https://snowflakecomputing.com")
                .unwrap();
            builder.into_iter().collect::<Vec<_>>()
        };
        let fingerprint1 = fingerprint_config(config1.iter());
        let fingerprint2 = fingerprint_config(config2.iter());
        let fingerprint3 = fingerprint_config(config3.iter());
        let fingerprint_set: HashSet<Fingerprint> =
            HashSet::from_iter([fingerprint1, fingerprint2, fingerprint3]);
        assert_eq!(fingerprint_set.len(), 3);
    }
}
