impl serde::Serialize for HookOutcome {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::Unspecified => "HOOK_OUTCOME_UNSPECIFIED",
            Self::Success => "HOOK_OUTCOME_SUCCESS",
            Self::Error => "HOOK_OUTCOME_ERROR",
            Self::Canceled => "HOOK_OUTCOME_CANCELED",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for HookOutcome {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "HOOK_OUTCOME_UNSPECIFIED",
            "HOOK_OUTCOME_SUCCESS",
            "HOOK_OUTCOME_ERROR",
            "HOOK_OUTCOME_CANCELED",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = HookOutcome;

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
                    "HOOK_OUTCOME_UNSPECIFIED" => Ok(HookOutcome::Unspecified),
                    "HOOK_OUTCOME_SUCCESS" => Ok(HookOutcome::Success),
                    "HOOK_OUTCOME_ERROR" => Ok(HookOutcome::Error),
                    "HOOK_OUTCOME_CANCELED" => Ok(HookOutcome::Canceled),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
impl serde::Serialize for HookProcessed {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if !self.package_name.is_empty() {
            len += 1;
        }
        if self.name.is_some() {
            len += 1;
        }
        if self.hook_type != 0 {
            len += 1;
        }
        if self.hook_index != 0 {
            len += 1;
        }
        if !self.unique_id.is_empty() {
            len += 1;
        }
        if self.hook_outcome != 0 {
            len += 1;
        }
        if !self.dbt_core_event_code.is_empty() {
            len += 1;
        }
        if self.phase != 0 {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.hook.HookProcessed", len)?;
        if !self.package_name.is_empty() {
            struct_ser.serialize_field("package_name", &self.package_name)?;
        }
        if let Some(v) = self.name.as_ref() {
            struct_ser.serialize_field("name", v)?;
        }
        if self.hook_type != 0 {
            let v = HookType::try_from(self.hook_type)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.hook_type)))?;
            struct_ser.serialize_field("hook_type", &v)?;
        }
        if self.hook_index != 0 {
            struct_ser.serialize_field("hook_index", &self.hook_index)?;
        }
        if !self.unique_id.is_empty() {
            struct_ser.serialize_field("unique_id", &self.unique_id)?;
        }
        if self.hook_outcome != 0 {
            let v = HookOutcome::try_from(self.hook_outcome)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.hook_outcome)))?;
            struct_ser.serialize_field("hook_outcome", &v)?;
        }
        if !self.dbt_core_event_code.is_empty() {
            struct_ser.serialize_field("dbt_core_event_code", &self.dbt_core_event_code)?;
        }
        if self.phase != 0 {
            let v = super::phase::ExecutionPhase::try_from(self.phase)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.phase)))?;
            struct_ser.serialize_field("phase", &v)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for HookProcessed {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "package_name",
            "packageName",
            "name",
            "hook_type",
            "hookType",
            "hook_index",
            "hookIndex",
            "unique_id",
            "uniqueId",
            "hook_outcome",
            "hookOutcome",
            "dbt_core_event_code",
            "dbtCoreEventCode",
            "phase",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            PackageName,
            Name,
            HookType,
            HookIndex,
            UniqueId,
            HookOutcome,
            DbtCoreEventCode,
            Phase,
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
                            "packageName" | "package_name" => Ok(GeneratedField::PackageName),
                            "name" => Ok(GeneratedField::Name),
                            "hookType" | "hook_type" => Ok(GeneratedField::HookType),
                            "hookIndex" | "hook_index" => Ok(GeneratedField::HookIndex),
                            "uniqueId" | "unique_id" => Ok(GeneratedField::UniqueId),
                            "hookOutcome" | "hook_outcome" => Ok(GeneratedField::HookOutcome),
                            "dbtCoreEventCode" | "dbt_core_event_code" => Ok(GeneratedField::DbtCoreEventCode),
                            "phase" => Ok(GeneratedField::Phase),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = HookProcessed;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.hook.HookProcessed")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<HookProcessed, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut package_name__ = None;
                let mut name__ = None;
                let mut hook_type__ = None;
                let mut hook_index__ = None;
                let mut unique_id__ = None;
                let mut hook_outcome__ = None;
                let mut dbt_core_event_code__ = None;
                let mut phase__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::PackageName => {
                            if package_name__.is_some() {
                                return Err(serde::de::Error::duplicate_field("packageName"));
                            }
                            package_name__ = Some(map_.next_value()?);
                        }
                        GeneratedField::Name => {
                            if name__.is_some() {
                                return Err(serde::de::Error::duplicate_field("name"));
                            }
                            name__ = map_.next_value()?;
                        }
                        GeneratedField::HookType => {
                            if hook_type__.is_some() {
                                return Err(serde::de::Error::duplicate_field("hookType"));
                            }
                            hook_type__ = Some(map_.next_value::<HookType>()? as i32);
                        }
                        GeneratedField::HookIndex => {
                            if hook_index__.is_some() {
                                return Err(serde::de::Error::duplicate_field("hookIndex"));
                            }
                            hook_index__ = 
                                Some(map_.next_value::<::pbjson::private::NumberDeserialize<_>>()?.0)
                            ;
                        }
                        GeneratedField::UniqueId => {
                            if unique_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("uniqueId"));
                            }
                            unique_id__ = Some(map_.next_value()?);
                        }
                        GeneratedField::HookOutcome => {
                            if hook_outcome__.is_some() {
                                return Err(serde::de::Error::duplicate_field("hookOutcome"));
                            }
                            hook_outcome__ = Some(map_.next_value::<HookOutcome>()? as i32);
                        }
                        GeneratedField::DbtCoreEventCode => {
                            if dbt_core_event_code__.is_some() {
                                return Err(serde::de::Error::duplicate_field("dbtCoreEventCode"));
                            }
                            dbt_core_event_code__ = Some(map_.next_value()?);
                        }
                        GeneratedField::Phase => {
                            if phase__.is_some() {
                                return Err(serde::de::Error::duplicate_field("phase"));
                            }
                            phase__ = Some(map_.next_value::<super::phase::ExecutionPhase>()? as i32);
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(HookProcessed {
                    package_name: package_name__.unwrap_or_default(),
                    name: name__,
                    hook_type: hook_type__.unwrap_or_default(),
                    hook_index: hook_index__.unwrap_or_default(),
                    unique_id: unique_id__.unwrap_or_default(),
                    hook_outcome: hook_outcome__.unwrap_or_default(),
                    dbt_core_event_code: dbt_core_event_code__.unwrap_or_default(),
                    phase: phase__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.hook.HookProcessed", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for HookType {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::Unspecified => "HOOK_TYPE_UNSPECIFIED",
            Self::OnRunStart => "HOOK_TYPE_ON_RUN_START",
            Self::OnRunEnd => "HOOK_TYPE_ON_RUN_END",
            Self::PreHook => "HOOK_TYPE_PRE_HOOK",
            Self::PostHook => "HOOK_TYPE_POST_HOOK",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for HookType {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "HOOK_TYPE_UNSPECIFIED",
            "HOOK_TYPE_ON_RUN_START",
            "HOOK_TYPE_ON_RUN_END",
            "HOOK_TYPE_PRE_HOOK",
            "HOOK_TYPE_POST_HOOK",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = HookType;

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
                    "HOOK_TYPE_UNSPECIFIED" => Ok(HookType::Unspecified),
                    "HOOK_TYPE_ON_RUN_START" => Ok(HookType::OnRunStart),
                    "HOOK_TYPE_ON_RUN_END" => Ok(HookType::OnRunEnd),
                    "HOOK_TYPE_PRE_HOOK" => Ok(HookType::PreHook),
                    "HOOK_TYPE_POST_HOOK" => Ok(HookType::PostHook),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
