impl serde::Serialize for NodeCacheDetail {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if self.node_cache_reason != 0 {
            len += 1;
        }
        if self.build_after_seconds.is_some() {
            len += 1;
        }
        if self.last_updated_seconds.is_some() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.node.NodeCacheDetail", len)?;
        if self.node_cache_reason != 0 {
            let v = NodeCacheReason::try_from(self.node_cache_reason)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.node_cache_reason)))?;
            struct_ser.serialize_field("node_cache_reason", &v)?;
        }
        if let Some(v) = self.build_after_seconds.as_ref() {
            #[allow(clippy::needless_borrow)]
            #[allow(clippy::needless_borrows_for_generic_args)]
            struct_ser.serialize_field("build_after_seconds", ToString::to_string(&v).as_str())?;
        }
        if let Some(v) = self.last_updated_seconds.as_ref() {
            #[allow(clippy::needless_borrow)]
            #[allow(clippy::needless_borrows_for_generic_args)]
            struct_ser.serialize_field("last_updated_seconds", ToString::to_string(&v).as_str())?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for NodeCacheDetail {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "node_cache_reason",
            "nodeCacheReason",
            "build_after_seconds",
            "buildAfterSeconds",
            "last_updated_seconds",
            "lastUpdatedSeconds",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            NodeCacheReason,
            BuildAfterSeconds,
            LastUpdatedSeconds,
            __SkipField__,
        }
        impl<'de> serde::Deserialize<'de> for GeneratedField {
            fn deserialize<D>(deserializer: D) -> std::result::Result<GeneratedField, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct GeneratedVisitor;

                impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
                    type Value = GeneratedField;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        write!(formatter, "expected one of: {:?}", &FIELDS)
                    }

                    #[allow(unused_variables)]
                    fn visit_str<E>(self, value: &str) -> std::result::Result<GeneratedField, E>
                    where
                        E: serde::de::Error,
                    {
                        match value {
                            "nodeCacheReason" | "node_cache_reason" => Ok(GeneratedField::NodeCacheReason),
                            "buildAfterSeconds" | "build_after_seconds" => Ok(GeneratedField::BuildAfterSeconds),
                            "lastUpdatedSeconds" | "last_updated_seconds" => Ok(GeneratedField::LastUpdatedSeconds),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = NodeCacheDetail;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.node.NodeCacheDetail")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<NodeCacheDetail, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut node_cache_reason__ = None;
                let mut build_after_seconds__ = None;
                let mut last_updated_seconds__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::NodeCacheReason => {
                            if node_cache_reason__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeCacheReason"));
                            }
                            node_cache_reason__ = Some(map_.next_value::<NodeCacheReason>()? as i32);
                        }
                        GeneratedField::BuildAfterSeconds => {
                            if build_after_seconds__.is_some() {
                                return Err(serde::de::Error::duplicate_field("buildAfterSeconds"));
                            }
                            build_after_seconds__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::LastUpdatedSeconds => {
                            if last_updated_seconds__.is_some() {
                                return Err(serde::de::Error::duplicate_field("lastUpdatedSeconds"));
                            }
                            last_updated_seconds__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(NodeCacheDetail {
                    node_cache_reason: node_cache_reason__.unwrap_or_default(),
                    build_after_seconds: build_after_seconds__,
                    last_updated_seconds: last_updated_seconds__,
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.node.NodeCacheDetail", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for NodeCacheReason {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::NoChanges => "NODE_CACHE_REASON_NO_CHANGES",
            Self::StillFresh => "NODE_CACHE_REASON_STILL_FRESH",
            Self::UpdateCriteriaNotMet => "NODE_CACHE_REASON_UPDATE_CRITERIA_NOT_MET",
            Self::ClonedExisting => "NODE_CACHE_REASON_CLONED_EXISTING",
            Self::ClonedExistingStillFresh => "NODE_CACHE_REASON_CLONED_EXISTING_STILL_FRESH",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for NodeCacheReason {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "NODE_CACHE_REASON_NO_CHANGES",
            "NODE_CACHE_REASON_STILL_FRESH",
            "NODE_CACHE_REASON_UPDATE_CRITERIA_NOT_MET",
            "NODE_CACHE_REASON_CLONED_EXISTING",
            "NODE_CACHE_REASON_CLONED_EXISTING_STILL_FRESH",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = NodeCacheReason;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "expected one of: {:?}", &FIELDS)
            }

            fn visit_i64<E>(self, v: i64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Signed(v), &self)
                    })
            }

            fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Unsigned(v), &self)
                    })
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "NODE_CACHE_REASON_NO_CHANGES" => Ok(NodeCacheReason::NoChanges),
                    "NODE_CACHE_REASON_STILL_FRESH" => Ok(NodeCacheReason::StillFresh),
                    "NODE_CACHE_REASON_UPDATE_CRITERIA_NOT_MET" => Ok(NodeCacheReason::UpdateCriteriaNotMet),
                    "NODE_CACHE_REASON_CLONED_EXISTING" => Ok(NodeCacheReason::ClonedExisting),
                    "NODE_CACHE_REASON_CLONED_EXISTING_STILL_FRESH" => Ok(NodeCacheReason::ClonedExistingStillFresh),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
impl serde::Serialize for NodeCancelReason {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::UserCancelled => "NODE_CANCEL_REASON_USER_CANCELLED",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for NodeCancelReason {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "NODE_CANCEL_REASON_USER_CANCELLED",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = NodeCancelReason;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "expected one of: {:?}", &FIELDS)
            }

            fn visit_i64<E>(self, v: i64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Signed(v), &self)
                    })
            }

            fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Unsigned(v), &self)
                    })
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "NODE_CANCEL_REASON_USER_CANCELLED" => Ok(NodeCancelReason::UserCancelled),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
impl serde::Serialize for NodeErrorType {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::Internal => "NODE_ERROR_TYPE_INTERNAL",
            Self::External => "NODE_ERROR_TYPE_EXTERNAL",
            Self::User => "NODE_ERROR_TYPE_USER",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for NodeErrorType {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "NODE_ERROR_TYPE_INTERNAL",
            "NODE_ERROR_TYPE_EXTERNAL",
            "NODE_ERROR_TYPE_USER",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = NodeErrorType;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "expected one of: {:?}", &FIELDS)
            }

            fn visit_i64<E>(self, v: i64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Signed(v), &self)
                    })
            }

            fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Unsigned(v), &self)
                    })
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "NODE_ERROR_TYPE_INTERNAL" => Ok(NodeErrorType::Internal),
                    "NODE_ERROR_TYPE_EXTERNAL" => Ok(NodeErrorType::External),
                    "NODE_ERROR_TYPE_USER" => Ok(NodeErrorType::User),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
