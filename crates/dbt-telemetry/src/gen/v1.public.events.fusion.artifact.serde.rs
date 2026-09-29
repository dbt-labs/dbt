impl serde::Serialize for ArtifactType {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::Unspecified => "ARTIFACT_TYPE_UNSPECIFIED",
            Self::Manifest => "ARTIFACT_TYPE_MANIFEST",
            Self::SemanticManifest => "ARTIFACT_TYPE_SEMANTIC_MANIFEST",
            Self::Catalog => "ARTIFACT_TYPE_CATALOG",
            Self::Sources => "ARTIFACT_TYPE_SOURCES",
            Self::Freshness => "ARTIFACT_TYPE_FRESHNESS",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for ArtifactType {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "ARTIFACT_TYPE_UNSPECIFIED",
            "ARTIFACT_TYPE_MANIFEST",
            "ARTIFACT_TYPE_SEMANTIC_MANIFEST",
            "ARTIFACT_TYPE_CATALOG",
            "ARTIFACT_TYPE_SOURCES",
            "ARTIFACT_TYPE_FRESHNESS",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = ArtifactType;

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
                    "ARTIFACT_TYPE_UNSPECIFIED" => Ok(ArtifactType::Unspecified),
                    "ARTIFACT_TYPE_MANIFEST" => Ok(ArtifactType::Manifest),
                    "ARTIFACT_TYPE_SEMANTIC_MANIFEST" => Ok(ArtifactType::SemanticManifest),
                    "ARTIFACT_TYPE_CATALOG" => Ok(ArtifactType::Catalog),
                    "ARTIFACT_TYPE_SOURCES" => Ok(ArtifactType::Sources),
                    "ARTIFACT_TYPE_FRESHNESS" => Ok(ArtifactType::Freshness),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
impl serde::Serialize for ArtifactWritten {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if !self.relative_path.is_empty() {
            len += 1;
        }
        if self.artifact_type != 0 {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.artifact.ArtifactWritten", len)?;
        if !self.relative_path.is_empty() {
            struct_ser.serialize_field("relative_path", &self.relative_path)?;
        }
        if self.artifact_type != 0 {
            let v = ArtifactType::try_from(self.artifact_type)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.artifact_type)))?;
            struct_ser.serialize_field("artifact_type", &v)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for ArtifactWritten {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "relative_path",
            "relativePath",
            "artifact_type",
            "artifactType",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            RelativePath,
            ArtifactType,
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
                            "relativePath" | "relative_path" => Ok(GeneratedField::RelativePath),
                            "artifactType" | "artifact_type" => Ok(GeneratedField::ArtifactType),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = ArtifactWritten;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.artifact.ArtifactWritten")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<ArtifactWritten, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut relative_path__ = None;
                let mut artifact_type__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::RelativePath => {
                            if relative_path__.is_some() {
                                return Err(serde::de::Error::duplicate_field("relativePath"));
                            }
                            relative_path__ = Some(map_.next_value()?);
                        }
                        GeneratedField::ArtifactType => {
                            if artifact_type__.is_some() {
                                return Err(serde::de::Error::duplicate_field("artifactType"));
                            }
                            artifact_type__ = Some(map_.next_value::<ArtifactType>()? as i32);
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(ArtifactWritten {
                    relative_path: relative_path__.unwrap_or_default(),
                    artifact_type: artifact_type__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.artifact.ArtifactWritten", FIELDS, GeneratedVisitor)
    }
}
