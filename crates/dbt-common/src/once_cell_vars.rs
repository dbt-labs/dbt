use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

pub static DISPATCH_CONFIG: OnceLock<RwLock<BTreeMap<String, Vec<String>>>> = OnceLock::new();