impl serde::Serialize for NodeEvaluated {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if !self.unique_id.is_empty() {
            len += 1;
        }
        if !self.name.is_empty() {
            len += 1;
        }
        if self.database.is_some() {
            len += 1;
        }
        if self.schema.is_some() {
            len += 1;
        }
        if self.identifier.is_some() {
            len += 1;
        }
        if self.materialization.is_some() {
            len += 1;
        }
        if self.custom_materialization.is_some() {
            len += 1;
        }
        if self.node_type != 0 {
            len += 1;
        }
        if self.node_outcome != 0 {
            len += 1;
        }
        if self.phase != 0 {
            len += 1;
        }
        if !self.relative_path.is_empty() {
            len += 1;
        }
        if self.defined_at_line.is_some() {
            len += 1;
        }
        if self.defined_at_col.is_some() {
            len += 1;
        }
        if !self.node_checksum.is_empty() {
            len += 1;
        }
        if self.sao_enabled.is_some() {
            len += 1;
        }
        if self.node_error_type.is_some() {
            len += 1;
        }
        if self.node_cancel_reason.is_some() {
            len += 1;
        }
        if self.node_skip_reason.is_some() {
            len += 1;
        }
        if self.dbt_core_event_code.is_some() {
            len += 1;
        }
        if self.rows_affected.is_some() {
            len += 1;
        }
        if self.idle_time_ms.is_some() {
            len += 1;
        }
        if self.state_decision_id.is_some() {
            len += 1;
        }
        if self.node_outcome_detail.is_some() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.node.NodeEvaluated", len)?;
        if !self.unique_id.is_empty() {
            struct_ser.serialize_field("unique_id", &self.unique_id)?;
        }
        if !self.name.is_empty() {
            struct_ser.serialize_field("name", &self.name)?;
        }
        if let Some(v) = self.database.as_ref() {
            struct_ser.serialize_field("database", v)?;
        }
        if let Some(v) = self.schema.as_ref() {
            struct_ser.serialize_field("schema", v)?;
        }
        if let Some(v) = self.identifier.as_ref() {
            struct_ser.serialize_field("identifier", v)?;
        }
        if let Some(v) = self.materialization.as_ref() {
            let v = NodeMaterialization::try_from(*v)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", *v)))?;
            struct_ser.serialize_field("materialization", &v)?;
        }
        if let Some(v) = self.custom_materialization.as_ref() {
            struct_ser.serialize_field("custom_materialization", v)?;
        }
        if self.node_type != 0 {
            let v = NodeType::try_from(self.node_type)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.node_type)))?;
            struct_ser.serialize_field("node_type", &v)?;
        }
        if self.node_outcome != 0 {
            let v = NodeOutcome::try_from(self.node_outcome)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.node_outcome)))?;
            struct_ser.serialize_field("node_outcome", &v)?;
        }
        if self.phase != 0 {
            let v = super::phase::ExecutionPhase::try_from(self.phase)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.phase)))?;
            struct_ser.serialize_field("phase", &v)?;
        }
        if !self.relative_path.is_empty() {
            struct_ser.serialize_field("relative_path", &self.relative_path)?;
        }
        if let Some(v) = self.defined_at_line.as_ref() {
            struct_ser.serialize_field("defined_at_line", v)?;
        }
        if let Some(v) = self.defined_at_col.as_ref() {
            struct_ser.serialize_field("defined_at_col", v)?;
        }
        if !self.node_checksum.is_empty() {
            struct_ser.serialize_field("node_checksum", &self.node_checksum)?;
        }
        if let Some(v) = self.sao_enabled.as_ref() {
            struct_ser.serialize_field("sao_enabled", v)?;
        }
        if let Some(v) = self.node_error_type.as_ref() {
            let v = NodeErrorType::try_from(*v)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", *v)))?;
            struct_ser.serialize_field("node_error_type", &v)?;
        }
        if let Some(v) = self.node_cancel_reason.as_ref() {
            let v = NodeCancelReason::try_from(*v)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", *v)))?;
            struct_ser.serialize_field("node_cancel_reason", &v)?;
        }
        if let Some(v) = self.node_skip_reason.as_ref() {
            let v = NodeSkipReason::try_from(*v)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", *v)))?;
            struct_ser.serialize_field("node_skip_reason", &v)?;
        }
        if let Some(v) = self.dbt_core_event_code.as_ref() {
            struct_ser.serialize_field("dbt_core_event_code", v)?;
        }
        if let Some(v) = self.rows_affected.as_ref() {
            #[allow(clippy::needless_borrow)]
            #[allow(clippy::needless_borrows_for_generic_args)]
            struct_ser.serialize_field("rows_affected", ToString::to_string(&v).as_str())?;
        }
        if let Some(v) = self.idle_time_ms.as_ref() {
            #[allow(clippy::needless_borrow)]
            #[allow(clippy::needless_borrows_for_generic_args)]
            struct_ser.serialize_field("idle_time_ms", ToString::to_string(&v).as_str())?;
        }
        if let Some(v) = self.state_decision_id.as_ref() {
            struct_ser.serialize_field("state_decision_id", v)?;
        }
        if let Some(v) = self.node_outcome_detail.as_ref() {
            match v {
                node_evaluated::NodeOutcomeDetail::NodeCacheDetail(v) => {
                    struct_ser.serialize_field("node_cache_detail", v)?;
                }
                node_evaluated::NodeOutcomeDetail::NodeTestDetail(v) => {
                    struct_ser.serialize_field("node_test_detail", v)?;
                }
                node_evaluated::NodeOutcomeDetail::NodeFreshnessOutcome(v) => {
                    struct_ser.serialize_field("node_freshness_outcome", v)?;
                }
                node_evaluated::NodeOutcomeDetail::NodeSkipUpstreamDetail(v) => {
                    struct_ser.serialize_field("node_skip_upstream_detail", v)?;
                }
                node_evaluated::NodeOutcomeDetail::NodeEvaluationDetail(v) => {
                    struct_ser.serialize_field("node_evaluation_detail", v)?;
                }
            }
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for NodeEvaluated {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "unique_id",
            "uniqueId",
            "name",
            "database",
            "schema",
            "identifier",
            "materialization",
            "custom_materialization",
            "customMaterialization",
            "node_type",
            "nodeType",
            "node_outcome",
            "nodeOutcome",
            "phase",
            "relative_path",
            "relativePath",
            "defined_at_line",
            "definedAtLine",
            "defined_at_col",
            "definedAtCol",
            "node_checksum",
            "nodeChecksum",
            "sao_enabled",
            "saoEnabled",
            "node_error_type",
            "nodeErrorType",
            "node_cancel_reason",
            "nodeCancelReason",
            "node_skip_reason",
            "nodeSkipReason",
            "dbt_core_event_code",
            "dbtCoreEventCode",
            "rows_affected",
            "rowsAffected",
            "idle_time_ms",
            "idleTimeMs",
            "state_decision_id",
            "stateDecisionId",
            "node_cache_detail",
            "nodeCacheDetail",
            "node_test_detail",
            "nodeTestDetail",
            "node_freshness_outcome",
            "nodeFreshnessOutcome",
            "node_skip_upstream_detail",
            "nodeSkipUpstreamDetail",
            "node_evaluation_detail",
            "nodeEvaluationDetail",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            UniqueId,
            Name,
            Database,
            Schema,
            Identifier,
            Materialization,
            CustomMaterialization,
            NodeType,
            NodeOutcome,
            Phase,
            RelativePath,
            DefinedAtLine,
            DefinedAtCol,
            NodeChecksum,
            SaoEnabled,
            NodeErrorType,
            NodeCancelReason,
            NodeSkipReason,
            DbtCoreEventCode,
            RowsAffected,
            IdleTimeMs,
            StateDecisionId,
            NodeCacheDetail,
            NodeTestDetail,
            NodeFreshnessOutcome,
            NodeSkipUpstreamDetail,
            NodeEvaluationDetail,
            __SkipField__,
        }
        impl<'de> serde::Deserialize<'de> for GeneratedField {
            fn deserialize<D>(deserializer: D) -> std::result::Result<GeneratedField, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct GeneratedVisitor;

                impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
                    type Value = GeneratedField;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        write!(formatter, "expected one of: {:?}", &FIELDS)
                    }

                    #[allow(unused_variables)]
                    fn visit_str<E>(self, value: &str) -> std::result::Result<GeneratedField, E>
                    where
                        E: serde::de::Error,
                    {
                        match value {
                            "uniqueId" | "unique_id" => Ok(GeneratedField::UniqueId),
                            "name" => Ok(GeneratedField::Name),
                            "database" => Ok(GeneratedField::Database),
                            "schema" => Ok(GeneratedField::Schema),
                            "identifier" => Ok(GeneratedField::Identifier),
                            "materialization" => Ok(GeneratedField::Materialization),
                            "customMaterialization" | "custom_materialization" => Ok(GeneratedField::CustomMaterialization),
                            "nodeType" | "node_type" => Ok(GeneratedField::NodeType),
                            "nodeOutcome" | "node_outcome" => Ok(GeneratedField::NodeOutcome),
                            "phase" => Ok(GeneratedField::Phase),
                            "relativePath" | "relative_path" => Ok(GeneratedField::RelativePath),
                            "definedAtLine" | "defined_at_line" => Ok(GeneratedField::DefinedAtLine),
                            "definedAtCol" | "defined_at_col" => Ok(GeneratedField::DefinedAtCol),
                            "nodeChecksum" | "node_checksum" => Ok(GeneratedField::NodeChecksum),
                            "saoEnabled" | "sao_enabled" => Ok(GeneratedField::SaoEnabled),
                            "nodeErrorType" | "node_error_type" => Ok(GeneratedField::NodeErrorType),
                            "nodeCancelReason" | "node_cancel_reason" => Ok(GeneratedField::NodeCancelReason),
                            "nodeSkipReason" | "node_skip_reason" => Ok(GeneratedField::NodeSkipReason),
                            "dbtCoreEventCode" | "dbt_core_event_code" => Ok(GeneratedField::DbtCoreEventCode),
                            "rowsAffected" | "rows_affected" => Ok(GeneratedField::RowsAffected),
                            "idleTimeMs" | "idle_time_ms" => Ok(GeneratedField::IdleTimeMs),
                            "stateDecisionId" | "state_decision_id" => Ok(GeneratedField::StateDecisionId),
                            "nodeCacheDetail" | "node_cache_detail" => Ok(GeneratedField::NodeCacheDetail),
                            "nodeTestDetail" | "node_test_detail" => Ok(GeneratedField::NodeTestDetail),
                            "nodeFreshnessOutcome" | "node_freshness_outcome" => Ok(GeneratedField::NodeFreshnessOutcome),
                            "nodeSkipUpstreamDetail" | "node_skip_upstream_detail" => Ok(GeneratedField::NodeSkipUpstreamDetail),
                            "nodeEvaluationDetail" | "node_evaluation_detail" => Ok(GeneratedField::NodeEvaluationDetail),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = NodeEvaluated;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.node.NodeEvaluated")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<NodeEvaluated, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut unique_id__ = None;
                let mut name__ = None;
                let mut database__ = None;
                let mut schema__ = None;
                let mut identifier__ = None;
                let mut materialization__ = None;
                let mut custom_materialization__ = None;
                let mut node_type__ = None;
                let mut node_outcome__ = None;
                let mut phase__ = None;
                let mut relative_path__ = None;
                let mut defined_at_line__ = None;
                let mut defined_at_col__ = None;
                let mut node_checksum__ = None;
                let mut sao_enabled__ = None;
                let mut node_error_type__ = None;
                let mut node_cancel_reason__ = None;
                let mut node_skip_reason__ = None;
                let mut dbt_core_event_code__ = None;
                let mut rows_affected__ = None;
                let mut idle_time_ms__ = None;
                let mut state_decision_id__ = None;
                let mut node_outcome_detail__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::UniqueId => {
                            if unique_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("uniqueId"));
                            }
                            unique_id__ = Some(map_.next_value()?);
                        }
                        GeneratedField::Name => {
                            if name__.is_some() {
                                return Err(serde::de::Error::duplicate_field("name"));
                            }
                            name__ = Some(map_.next_value()?);
                        }
                        GeneratedField::Database => {
                            if database__.is_some() {
                                return Err(serde::de::Error::duplicate_field("database"));
                            }
                            database__ = map_.next_value()?;
                        }
                        GeneratedField::Schema => {
                            if schema__.is_some() {
                                return Err(serde::de::Error::duplicate_field("schema"));
                            }
                            schema__ = map_.next_value()?;
                        }
                        GeneratedField::Identifier => {
                            if identifier__.is_some() {
                                return Err(serde::de::Error::duplicate_field("identifier"));
                            }
                            identifier__ = map_.next_value()?;
                        }
                        GeneratedField::Materialization => {
                            if materialization__.is_some() {
                                return Err(serde::de::Error::duplicate_field("materialization"));
                            }
                            materialization__ = map_.next_value::<::std::option::Option<NodeMaterialization>>()?.map(|x| x as i32);
                        }
                        GeneratedField::CustomMaterialization => {
                            if custom_materialization__.is_some() {
                                return Err(serde::de::Error::duplicate_field("customMaterialization"));
                            }
                            custom_materialization__ = map_.next_value()?;
                        }
                        GeneratedField::NodeType => {
                            if node_type__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeType"));
                            }
                            node_type__ = Some(map_.next_value::<NodeType>()? as i32);
                        }
                        GeneratedField::NodeOutcome => {
                            if node_outcome__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeOutcome"));
                            }
                            node_outcome__ = Some(map_.next_value::<NodeOutcome>()? as i32);
                        }
                        GeneratedField::Phase => {
                            if phase__.is_some() {
                                return Err(serde::de::Error::duplicate_field("phase"));
                            }
                            phase__ = Some(map_.next_value::<super::phase::ExecutionPhase>()? as i32);
                        }
                        GeneratedField::RelativePath => {
                            if relative_path__.is_some() {
                                return Err(serde::de::Error::duplicate_field("relativePath"));
                            }
                            relative_path__ = Some(map_.next_value()?);
                        }
                        GeneratedField::DefinedAtLine => {
                            if defined_at_line__.is_some() {
                                return Err(serde::de::Error::duplicate_field("definedAtLine"));
                            }
                            defined_at_line__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::DefinedAtCol => {
                            if defined_at_col__.is_some() {
                                return Err(serde::de::Error::duplicate_field("definedAtCol"));
                            }
                            defined_at_col__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::NodeChecksum => {
                            if node_checksum__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeChecksum"));
                            }
                            node_checksum__ = Some(map_.next_value()?);
                        }
                        GeneratedField::SaoEnabled => {
                            if sao_enabled__.is_some() {
                                return Err(serde::de::Error::duplicate_field("saoEnabled"));
                            }
                            sao_enabled__ = map_.next_value()?;
                        }
                        GeneratedField::NodeErrorType => {
                            if node_error_type__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeErrorType"));
                            }
                            node_error_type__ = map_.next_value::<::std::option::Option<NodeErrorType>>()?.map(|x| x as i32);
                        }
                        GeneratedField::NodeCancelReason => {
                            if node_cancel_reason__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeCancelReason"));
                            }
                            node_cancel_reason__ = map_.next_value::<::std::option::Option<NodeCancelReason>>()?.map(|x| x as i32);
                        }
                        GeneratedField::NodeSkipReason => {
                            if node_skip_reason__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeSkipReason"));
                            }
                            node_skip_reason__ = map_.next_value::<::std::option::Option<NodeSkipReason>>()?.map(|x| x as i32);
                        }
                        GeneratedField::DbtCoreEventCode => {
                            if dbt_core_event_code__.is_some() {
                                return Err(serde::de::Error::duplicate_field("dbtCoreEventCode"));
                            }
                            dbt_core_event_code__ = map_.next_value()?;
                        }
                        GeneratedField::RowsAffected => {
                            if rows_affected__.is_some() {
                                return Err(serde::de::Error::duplicate_field("rowsAffected"));
                            }
                            rows_affected__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::IdleTimeMs => {
                            if idle_time_ms__.is_some() {
                                return Err(serde::de::Error::duplicate_field("idleTimeMs"));
                            }
                            idle_time_ms__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::StateDecisionId => {
                            if state_decision_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("stateDecisionId"));
                            }
                            state_decision_id__ = map_.next_value()?;
                        }
                        GeneratedField::NodeCacheDetail => {
                            if node_outcome_detail__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeCacheDetail"));
                            }
                            node_outcome_detail__ = map_.next_value::<::std::option::Option<_>>()?.map(node_evaluated::NodeOutcomeDetail::NodeCacheDetail)
