impl serde::Serialize for CallTrace {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if !self.name.is_empty() {
            len += 1;
        }
        if self.file.is_some() {
            len += 1;
        }
        if self.line.is_some() {
            len += 1;
        }
        if !self.extra.is_empty() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.dev.CallTrace", len)?;
        if !self.name.is_empty() {
            struct_ser.serialize_field("name", &self.name)?;
        }
        if let Some(v) = self.file.as_ref() {
            struct_ser.serialize_field("file", v)?;
        }
        if let Some(v) = self.line.as_ref() {
            struct_ser.serialize_field("line", v)?;
        }
        if !self.extra.is_empty() {
            struct_ser.serialize_field("extra", &self.extra)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for CallTrace {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "name",
            "file",
            "line",
            "extra",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            Name,
            File,
            Line,
            Extra,
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
                            "name" => Ok(GeneratedField::Name),
                            "file" => Ok(GeneratedField::File),
                            "line" => Ok(GeneratedField::Line),
                            "extra" => Ok(GeneratedField::Extra),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = CallTrace;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.dev.CallTrace")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<CallTrace, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut name__ = None;
                let mut file__ = None;
                let mut line__ = None;
                let mut extra__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::Name => {
                            if name__.is_some() {
                                return Err(serde::de::Error::duplicate_field("name"));
                            }
                            name__ = Some(map_.next_value()?);
                        }
                        GeneratedField::File => {
                            if file__.is_some() {
                                return Err(serde::de::Error::duplicate_field("file"));
                            }
                            file__ = map_.next_value()?;
                        }
                        GeneratedField::Line => {
                            if line__.is_some() {
                                return Err(serde::de::Error::duplicate_field("line"));
                            }
                            line__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::Extra => {
                            if extra__.is_some() {
                                return Err(serde::de::Error::duplicate_field("extra"));
                            }
                            extra__ = Some(
                                map_.next_value::<std::collections::BTreeMap<_, _>>()?
                            );
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(CallTrace {
                    name: name__.unwrap_or_default(),
                    file: file__,
                    line: line__,
                    extra: extra__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.dev.CallTrace", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for DebugValue {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if self.value.is_some() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.dev.DebugValue", len)?;
        if let Some(v) = self.value.as_ref() {
            match v {
                debug_value::Value::Float64(v) => {
                    struct_ser.serialize_field("float64", v)?;
                }
                debug_value::Value::Bool(v) => {
                    struct_ser.serialize_field("bool", v)?;
                }
                debug_value::Value::String(v) => {
                    struct_ser.serialize_field("string", v)?;
                }
                debug_value::Value::Bytes(v) => {
                    #[allow(clippy::needless_borrow)]
                    #[allow(clippy::needless_borrows_for_generic_args)]
                    struct_ser.serialize_field("bytes", pbjson::private::base64::encode(&v).as_str())?;
                }
            }
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for DebugValue {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "float64",
            "bool",
            "string",
            "bytes",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            Float64,
            Bool,
            String,
            Bytes,
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
                            "float64" => Ok(GeneratedField::Float64),
                            "bool" => Ok(GeneratedField::Bool),
                            "string" => Ok(GeneratedField::String),
                            "bytes" => Ok(GeneratedField::Bytes),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = DebugValue;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.dev.DebugValue")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<DebugValue, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut value__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::Float64 => {
                            if value__.is_some() {
                                return Err(serde::de::Error::duplicate_field("float64"));
                            }
                            value__ = map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| debug_value::Value::Float64(x.0));
                        }
                        GeneratedField::Bool => {
                            if value__.is_some() {
                                return Err(serde::de::Error::duplicate_field("bool"));
                            }
                            value__ = map_.next_value::<::std::option::Option<_>>()?.map(debug_value::Value::Bool);
                        }
                        GeneratedField::String => {
                            if value__.is_some() {
                                return Err(serde::de::Error::duplicate_field("string"));
                            }
                            value__ = map_.next_value::<::std::option::Option<_>>()?.map(debug_value::Value::String);
                        }
                        GeneratedField::Bytes => {
                            if value__.is_some() {
                                return Err(serde::de::Error::duplicate_field("bytes"));
                            }
                            value__ = map_.next_value::<::std::option::Option<::pbjson::private::BytesDeserialize<_>>>()?.map(|x| debug_value::Value::Bytes(x.0));
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(DebugValue {
                    value: value__,
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.dev.DebugValue", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for Unknown {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if !self.name.is_empty() {
            len += 1;
        }
        if !self.file.is_empty() {
            len += 1;
        }
        if self.line != 0 {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.dev.Unknown", len)?;
        if !self.name.is_empty() {
            struct_ser.serialize_field("name", &self.name)?;
        }
        if !self.file.is_empty() {
            struct_ser.serialize_field("file", &self.file)?;
        }
        if self.line != 0 {
            struct_ser.serialize_field("line", &self.line)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for Unknown {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "name",
            "file",
            "line",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            Name,
            File,
            Line,
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
                            "name" => Ok(GeneratedField::Name),
                            "file" => Ok(GeneratedField::File),
                            "line" => Ok(GeneratedField::Line),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = Unknown;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.dev.Unknown")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<Unknown, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut name__ = None;
                let mut file__ = None;
                let mut line__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::Name => {
                            if name__.is_some() {
                                return Err(serde::de::Error::duplicate_field("name"));
                            }
                            name__ = Some(map_.next_value()?);
                        }
                        GeneratedField::File => {
                            if file__.is_some() {
                                return Err(serde::de::Error::duplicate_field("file"));
                            }
                            file__ = Some(map_.next_value()?);
                        }
                        GeneratedField::Line => {
                            if line__.is_some() {
                                return Err(serde::de::Error::duplicate_field("line"));
                            }
                            line__ = 
                                Some(map_.next_value::<::pbjson::private::NumberDeserialize<_>>()?.0)
                            ;
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(Unknown {
                    name: name__.unwrap_or_default(),
                    file: file__.unwrap_or_default(),
                    line: line__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.dev.Unknown", FIELDS, GeneratedVisitor)
    }
}
