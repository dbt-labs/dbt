//! Comprehensive tests for the omissible utility functions

#[cfg(test)]
mod tests {
    use super::super::config_merge::DefaultTo;
    use crate::schemas::project::ResolvableConfig;
    use crate::schemas::project::dbt_project::ResolvedConfig;
    use dbt_common::serde_utils::Omissible;
    use serde::{Deserialize, Serialize};

    /// Test configuration for validating OmissibleHandler behavior
    #[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
    struct TestConfig {
        pub name: Omissible<Option<String>>,
        pub value: Omissible<Option<i32>>,
        pub enabled: Option<bool>,
    }

    impl TestConfig {
        fn name(&self) -> Option<String> {
            self.name.clone().into_inner().unwrap_or(None)
        }

        fn value(&self) -> Option<i32> {
            match &self.value {
                Omissible::Present(opt) => *opt,
                Omissible::Omitted => None,
            }
        }
    }

    impl ResolvedConfig for TestConfig {
        fn enabled(&self) -> bool {
            self.enabled.unwrap_or(true)
        }
    }

    impl ResolvableConfig<TestConfig> for TestConfig {
        type Resolved = Self;
        type PackageDefaults = ();
        type ResolveDefaults = ();

        fn default_to(&mut self, parent: &TestConfig) {
            self.name.inherit_from(&parent.name);
            self.value.inherit_from(&parent.value);
            self.enabled.inherit_from(&parent.enabled);
        }

        fn get_enabled_with_default(&self) -> bool {
            self.enabled.unwrap_or(true)
        }

        fn disable(&mut self) {
            self.enabled = Some(false);
        }

        fn apply_package_defaults(&mut self, _: ()) {}

        fn finalize(mut self) -> Self {
            self.enabled = Some(self.get_enabled_with_default());
            self
        }
    }

    #[test]
    fn test_omitted_inherits_parent() {
        let mut child = TestConfig {
            name: Omissible::Omitted,
            value: Omissible::Omitted,
            enabled: Some(true),
        };

        let parent = TestConfig {
            name: Omissible::Present(Some("parent_name".to_string())),
            value: Omissible::Present(Some(42)),
            enabled: Some(false),
        };

        child.default_to(&parent);

        assert_eq!(child.name(), Some("parent_name".to_string()));
        assert_eq!(child.value(), Some(42));
        assert_eq!(child.enabled, Some(true)); // Regular field keeps child value
    }

    #[test]
    fn test_present_none_overrides_parent_some() {
        let mut child = TestConfig {
            name: Omissible::Present(None),
            value: Omissible::Present(None),
            enabled: None,
        };

        let parent = TestConfig {
            name: Omissible::Present(Some("parent_name".to_string())),
            value: Omissible::Present(Some(42)),
            enabled: Some(true),
        };

        child.default_to(&parent);

        assert_eq!(child.name(), None); // Child's explicit None overrides parent
        assert_eq!(child.value(), None);
        assert_eq!(child.enabled, Some(true)); // Regular field inherits from parent
    }

    #[test]
    fn test_present_some_keeps_child_value() {
        let mut child = TestConfig {
            name: Omissible::Present(Some("child_name".to_string())),
            value: Omissible::Present(Some(100)),
            enabled: Some(false),
        };

        let parent = TestConfig {
            name: Omissible::Present(Some("parent_name".to_string())),
            value: Omissible::Present(Some(42)),
            enabled: Some(true),
        };

        child.default_to(&parent);

        assert_eq!(child.name(), Some("child_name".to_string())); // Child keeps its value
        assert_eq!(child.value(), Some(100));
        assert_eq!(child.enabled, Some(false));
    }

    #[test]
    fn test_parent_none_does_not_override_child_some() {
        let mut child = TestConfig {
            name: Omissible::Present(Some("child_name".to_string())),
            value: Omissible::Present(Some(100)),
            enabled: Some(true),
        };

        let parent = TestConfig {
            name: Omissible::Present(None),
            value: Omissible::Present(None),
            enabled: None,
        };

        child.default_to(&parent);

        // Child's explicit Some value overrides parent's None
        assert_eq!(child.name(), Some("child_name".to_string()));
        assert_eq!(child.value(), Some(100));
        assert_eq!(child.enabled, Some(true)); // Regular field keeps child value
    }

    #[test]
    fn test_parent_omitted_does_not_override_child() {
        let mut child = TestConfig {
            name: Omissible::Present(Some("child_name".to_string())),
            value: Omissible::Present(Some(100)),
            enabled: Some(true),
        };

        let parent = TestConfig {
            name: Omissible::Omitted,
            value: Omissible::Omitted,
            enabled: Some(false),
        };

        child.default_to(&parent);

        // Parent Omitted doesn't override child's value
        assert_eq!(child.name(), Some("child_name".to_string()));
        assert_eq!(child.value(), Some(100));
        assert_eq!(child.enabled, Some(true));
    }

    #[test]
    fn test_both_omitted() {
        let mut child = TestConfig {
            name: Omissible::Omitted,
            value: Omissible::Omitted,
            enabled: None,
        };

        let parent = TestConfig {
            name: Omissible::Omitted,
            value: Omissible::Omitted,
            enabled: None,
        };

        child.default_to(&parent);

        assert_eq!(child.name(), None);
        assert_eq!(child.value(), None);
        assert_eq!(child.enabled, None);
    }

    #[test]
    fn test_chain_of_overrides() {
        // Simulate a chain: grandparent -> parent -> child
        let grandparent = TestConfig {
            name: Omissible::Present(Some("grandparent".to_string())),
            value: Omissible::Present(Some(1)),
            enabled: Some(true),
        };

        let mut parent = TestConfig {
            name: Omissible::Omitted,
            value: Omissible::Present(Some(2)),
            enabled: None,
        };

        let mut child = TestConfig {
            name: Omissible::Omitted,
            value: Omissible::Omitted,
            enabled: Some(false),
        };

        // Apply inheritance
        parent.default_to(&grandparent);
        child.default_to(&parent);

        assert_eq!(parent.name(), Some("grandparent".to_string())); // Inherited from grandparent
        assert_eq!(parent.value(), Some(2)); // Kept own value
        assert_eq!(parent.enabled, Some(true)); // Inherited from grandparent

        assert_eq!(child.name(), Some("grandparent".to_string())); // Inherited through parent
        assert_eq!(child.value(), Some(2)); // Inherited from parent
        assert_eq!(child.enabled, Some(false)); // Kept own value
    }

    #[test]
    fn test_serialization_deserialization() {
        // Test Present(Some) case
        let config1 = TestConfig {
            name: Omissible::Present(Some("test".to_string())),
            value: Omissible::Present(Some(42)),
            enabled: Some(true),
        };

        let json1 = serde_json::to_string(&config1).unwrap();
        let deserialized1: TestConfig = serde_json::from_str(&json1).unwrap();
        assert_eq!(config1, deserialized1);

        // Test Omitted case
        let config2 = TestConfig {
            name: Omissible::Omitted,
            value: Omissible::Omitted,
            enabled: None,
        };

        let json2 = serde_json::to_string(&config2).unwrap();
        let deserialized2: TestConfig = serde_json::from_str(&json2).unwrap();
        assert_eq!(config2, deserialized2);

        // Note: Present(None) may deserialize as Omitted due to serde behavior
        // This is expected and handled by the Omissible implementation
    }
}
