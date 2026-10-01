use super::*;

impl CatalogRelation {
    pub(crate) fn deprecated_from_model_config_and_catalogs(
        adapter_type: AdapterType,
        model: &Value,
        catalogs: Option<Arc<DbtCatalogs>>,
    ) -> AdapterResult<CatalogRelation> {
        match adapter_type {
            AdapterType::Databricks => {
                Self::deprecated_from_model_config_and_catalogs_databricks(model, catalogs)
            }
            AdapterType::Snowflake => {
                Self::deprecated_from_model_config_and_catalogs_snowflake(model, catalogs)
            }
            AdapterType::Bigquery => {
                Self::deprecated_from_model_config_and_catalogs_bigquery(model, catalogs)
            }
            // Lake compute is DuckDB-backed for relation-building purposes;
            // the deprecated catalogs.yml shape never supported per-catalog DuckDB
            // selection either, so this mirrors the DuckDB arm exactly.
            // GizmoSQL is a remote DuckDB and shares DuckDB's macros, which read
            // `duckdb_write_strategy` off this relation.
            AdapterType::DuckDB | AdapterType::LakeCompute | AdapterType::GizmoSQL => {
                Ok(Self::default_catalog_relation_duckdb())
            }
            _ => Err(AdapterError::new(
                AdapterErrorKind::Internal,
                format!("build_relation_catalog cannot be invoked by an adapter {adapter_type:?}"),
            )),
        }
    }

