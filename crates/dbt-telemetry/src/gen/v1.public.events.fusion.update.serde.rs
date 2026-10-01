impl serde::Serialize for PackageUpdate {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if !self.version.is_empty() {
            len += 1;
        }
        if !self.package.is_empty() {
            len += 1;
        }
        if self.exe_path.is_some() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.update.PackageUpdate", len)?;
        if !self.version.is_empty() {
            struct_ser.serialize_field("version", &self.version)?;
        }
        if !self.package.is_empty() {
            struct_ser.serialize_field("package", &self.package)?;
        }
        if let Some(v) = self.exe_path.as_ref() {
            struct_ser.serialize_field("exe_path", v)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for PackageUpdate {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "version",
            "package",
            "exe_path",
            "exePath",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            Version,
            Package,
            ExePath,
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
                            "version" => Ok(GeneratedField::Version),
                            "package" => Ok(GeneratedField::Package),
                            "exePath" | "exe_path" => Ok(GeneratedField::ExePath),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = PackageUpdate;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.update.PackageUpdate")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<PackageUpdate, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut version__ = None;
                let mut package__ = None;
                let mut exe_path__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::Version => {
                            if version__.is_some() {
                                return Err(serde::de::Error::duplicate_field("version"));
                            }
                            version__ = Some(map_.next_value()?);
                        }
                        GeneratedField::Package => {
                            if package__.is_some() {
                                return Err(serde::de::Error::duplicate_field("package"));
                            }
                            package__ = Some(map_.next_value()?);
                        }
                        GeneratedField::ExePath => {
                            if exe_path__.is_some() {
                                return Err(serde::de::Error::duplicate_field("exePath"));
                            }
                            exe_path__ = map_.next_value()?;
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(PackageUpdate {
                    version: version__.unwrap_or_default(),
                    package: package__.unwrap_or_default(),
                    exe_path: exe_path__,
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.update.PackageUpdate", FIELDS, GeneratedVisitor)
    }
}
