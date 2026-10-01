impl serde::Serialize for Process {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if !self.package.is_empty() {
            len += 1;
        }
        if !self.version.is_empty() {
            len += 1;
        }
        if !self.host_os.is_empty() {
            len += 1;
        }
        if !self.host_arch.is_empty() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.process.Process", len)?;
        if !self.package.is_empty() {
            struct_ser.serialize_field("package", &self.package)?;
        }
        if !self.version.is_empty() {
            struct_ser.serialize_field("version", &self.version)?;
        }
        if !self.host_os.is_empty() {
            struct_ser.serialize_field("host_os", &self.host_os)?;
        }
        if !self.host_arch.is_empty() {
            struct_ser.serialize_field("host_arch", &self.host_arch)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for Process {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "package",
            "version",
            "host_os",
            "hostOs",
            "host_arch",
            "hostArch",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            Package,
            Version,
            HostOs,
            HostArch,
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
                            "package" => Ok(GeneratedField::Package),
                            "version" => Ok(GeneratedField::Version),
                            "hostOs" | "host_os" => Ok(GeneratedField::HostOs),
                            "hostArch" | "host_arch" => Ok(GeneratedField::HostArch),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = Process;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.process.Process")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<Process, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut package__ = None;
                let mut version__ = None;
                let mut host_os__ = None;
                let mut host_arch__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::Package => {
                            if package__.is_some() {
                                return Err(serde::de::Error::duplicate_field("package"));
                            }
                            package__ = Some(map_.next_value()?);
                        }
                        GeneratedField::Version => {
                            if version__.is_some() {
                                return Err(serde::de::Error::duplicate_field("version"));
                            }
                            version__ = Some(map_.next_value()?);
                        }
                        GeneratedField::HostOs => {
                            if host_os__.is_some() {
                                return Err(serde::de::Error::duplicate_field("hostOs"));
                            }
                            host_os__ = Some(map_.next_value()?);
                        }
                        GeneratedField::HostArch => {
                            if host_arch__.is_some() {
                                return Err(serde::de::Error::duplicate_field("hostArch"));
                            }
                            host_arch__ = Some(map_.next_value()?);
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(Process {
                    package: package__.unwrap_or_default(),
                    version: version__.unwrap_or_default(),
                    host_os: host_os__.unwrap_or_default(),
                    host_arch: host_arch__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.process.Process", FIELDS, GeneratedVisitor)
    }
}