    pub(crate) fn deprecated_from_model_config_and_catalogs_bigquery(
        model: &Value,
        catalogs: Option<Arc<DbtCatalogs>>,
    ) -> AdapterResult<CatalogRelation> {
        debug_assert!(
            model.kind() != ValueKind::String,
            "Bigquery adapter received a bare string model config; this is unsupported and indicates a parser bug."
        );

        let model_catalog_name =
            Self::get_model_config_value(model, "catalog_name", AdapterType::Bigquery).and_then(
                |s| {
                    let t = s.trim();
                    if t.eq_ignore_ascii_case("none") {
                        None
                    } else {
                        Some(t.to_string())
                    }
                },
            );

        let wants_iceberg = TableFormat::parse(
            Self::get_model_config_value(model, "table_format", AdapterType::Bigquery).as_deref(),
        )
        .map_err(|e| AdapterError::new(AdapterErrorKind::Configuration, e.to_string()))?
        .is_iceberg();

        match (model_catalog_name.as_deref(), catalogs.as_ref()) {
            (None, _) if !wants_iceberg => Ok(Self::default_catalog_relation_bigquery()),
            (None, _) => Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "On Bigquery, table_format=iceberg requires catalogs.yml and a `catalog_name` that selects a write integration.",
            )),
            (Some(catalog_name), None) => Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!(
                    "Model specifies catalog_name '{catalog_name}', but catalogs.yml was not found"
                ),
            )),
            (Some(catalog_name), Some(catalogs)) => Self::deprecated_build_bigquery_with_catalogs(
                model,
                catalogs.mapping(),
                catalog_name,
            ),
        }
    }

    fn deprecated_build_bigquery_with_catalogs(
        model: &Value,
        catalogs: &YmlMapping,
        catalog_name: &str,
    ) -> AdapterResult<CatalogRelation> {
        let catalog = find_catalog(catalogs, catalog_name).ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("Catalog '{catalog_name}' not found in catalogs.yml"),
            )
        })?;

        // 1) active integration name
        let integration_name =
            lookup_integration_name(catalogs, catalog_name).ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    format!("Catalog '{catalog_name}' missing 'active_write_integration'"),
                )
            })?;

        // 2) resolve the selected write_integration mapping
        let write_integration = Self::lookup_write_integration(catalog, &integration_name);

        // 3) catalog_type must be in YAML (no model override)
        if Self::get_model_config_value(model, "catalog_type", AdapterType::Bigquery).is_some() {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "catalog_type may only be specified in write integration entries of catalogs.yml",
            ));
        }

        let catalog_type = match Self::yml_str(write_integration, "catalog_type".to_owned())
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "catalog_type is required by the catalogs.yml schema on every write integration. Validation should have already rejected its absence",
                )
            })? {
            s if s.eq_ignore_ascii_case("biglake_metastore") => CatalogType::BiglakeMetastore,
            s => {
                return Err(AdapterError::new(
                    AdapterErrorKind::Configuration,
                    format!("Invalid Bigquery catalog_type '{s}'"),
                ));
            }
        };

        let model_table_format =
            Self::get_model_config_value(model, "table_format", AdapterType::Bigquery);
        let yml_table_format = Self::yml_str(write_integration, "table_format".to_string());
        let table_format = TableFormat::parse(
            model_table_format
                .as_deref()
                .or(yml_table_format.as_deref()),
        )
        .map_err(|e| AdapterError::new(AdapterErrorKind::Configuration, e.to_string()))?;

        // file_format: model > YAML
        let mut file_format =
            Self::get_model_config_value(model, "file_format", AdapterType::Bigquery)
                .or_else(|| Self::yml_str(write_integration, "file_format".to_string()))
                .ok_or_else(|| {
                    AdapterError::new(
                        AdapterErrorKind::Configuration,
                        "file_format is required by the catalogs.yml schema on every write integration. Validation should have already rejected its absence",
                    )
                })?;
        file_format.make_ascii_lowercase();

        // 6) adapter_properties:
        //    - base_location_root (optional)
        //    - base_location_subpath (optional)
        //    - storage_uri
        //    - connection_id
        let yaml_adapter_props = Self::get_yaml_adapter_properties(write_integration);
        let model_adapter_props = Self::get_model_adapter_properties(model, AdapterType::Bigquery);

        // base_location_root: model(adapter_properties) > model (legacy) > YAML > default
        let base_location_root =
            Self::get_adapter_property(model_adapter_props.as_ref(), "base_location_root")
                .or_else(|| {
                    Self::get_model_config_value(model, "base_location_root", AdapterType::Bigquery)
                })
                .or_else(|| {
                    Self::get_adapter_property(yaml_adapter_props.as_ref(), "base_location_root")
                });

        // base_location_subpath: model (adapter_properties) > model (legacy) > default
        if Self::yml_str(write_integration, "base_location_subpath".to_string()).is_some() {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "base_location_subpath is forbidden by the catalogs.yml schema on write integrations. Model-level base_location_subpath must be used instead, and validation should have already rejected this",
            ));
        }
        let base_location_subpath =
            Self::get_adapter_property(model_adapter_props.as_ref(), "base_location_subpath")
                .or_else(|| {
                    Self::get_model_config_value(
                        model,
                        "base_location_subpath",
                        AdapterType::Bigquery,
                    )
                });

        let schema = Self::get_model_config_value(model, "schema", AdapterType::Bigquery);
        let identifier = Self::get_model_config_value(model, "alias", AdapterType::Bigquery)
            .or_else(|| Self::get_model_config_value(model, "identifier", AdapterType::Bigquery));

        let base_location = Self::build_base_location(
            &base_location_root,
            &base_location_subpath,
            &schema,
            &identifier,
        );

        // external_volume must be in YAML (no model override)
        if Self::get_model_config_value(model, "external_volume", AdapterType::Bigquery).is_some() {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "external_volume may only be specified in write integration entries of catalogs.yml",
            ));
        }

        let external_volume = Self::yml_str(write_integration, "external_volume".to_owned())
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "external_volume is required by the catalogs.yml schema on every write integration. Validation should have already rejected its absence",
                )
            })?;

        // storage_uri: model (adapter_properties) > model (legacy) > default
        let storage_uri = Self::get_adapter_property(model_adapter_props.as_ref(), "storage_uri")
            .or_else(|| Self::get_model_config_value(model, "storage_uri", AdapterType::Bigquery))
            .unwrap_or_else(|| format!("{external_volume}/{base_location}"));

        let mut adapter_properties =
            Self::merged_adapter_properties(yaml_adapter_props, model_adapter_props);

        adapter_properties.insert("storage_uri".to_owned(), storage_uri);

        Ok(CatalogRelation {
            adapter_type: AdapterType::Bigquery,
            catalog_name: Some(catalog_name.to_string()),
            integration_name: Some(integration_name),
            catalog_type,
            table_format,
            adapter_properties,
            is_transient: None,
            external_volume: None,
            catalog_database: None,
            lakehouse_catalog: None,
            base_location: None,
            file_format: Some(file_format),
        })
    }

    pub(crate) fn deprecated_from_model_config_and_catalogs_databricks(
        model: &Value,
        catalogs: Option<Arc<DbtCatalogs>>,
    ) -> AdapterResult<CatalogRelation> {
        debug_assert!(
            model.kind() != ValueKind::String,
            "Databricks adapter received a bare string model config; this is unsupported and indicates a parser bug."
        );

        let model_catalog_name =
            Self::get_model_config_value(model, "catalog_name", AdapterType::Databricks).and_then(
                |s| {
                    let t = s.trim();
                    if t.eq_ignore_ascii_case("none") {
                        None
                    } else {
                        Some(t.to_string())
                    }
                },
            );

        let wants_iceberg = TableFormat::parse(
            Self::get_model_config_value(model, "table_format", AdapterType::Databricks).as_deref(),
        )
        .map_err(|e| AdapterError::new(AdapterErrorKind::Configuration, e.to_string()))?
        .is_iceberg();

        match (model_catalog_name.as_deref(), catalogs.as_ref()) {
            (None, None) if !wants_iceberg => {
                Ok(Self::default_catalog_relation_databricks_for_model(model))
            }
            (None, None) => Ok(Self::default_catalog_relation_databricks_for_model(model)
                .with_table_format(TableFormat::Iceberg)
                .with_adapter_property(ADAPTER_PROP_USE_UNIFORM, "false")),

            (None, Some(_)) if !wants_iceberg => {
                Ok(Self::default_catalog_relation_databricks_for_model(model))
            }
            (None, Some(_)) => Ok(Self::default_catalog_relation_databricks_for_model(model)
                .with_table_format(TableFormat::Iceberg)
                .with_adapter_property(ADAPTER_PROP_USE_UNIFORM, "false")),

            (Some(catalog_name), None) => Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!(
                    "Model specifies catalog_name '{catalog_name}', but catalogs.yml was not found"
                ),
            )),

            (Some(catalog_name), Some(catalogs)) => {
                Self::deprecated_build_databricks_with_catalogs(
                    model,
                    catalogs.mapping(),
                    catalog_name,
                )
            }
        }
    }

    fn deprecated_build_databricks_with_catalogs(
        model: &Value,
        catalogs: &YmlMapping,
        catalog_name: &str,
    ) -> AdapterResult<CatalogRelation> {
        let catalog = find_catalog(catalogs, catalog_name).ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("Catalog '{catalog_name}' not found in catalogs.yml"),
            )
        })?;

        // 1) active integration name
        let integration_name =
            lookup_integration_name(catalogs, catalog_name).ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    format!("Catalog '{catalog_name}' missing 'active_write_integration'"),
                )
            })?;

        // 2) resolve the selected write_integration mapping
        let write_integration = Self::lookup_write_integration(catalog, &integration_name);

        // 3) catalog_type must be in YAML (no model override)
        if Self::get_model_config_value(model, "catalog_type", AdapterType::Databricks).is_some() {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "catalog_type may only be specified in write integration entries of catalogs.yml",
            ));
        }

        let raw_catalog_type = Self::yml_str(write_integration, "catalog_type".to_owned())
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "catalog_type is required by the catalogs.yml schema on every write integration. Validation should have already rejected its absence",
                )
            })?;

        let catalog_type = match raw_catalog_type.as_str() {
            s if s.eq_ignore_ascii_case("unity") => CatalogType::Unity,
            s if s.eq_ignore_ascii_case("hive_metastore") => CatalogType::HiveMetastore,
            s => {
                return Err(AdapterError::new(
                    AdapterErrorKind::Configuration,
                    format!("Invalid Databricks catalog_type '{s}'"),
                ));
            }
        };

        let model_table_format =
            Self::get_model_config_value(model, "table_format", AdapterType::Databricks);
        let yml_table_format = Self::yml_str(write_integration, "table_format".to_string());
        let table_format = TableFormat::parse(
            model_table_format
                .as_deref()
                .or(yml_table_format.as_deref()),
        )
        .map_err(|e| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("Invalid table_format in catalog '{catalog_name}': {e}"),
            )
        })?;

        // 5) file_format: model > YAML > default(delta)
        let mut file_format =
            Self::get_model_config_value(model, "file_format", AdapterType::Databricks)
                .or_else(|| Self::yml_str(write_integration, "file_format".to_string()))
                .unwrap_or_else(|| String::from(DBX_DEFAULT_TABLE_FORMAT));
        file_format.make_ascii_lowercase();
        let file_format = file_format;

        // 6) adapter_properties:
        //    - UNITY: allow only location_root (optional; non-blank)
        //    - HMS: disallow adapter_properties entirely
        let yaml_adapter_props = Self::get_yaml_adapter_properties(write_integration);
        let model_adapter_props =
            Self::get_model_adapter_properties(model, AdapterType::Databricks);
        let mut external_volume = None;

        // location_root: model(adapter_properties) > model (legacy) > YAML > default
        let location_root =
            Self::get_adapter_property(model_adapter_props.as_ref(), "location_root")
                .or_else(|| {
                    Self::get_model_config_value(model, "location_root", AdapterType::Databricks)
                })
                .or_else(|| {
                    Self::get_adapter_property(yaml_adapter_props.as_ref(), "location_root")
                });

        let adapter_properties =
            Self::merged_adapter_properties(yaml_adapter_props, model_adapter_props);

        if raw_catalog_type.eq_ignore_ascii_case(DATABRICKS_UNITY_CATALOG)
            && let Some(location_root) = location_root
        {
            if location_root.trim().is_empty() {
                return Err(AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "adapter_properties.location_root cannot be blank or whitespace",
                ));
            }
            external_volume = Self::dbx_build_external_volume_for_location(model, &location_root);
        } else if raw_catalog_type.eq_ignore_ascii_case(DATABRICKS_HIVE_METASTORE)
            && !adapter_properties.is_empty()
        {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "adapter_properties not allowed for hive_metastore",
            ));
        };
        let external_volume = external_volume;

        Ok(CatalogRelation {
            adapter_type: AdapterType::Databricks,
            catalog_name: Some(catalog_name.to_string()),
            integration_name: Some(integration_name),
            catalog_type,
            table_format,
            file_format: Some(file_format),
            external_volume,
            catalog_database: None,
            lakehouse_catalog: None,
            base_location: None,
            adapter_properties,
            is_transient: None,
        })
    }

    // The bare-string "linked database name" call shape (used by
    // `drop.sql`) is intercepted earlier, in `from_model_config_and_catalogs`,
    // via `from_linked_database_name` -- before this deprecated/active branch, since
    // the active path has no concept of it. It should never reach this function.
    pub(crate) fn deprecated_from_model_config_and_catalogs_snowflake(
        model: &Value,
        catalogs: Option<Arc<DbtCatalogs>>,
    ) -> AdapterResult<Self> {
        debug_assert!(
            model.kind() != ValueKind::String,
            "Snowflake adapter received a bare string model config in deprecated_from_model_config_and_catalogs_snowflake; \
             this should have been intercepted by from_model_config_and_catalogs's linked-database-name check."
        );

        let model_catalog_name =
            Self::get_model_config_value(model, "catalog_name", AdapterType::Snowflake).and_then(
                |s| {
                    let t = s.trim();
                    // [DELIBERATE CHANGE] Serialization sometimes makes model configs parse a none
                    // value into Some("none"). Unlikely many users will be naming their catalog names 'none'.
                    // TODO: track that down and patch
                    if t.eq_ignore_ascii_case("none") {
                        None
                    } else {
                        Some(t.to_string())
                    }
                },
            );

        match (model_catalog_name.as_deref(), catalogs.as_ref()) {
            // No reconciliation path: only values present on the model config are used.
            // This represents the legacy/deprecated-schema Iceberg tables
            // which are Snowflake only and do not use the catalogs.yml.
            (None, _) => Self::build_without_catalogs_yml(model),

            // Catalog-driven path: both catalog_name and catalogs.yml need be present
            (Some(catalog_name), None) => Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!(
                    "Model specifies catalog_name '{catalog_name}', but catalogs.yml was not found"
                ),
            )),

            (Some(catalog_name), Some(catalogs)) => {
                Self::deprecated_build_with_catalogs(model, catalogs.mapping(), catalog_name)
            }
        }
    }

    /// Helper for building a catalog relation of any type supported in catalogs.yml
    ///
    /// A catalog write integration holds fallback metadata for model materialization DDL.
    /// Any individual model may override the catalog metadata with their own model configs.
    pub(crate) fn deprecated_build_with_catalogs(
        model: &Value,
        catalogs: &YmlMapping,
        catalog_name: &str,
    ) -> AdapterResult<CatalogRelation> {
        let catalog = find_catalog(catalogs, catalog_name).ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("Catalog '{catalog_name}' not found in catalogs.yml"),
            )
        })?;

        // 1) identity: catalog comes from MC; integration is the catalog's active one
        let integration_name = lookup_integration_name(catalogs, catalog_name).unwrap_or_default();

        // 2) write integration lookup (may be None)
        let write_integration = Self::lookup_write_integration(catalog, &integration_name);

        // 3) resolve fields: model > write_integration > default/None

        // === catalog_type logic forbids overrides as Core hardcodes in catalogs.yml
        if Self::get_model_config_value(model, "catalog_type", AdapterType::Snowflake).is_some() {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "catalog_type may only be specified in write integration entries of catalogs.yml",
            ));
        }

        let raw_catalog_type = Self::yml_str(write_integration, "catalog_type".to_owned())
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "catalog_type is required by the catalogs.yml schema on every write integration. Validation should have already rejected its absence",
                )
            })?;

        let catalog_type = if raw_catalog_type.eq_ignore_ascii_case("built_in") {
            CatalogType::SnowflakeBuiltIn
        } else if raw_catalog_type.eq_ignore_ascii_case("snowflake") {
            CatalogType::SnowflakeNative
        } else if raw_catalog_type.eq_ignore_ascii_case("iceberg_rest") {
            CatalogType::IcebergRest
        } else {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("Invalid Snowflake catalog_type '{raw_catalog_type}'"),
            ));
        };

        let model_table_format =
            Self::get_model_config_value(model, "table_format", AdapterType::Snowflake);
        let yml_table_format = Self::yml_str(write_integration, "table_format".to_string());
        let table_format = TableFormat::parse(
            model_table_format
                .as_deref()
                .or(yml_table_format.as_deref()),
        )
        .map_err(|e| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("Invalid table_format in catalog '{catalog_name}': {e}"),
            )
        })?;

        // === Build up the external volume
        let external_volume =
            Self::get_model_config_value(model, "external_volume", AdapterType::Snowflake)
                .or_else(|| Self::yml_str(write_integration, "external_volume".to_string()));

        // === Build up base location
        let yaml_adapter_props = Self::get_yaml_adapter_properties(write_integration);
        let model_adapter_props = Self::get_model_adapter_properties(model, AdapterType::Snowflake);

        // base_location_root: model(adapter_properties) > model (legacy) > YAML > default
        let base_location_root =
            Self::get_adapter_property(model_adapter_props.as_ref(), "base_location_root")
                .or_else(|| {
                    Self::get_model_config_value(
                        model,
                        "base_location_root",
                        AdapterType::Snowflake,
                    )
                })
                .or_else(|| {
                    Self::get_adapter_property(yaml_adapter_props.as_ref(), "base_location_root")
                });

        // base_location_subpath: model (adapter_properties) > model (legacy) > default
        if Self::yml_str(write_integration, "base_location_subpath".to_string()).is_some() {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "base_location_subpath is forbidden by the catalogs.yml schema on write integrations. Model-level base_location_subpath must be used instead, and validation should have already rejected this",
            ));
        }
        let base_location_subpath =
            Self::get_adapter_property(model_adapter_props.as_ref(), "base_location_subpath")
                .or_else(|| {
                    Self::get_model_config_value(
                        model,
                        "base_location_subpath",
                        AdapterType::Snowflake,
                    )
                });

        let schema = Self::get_model_config_value(model, "schema", AdapterType::Snowflake);
        let identifier = Self::get_model_config_value(model, "alias", AdapterType::Snowflake)
            .or_else(|| Self::get_model_config_value(model, "identifier", AdapterType::Snowflake));

        let base_location = Self::build_base_location(
            &base_location_root,
            &base_location_subpath,
            &schema,
            &identifier,
        );

        // 4) adapter_properties from YAML write_integration.adapter_properties and model config overrides
        let mut adapter_properties =
            Self::merged_adapter_properties(yaml_adapter_props, model_adapter_props);

        // Model-level iceberg_version takes precedence over catalog adapter_properties
        if let Some(v) =
            Self::get_model_config_value(model, "iceberg_version", AdapterType::Snowflake)
        {
            adapter_properties.insert("iceberg_version".to_string(), v);
        }

        // 5) transient handling
        //
        // FIXME(versusfacit): same swallowed transient case as in build_without_catalogs_yml
        // above.

        Ok(CatalogRelation {
            adapter_type: AdapterType::Snowflake,
            catalog_name: Some(catalog_name.to_string()),
            integration_name: Some(integration_name),
            catalog_type,
            table_format,
            external_volume,
            catalog_database: None,
            lakehouse_catalog: None,
            base_location: Some(base_location),
            adapter_properties,
            is_transient: Some(false),
            file_format: None,
        })
    }
}
