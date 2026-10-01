use dbt_common::{ErrorCode, FsResult, fs_err};
use dialoguer::{Confirm, Input, Password, Select};
use std::collections::HashMap;

/// Trait for interactive setup of configuration structs
pub trait InteractiveSetup: Clone {
    /// Get the declarative field definitions for this config type
    fn get_fields() -> Vec<ConfigField>;

    /// Set a field value by name
    fn set_field(&mut self, field_name: &str, value: FieldValue) -> FsResult<()>;

    /// Get a field value by name (for conditional logic)
    fn get_field(&self, field_name: &str) -> Option<FieldValue>;

    /// Check if a field is currently set/populated
    fn is_field_set(&self, field_name: &str) -> bool;
}

/// Field value types that can be collected from user input
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    String(String),
    Integer(i64),
    Boolean(bool),
}

/// Types of input fields
#[derive(Debug, Clone)]
pub enum FieldType {
    Input {
        default: Option<String>,
    },
    Password,
    Select {
        options: Vec<String>,
        default_index: usize,
    },
    Confirm {
        default: bool,
    },
}

/// Conditions for when fields should be shown
#[derive(Debug, Clone)]
pub enum FieldCondition {
    Always,
    IfFieldEquals {
        field_name: String,
        value: FieldValue,
    },
    IfFieldNotEquals {
        field_name: String,
        value: FieldValue,
    },
}

/// Declarative field definition
#[derive(Debug, Clone)]
pub struct ConfigField {
    pub name: String,
    pub prompt: String,
    pub field_type: FieldType,
    pub condition: FieldCondition,
    pub required: bool,
}

impl ConfigField {
    /// Create an input field
    pub fn input(name: &str, prompt: &str) -> Self {
        Self {
            name: name.to_string(),
            prompt: prompt.to_string(),
            field_type: FieldType::Input { default: None },
            condition: FieldCondition::Always,
            required: true,
        }
    }

    /// Create an optional input field
    pub fn optional_input(name: &str, prompt: &str, default: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            prompt: prompt.to_string(),
            field_type: FieldType::Input {
                default: default.map(|s| s.to_string()),
            },
            condition: FieldCondition::Always,
            required: false,
        }
    }

    /// Create a password field
    pub fn password(name: &str, prompt: &str) -> Self {
        Self {
            name: name.to_string(),
            prompt: prompt.to_string(),
            field_type: FieldType::Password,
            condition: FieldCondition::Always,
            required: true,
        }
    }

    /// Create a select field
    pub fn select(name: &str, prompt: &str, options: Vec<&str>, default_index: usize) -> Self {
        Self {
            name: name.to_string(),
            prompt: prompt.to_string(),
            field_type: FieldType::Select {
                options: options.into_iter().map(|s| s.to_string()).collect(),
                default_index,
            },
            condition: FieldCondition::Always,
            required: true,
        }
    }

    /// Create a confirm field
    pub fn confirm(name: &str, prompt: &str, default: bool) -> Self {
        Self {
            name: name.to_string(),
            prompt: prompt.to_string(),
            field_type: FieldType::Confirm { default },
            condition: FieldCondition::Always,
            required: true,
        }
    }

    /// Add a condition to this field
    pub fn when_field_equals(mut self, field_name: &str, value: FieldValue) -> Self {
        self.condition = FieldCondition::IfFieldEquals {
            field_name: field_name.to_string(),
            value,
        };
        self
    }

    /// Make this field optional
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }
}

/// Interactive configuration processor
pub struct ConfigProcessor;

impl ConfigProcessor {
    /// Process a configuration interactively using declarative field definitions
    pub fn process_config<T: InteractiveSetup>(existing_config: Option<&T>) -> FsResult<T> {
        let mut config = existing_config.cloned().expect("Config must be provided");
        let fields = T::get_fields();

        // Collect values for all fields
        let mut collected_values = HashMap::new();

        for field in &fields {
            // Check if field should be shown based on condition
            if !Self::should_show_field(field, &collected_values, &config) {
                continue;
            }

            // Always collect user input for this field, using existing value as default when available
            let value = Self::collect_field_value(field, &config)?;

            // Store value for condition checking and config setting
            collected_values.insert(field.name.clone(), value.clone());
            config.set_field(&field.name, value)?;
        }

        Ok(config)
    }

