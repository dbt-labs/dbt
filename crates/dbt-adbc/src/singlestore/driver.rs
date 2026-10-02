use std::sync::Arc;

use super::sqlx::mysql::MySqlConnectOptions;
use adbc_core::error::{Error, Result, Status};
use adbc_core::options::{OptionDatabase, OptionValue};

use super::database::SingleStoreDatabase;
use crate::database::Database;
use crate::driver::Driver;
use crate::semaphore::Semaphore;

pub struct SingleStoreDriver {
    pub(crate) semaphore: Option<Arc<Semaphore>>,
}

impl SingleStoreDriver {
    pub fn new(semaphore: Option<Arc<Semaphore>>) -> Self {
        Self { semaphore }
    }
}

impl Driver for SingleStoreDriver {
    fn new_database(&mut self) -> Result<Box<dyn Database>> {
        self.new_database_with_opts(vec![])
    }

    fn new_database_with_opts(
        &mut self,
        opts: Vec<(OptionDatabase, OptionValue)>,
    ) -> Result<Box<dyn Database>> {
        let mut uri_str = None;
        let mut host = None;
        let mut port = None;
        let mut user = None;
        let mut password = None;
        let mut database = None;

        for (k, v) in opts {
            match k {
                OptionDatabase::Uri => {
                    if let OptionValue::String(u) = v {
                        uri_str = Some(u);
                    }
                }
                OptionDatabase::Other(key) => match key.to_ascii_lowercase().as_str() {
                    "host" => {
                        if let OptionValue::String(s) = v {
                            host = Some(s);
                        }
                    }
                    "port" => match v {
                        OptionValue::Int(p) => port = Some(p as u16),
                        OptionValue::String(s) => port = s.parse::<u16>().ok(),
                        _ => {}
                    },
                    "user" | "username" => {
                        if let OptionValue::String(s) = v {
                            user = Some(s);
                        }
                    }
                    "password" => {
                        if let OptionValue::String(s) = v {
                            password = Some(s);
                        }
                    }
                    "database" | "dbname" => {
                        if let OptionValue::String(s) = v {
                            database = Some(s);
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        let connect_options = if let Some(uri) = uri_str {
            uri.parse::<MySqlConnectOptions>().map_err(|e| {
                Error::with_message_and_status(
                    format!("Failed to parse SingleStore connection URI '{uri}': {e}"),
                    Status::InvalidArguments,
                )
            })?
        } else {
            let mut opts = MySqlConnectOptions::new();
            if let Some(h) = host {
                opts = opts.host(&h);
            }
            if let Some(p) = port {
                opts = opts.port(p);
            }
            if let Some(u) = user {
                opts = opts.username(&u);
            }
            if let Some(pwd) = password {
                opts = opts.password(&pwd);
            }
            if let Some(db) = database {
                opts = opts.database(&db);
            }
            opts
        };

        Ok(Box::new(SingleStoreDatabase::new(
            connect_options,
            self.semaphore.clone(),
        )))
    }
}
