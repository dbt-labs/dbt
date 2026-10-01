impl serde::Serialize for DepsAddPackage {
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
        if self.package_type != 0 {
            len += 1;
        }
        if self.package_version.is_some() {
            len += 1;
        }
        if !self.dbt_core_event_code.is_empty() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.deps.DepsAddPackage", len)?;
        if !self.package_name.is_empty() {
            struct_ser.serialize_field("package_name", &self.package_name)?;
        }
        if self.package_type != 0 {
            let v = PackageType::try_from(self.package_type)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.package_type)))?;
            struct_ser.serialize_field("package_type", &v)?;
        }
        if let Some(v) = self.package_version.as_ref() {
            struct_ser.serialize_field("package_version", v)?;
        }
        if !self.dbt_core_event_code.is_empty() {
            struct_ser.serialize_field("dbt_core_event_code", &self.dbt_core_event_code)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for DepsAddPackage {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "package_name",
            "packageName",
            "package_type",
            "packageType",
            "package_version",
            "packageVersion",
            "dbt_core_event_code",
            "dbtCoreEventCode",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            PackageName,
            PackageType,
            PackageVersion,
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
                            "packageName" | "package_name" => Ok(GeneratedField::PackageName),
                            "packageType" | "package_type" => Ok(GeneratedField::PackageType),
                            "packageVersion" | "package_version" => Ok(GeneratedField::PackageVersion),
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
            type Value = DepsAddPackage;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.deps.DepsAddPackage")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<DepsAddPackage, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut package_name__ = None;
                let mut package_type__ = None;
                let mut package_version__ = None;
                let mut dbt_core_event_code__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::PackageName => {
                            if package_name__.is_some() {
                                return Err(serde::de::Error::duplicate_field("packageName"));
                            }
                            package_name__ = Some(map_.next_value()?);
                        }
                        GeneratedField::PackageType => {
                            if package_type__.is_some() {
                                return Err(serde::de::Error::duplicate_field("packageType"));
                            }
                            package_type__ = Some(map_.next_value::<PackageType>()? as i32);
                        }
                        GeneratedField::PackageVersion => {
                            if package_version__.is_some() {
                                return Err(serde::de::Error::duplicate_field("packageVersion"));
                            }
                            package_version__ = map_.next_value()?;
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
                Ok(DepsAddPackage {
                    package_name: package_name__.unwrap_or_default(),
                    package_type: package_type__.unwrap_or_default(),
                    package_version: package_version__,
                    dbt_core_event_code: dbt_core_event_code__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.deps.DepsAddPackage", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for DepsAllPackagesInstalled {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if self.package_count != 0 {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.deps.DepsAllPackagesInstalled", len)?;
        if self.package_count != 0 {
            #[allow(clippy::needless_borrow)]
            #[allow(clippy::needless_borrows_for_generic_args)]
            struct_ser.serialize_field("package_count", ToString::to_string(&self.package_count).as_str())?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for DepsAllPackagesInstalled {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "package_count",
            "packageCount",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            PackageCount,
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
                            "packageCount" | "package_count" => Ok(GeneratedField::PackageCount),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = DepsAllPackagesInstalled;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.deps.DepsAllPackagesInstalled")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<DepsAllPackagesInstalled, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut package_count__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::PackageCount => {
                            if package_count__.is_some() {
                                return Err(serde::de::Error::duplicate_field("packageCount"));
                            }
                            package_count__ = 
                                Some(map_.next_value::<::pbjson::private::NumberDeserialize<_>>()?.0)
                            ;
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(DepsAllPackagesInstalled {
                    package_count: package_count__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.deps.DepsAllPackagesInstalled", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for DepsPackageInstalled {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if self.package_name.is_some() {
            len += 1;
        }
        if self.package_type != 0 {
            len += 1;
        }
        if self.package_version.is_some() {
            len += 1;
        }
        if self.package_url_or_path.is_some() {
            len += 1;
        }
        if !self.dbt_core_event_code.is_empty() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.deps.DepsPackageInstalled", len)?;
        if let Some(v) = self.package_name.as_ref() {
            struct_ser.serialize_field("package_name", v)?;
        }
        if self.package_type != 0 {
            let v = PackageType::try_from(self.package_type)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.package_type)))?;
            struct_ser.serialize_field("package_type", &v)?;
        }
        if let Some(v) = self.package_version.as_ref() {
            struct_ser.serialize_field("package_version", v)?;
        }
        if let Some(v) = self.package_url_or_path.as_ref() {
            struct_ser.serialize_field("package_url_or_path", v)?;
        }
        if !self.dbt_core_event_code.is_empty() {
            struct_ser.serialize_field("dbt_core_event_code", &self.dbt_core_event_code)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for DepsPackageInstalled {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "package_name",
            "packageName",
            "package_type",
            "packageType",
            "package_version",
            "packageVersion",
            "package_url_or_path",
            "packageUrlOrPath",
            "dbt_core_event_code",
            "dbtCoreEventCode",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            PackageName,
            PackageType,
            PackageVersion,
            PackageUrlOrPath,
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
                            "packageName" | "package_name" => Ok(GeneratedField::PackageName),
                            "packageType" | "package_type" => Ok(GeneratedField::PackageType),
                            "packageVersion" | "package_version" => Ok(GeneratedField::PackageVersion),
                            "packageUrlOrPath" | "package_url_or_path" => Ok(GeneratedField::PackageUrlOrPath),
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
            type Value = DepsPackageInstalled;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.deps.DepsPackageInstalled")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<DepsPackageInstalled, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut package_name__ = None;
                let mut package_type__ = None;
                let mut package_version__ = None;
                let mut package_url_or_path__ = None;
                let mut dbt_core_event_code__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::PackageName => {
                            if package_name__.is_some() {
                                return Err(serde::de::Error::duplicate_field("packageName"));
                            }
                            package_name__ = map_.next_value()?;
                        }
                        GeneratedField::PackageType => {
                            if package_type__.is_some() {
                                return Err(serde::de::Error::duplicate_field("packageType"));
                            }
                            package_type__ = Some(map_.next_value::<PackageType>()? as i32);
                        }
                        GeneratedField::PackageVersion => {
                            if package_version__.is_some() {
                                return Err(serde::de::Error::duplicate_field("packageVersion"));
                            }
                            package_version__ = map_.next_value()?;
                        }
                        GeneratedField::PackageUrlOrPath => {
                            if package_url_or_path__.is_some() {
                                return Err(serde::de::Error::duplicate_field("packageUrlOrPath"));
                            }
                            package_url_or_path__ = map_.next_value()?;
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
                Ok(DepsPackageInstalled {
                    package_name: package_name__,
                    package_type: package_type__.unwrap_or_default(),
                    package_version: package_version__,
                    package_url_or_path: package_url_or_path__,
                    dbt_core_event_code: dbt_core_event_code__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.deps.DepsPackageInstalled", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for PackageType {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::Unspecified => "PACKAGE_TYPE_UNSPECIFIED",
            Self::Hub => "PACKAGE_TYPE_HUB",
            Self::Git => "PACKAGE_TYPE_GIT",
            Self::Local => "PACKAGE_TYPE_LOCAL",
            Self::Private => "PACKAGE_TYPE_PRIVATE",
            Self::Tarball => "PACKAGE_TYPE_TARBALL",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for PackageType {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "PACKAGE_TYPE_UNSPECIFIED",
            "PACKAGE_TYPE_HUB",
            "PACKAGE_TYPE_GIT",
            "PACKAGE_TYPE_LOCAL",
            "PACKAGE_TYPE_PRIVATE",
            "PACKAGE_TYPE_TARBALL",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = PackageType;

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
                    "PACKAGE_TYPE_UNSPECIFIED" => Ok(PackageType::Unspecified),
                    "PACKAGE_TYPE_HUB" => Ok(PackageType::Hub),
                    "PACKAGE_TYPE_GIT" => Ok(PackageType::Git),
                    "PACKAGE_TYPE_LOCAL" => Ok(PackageType::Local),
                    "PACKAGE_TYPE_PRIVATE" => Ok(PackageType::Private),
                    "PACKAGE_TYPE_TARBALL" => Ok(PackageType::Tarball),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
