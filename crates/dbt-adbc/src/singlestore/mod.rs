pub mod connection;
pub mod convert;
pub mod database;
pub mod driver;
pub mod statement;

pub(crate) mod sqlx {
    pub use sqlx_core::column::Column;
    pub use sqlx_core::connection::Connection;
    pub use sqlx_core::query::query;
    pub use sqlx_core::row::Row;
    pub use sqlx_core::type_info::TypeInfo;
    pub mod mysql {
        pub use sqlx_mysql::{MySqlConnectOptions, MySqlRow};
    }
    pub use sqlx_mysql::MySqlConnection;
}

pub use connection::SingleStoreConnection;
pub use database::SingleStoreDatabase;
pub use driver::SingleStoreDriver;
pub use statement::SingleStoreStatement;
