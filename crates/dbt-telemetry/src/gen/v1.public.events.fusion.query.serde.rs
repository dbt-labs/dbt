impl serde::Serialize for AdapterConnectionClose {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if !self.repr.is_empty() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.query.AdapterConnectionClose", len)?;
        if !self.repr.is_empty() {
            struct_ser.serialize_field("repr", &self.repr)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for AdapterConnectionClose {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "repr",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            Repr,
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
                            "repr" => Ok(GeneratedField::Repr),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = AdapterConnectionClose;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.query.AdapterConnectionClose")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<AdapterConnectionClose, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut repr__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::Repr => {
                            if repr__.is_some() {
                                return Err(serde::de::Error::duplicate_field("repr"));
                            }
                            repr__ = Some(map_.next_value()?);
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(AdapterConnectionClose {
                    repr: repr__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.query.AdapterConnectionClose", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for AdapterConnectionOpen {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if !self.adapter_type.is_empty() {
            len += 1;
        }
        if !self.adapter_backend.is_empty() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.query.AdapterConnectionOpen", len)?;
        if !self.adapter_type.is_empty() {
            struct_ser.serialize_field("adapter_type", &self.adapter_type)?;
        }
        if !self.adapter_backend.is_empty() {
            struct_ser.serialize_field("adapter_backend", &self.adapter_backend)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for AdapterConnectionOpen {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "adapter_type",
            "adapterType",
            "adapter_backend",
            "adapterBackend",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            AdapterType,
            AdapterBackend,
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
                            "adapterType" | "adapter_type" => Ok(GeneratedField::AdapterType),
                            "adapterBackend" | "adapter_backend" => Ok(GeneratedField::AdapterBackend),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = AdapterConnectionOpen;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.query.AdapterConnectionOpen")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<AdapterConnectionOpen, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut adapter_type__ = None;
                let mut adapter_backend__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::AdapterType => {
                            if adapter_type__.is_some() {
                                return Err(serde::de::Error::duplicate_field("adapterType"));
                            }
                            adapter_type__ = Some(map_.next_value()?);
                        }
                        GeneratedField::AdapterBackend => {
                            if adapter_backend__.is_some() {
                                return Err(serde::de::Error::duplicate_field("adapterBackend"));
                            }
                            adapter_backend__ = Some(map_.next_value()?);
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(AdapterConnectionOpen {
                    adapter_type: adapter_type__.unwrap_or_default(),
                    adapter_backend: adapter_backend__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.query.AdapterConnectionOpen", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for ConnectionLimitWait {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if self.active_nodes.is_some() {
            len += 1;
        }
        if self.active_connections.is_some() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.query.ConnectionLimitWait", len)?;
        if let Some(v) = self.active_nodes.as_ref() {
            struct_ser.serialize_field("active_nodes", v)?;
        }
        if let Some(v) = self.active_connections.as_ref() {
            struct_ser.serialize_field("active_connections", v)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for ConnectionLimitWait {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "active_nodes",
            "activeNodes",
            "active_connections",
            "activeConnections",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            ActiveNodes,
            ActiveConnections,
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
                            "activeNodes" | "active_nodes" => Ok(GeneratedField::ActiveNodes),
                            "activeConnections" | "active_connections" => Ok(GeneratedField::ActiveConnections),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = ConnectionLimitWait;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.query.ConnectionLimitWait")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<ConnectionLimitWait, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut active_nodes__ = None;
                let mut active_connections__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::ActiveNodes => {
                            if active_nodes__.is_some() {
                                return Err(serde::de::Error::duplicate_field("activeNodes"));
                            }
                            active_nodes__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::ActiveConnections => {
                            if active_connections__.is_some() {
                                return Err(serde::de::Error::duplicate_field("activeConnections"));
                            }
                            active_connections__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(ConnectionLimitWait {
                    active_nodes: active_nodes__,
                    active_connections: active_connections__,
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.query.ConnectionLimitWait", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for QueryExecuted {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if !self.sql.is_empty() {
            len += 1;
        }
        if !self.sql_hash.is_empty() {
            len += 1;
        }
        if !self.adapter_type.is_empty() {
            len += 1;
        }
        if self.query_description.is_some() {
            len += 1;
        }
        if self.query_id.is_some() {
            len += 1;
        }
        if self.unique_id.is_some() {
            len += 1;
        }
        if self.query_outcome != 0 {
            len += 1;
        }
        if self.phase.is_some() {
            len += 1;
        }
        if self.query_error_adapter_message.is_some() {
            len += 1;
        }
        if self.query_error_vendor_code.is_some() {
            len += 1;
        }
        if !self.dbt_core_event_code.is_empty() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.query.QueryExecuted", len)?;
        if !self.sql.is_empty() {
            struct_ser.serialize_field("sql", &self.sql)?;
        }
        if !self.sql_hash.is_empty() {
            struct_ser.serialize_field("sql_hash", &self.sql_hash)?;
        }
        if !self.adapter_type.is_empty() {
            struct_ser.serialize_field("adapter_type", &self.adapter_type)?;
        }
        if let Some(v) = self.query_description.as_ref() {
            struct_ser.serialize_field("query_description", v)?;
        }
        if let Some(v) = self.query_id.as_ref() {
            struct_ser.serialize_field("query_id", v)?;
        }
        if let Some(v) = self.unique_id.as_ref() {
            struct_ser.serialize_field("unique_id", v)?;
        }
        if self.query_outcome != 0 {
            let v = QueryOutcome::try_from(self.query_outcome)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.query_outcome)))?;
            struct_ser.serialize_field("query_outcome", &v)?;
        }
        if let Some(v) = self.phase.as_ref() {
            let v = super::phase::ExecutionPhase::try_from(*v)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", *v)))?;
            struct_ser.serialize_field("phase", &v)?;
        }
        if let Some(v) = self.query_error_adapter_message.as_ref() {
            struct_ser.serialize_field("query_error_adapter_message", v)?;
        }
        if let Some(v) = self.query_error_vendor_code.as_ref() {
            struct_ser.serialize_field("query_error_vendor_code", v)?;
        }
        if !self.dbt_core_event_code.is_empty() {
            struct_ser.serialize_field("dbt_core_event_code", &self.dbt_core_event_code)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for QueryExecuted {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "sql",
            "sql_hash",
            "sqlHash",
            "adapter_type",
            "adapterType",
            "query_description",
            "queryDescription",
            "query_id",
            "queryId",
            "unique_id",
            "uniqueId",
            "query_outcome",
            "queryOutcome",
            "phase",
            "query_error_adapter_message",
            "queryErrorAdapterMessage",
            "query_error_vendor_code",
            "queryErrorVendorCode",
            "dbt_core_event_code",
            "dbtCoreEventCode",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            Sql,
            SqlHash,
            AdapterType,
            QueryDescription,
            QueryId,
            UniqueId,
            QueryOutcome,
            Phase,
            QueryErrorAdapterMessage,
            QueryErrorVendorCode,
            DbtCoreEventCode,
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
                            "sql" => Ok(GeneratedField::Sql),
                            "sqlHash" | "sql_hash" => Ok(GeneratedField::SqlHash),
                            "adapterType" | "adapter_type" => Ok(GeneratedField::AdapterType),
                            "queryDescription" | "query_description" => Ok(GeneratedField::QueryDescription),
                            "queryId" | "query_id" => Ok(GeneratedField::QueryId),
                            "uniqueId" | "unique_id" => Ok(GeneratedField::UniqueId),
                            "queryOutcome" | "query_outcome" => Ok(GeneratedField::QueryOutcome),
                            "phase" => Ok(GeneratedField::Phase),
                            "queryErrorAdapterMessage" | "query_error_adapter_message" => Ok(GeneratedField::QueryErrorAdapterMessage),
                            "queryErrorVendorCode" | "query_error_vendor_code" => Ok(GeneratedField::QueryErrorVendorCode),
                            "dbtCoreEventCode" | "dbt_core_event_code" => Ok(GeneratedField::DbtCoreEventCode),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = QueryExecuted;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.query.QueryExecuted")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<QueryExecuted, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut sql__ = None;
                let mut sql_hash__ = None;
                let mut adapter_type__ = None;
                let mut query_description__ = None;
                let mut query_id__ = None;
                let mut unique_id__ = None;
                let mut query_outcome__ = None;
                let mut phase__ = None;
                let mut query_error_adapter_message__ = None;
                let mut query_error_vendor_code__ = None;
                let mut dbt_core_event_code__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::Sql => {
                            if sql__.is_some() {
                                return Err(serde::de::Error::duplicate_field("sql"));
                            }
                            sql__ = Some(map_.next_value()?);
                        }
                        GeneratedField::SqlHash => {
                            if sql_hash__.is_some() {
                                return Err(serde::de::Error::duplicate_field("sqlHash"));
                            }
                            sql_hash__ = Some(map_.next_value()?);
                        }
                        GeneratedField::AdapterType => {
                            if adapter_type__.is_some() {
                                return Err(serde::de::Error::duplicate_field("adapterType"));
                            }
                            adapter_type__ = Some(map_.next_value()?);
                        }
                        GeneratedField::QueryDescription => {
                            if query_description__.is_some() {
                                return Err(serde::de::Error::duplicate_field("queryDescription"));
                            }
                            query_description__ = map_.next_value()?;
                        }
                        GeneratedField::QueryId => {
                            if query_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("queryId"));
                            }
                            query_id__ = map_.next_value()?;
                        }
                        GeneratedField::UniqueId => {
                            if unique_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("uniqueId"));
                            }
                            unique_id__ = map_.next_value()?;
                        }
                        GeneratedField::QueryOutcome => {
                            if query_outcome__.is_some() {
                                return Err(serde::de::Error::duplicate_field("queryOutcome"));
                            }
                            query_outcome__ = Some(map_.next_value::<QueryOutcome>()? as i32);
                        }
                        GeneratedField::Phase => {
                            if phase__.is_some() {
                                return Err(serde::de::Error::duplicate_field("phase"));
                            }
                            phase__ = map_.next_value::<::std::option::Option<super::phase::ExecutionPhase>>()?.map(|x| x as i32);
                        }
                        GeneratedField::QueryErrorAdapterMessage => {
                            if query_error_adapter_message__.is_some() {
                                return Err(serde::de::Error::duplicate_field("queryErrorAdapterMessage"));
                            }
                            query_error_adapter_message__ = map_.next_value()?;
                        }
                        GeneratedField::QueryErrorVendorCode => {
                            if query_error_vendor_code__.is_some() {
                                return Err(serde::de::Error::duplicate_field("queryErrorVendorCode"));
                            }
                            query_error_vendor_code__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::DbtCoreEventCode => {
                            if dbt_core_event_code__.is_some() {
                                return Err(serde::de::Error::duplicate_field("dbtCoreEventCode"));
                            }
                            dbt_core_event_code__ = Some(map_.next_value()?);
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(QueryExecuted {
                    sql: sql__.unwrap_or_default(),
                    sql_hash: sql_hash__.unwrap_or_default(),
                    adapter_type: adapter_type__.unwrap_or_default(),
                    query_description: query_description__,
                    query_id: query_id__,
                    unique_id: unique_id__,
                    query_outcome: query_outcome__.unwrap_or_default(),
                    phase: phase__,
                    query_error_adapter_message: query_error_adapter_message__,
                    query_error_vendor_code: query_error_vendor_code__,
                    dbt_core_event_code: dbt_core_event_code__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.query.QueryExecuted", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for QueryOutcome {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::Unspecified => "QUERY_OUTCOME_UNSPECIFIED",
            Self::Success => "QUERY_OUTCOME_SUCCESS",
            Self::Error => "QUERY_OUTCOME_ERROR",
            Self::Canceled => "QUERY_OUTCOME_CANCELED",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for QueryOutcome {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "QUERY_OUTCOME_UNSPECIFIED",
            "QUERY_OUTCOME_SUCCESS",
            "QUERY_OUTCOME_ERROR",
            "QUERY_OUTCOME_CANCELED",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = QueryOutcome;

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
                    "QUERY_OUTCOME_UNSPECIFIED" => Ok(QueryOutcome::Unspecified),
                    "QUERY_OUTCOME_SUCCESS" => Ok(QueryOutcome::Success),
                    "QUERY_OUTCOME_ERROR" => Ok(QueryOutcome::Error),
                    "QUERY_OUTCOME_CANCELED" => Ok(QueryOutcome::Canceled),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
