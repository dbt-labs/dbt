impl serde::Serialize for AssetParsed {
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
        if !self.name.is_empty() {
            len += 1;
        }
        if !self.relative_path.is_empty() {
            len += 1;
        }
        if !self.display_path.is_empty() {
            len += 1;
        }
        if self.unique_id.is_some() {
            len += 1;
        }
        if self.phase != 0 {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.asset.AssetParsed", len)?;
        if !self.package_name.is_empty() {
            struct_ser.serialize_field("package_name", &self.package_name)?;
        }
        if !self.name.is_empty() {
            struct_ser.serialize_field("name", &self.name)?;
        }
        if !self.relative_path.is_empty() {
            struct_ser.serialize_field("relative_path", &self.relative_path)?;
        }
        if !self.display_path.is_empty() {
            struct_ser.serialize_field("display_path", &self.display_path)?;
        }
        if let Some(v) = self.unique_id.as_ref() {
            struct_ser.serialize_field("unique_id", v)?;
        }
        if self.phase != 0 {
            let v = super::phase::ExecutionPhase::try_from(self.phase)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.phase)))?;
            struct_ser.serialize_field("phase", &v)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for AssetParsed {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "package_name",
            "packageName",
            "name",
            "relative_path",
            "relativePath",
            "display_path",
            "displayPath",
            "unique_id",
            "uniqueId",
            "phase",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            PackageName,
            Name,
            RelativePath,
            DisplayPath,
            UniqueId,
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
                            "relativePath" | "relative_path" => Ok(GeneratedField::RelativePath),
                            "displayPath" | "display_path" => Ok(GeneratedField::DisplayPath),
                            "uniqueId" | "unique_id" => Ok(GeneratedField::UniqueId),
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
            type Value = AssetParsed;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.asset.AssetParsed")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<AssetParsed, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut package_name__ = None;
                let mut name__ = None;
                let mut relative_path__ = None;
                let mut display_path__ = None;
                let mut unique_id__ = None;
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
                            name__ = Some(map_.next_value()?);
                        }
                        GeneratedField::RelativePath => {
                            if relative_path__.is_some() {
                                return Err(serde::de::Error::duplicate_field("relativePath"));
                            }
                            relative_path__ = Some(map_.next_value()?);
                        }
                        GeneratedField::DisplayPath => {
                            if display_path__.is_some() {
                                return Err(serde::de::Error::duplicate_field("displayPath"));
                            }
                            display_path__ = Some(map_.next_value()?);
                        }
                        GeneratedField::UniqueId => {
                            if unique_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("uniqueId"));
                            }
                            unique_id__ = map_.next_value()?;
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
                Ok(AssetParsed {
                    package_name: package_name__.unwrap_or_default(),
                    name: name__.unwrap_or_default(),
                    relative_path: relative_path__.unwrap_or_default(),
                    display_path: display_path__.unwrap_or_default(),
                    unique_id: unique_id__,
                    phase: phase__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.asset.AssetParsed", FIELDS, GeneratedVisitor)
    }
}