;
                        }
                        GeneratedField::NodeTestDetail => {
                            if node_outcome_detail__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeTestDetail"));
                            }
                            node_outcome_detail__ = map_.next_value::<::std::option::Option<_>>()?.map(node_evaluated::NodeOutcomeDetail::NodeTestDetail)
;
                        }
                        GeneratedField::NodeFreshnessOutcome => {
                            if node_outcome_detail__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeFreshnessOutcome"));
                            }
                            node_outcome_detail__ = map_.next_value::<::std::option::Option<_>>()?.map(node_evaluated::NodeOutcomeDetail::NodeFreshnessOutcome)
;
                        }
                        GeneratedField::NodeSkipUpstreamDetail => {
                            if node_outcome_detail__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeSkipUpstreamDetail"));
                            }
                            node_outcome_detail__ = map_.next_value::<::std::option::Option<_>>()?.map(node_evaluated::NodeOutcomeDetail::NodeSkipUpstreamDetail)
;
                        }
                        GeneratedField::NodeEvaluationDetail => {
                            if node_outcome_detail__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeEvaluationDetail"));
                            }
                            node_outcome_detail__ = map_.next_value::<::std::option::Option<_>>()?.map(node_evaluated::NodeOutcomeDetail::NodeEvaluationDetail)
;
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(NodeEvaluated {
                    unique_id: unique_id__.unwrap_or_default(),
                    name: name__.unwrap_or_default(),
                    database: database__,
                    schema: schema__,
                    identifier: identifier__,
                    materialization: materialization__,
                    custom_materialization: custom_materialization__,
                    node_type: node_type__.unwrap_or_default(),
                    node_outcome: node_outcome__.unwrap_or_default(),
                    phase: phase__.unwrap_or_default(),
                    relative_path: relative_path__.unwrap_or_default(),
                    defined_at_line: defined_at_line__,
                    defined_at_col: defined_at_col__,
                    node_checksum: node_checksum__.unwrap_or_default(),
                    sao_enabled: sao_enabled__,
                    node_error_type: node_error_type__,
                    node_cancel_reason: node_cancel_reason__,
                    node_skip_reason: node_skip_reason__,
                    dbt_core_event_code: dbt_core_event_code__,
                    rows_affected: rows_affected__,
                    idle_time_ms: idle_time_ms__,
                    state_decision_id: state_decision_id__,
                    node_outcome_detail: node_outcome_detail__,
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.node.NodeEvaluated", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for NodeEvaluationDetail {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if self.node_warning_outcome != 0 {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.node.NodeEvaluationDetail", len)?;
        if self.node_warning_outcome != 0 {
            let v = NodeWarningOutcome::try_from(self.node_warning_outcome)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.node_warning_outcome)))?;
            struct_ser.serialize_field("node_warning_outcome", &v)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for NodeEvaluationDetail {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "node_warning_outcome",
            "nodeWarningOutcome",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            NodeWarningOutcome,
            __SkipField__,
        }
        impl<'de> serde::Deserialize<'de> for GeneratedField {
            fn deserialize<D>(deserializer: D) -> std::result::Result<GeneratedField, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct GeneratedVisitor;

                impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
                    type Value = GeneratedField;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        write!(formatter, "expected one of: {:?}", &FIELDS)
                    }

                    #[allow(unused_variables)]
                    fn visit_str<E>(self, value: &str) -> std::result::Result<GeneratedField, E>
                    where
                        E: serde::de::Error,
                    {
                        match value {
                            "nodeWarningOutcome" | "node_warning_outcome" => Ok(GeneratedField::NodeWarningOutcome),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = NodeEvaluationDetail;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.node.NodeEvaluationDetail")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<NodeEvaluationDetail, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut node_warning_outcome__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::NodeWarningOutcome => {
                            if node_warning_outcome__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeWarningOutcome"));
                            }
                            node_warning_outcome__ = Some(map_.next_value::<NodeWarningOutcome>()? as i32);
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(NodeEvaluationDetail {
                    node_warning_outcome: node_warning_outcome__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.node.NodeEvaluationDetail", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for NodeMaterialization {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::Unknown => "NODE_MATERIALIZATION_UNKNOWN",
            Self::Snapshot => "NODE_MATERIALIZATION_SNAPSHOT",
            Self::Seed => "NODE_MATERIALIZATION_SEED",
            Self::View => "NODE_MATERIALIZATION_VIEW",
            Self::Table => "NODE_MATERIALIZATION_TABLE",
            Self::Incremental => "NODE_MATERIALIZATION_INCREMENTAL",
            Self::MaterializedView => "NODE_MATERIALIZATION_MATERIALIZED_VIEW",
            Self::External => "NODE_MATERIALIZATION_EXTERNAL",
            Self::Test => "NODE_MATERIALIZATION_TEST",
            Self::Ephemeral => "NODE_MATERIALIZATION_EPHEMERAL",
            Self::Unit => "NODE_MATERIALIZATION_UNIT",
            Self::Analysis => "NODE_MATERIALIZATION_ANALYSIS",
            Self::StreamingTable => "NODE_MATERIALIZATION_STREAMING_TABLE",
            Self::DynamicTable => "NODE_MATERIALIZATION_DYNAMIC_TABLE",
            Self::Function => "NODE_MATERIALIZATION_FUNCTION",
            Self::InteractiveTable => "NODE_MATERIALIZATION_INTERACTIVE_TABLE",
            Self::MetricView => "NODE_MATERIALIZATION_METRIC_VIEW",
            Self::Custom => "NODE_MATERIALIZATION_CUSTOM",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for NodeMaterialization {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "NODE_MATERIALIZATION_UNKNOWN",
            "NODE_MATERIALIZATION_SNAPSHOT",
            "NODE_MATERIALIZATION_SEED",
            "NODE_MATERIALIZATION_VIEW",
            "NODE_MATERIALIZATION_TABLE",
            "NODE_MATERIALIZATION_INCREMENTAL",
            "NODE_MATERIALIZATION_MATERIALIZED_VIEW",
            "NODE_MATERIALIZATION_EXTERNAL",
            "NODE_MATERIALIZATION_TEST",
            "NODE_MATERIALIZATION_EPHEMERAL",
            "NODE_MATERIALIZATION_UNIT",
            "NODE_MATERIALIZATION_ANALYSIS",
            "NODE_MATERIALIZATION_STREAMING_TABLE",
            "NODE_MATERIALIZATION_DYNAMIC_TABLE",
            "NODE_MATERIALIZATION_FUNCTION",
            "NODE_MATERIALIZATION_INTERACTIVE_TABLE",
            "NODE_MATERIALIZATION_METRIC_VIEW",
            "NODE_MATERIALIZATION_CUSTOM",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = NodeMaterialization;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "expected one of: {:?}", &FIELDS)
            }

            fn visit_i64<E>(self, v: i64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Signed(v), &self)
                    })
            }

            fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Unsigned(v), &self)
                    })
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "NODE_MATERIALIZATION_UNKNOWN" => Ok(NodeMaterialization::Unknown),
                    "NODE_MATERIALIZATION_SNAPSHOT" => Ok(NodeMaterialization::Snapshot),
                    "NODE_MATERIALIZATION_SEED" => Ok(NodeMaterialization::Seed),
                    "NODE_MATERIALIZATION_VIEW" => Ok(NodeMaterialization::View),
                    "NODE_MATERIALIZATION_TABLE" => Ok(NodeMaterialization::Table),
                    "NODE_MATERIALIZATION_INCREMENTAL" => Ok(NodeMaterialization::Incremental),
                    "NODE_MATERIALIZATION_MATERIALIZED_VIEW" => Ok(NodeMaterialization::MaterializedView),
                    "NODE_MATERIALIZATION_EXTERNAL" => Ok(NodeMaterialization::External),
                    "NODE_MATERIALIZATION_TEST" => Ok(NodeMaterialization::Test),
                    "NODE_MATERIALIZATION_EPHEMERAL" => Ok(NodeMaterialization::Ephemeral),
                    "NODE_MATERIALIZATION_UNIT" => Ok(NodeMaterialization::Unit),
                    "NODE_MATERIALIZATION_ANALYSIS" => Ok(NodeMaterialization::Analysis),
                    "NODE_MATERIALIZATION_STREAMING_TABLE" => Ok(NodeMaterialization::StreamingTable),
                    "NODE_MATERIALIZATION_DYNAMIC_TABLE" => Ok(NodeMaterialization::DynamicTable),
                    "NODE_MATERIALIZATION_FUNCTION" => Ok(NodeMaterialization::Function),
                    "NODE_MATERIALIZATION_INTERACTIVE_TABLE" => Ok(NodeMaterialization::InteractiveTable),
                    "NODE_MATERIALIZATION_METRIC_VIEW" => Ok(NodeMaterialization::MetricView),
                    "NODE_MATERIALIZATION_CUSTOM" => Ok(NodeMaterialization::Custom),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