    pub fn should_show_field<T: InteractiveSetup>(
        field: &ConfigField,
        collected_values: &HashMap<String, FieldValue>,
        config: &T,
    ) -> bool {
        match &field.condition {
            FieldCondition::Always => true,
            FieldCondition::IfFieldEquals { field_name, value } => {
                if let Some(collected_value) = collected_values.get(field_name) {
                    collected_value == value
                } else if let Some(config_value) = config.get_field(field_name) {
                    &config_value == value
                } else {
                    false
                }
            }
            FieldCondition::IfFieldNotEquals { field_name, value } => {
                if let Some(collected_value) = collected_values.get(field_name) {
                    collected_value != value
                } else if let Some(config_value) = config.get_field(field_name) {
                    &config_value != value
                } else {
                    true
                }
            }
        }
    }

    fn collect_field_value<T: InteractiveSetup>(
        field: &ConfigField,
        config: &T,
    ) -> FsResult<FieldValue> {
        match &field.field_type {
            FieldType::Input { default } => {
                let mut input = Input::<String>::new()
                    .allow_empty(!field.required)
                    .with_prompt(&field.prompt);

                // Use existing value as default if available, otherwise use field default
                let default_val = if config.is_field_set(&field.name) {
                    config.get_field(&field.name).and_then(|v| match v {
                        FieldValue::String(s) => Some(s),
                        _ => None,
                    })
                } else {
                    default.clone()
                };

                if let Some(default_val) = default_val {
                    input = input.default(default_val);
                }
                let value = input.interact_text().map_err(|e| {
                    fs_err!(
                        ErrorCode::IoError,
                        "Failed to get input for {}: {}",
                        field.name,
                        e
                    )
                })?;
                Ok(FieldValue::String(value))
            }
            FieldType::Password => {
                // For passwords, we don't show existing values as defaults for security
                let value = Password::new()
                    .with_prompt(&field.prompt)
                    .allow_empty_password(!field.required)
                    .interact()
                    .map_err(|e| {
                        fs_err!(
                            ErrorCode::IoError,
                            "Failed to get password for {}: {}",
                            field.name,
                            e
                        )
                    })?;
                Ok(FieldValue::String(value))
            }
            FieldType::Select {
                options,
                default_index,
            } => {
                let mut select = Select::new().with_prompt(&field.prompt).items(options);

                // Use existing value as default if available
                let default_idx = if config.is_field_set(&field.name) {
                    config
                        .get_field(&field.name)
                        .and_then(|v| match v {
                            FieldValue::Integer(i) => Some(i as usize),
                            _ => None,
                        })
                        .unwrap_or(*default_index)
                } else {
                    *default_index
                };

                select = select.default(default_idx);
                let selection = select.interact().map_err(|e| {
                    fs_err!(
                        ErrorCode::IoError,
                        "Failed to get selection for {}: {}",
                        field.name,
                        e
                    )
                })?;
                Ok(FieldValue::Integer(selection as i64))
            }
            FieldType::Confirm { default } => {
                let mut confirm = Confirm::new().with_prompt(&field.prompt);

                // Use existing value as default if available
                let default_val = if config.is_field_set(&field.name) {
                    config
                        .get_field(&field.name)
                        .and_then(|v| match v {
                            FieldValue::Boolean(b) => Some(b),
                            _ => None,
                        })
                        .unwrap_or(*default)
                } else {
                    *default
                };

                confirm = confirm.default(default_val);
                let value = confirm.interact().map_err(|e| {
                    fs_err!(
                        ErrorCode::IoError,
                        "Failed to get confirmation for {}: {}",
                        field.name,
                        e
                    )
                })?;
                Ok(FieldValue::Boolean(value))
            }
        }
    }
}
