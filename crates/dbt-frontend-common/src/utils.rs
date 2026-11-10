use itertools::Itertools;

const HASH_SIZE: usize = 16;
const VERSION_SIZE: usize = 17;

pub fn strip_version_hash(
    table_name: &str,
    version: &Option<String>,
    hash: &Option<String>,
) -> String {
    if let Some(version) = version {
        let suffix = if let Some(hash) = hash {
            format!("_{version}_{hash}")
        } else {
            format!("_{version}")
        };

        table_name
            .strip_suffix(&suffix)
            .unwrap_or(table_name)
            .to_string()
    } else {
        table_name.to_string()
    }
}

pub fn get_version_hash(table_name: &str) -> (Option<String>, Option<String>) {
    let parts = table_name.split('_').collect_vec();
    if parts.len() > 1 {
        let suffix = (*parts.last().unwrap()).to_string();
        if suffix.len() == HASH_SIZE {
            // Get second to last part
            if let Some(maybe_version) = parts.get(parts.len() - 2) {
                if maybe_version.len() == VERSION_SIZE
                    && maybe_version.chars().all(|c| c.is_ascii_digit())
                {
                    (Some((*maybe_version).to_string()), Some(suffix))
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            }
        } else if suffix.len() == VERSION_SIZE  // we have a version but no hash
            && suffix.chars().all(|c| c.is_ascii_digit())
        {
            (Some(suffix), None)
        } else {
            (None, None)
        }
    } else {
        (None, None)
    }
}