impl serde::Serialize for NodeOutcome {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::Unspecified => "NODE_OUTCOME_UNSPECIFIED",
            Self::Success => "NODE_OUTCOME_SUCCESS",
            Self::Error => "NODE_OUTCOME_ERROR",
            Self::Canceled => "NODE_OUTCOME_CANCELED",
            Self::Skipped => "NODE_OUTCOME_SKIPPED",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for NodeOutcome {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "NODE_OUTCOME_UNSPECIFIED",
            "NODE_OUTCOME_SUCCESS",
            "NODE_OUTCOME_ERROR",
            "NODE_OUTCOME_CANCELED",
            "NODE_OUTCOME_SKIPPED",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = NodeOutcome;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "expected one of: {:?}", &FIELDS)
            }

            fn visit_i64<E>(self, v: i64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Signed(v), &self)
                    })
            }

            fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Unsigned(v), &self)
                    })
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "NODE_OUTCOME_UNSPECIFIED" => Ok(NodeOutcome::Unspecified),
                    "NODE_OUTCOME_SUCCESS" => Ok(NodeOutcome::Success),
                    "NODE_OUTCOME_ERROR" => Ok(NodeOutcome::Error),
                    "NODE_OUTCOME_CANCELED" => Ok(NodeOutcome::Canceled),
                    "NODE_OUTCOME_SKIPPED" => Ok(NodeOutcome::Skipped),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
