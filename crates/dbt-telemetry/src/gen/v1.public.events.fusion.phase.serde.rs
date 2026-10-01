impl serde::Serialize for ExecutionPhase {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::Unspecified => "EXECUTION_PHASE_UNSPECIFIED",
            Self::Clean => "EXECUTION_PHASE_CLEAN",
            Self::LoadProject => "EXECUTION_PHASE_LOAD_PROJECT",
            Self::Parse => "EXECUTION_PHASE_PARSE",
            Self::Schedule => "EXECUTION_PHASE_SCHEDULE",
            Self::Compare => "EXECUTION_PHASE_COMPARE",
            Self::InitAdapter => "EXECUTION_PHASE_INIT_ADAPTER",
            Self::DeferHydration => "EXECUTION_PHASE_DEFER_HYDRATION",
            Self::SchemaHydration => "EXECUTION_PHASE_SCHEMA_HYDRATION",
            Self::TaskGraphBuild => "EXECUTION_PHASE_TASK_GRAPH_BUILD",
            Self::NodeCacheHydration => "EXECUTION_PHASE_NODE_CACHE_HYDRATION",
            Self::Render => "EXECUTION_PHASE_RENDER",
            Self::Analyze => "EXECUTION_PHASE_ANALYZE",
            Self::Run => "EXECUTION_PHASE_RUN",
            Self::FreshnessAnalysis => "EXECUTION_PHASE_FRESHNESS_ANALYSIS",
            Self::Lineage => "EXECUTION_PHASE_LINEAGE",
            Self::Debug => "EXECUTION_PHASE_DEBUG",
            Self::OnRunStart => "EXECUTION_PHASE_ON_RUN_START",
            Self::OnRunEnd => "EXECUTION_PHASE_ON_RUN_END",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for ExecutionPhase {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "EXECUTION_PHASE_UNSPECIFIED",
            "EXECUTION_PHASE_CLEAN",
            "EXECUTION_PHASE_LOAD_PROJECT",
            "EXECUTION_PHASE_PARSE",
            "EXECUTION_PHASE_SCHEDULE",
            "EXECUTION_PHASE_COMPARE",
            "EXECUTION_PHASE_INIT_ADAPTER",
            "EXECUTION_PHASE_DEFER_HYDRATION",
            "EXECUTION_PHASE_SCHEMA_HYDRATION",
            "EXECUTION_PHASE_TASK_GRAPH_BUILD",
            "EXECUTION_PHASE_NODE_CACHE_HYDRATION",
            "EXECUTION_PHASE_RENDER",
            "EXECUTION_PHASE_ANALYZE",
            "EXECUTION_PHASE_RUN",
            "EXECUTION_PHASE_FRESHNESS_ANALYSIS",
            "EXECUTION_PHASE_LINEAGE",
            "EXECUTION_PHASE_DEBUG",
            "EXECUTION_PHASE_ON_RUN_START",
            "EXECUTION_PHASE_ON_RUN_END",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = ExecutionPhase;

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
                    "EXECUTION_PHASE_UNSPECIFIED" => Ok(ExecutionPhase::Unspecified),
                    "EXECUTION_PHASE_CLEAN" => Ok(ExecutionPhase::Clean),
                    "EXECUTION_PHASE_LOAD_PROJECT" => Ok(ExecutionPhase::LoadProject),
                    "EXECUTION_PHASE_PARSE" => Ok(ExecutionPhase::Parse),
                    "EXECUTION_PHASE_SCHEDULE" => Ok(ExecutionPhase::Schedule),
                    "EXECUTION_PHASE_COMPARE" => Ok(ExecutionPhase::Compare),
                    "EXECUTION_PHASE_INIT_ADAPTER" => Ok(ExecutionPhase::InitAdapter),
                    "EXECUTION_PHASE_DEFER_HYDRATION" => Ok(ExecutionPhase::DeferHydration),
                    "EXECUTION_PHASE_SCHEMA_HYDRATION" => Ok(ExecutionPhase::SchemaHydration),
                    "EXECUTION_PHASE_TASK_GRAPH_BUILD" => Ok(ExecutionPhase::TaskGraphBuild),
                    "EXECUTION_PHASE_NODE_CACHE_HYDRATION" => Ok(ExecutionPhase::NodeCacheHydration),
                    "EXECUTION_PHASE_RENDER" => Ok(ExecutionPhase::Render),
                    "EXECUTION_PHASE_ANALYZE" => Ok(ExecutionPhase::Analyze),
                    "EXECUTION_PHASE_RUN" => Ok(ExecutionPhase::Run),
                    "EXECUTION_PHASE_FRESHNESS_ANALYSIS" => Ok(ExecutionPhase::FreshnessAnalysis),
                    "EXECUTION_PHASE_LINEAGE" => Ok(ExecutionPhase::Lineage),
                    "EXECUTION_PHASE_DEBUG" => Ok(ExecutionPhase::Debug),
                    "EXECUTION_PHASE_ON_RUN_START" => Ok(ExecutionPhase::OnRunStart),
                    "EXECUTION_PHASE_ON_RUN_END" => Ok(ExecutionPhase::OnRunEnd),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
impl serde::Serialize for PhaseExecuted {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if self.phase != 0 {
            len += 1;
        }
        if self.node_count_total.is_some() {
            len += 1;
        }
        if self.node_count_skipped.is_some() {
            len += 1;
        }
        if self.node_count_error.is_some() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.phase.PhaseExecuted", len)?;
        if self.phase != 0 {
            let v = ExecutionPhase::try_from(self.phase)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.phase)))?;
            struct_ser.serialize_field("phase", &v)?;
        }
        if let Some(v) = self.node_count_total.as_ref() {
            #[allow(clippy::needless_borrow)]
            #[allow(clippy::needless_borrows_for_generic_args)]
            struct_ser.serialize_field("node_count_total", ToString::to_string(&v).as_str())?;
        }
        if let Some(v) = self.node_count_skipped.as_ref() {
            #[allow(clippy::needless_borrow)]
            #[allow(clippy::needless_borrows_for_generic_args)]
            struct_ser.serialize_field("node_count_skipped", ToString::to_string(&v).as_str())?;
        }
        if let Some(v) = self.node_count_error.as_ref() {
            #[allow(clippy::needless_borrow)]
            #[allow(clippy::needless_borrows_for_generic_args)]
            struct_ser.serialize_field("node_count_error", ToString::to_string(&v).as_str())?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for PhaseExecuted {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "phase",
            "node_count_total",
            "nodeCountTotal",
            "node_count_skipped",
            "nodeCountSkipped",
            "node_count_error",
            "nodeCountError",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            Phase,
            NodeCountTotal,
            NodeCountSkipped,
            NodeCountError,
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
                            "phase" => Ok(GeneratedField::Phase),
                            "nodeCountTotal" | "node_count_total" => Ok(GeneratedField::NodeCountTotal),
                            "nodeCountSkipped" | "node_count_skipped" => Ok(GeneratedField::NodeCountSkipped),
                            "nodeCountError" | "node_count_error" => Ok(GeneratedField::NodeCountError),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = PhaseExecuted;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.phase.PhaseExecuted")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<PhaseExecuted, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut phase__ = None;
                let mut node_count_total__ = None;
                let mut node_count_skipped__ = None;
                let mut node_count_error__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::Phase => {
                            if phase__.is_some() {
                                return Err(serde::de::Error::duplicate_field("phase"));
                            }
                            phase__ = Some(map_.next_value::<ExecutionPhase>()? as i32);
                        }
                        GeneratedField::NodeCountTotal => {
                            if node_count_total__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeCountTotal"));
                            }
                            node_count_total__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::NodeCountSkipped => {
                            if node_count_skipped__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeCountSkipped"));
                            }
                            node_count_skipped__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::NodeCountError => {
                            if node_count_error__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeCountError"));
                            }
                            node_count_error__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(PhaseExecuted {
                    phase: phase__.unwrap_or_default(),
                    node_count_total: node_count_total__,
                    node_count_skipped: node_count_skipped__,
                    node_count_error: node_count_error__,
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.phase.PhaseExecuted", FIELDS, GeneratedVisitor)
    }
}
