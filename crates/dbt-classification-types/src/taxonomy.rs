//! Public classifier definition shapes and built-in defaults.
//!
//! The data types here are the on-the-wire description of a classifier (its
//! name, labels, propagation flag, etc.) plus the static list of built-ins
//! shipped with dbt. Registry assembly, YAML loading, validation, and merge
//! semantics live in the engine crate.

use serde::{Deserialize, Serialize};

/// A single label belonging to a classifier (e.g. "email" in "pii.email").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

pub fn default_propagate() -> bool {
    true
}

/// A classifier definition (e.g. "pii" with labels ["email", "ssn", …]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifierDef {
    pub name: String,
    pub labels: Vec<LabelDef>,
    /// When false, labels from this classifier are stripped before they
    /// enter the propagation environment (they are never propagated downstream).
    #[serde(default = "default_propagate")]
    pub propagate: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional Snowflake tag binding for this classifier.
    ///
    /// When present:
    ///   - Phase 1 imports only column tags whose `tag_name` matches this
    ///     value (case-insensitive) under this classifier.
    ///   - Phase 3 writes labels of this classifier back to Snowflake using
    ///     this tag name in the `ALTER TABLE … SET TAG` statement.
    ///
    /// May be a bare tag name (`"sensitivity"`) or a fully-qualified name
    /// (`"governance.pii.sensitivity"`).  When absent, the classifier is
    /// not bound to any specific Snowflake tag — free-form import applies
    /// and Phase 3 write-back is skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snowflake_tag: Option<String>,
}

/// Built-in classifiers shipped with Fusion (pii, finance, health).
pub fn builtin_classifiers() -> Vec<ClassifierDef> {
    fn labels(names: &[&str]) -> Vec<LabelDef> {
        names
            .iter()
            .map(|n| LabelDef {
                name: (*n).to_string(),
                description: None,
            })
            .collect()
    }
    vec![
        ClassifierDef {
            name: "pii".to_string(),
            labels: labels(&["name", "phone", "address", "email", "ssn", "dob", "hashed"]),
            propagate: true,
            description: Some("Personally Identifiable Information".to_string()),
            snowflake_tag: None,
        },
        ClassifierDef {
            name: "finance".to_string(),
            labels: labels(&["card_number", "account_number", "routing_number", "amount"]),
            propagate: true,
            description: Some("Financial data".to_string()),
            snowflake_tag: None,
        },
        ClassifierDef {
            name: "health".to_string(),
            labels: labels(&["diagnosis", "medication", "patient_id", "provider_id"]),
            propagate: true,
            description: Some("Health / medical data".to_string()),
            snowflake_tag: None,
        },
    ]
}