impl serde::Serialize for NodeProcessed {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if !self.unique_id.is_empty() {
            len += 1;
        }
        if !self.name.is_empty() {
            len += 1;
        }
        if self.database.is_some() {
            len += 1;
        }
        if self.schema.is_some() {
            len += 1;
        }
        if self.identifier.is_some() {
            len += 1;
        }
        if self.source_name.is_some() {
            len += 1;
        }
        if self.materialization.is_some() {
            len += 1;
        }
        if self.custom_materialization.is_some() {
            len += 1;
        }
        if self.node_type != 0 {
            len += 1;
        }
        if self.node_outcome != 0 {
            len += 1;
        }
        if self.last_phase != 0 {
            len += 1;
        }
        if !self.relative_path.is_empty() {
            len += 1;
        }
        if self.defined_at_line.is_some() {
            len += 1;
        }
        if self.defined_at_col.is_some() {
            len += 1;
        }
        if !self.node_checksum.is_empty() {
            len += 1;
        }
        if self.sao_enabled.is_some() {
            len += 1;
        }
        if self.node_error_type.is_some() {
            len += 1;
        }
        if self.node_cancel_reason.is_some() {
            len += 1;
        }
        if self.node_skip_reason.is_some() {
            len += 1;
        }
        if !self.dbt_core_event_code.is_empty() {
            len += 1;
        }
        if self.duration_ms.is_some() {
            len += 1;
        }
        if self.in_selection {
            len += 1;
        }
        if self.rows_affected.is_some() {
            len += 1;
        }
        if self.group.is_some() {
            len += 1;
        }
        if self.idle_time_ms.is_some() {
            len += 1;
        }
        if self.node_index.is_some() {
            len += 1;
        }
        if self.node_count_total.is_some() {
            len += 1;
        }
        if self.node_outcome_detail.is_some() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.node.NodeProcessed", len)?;
        if !self.unique_id.is_empty() {
            struct_ser.serialize_field("unique_id", &self.unique_id)?;
        }
        if !self.name.is_empty() {
            struct_ser.serialize_field("name", &self.name)?;
        }
        if let Some(v) = self.database.as_ref() {
            struct_ser.serialize_field("database", v)?;
        }
        if let Some(v) = self.schema.as_ref() {
            struct_ser.serialize_field("schema", v)?;
        }
        if let Some(v) = self.identifier.as_ref() {
            struct_ser.serialize_field("identifier", v)?;
        }
        if let Some(v) = self.source_name.as_ref() {
            struct_ser.serialize_field("source_name", v)?;
        }
        if let Some(v) = self.materialization.as_ref() {
            let v = NodeMaterialization::try_from(*v)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", *v)))?;
            struct_ser.serialize_field("materialization", &v)?;
        }
        if let Some(v) = self.custom_materialization.as_ref() {
            struct_ser.serialize_field("custom_materialization", v)?;
        }
        if self.node_type != 0 {
            let v = NodeType::try_from(self.node_type)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.node_type)))?;
            struct_ser.serialize_field("node_type", &v)?;
        }
        if self.node_outcome != 0 {
            let v = NodeOutcome::try_from(self.node_outcome)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.node_outcome)))?;
            struct_ser.serialize_field("node_outcome", &v)?;
        }
        if self.last_phase != 0 {
            let v = super::phase::ExecutionPhase::try_from(self.last_phase)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.last_phase)))?;
            struct_ser.serialize_field("last_phase", &v)?;
        }
        if !self.relative_path.is_empty() {
            struct_ser.serialize_field("relative_path", &self.relative_path)?;
        }
        if let Some(v) = self.defined_at_line.as_ref() {
            struct_ser.serialize_field("defined_at_line", v)?;
        }
        if let Some(v) = self.defined_at_col.as_ref() {
            struct_ser.serialize_field("defined_at_col", v)?;
        }
        if !self.node_checksum.is_empty() {
            struct_ser.serialize_field("node_checksum", &self.node_checksum)?;
        }
        if let Some(v) = self.sao_enabled.as_ref() {
            struct_ser.serialize_field("sao_enabled", v)?;
        }
        if let Some(v) = self.node_error_type.as_ref() {
            let v = NodeErrorType::try_from(*v)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", *v)))?;
            struct_ser.serialize_field("node_error_type", &v)?;
        }
        if let Some(v) = self.node_cancel_reason.as_ref() {
            let v = NodeCancelReason::try_from(*v)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", *v)))?;
            struct_ser.serialize_field("node_cancel_reason", &v)?;
        }
        if let Some(v) = self.node_skip_reason.as_ref() {
            let v = NodeSkipReason::try_from(*v)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", *v)))?;
            struct_ser.serialize_field("node_skip_reason", &v)?;
        }
        if !self.dbt_core_event_code.is_empty() {
            struct_ser.serialize_field("dbt_core_event_code", &self.dbt_core_event_code)?;
        }
        if let Some(v) = self.duration_ms.as_ref() {
            #[allow(clippy::needless_borrow)]
            #[allow(clippy::needless_borrows_for_generic_args)]
            struct_ser.serialize_field("duration_ms", ToString::to_string(&v).as_str())?;
        }
        if self.in_selection {
            struct_ser.serialize_field("in_selection", &self.in_selection)?;
        }
        if let Some(v) = self.rows_affected.as_ref() {
            #[allow(clippy::needless_borrow)]
            #[allow(clippy::needless_borrows_for_generic_args)]
            struct_ser.serialize_field("rows_affected", ToString::to_string(&v).as_str())?;
        }
        if let Some(v) = self.group.as_ref() {
            struct_ser.serialize_field("group", v)?;
        }
        if let Some(v) = self.idle_time_ms.as_ref() {
            #[allow(clippy::needless_borrow)]
            #[allow(clippy::needless_borrows_for_generic_args)]
            struct_ser.serialize_field("idle_time_ms", ToString::to_string(&v).as_str())?;
        }
        if let Some(v) = self.node_index.as_ref() {
            struct_ser.serialize_field("node_index", v)?;
        }
        if let Some(v) = self.node_count_total.as_ref() {
            struct_ser.serialize_field("node_count_total", v)?;
        }
        if let Some(v) = self.node_outcome_detail.as_ref() {
            match v {
                node_processed::NodeOutcomeDetail::NodeCacheDetail(v) => {
                    struct_ser.serialize_field("node_cache_detail", v)?;
                }
                node_processed::NodeOutcomeDetail::NodeTestDetail(v) => {
                    struct_ser.serialize_field("node_test_detail", v)?;
                }
                node_processed::NodeOutcomeDetail::NodeFreshnessOutcome(v) => {
                    struct_ser.serialize_field("node_freshness_outcome", v)?;
                }
                node_processed::NodeOutcomeDetail::NodeSkipUpstreamDetail(v) => {
                    struct_ser.serialize_field("node_skip_upstream_detail", v)?;
                }
                node_processed::NodeOutcomeDetail::NodeEvaluationDetail(v) => {
                    struct_ser.serialize_field("node_evaluation_detail", v)?;
                }
            }
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for NodeProcessed {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "unique_id",
            "uniqueId",
            "name",
            "database",
            "schema",
            "identifier",
            "source_name",
            "sourceName",
            "materialization",
            "custom_materialization",
            "customMaterialization",
            "node_type",
            "nodeType",
            "node_outcome",
            "nodeOutcome",
            "last_phase",
            "lastPhase",
            "relative_path",
            "relativePath",
            "defined_at_line",
            "definedAtLine",
            "defined_at_col",
            "definedAtCol",
            "node_checksum",
            "nodeChecksum",
            "sao_enabled",
            "saoEnabled",
            "node_error_type",
            "nodeErrorType",
            "node_cancel_reason",
            "nodeCancelReason",
            "node_skip_reason",
            "nodeSkipReason",
            "dbt_core_event_code",
            "dbtCoreEventCode",
            "duration_ms",
            "durationMs",
            "in_selection",
            "inSelection",
            "rows_affected",
            "rowsAffected",
            "group",
            "idle_time_ms",
            "idleTimeMs",
            "node_index",
            "nodeIndex",
            "node_count_total",
            "nodeCountTotal",
            "node_cache_detail",
            "nodeCacheDetail",
            "node_test_detail",
            "nodeTestDetail",
            "node_freshness_outcome",
            "nodeFreshnessOutcome",
            "node_skip_upstream_detail",
            "nodeSkipUpstreamDetail",
            "node_evaluation_detail",
            "nodeEvaluationDetail",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            UniqueId,
            Name,
            Database,
            Schema,
            Identifier,
            SourceName,
            Materialization,
            CustomMaterialization,
            NodeType,
            NodeOutcome,
            LastPhase,
            RelativePath,
            DefinedAtLine,
            DefinedAtCol,
            NodeChecksum,
            SaoEnabled,
            NodeErrorType,
            NodeCancelReason,
            NodeSkipReason,
            DbtCoreEventCode,
            DurationMs,
            InSelection,
            RowsAffected,
            Group,
            IdleTimeMs,
            NodeIndex,
            NodeCountTotal,
            NodeCacheDetail,
            NodeTestDetail,
            NodeFreshnessOutcome,
            NodeSkipUpstreamDetail,
            NodeEvaluationDetail,
            __SkipField__,
        }
        impl<'de> serde::Deserialize<'de> for GeneratedField {
            fn deserialize<D>(deserializer: D) -> std::result::Result<GeneratedField, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct GeneratedVisitor;

                impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
                    type Value = GeneratedField;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        write!(formatter, "expected one of: {:?}", &FIELDS)
                    }

                    #[allow(unused_variables)]
                    fn visit_str<E>(self, value: &str) -> std::result::Result<GeneratedField, E>
                    where
                        E: serde::de::Error,
                    {
                        match value {
                            "uniqueId" | "unique_id" => Ok(GeneratedField::UniqueId),
                            "name" => Ok(GeneratedField::Name),
                            "database" => Ok(GeneratedField::Database),
                            "schema" => Ok(GeneratedField::Schema),
                            "identifier" => Ok(GeneratedField::Identifier),
                            "sourceName" | "source_name" => Ok(GeneratedField::SourceName),
                            "materialization" => Ok(GeneratedField::Materialization),
                            "customMaterialization" | "custom_materialization" => Ok(GeneratedField::CustomMaterialization),
                            "nodeType" | "node_type" => Ok(GeneratedField::NodeType),
                            "nodeOutcome" | "node_outcome" => Ok(GeneratedField::NodeOutcome),
                            "lastPhase" | "last_phase" => Ok(GeneratedField::LastPhase),
                            "relativePath" | "relative_path" => Ok(GeneratedField::RelativePath),
                            "definedAtLine" | "defined_at_line" => Ok(GeneratedField::DefinedAtLine),
                            "definedAtCol" | "defined_at_col" => Ok(GeneratedField::DefinedAtCol),
                            "nodeChecksum" | "node_checksum" => Ok(GeneratedField::NodeChecksum),
                            "saoEnabled" | "sao_enabled" => Ok(GeneratedField::SaoEnabled),
                            "nodeErrorType" | "node_error_type" => Ok(GeneratedField::NodeErrorType),
                            "nodeCancelReason" | "node_cancel_reason" => Ok(GeneratedField::NodeCancelReason),
                            "nodeSkipReason" | "node_skip_reason" => Ok(GeneratedField::NodeSkipReason),
                            "dbtCoreEventCode" | "dbt_core_event_code" => Ok(GeneratedField::DbtCoreEventCode),
                            "durationMs" | "duration_ms" => Ok(GeneratedField::DurationMs),
                            "inSelection" | "in_selection" => Ok(GeneratedField::InSelection),
                            "rowsAffected" | "rows_affected" => Ok(GeneratedField::RowsAffected),
                            "group" => Ok(GeneratedField::Group),
                            "idleTimeMs" | "idle_time_ms" => Ok(GeneratedField::IdleTimeMs),
                            "nodeIndex" | "node_index" => Ok(GeneratedField::NodeIndex),
                            "nodeCountTotal" | "node_count_total" => Ok(GeneratedField::NodeCountTotal),
                            "nodeCacheDetail" | "node_cache_detail" => Ok(GeneratedField::NodeCacheDetail),
                            "nodeTestDetail" | "node_test_detail" => Ok(GeneratedField::NodeTestDetail),
                            "nodeFreshnessOutcome" | "node_freshness_outcome" => Ok(GeneratedField::NodeFreshnessOutcome),
                            "nodeSkipUpstreamDetail" | "node_skip_upstream_detail" => Ok(GeneratedField::NodeSkipUpstreamDetail),
                            "nodeEvaluationDetail" | "node_evaluation_detail" => Ok(GeneratedField::NodeEvaluationDetail),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = NodeProcessed;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.node.NodeProcessed")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<NodeProcessed, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut unique_id__ = None;
                let mut name__ = None;
                let mut database__ = None;
                let mut schema__ = None;
                let mut identifier__ = None;
                let mut source_name__ = None;
                let mut materialization__ = None;
                let mut custom_materialization__ = None;
                let mut node_type__ = None;
                let mut node_outcome__ = None;
                let mut last_phase__ = None;
                let mut relative_path__ = None;
                let mut defined_at_line__ = None;
                let mut defined_at_col__ = None;
                let mut node_checksum__ = None;
                let mut sao_enabled__ = None;
                let mut node_error_type__ = None;
                let mut node_cancel_reason__ = None;
                let mut node_skip_reason__ = None;
                let mut dbt_core_event_code__ = None;
                let mut duration_ms__ = None;
                let mut in_selection__ = None;
                let mut rows_affected__ = None;
                let mut group__ = None;
                let mut idle_time_ms__ = None;
                let mut node_index__ = None;
                let mut node_count_total__ = None;
                let mut node_outcome_detail__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::UniqueId => {
                            if unique_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("uniqueId"));
                            }
                            unique_id__ = Some(map_.next_value()?);
                        }
                        GeneratedField::Name => {
                            if name__.is_some() {
                                return Err(serde::de::Error::duplicate_field("name"));
                            }
                            name__ = Some(map_.next_value()?);
                        }
                        GeneratedField::Database => {
                            if database__.is_some() {
                                return Err(serde::de::Error::duplicate_field("database"));
                            }
                            database__ = map_.next_value()?;
                        }
                        GeneratedField::Schema => {
                            if schema__.is_some() {
                                return Err(serde::de::Error::duplicate_field("schema"));
                            }
                            schema__ = map_.next_value()?;
                        }
                        GeneratedField::Identifier => {
                            if identifier__.is_some() {
                                return Err(serde::de::Error::duplicate_field("identifier"));
                            }
                            identifier__ = map_.next_value()?;
                        }
                        GeneratedField::SourceName => {
                            if source_name__.is_some() {
                                return Err(serde::de::Error::duplicate_field("sourceName"));
                            }
                            source_name__ = map_.next_value()?;
                        }
                        GeneratedField::Materialization => {
                            if materialization__.is_some() {
                                return Err(serde::de::Error::duplicate_field("materialization"));
                            }
                            materialization__ = map_.next_value::<::std::option::Option<NodeMaterialization>>()?.map(|x| x as i32);
                        }
                        GeneratedField::CustomMaterialization => {
                            if custom_materialization__.is_some() {
                                return Err(serde::de::Error::duplicate_field("customMaterialization"));
                            }
                            custom_materialization__ = map_.next_value()?;
                        }
                        GeneratedField::NodeType => {
                            if node_type__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeType"));
                            }
                            node_type__ = Some(map_.next_value::<NodeType>()? as i32);
                        }
                        GeneratedField::NodeOutcome => {
                            if node_outcome__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeOutcome"));
                            }
                            node_outcome__ = Some(map_.next_value::<NodeOutcome>()? as i32);
                        }
                        GeneratedField::LastPhase => {
                            if last_phase__.is_some() {
                                return Err(serde::de::Error::duplicate_field("lastPhase"));
                            }
                            last_phase__ = Some(map_.next_value::<super::phase::ExecutionPhase>()? as i32);
                        }
                        GeneratedField::RelativePath => {
                            if relative_path__.is_some() {
                                return Err(serde::de::Error::duplicate_field("relativePath"));
                            }
                            relative_path__ = Some(map_.next_value()?);
                        }
                        GeneratedField::DefinedAtLine => {
                            if defined_at_line__.is_some() {
                                return Err(serde::de::Error::duplicate_field("definedAtLine"));
                            }
                            defined_at_line__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::DefinedAtCol => {
                            if defined_at_col__.is_some() {
                                return Err(serde::de::Error::duplicate_field("definedAtCol"));
                            }
                            defined_at_col__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::NodeChecksum => {
                            if node_checksum__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeChecksum"));
                            }
                            node_checksum__ = Some(map_.next_value()?);
                        }
                        GeneratedField::SaoEnabled => {
                            if sao_enabled__.is_some() {
                                return Err(serde::de::Error::duplicate_field("saoEnabled"));
                            }
                            sao_enabled__ = map_.next_value()?;
                        }
                        GeneratedField::NodeErrorType => {
                            if node_error_type__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeErrorType"));
                            }
                            node_error_type__ = map_.next_value::<::std::option::Option<NodeErrorType>>()?.map(|x| x as i32);
                        }
                        GeneratedField::NodeCancelReason => {
                            if node_cancel_reason__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeCancelReason"));
                            }
                            node_cancel_reason__ = map_.next_value::<::std::option::Option<NodeCancelReason>>()?.map(|x| x as i32);
                        }
                        GeneratedField::NodeSkipReason => {
                            if node_skip_reason__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeSkipReason"));
                            }
                            node_skip_reason__ = map_.next_value::<::std::option::Option<NodeSkipReason>>()?.map(|x| x as i32);
                        }
                        GeneratedField::DbtCoreEventCode => {
                            if dbt_core_event_code__.is_some() {
                                return Err(serde::de::Error::duplicate_field("dbtCoreEventCode"));
                            }
                            dbt_core_event_code__ = Some(map_.next_value()?);
                        }
                        GeneratedField::DurationMs => {
                            if duration_ms__.is_some() {
                                return Err(serde::de::Error::duplicate_field("durationMs"));
                            }
                            duration_ms__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::InSelection => {
                            if in_selection__.is_some() {
                                return Err(serde::de::Error::duplicate_field("inSelection"));
                            }
                            in_selection__ = Some(map_.next_value()?);
                        }
                        GeneratedField::RowsAffected => {
                            if rows_affected__.is_some() {
                                return Err(serde::de::Error::duplicate_field("rowsAffected"));
                            }
                            rows_affected__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::Group => {
                            if group__.is_some() {
                                return Err(serde::de::Error::duplicate_field("group"));
                            }
                            group__ = map_.next_value()?;
                        }
                        GeneratedField::IdleTimeMs => {
                            if idle_time_ms__.is_some() {
                                return Err(serde::de::Error::duplicate_field("idleTimeMs"));
                            }
                            idle_time_ms__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::NodeIndex => {
                            if node_index__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeIndex"));
                            }
                            node_index__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::NodeCountTotal => {
                            if node_count_total__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeCountTotal"));
                            }
                            node_count_total__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::NodeCacheDetail => {
                            if node_outcome_detail__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeCacheDetail"));
                            }
                            node_outcome_detail__ = map_.next_value::<::std::option::Option<_>>()?.map(node_processed::NodeOutcomeDetail::NodeCacheDetail)
;
                        }
                        GeneratedField::NodeTestDetail => {
                            if node_outcome_detail__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeTestDetail"));
                            }
                            node_outcome_detail__ = map_.next_value::<::std::option::Option<_>>()?.map(node_processed::NodeOutcomeDetail::NodeTestDetail)
;
                        }
                        GeneratedField::NodeFreshnessOutcome => {
                            if node_outcome_detail__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeFreshnessOutcome"));
                            }
                            node_outcome_detail__ = map_.next_value::<::std::option::Option<_>>()?.map(node_processed::NodeOutcomeDetail::NodeFreshnessOutcome)
;
                        }
                        GeneratedField::NodeSkipUpstreamDetail => {
                            if node_outcome_detail__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeSkipUpstreamDetail"));
                            }
                            node_outcome_detail__ = map_.next_value::<::std::option::Option<_>>()?.map(node_processed::NodeOutcomeDetail::NodeSkipUpstreamDetail)
