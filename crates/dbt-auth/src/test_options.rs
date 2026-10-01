use adbc_core::options::{OptionDatabase, OptionValue};
use dbt_adbc::database;

pub(crate) fn option_str_value(option_value: &OptionValue) -> &str {
    match option_value {
        OptionValue::String(s) => s.as_str(),
        _ => panic!("Expected OptionValue to be String"),
    }
}

pub(crate) fn other_option_value<'a>(builder: &'a database::Builder, key: &str) -> Option<&'a str> {
    let option = OptionDatabase::Other(key.to_string());
    builder.other.iter().find_map(|(k, v)| {
        if *k == option {
            Some(option_str_value(v))
        } else {
            None
        }
    })
}

pub(crate) fn uri_value(builder: &database::Builder) -> String {
    builder
        .clone()
        .into_iter()
        .find_map(|(k, v)| match k {
            OptionDatabase::Uri => match v {
                OptionValue::String(s) => Some(s),
                _ => panic!("Expected OptionValue to be String"),
            },
            _ => None,
        })
        .expect("missing uri option")
}
