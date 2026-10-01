use dbt_common::{ErrorCode, FsError, fs_err};

pub fn from_pretty_table_error(e: dbt_pretty_table::Error) -> Box<FsError> {
    use dbt_pretty_table::Error::*;
    match e {
        FromUtf8 => fs_err!(ErrorCode::Generic, "bytes to utf8 conversion failed"),
        Arrow(e) => e.into(),
        UnsupportedFormat(format) => fs_err!(
            ErrorCode::UnsupportedFeature,
            "DisplayFormat::{format:?} is not supported for tabular data display"
        ),
    }
}
