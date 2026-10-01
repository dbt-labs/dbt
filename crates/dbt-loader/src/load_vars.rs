use dbt_common::FsResult;
use dbt_jinja_vars::DbtVars;
use indexmap::IndexMap;

// Load vars
// If no vars have been set, this is the root package and we need to set the global vars
// It's required that we push the "true" global vars to the vars vector, because these have
// not been expanded to consider the local package override.
pub fn load_vars(
    package_name: &str,
    vars_val: Option<IndexMap<String, DbtVars>>,
    collected_vars: &mut Vec<(String, IndexMap<String, DbtVars>)>,
) -> FsResult<()> {
    // Check if vars are set on package
    if let Some(mut vars) = vars_val {
        // If no vars have been set yet, this is the root package and we need to set the global vars
        let global_vars = if collected_vars.is_empty() {
            collected_vars.push((package_name.to_string(), vars.clone()));
            IndexMap::new()
        // Else, simply return the first element which is the global vars
        } else {
            collected_vars.first().unwrap().1.clone()
        };
        // If there are package vars, extend the vars with the package vars
        if let Some(DbtVars::Vars(self_override)) = vars.get(package_name) {
            vars.extend(self_override.clone());
        }
        // Extend the vars with the global vars
        vars.extend(global_vars.clone());
        // If there's a global var matching the package name and it's a IndexMap, extend vars with it
        if let Some(DbtVars::Vars(global_package_vars)) = global_vars.get(package_name) {
            vars.extend(global_package_vars.clone());
        }
        collected_vars.push((package_name.to_string(), vars));
    // If package is not root (i.e. collected_vars is not empty) and package has no vars,
    // set the package vars to the global vars (first element of collected_vars)
    } else if !collected_vars.is_empty() {
        let mut package_vars = collected_vars.first().unwrap().1.clone();
        if let Some(DbtVars::Vars(self_override)) = package_vars.get(package_name) {
            package_vars.extend(self_override.clone());
        }
        collected_vars.push((package_name.to_string(), package_vars))
    // If package is root and has no vars, push empty vars to collected_vars
    } else {
        collected_vars.push((package_name.to_string(), IndexMap::new()));
    }
    Ok(())
}