;
                        }
                        GeneratedField::NodeEvaluationDetail => {
                            if node_outcome_detail__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeEvaluationDetail"));
                            }
                            node_outcome_detail__ = map_.next_value::<::std::option::Option<_>>()?.map(node_processed::NodeOutcomeDetail::NodeEvaluationDetail)
;
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(NodeProcessed {
                    unique_id: unique_id__.unwrap_or_default(),
                    name: name__.unwrap_or_default(),
                    database: database__,
                    schema: schema__,
                    identifier: identifier__,
                    source_name: source_name__,
                    materialization: materialization__,
                    custom_materialization: custom_materialization__,
                    node_type: node_type__.unwrap_or_default(),
                    node_outcome: node_outcome__.unwrap_or_default(),
                    last_phase: last_phase__.unwrap_or_default(),
                    relative_path: relative_path__.unwrap_or_default(),
                    defined_at_line: defined_at_line__,
                    defined_at_col: defined_at_col__,
                    node_checksum: node_checksum__.unwrap_or_default(),
                    sao_enabled: sao_enabled__,
                    node_error_type: node_error_type__,
                    node_cancel_reason: node_cancel_reason__,
                    node_skip_reason: node_skip_reason__,
                    dbt_core_event_code: dbt_core_event_code__.unwrap_or_default(),
                    duration_ms: duration_ms__,
                    in_selection: in_selection__.unwrap_or_default(),
                    rows_affected: rows_affected__,
                    group: group__,
                    idle_time_ms: idle_time_ms__,
                    node_index: node_index__,
                    node_count_total: node_count_total__,
                    node_outcome_detail: node_outcome_detail__,
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.node.NodeProcessed", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for NodeSkipReason {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::Unspecified => "NODE_SKIP_REASON_UNSPECIFIED",
            Self::Upstream => "NODE_SKIP_REASON_UPSTREAM",
            Self::Cached => "NODE_SKIP_REASON_CACHED",
            Self::PhaseDisabled => "NODE_SKIP_REASON_PHASE_DISABLED",
            Self::NoOp => "NODE_SKIP_REASON_NO_OP",
            Self::PhaseSkipped => "NODE_SKIP_REASON_PHASE_SKIPPED",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for NodeSkipReason {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "NODE_SKIP_REASON_UNSPECIFIED",
            "NODE_SKIP_REASON_UPSTREAM",
            "NODE_SKIP_REASON_CACHED",
            "NODE_SKIP_REASON_PHASE_DISABLED",
            "NODE_SKIP_REASON_NO_OP",
            "NODE_SKIP_REASON_PHASE_SKIPPED",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = NodeSkipReason;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "expected one of: {:?}", &FIELDS)
            }

            fn visit_i64<E>(self, v: i64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Signed(v), &self)
                    })
            }

            fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Unsigned(v), &self)
                    })
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "NODE_SKIP_REASON_UNSPECIFIED" => Ok(NodeSkipReason::Unspecified),
                    "NODE_SKIP_REASON_UPSTREAM" => Ok(NodeSkipReason::Upstream),
                    "NODE_SKIP_REASON_CACHED" => Ok(NodeSkipReason::Cached),
                    "NODE_SKIP_REASON_PHASE_DISABLED" => Ok(NodeSkipReason::PhaseDisabled),
                    "NODE_SKIP_REASON_NO_OP" => Ok(NodeSkipReason::NoOp),
                    "NODE_SKIP_REASON_PHASE_SKIPPED" => Ok(NodeSkipReason::PhaseSkipped),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
