use std::collections::HashSet;
// compares two sets of files and returns a changeset
// the changeset contains:
// - files that are the same in both sets
// - files that are different in both sets
// - files that are missing in the filesystem
// - files that are missing in the content-addressable storage (CAS)
// - whether the deps are the same e.g. (i.e dependencies.yml, package.lock and all dbt_packages)
// files are represented by their relative path to the project root
pub struct Changeset {
    pub same: HashSet<String>,
    pub different: HashSet<String>,
    pub missing_in_fs: HashSet<String>,
    pub missing_in_cas: HashSet<String>,
    pub are_deps_the_same: bool,
}
impl Changeset {
    pub fn no_change(&self) -> bool {
        self.different.is_empty()
            && self.missing_in_fs.is_empty()
            && self.missing_in_cas.is_empty()
            && !self.same.is_empty()
    }
}
impl Default for Changeset {
    fn default() -> Self {
        Self {
            same: HashSet::new(),
            different: HashSet::new(),
            missing_in_fs: HashSet::new(),
            missing_in_cas: HashSet::new(),
            are_deps_the_same: false,
        }
    }
}