impl serde::Serialize for NodeSkipUpstreamDetail {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if !self.upstream_unique_id.is_empty() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.node.NodeSkipUpstreamDetail", len)?;
        if !self.upstream_unique_id.is_empty() {
            struct_ser.serialize_field("upstream_unique_id", &self.upstream_unique_id)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for NodeSkipUpstreamDetail {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "upstream_unique_id",
            "upstreamUniqueId",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            UpstreamUniqueId,
            __SkipField__,
        }
        impl<'de> serde::Deserialize<'de> for GeneratedField {
            fn deserialize<D>(deserializer: D) -> std::result::Result<GeneratedField, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct GeneratedVisitor;

                impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
                    type Value = GeneratedField;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        write!(formatter, "expected one of: {:?}", &FIELDS)
                    }

                    #[allow(unused_variables)]
                    fn visit_str<E>(self, value: &str) -> std::result::Result<GeneratedField, E>
                    where
                        E: serde::de::Error,
                    {
                        match value {
                            "upstreamUniqueId" | "upstream_unique_id" => Ok(GeneratedField::UpstreamUniqueId),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = NodeSkipUpstreamDetail;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.node.NodeSkipUpstreamDetail")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<NodeSkipUpstreamDetail, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut upstream_unique_id__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::UpstreamUniqueId => {
                            if upstream_unique_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("upstreamUniqueId"));
                            }
                            upstream_unique_id__ = Some(map_.next_value()?);
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(NodeSkipUpstreamDetail {
                    upstream_unique_id: upstream_unique_id__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.node.NodeSkipUpstreamDetail", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for NodeType {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::Unspecified => "NODE_TYPE_UNSPECIFIED",
            Self::Model => "NODE_TYPE_MODEL",
            Self::Seed => "NODE_TYPE_SEED",
            Self::Snapshot => "NODE_TYPE_SNAPSHOT",
            Self::Source => "NODE_TYPE_SOURCE",
            Self::Test => "NODE_TYPE_TEST",
            Self::UnitTest => "NODE_TYPE_UNIT_TEST",
            Self::Macro => "NODE_TYPE_MACRO",
            Self::DocsMacro => "NODE_TYPE_DOCS_MACRO",
            Self::Analysis => "NODE_TYPE_ANALYSIS",
            Self::Operation => "NODE_TYPE_OPERATION",
            Self::Exposure => "NODE_TYPE_EXPOSURE",
            Self::Metric => "NODE_TYPE_METRIC",
            Self::SavedQuery => "NODE_TYPE_SAVED_QUERY",
            Self::SemanticModel => "NODE_TYPE_SEMANTIC_MODEL",
            Self::Function => "NODE_TYPE_FUNCTION",
            Self::Check => "NODE_TYPE_CHECK",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for NodeType {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "NODE_TYPE_UNSPECIFIED",
            "NODE_TYPE_MODEL",
            "NODE_TYPE_SEED",
            "NODE_TYPE_SNAPSHOT",
            "NODE_TYPE_SOURCE",
            "NODE_TYPE_TEST",
            "NODE_TYPE_UNIT_TEST",
            "NODE_TYPE_MACRO",
            "NODE_TYPE_DOCS_MACRO",
            "NODE_TYPE_ANALYSIS",
            "NODE_TYPE_OPERATION",
            "NODE_TYPE_EXPOSURE",
            "NODE_TYPE_METRIC",
            "NODE_TYPE_SAVED_QUERY",
            "NODE_TYPE_SEMANTIC_MODEL",
            "NODE_TYPE_FUNCTION",
            "NODE_TYPE_CHECK",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = NodeType;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "expected one of: {:?}", &FIELDS)
            }

            fn visit_i64<E>(self, v: i64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Signed(v), &self)
                    })
            }

            fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Unsigned(v), &self)
                    })
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "NODE_TYPE_UNSPECIFIED" => Ok(NodeType::Unspecified),
                    "NODE_TYPE_MODEL" => Ok(NodeType::Model),
                    "NODE_TYPE_SEED" => Ok(NodeType::Seed),
                    "NODE_TYPE_SNAPSHOT" => Ok(NodeType::Snapshot),
                    "NODE_TYPE_SOURCE" => Ok(NodeType::Source),
                    "NODE_TYPE_TEST" => Ok(NodeType::Test),
                    "NODE_TYPE_UNIT_TEST" => Ok(NodeType::UnitTest),
                    "NODE_TYPE_MACRO" => Ok(NodeType::Macro),
                    "NODE_TYPE_DOCS_MACRO" => Ok(NodeType::DocsMacro),
                    "NODE_TYPE_ANALYSIS" => Ok(NodeType::Analysis),
                    "NODE_TYPE_OPERATION" => Ok(NodeType::Operation),
                    "NODE_TYPE_EXPOSURE" => Ok(NodeType::Exposure),
                    "NODE_TYPE_METRIC" => Ok(NodeType::Metric),
                    "NODE_TYPE_SAVED_QUERY" => Ok(NodeType::SavedQuery),
                    "NODE_TYPE_SEMANTIC_MODEL" => Ok(NodeType::SemanticModel),
                    "NODE_TYPE_FUNCTION" => Ok(NodeType::Function),
                    "NODE_TYPE_CHECK" => Ok(NodeType::Check),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
impl serde::Serialize for NodeWarningOutcome {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::Unspecified => "NODE_WARNING_OUTCOME_UNSPECIFIED",
            Self::NoWarnings => "NODE_WARNING_OUTCOME_NO_WARNINGS",
            Self::WithWarnings => "NODE_WARNING_OUTCOME_WITH_WARNINGS",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for NodeWarningOutcome {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "NODE_WARNING_OUTCOME_UNSPECIFIED",
            "NODE_WARNING_OUTCOME_NO_WARNINGS",
            "NODE_WARNING_OUTCOME_WITH_WARNINGS",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = NodeWarningOutcome;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "expected one of: {:?}", &FIELDS)
            }

            fn visit_i64<E>(self, v: i64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Signed(v), &self)
                    })
            }

            fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Unsigned(v), &self)
                    })
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "NODE_WARNING_OUTCOME_UNSPECIFIED" => Ok(NodeWarningOutcome::Unspecified),
                    "NODE_WARNING_OUTCOME_NO_WARNINGS" => Ok(NodeWarningOutcome::NoWarnings),
                    "NODE_WARNING_OUTCOME_WITH_WARNINGS" => Ok(NodeWarningOutcome::WithWarnings),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
impl serde::Serialize for SourceFreshnessDetail {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if self.node_freshness_outcome != 0 {
            len += 1;
        }
        if self.age_seconds.is_some() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.node.SourceFreshnessDetail", len)?;
        if self.node_freshness_outcome != 0 {
            let v = SourceFreshnessOutcome::try_from(self.node_freshness_outcome)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.node_freshness_outcome)))?;
            struct_ser.serialize_field("node_freshness_outcome", &v)?;
        }
        if let Some(v) = self.age_seconds.as_ref() {
            #[allow(clippy::needless_borrow)]
            #[allow(clippy::needless_borrows_for_generic_args)]
            struct_ser.serialize_field("age_seconds", ToString::to_string(&v).as_str())?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for SourceFreshnessDetail {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "node_freshness_outcome",
            "nodeFreshnessOutcome",
            "age_seconds",
            "ageSeconds",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            NodeFreshnessOutcome,
            AgeSeconds,
            __SkipField__,
        }
        impl<'de> serde::Deserialize<'de> for GeneratedField {
            fn deserialize<D>(deserializer: D) -> std::result::Result<GeneratedField, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct GeneratedVisitor;

                impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
                    type Value = GeneratedField;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        write!(formatter, "expected one of: {:?}", &FIELDS)
                    }

                    #[allow(unused_variables)]
                    fn visit_str<E>(self, value: &str) -> std::result::Result<GeneratedField, E>
                    where
                        E: serde::de::Error,
                    {
                        match value {
                            "nodeFreshnessOutcome" | "node_freshness_outcome" => Ok(GeneratedField::NodeFreshnessOutcome),
                            "ageSeconds" | "age_seconds" => Ok(GeneratedField::AgeSeconds),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = SourceFreshnessDetail;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.node.SourceFreshnessDetail")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<SourceFreshnessDetail, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut node_freshness_outcome__ = None;
                let mut age_seconds__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::NodeFreshnessOutcome => {
                            if node_freshness_outcome__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeFreshnessOutcome"));
                            }
                            node_freshness_outcome__ = Some(map_.next_value::<SourceFreshnessOutcome>()? as i32);
                        }
                        GeneratedField::AgeSeconds => {
                            if age_seconds__.is_some() {
                                return Err(serde::de::Error::duplicate_field("ageSeconds"));
                            }
                            age_seconds__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(SourceFreshnessDetail {
                    node_freshness_outcome: node_freshness_outcome__.unwrap_or_default(),
                    age_seconds: age_seconds__,
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.node.SourceFreshnessDetail", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for SourceFreshnessOutcome {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::OutcomePassed => "SOURCE_FRESHNESS_OUTCOME_OUTCOME_PASSED",
            Self::OutcomeWarned => "SOURCE_FRESHNESS_OUTCOME_OUTCOME_WARNED",
            Self::OutcomeFailed => "SOURCE_FRESHNESS_OUTCOME_OUTCOME_FAILED",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for SourceFreshnessOutcome {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "SOURCE_FRESHNESS_OUTCOME_OUTCOME_PASSED",
            "SOURCE_FRESHNESS_OUTCOME_OUTCOME_WARNED",
            "SOURCE_FRESHNESS_OUTCOME_OUTCOME_FAILED",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = SourceFreshnessOutcome;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "expected one of: {:?}", &FIELDS)
            }

            fn visit_i64<E>(self, v: i64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Signed(v), &self)
                    })
            }

            fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Unsigned(v), &self)
                    })
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "SOURCE_FRESHNESS_OUTCOME_OUTCOME_PASSED" => Ok(SourceFreshnessOutcome::OutcomePassed),
                    "SOURCE_FRESHNESS_OUTCOME_OUTCOME_WARNED" => Ok(SourceFreshnessOutcome::OutcomeWarned),
                    "SOURCE_FRESHNESS_OUTCOME_OUTCOME_FAILED" => Ok(SourceFreshnessOutcome::OutcomeFailed),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
impl serde::Serialize for TestEvaluationDetail {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if self.test_outcome != 0 {
            len += 1;
        }
        if self.failing_rows != 0 {
            len += 1;
        }
        if self.diff_table.is_some() {
            len += 1;
        }
        if self.store_failures.is_some() {
            len += 1;
        }
        if self.statically_checked.is_some() {
            len += 1;
        }
        if self.batch_unique_id.is_some() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.node.TestEvaluationDetail", len)?;
        if self.test_outcome != 0 {
            let v = TestOutcome::try_from(self.test_outcome)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.test_outcome)))?;
            struct_ser.serialize_field("test_outcome", &v)?;
        }
        if self.failing_rows != 0 {
            struct_ser.serialize_field("failing_rows", &self.failing_rows)?;
        }
        if let Some(v) = self.diff_table.as_ref() {
            struct_ser.serialize_field("diff_table", v)?;
        }
        if let Some(v) = self.store_failures.as_ref() {
            struct_ser.serialize_field("store_failures", v)?;
        }
        if let Some(v) = self.statically_checked.as_ref() {
            struct_ser.serialize_field("statically_checked", v)?;
        }
        if let Some(v) = self.batch_unique_id.as_ref() {
            struct_ser.serialize_field("batch_unique_id", v)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for TestEvaluationDetail {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "test_outcome",
            "testOutcome",
            "failing_rows",
            "failingRows",
            "diff_table",
            "diffTable",
            "store_failures",
            "storeFailures",
            "statically_checked",
            "staticallyChecked",
            "batch_unique_id",
            "batchUniqueId",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            TestOutcome,
            FailingRows,
            DiffTable,
            StoreFailures,
            StaticallyChecked,
            BatchUniqueId,
            __SkipField__,
        }
        impl<'de> serde::Deserialize<'de> for GeneratedField {
            fn deserialize<D>(deserializer: D) -> std::result::Result<GeneratedField, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct GeneratedVisitor;

                impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
                    type Value = GeneratedField;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        write!(formatter, "expected one of: {:?}", &FIELDS)
                    }

                    #[allow(unused_variables)]
                    fn visit_str<E>(self, value: &str) -> std::result::Result<GeneratedField, E>
                    where
                        E: serde::de::Error,
                    {
                        match value {
                            "testOutcome" | "test_outcome" => Ok(GeneratedField::TestOutcome),
                            "failingRows" | "failing_rows" => Ok(GeneratedField::FailingRows),
                            "diffTable" | "diff_table" => Ok(GeneratedField::DiffTable),
                            "storeFailures" | "store_failures" => Ok(GeneratedField::StoreFailures),
                            "staticallyChecked" | "statically_checked" => Ok(GeneratedField::StaticallyChecked),
                            "batchUniqueId" | "batch_unique_id" => Ok(GeneratedField::BatchUniqueId),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = TestEvaluationDetail;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.node.TestEvaluationDetail")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<TestEvaluationDetail, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut test_outcome__ = None;
                let mut failing_rows__ = None;
                let mut diff_table__ = None;
                let mut store_failures__ = None;
                let mut statically_checked__ = None;
                let mut batch_unique_id__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::TestOutcome => {
                            if test_outcome__.is_some() {
                                return Err(serde::de::Error::duplicate_field("testOutcome"));
                            }
                            test_outcome__ = Some(map_.next_value::<TestOutcome>()? as i32);
                        }
                        GeneratedField::FailingRows => {
                            if failing_rows__.is_some() {
                                return Err(serde::de::Error::duplicate_field("failingRows"));
                            }
                            failing_rows__ = 
                                Some(map_.next_value::<::pbjson::private::NumberDeserialize<_>>()?.0)
                            ;
                        }
                        GeneratedField::DiffTable => {
                            if diff_table__.is_some() {
                                return Err(serde::de::Error::duplicate_field("diffTable"));
                            }
                            diff_table__ = map_.next_value()?;
                        }
                        GeneratedField::StoreFailures => {
                            if store_failures__.is_some() {
                                return Err(serde::de::Error::duplicate_field("storeFailures"));
                            }
                            store_failures__ = map_.next_value()?;
                        }
                        GeneratedField::StaticallyChecked => {
                            if statically_checked__.is_some() {
                                return Err(serde::de::Error::duplicate_field("staticallyChecked"));
                            }
                            statically_checked__ = map_.next_value()?;
                        }
                        GeneratedField::BatchUniqueId => {
                            if batch_unique_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("batchUniqueId"));
                            }
                            batch_unique_id__ = map_.next_value()?;
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(TestEvaluationDetail {
                    test_outcome: test_outcome__.unwrap_or_default(),
                    failing_rows: failing_rows__.unwrap_or_default(),
                    diff_table: diff_table__,
                    store_failures: store_failures__,
                    statically_checked: statically_checked__,
                    batch_unique_id: batch_unique_id__,
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.node.TestEvaluationDetail", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for TestOutcome {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::Passed => "TEST_OUTCOME_PASSED",
            Self::Warned => "TEST_OUTCOME_WARNED",
            Self::Failed => "TEST_OUTCOME_FAILED",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for TestOutcome {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "TEST_OUTCOME_PASSED",
            "TEST_OUTCOME_WARNED",
            "TEST_OUTCOME_FAILED",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = TestOutcome;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "expected one of: {:?}", &FIELDS)
            }

            fn visit_i64<E>(self, v: i64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Signed(v), &self)
                    })
            }

            fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                i32::try_from(v)
                    .ok()
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(|| {
                        serde::de::Error::invalid_value(serde::de::Unexpected::Unsigned(v), &self)
                    })
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "TEST_OUTCOME_PASSED" => Ok(TestOutcome::Passed),
                    "TEST_OUTCOME_WARNED" => Ok(TestOutcome::Warned),
                    "TEST_OUTCOME_FAILED" => Ok(TestOutcome::Failed),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
