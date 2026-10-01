impl serde::Serialize for CompiledCode {
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
        if !self.sql.is_empty() {
            len += 1;
        }
        if !self.unique_id.is_empty() {
            len += 1;
        }
        if !self.node_name.is_empty() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.log.CompiledCode", len)?;
        if !self.relative_path.is_empty() {
            struct_ser.serialize_field("relative_path", &self.relative_path)?;
        }
        if !self.sql.is_empty() {
            struct_ser.serialize_field("sql", &self.sql)?;
        }
        if !self.unique_id.is_empty() {
            struct_ser.serialize_field("unique_id", &self.unique_id)?;
        }
        if !self.node_name.is_empty() {
            struct_ser.serialize_field("node_name", &self.node_name)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for CompiledCode {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "relative_path",
            "relativePath",
            "sql",
            "unique_id",
            "uniqueId",
            "node_name",
            "nodeName",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            RelativePath,
            Sql,
            UniqueId,
            NodeName,
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
                            "sql" => Ok(GeneratedField::Sql),
                            "uniqueId" | "unique_id" => Ok(GeneratedField::UniqueId),
                            "nodeName" | "node_name" => Ok(GeneratedField::NodeName),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = CompiledCode;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.log.CompiledCode")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<CompiledCode, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut relative_path__ = None;
                let mut sql__ = None;
                let mut unique_id__ = None;
                let mut node_name__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::RelativePath => {
                            if relative_path__.is_some() {
                                return Err(serde::de::Error::duplicate_field("relativePath"));
                            }
                            relative_path__ = Some(map_.next_value()?);
                        }
                        GeneratedField::Sql => {
                            if sql__.is_some() {
                                return Err(serde::de::Error::duplicate_field("sql"));
                            }
                            sql__ = Some(map_.next_value()?);
                        }
                        GeneratedField::UniqueId => {
                            if unique_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("uniqueId"));
                            }
                            unique_id__ = Some(map_.next_value()?);
                        }
                        GeneratedField::NodeName => {
                            if node_name__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeName"));
                            }
                            node_name__ = Some(map_.next_value()?);
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(CompiledCode {
                    relative_path: relative_path__.unwrap_or_default(),
                    sql: sql__.unwrap_or_default(),
                    unique_id: unique_id__.unwrap_or_default(),
                    node_name: node_name__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.log.CompiledCode", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for CompiledCodeInline {
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
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.log.CompiledCodeInline", len)?;
        if !self.sql.is_empty() {
            struct_ser.serialize_field("sql", &self.sql)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for CompiledCodeInline {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "sql",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            Sql,
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
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = CompiledCodeInline;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.log.CompiledCodeInline")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<CompiledCodeInline, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut sql__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::Sql => {
                            if sql__.is_some() {
                                return Err(serde::de::Error::duplicate_field("sql"));
                            }
                            sql__ = Some(map_.next_value()?);
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(CompiledCodeInline {
                    sql: sql__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.log.CompiledCodeInline", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for ListItemOutput {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if self.output_format != 0 {
            len += 1;
        }
        if !self.content.is_empty() {
            len += 1;
        }
        if self.unique_id.is_some() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.log.ListItemOutput", len)?;
        if self.output_format != 0 {
            let v = ListOutputFormat::try_from(self.output_format)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.output_format)))?;
            struct_ser.serialize_field("output_format", &v)?;
        }
        if !self.content.is_empty() {
            struct_ser.serialize_field("content", &self.content)?;
        }
        if let Some(v) = self.unique_id.as_ref() {
            struct_ser.serialize_field("unique_id", v)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for ListItemOutput {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "output_format",
            "outputFormat",
            "content",
            "unique_id",
            "uniqueId",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            OutputFormat,
            Content,
            UniqueId,
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
                            "outputFormat" | "output_format" => Ok(GeneratedField::OutputFormat),
                            "content" => Ok(GeneratedField::Content),
                            "uniqueId" | "unique_id" => Ok(GeneratedField::UniqueId),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = ListItemOutput;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.log.ListItemOutput")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<ListItemOutput, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut output_format__ = None;
                let mut content__ = None;
                let mut unique_id__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::OutputFormat => {
                            if output_format__.is_some() {
                                return Err(serde::de::Error::duplicate_field("outputFormat"));
                            }
                            output_format__ = Some(map_.next_value::<ListOutputFormat>()? as i32);
                        }
                        GeneratedField::Content => {
                            if content__.is_some() {
                                return Err(serde::de::Error::duplicate_field("content"));
                            }
                            content__ = Some(map_.next_value()?);
                        }
                        GeneratedField::UniqueId => {
                            if unique_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("uniqueId"));
                            }
                            unique_id__ = map_.next_value()?;
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(ListItemOutput {
                    output_format: output_format__.unwrap_or_default(),
                    content: content__.unwrap_or_default(),
                    unique_id: unique_id__,
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.log.ListItemOutput", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for ListOutputFormat {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::Unspecified => "LIST_OUTPUT_FORMAT_UNSPECIFIED",
            Self::Json => "LIST_OUTPUT_FORMAT_JSON",
            Self::Selector => "LIST_OUTPUT_FORMAT_SELECTOR",
            Self::Name => "LIST_OUTPUT_FORMAT_NAME",
            Self::Path => "LIST_OUTPUT_FORMAT_PATH",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for ListOutputFormat {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "LIST_OUTPUT_FORMAT_UNSPECIFIED",
            "LIST_OUTPUT_FORMAT_JSON",
            "LIST_OUTPUT_FORMAT_SELECTOR",
            "LIST_OUTPUT_FORMAT_NAME",
            "LIST_OUTPUT_FORMAT_PATH",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = ListOutputFormat;

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
                    "LIST_OUTPUT_FORMAT_UNSPECIFIED" => Ok(ListOutputFormat::Unspecified),
                    "LIST_OUTPUT_FORMAT_JSON" => Ok(ListOutputFormat::Json),
                    "LIST_OUTPUT_FORMAT_SELECTOR" => Ok(ListOutputFormat::Selector),
                    "LIST_OUTPUT_FORMAT_NAME" => Ok(ListOutputFormat::Name),
                    "LIST_OUTPUT_FORMAT_PATH" => Ok(ListOutputFormat::Path),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
impl serde::Serialize for LogMessage {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if self.code.is_some() {
            len += 1;
        }
        if self.code_name.is_some() {
            len += 1;
        }
        if self.dbt_core_event_code.is_some() {
            len += 1;
        }
        if self.original_severity_number != 0 {
            len += 1;
        }
        if !self.original_severity_text.is_empty() {
            len += 1;
        }
        if self.unique_id.is_some() {
            len += 1;
        }
        if self.file.is_some() {
            len += 1;
        }
        if self.line.is_some() {
            len += 1;
        }
        if self.phase.is_some() {
            len += 1;
        }
        if self.package_name.is_some() {
            len += 1;
        }
        if self.relative_path.is_some() {
            len += 1;
        }
        if self.code_line.is_some() {
            len += 1;
        }
        if self.code_column.is_some() {
            len += 1;
        }
        if self.expanded_relative_path.is_some() {
            len += 1;
        }
        if self.expanded_line.is_some() {
            len += 1;
        }
        if self.expanded_column.is_some() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.log.LogMessage", len)?;
        if let Some(v) = self.code.as_ref() {
            struct_ser.serialize_field("code", v)?;
        }
        if let Some(v) = self.code_name.as_ref() {
            struct_ser.serialize_field("code_name", v)?;
        }
        if let Some(v) = self.dbt_core_event_code.as_ref() {
            struct_ser.serialize_field("dbt_core_event_code", v)?;
        }
        if self.original_severity_number != 0 {
            let v = super::compat::SeverityNumber::try_from(self.original_severity_number)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.original_severity_number)))?;
            struct_ser.serialize_field("original_severity_number", &v)?;
        }
        if !self.original_severity_text.is_empty() {
            struct_ser.serialize_field("original_severity_text", &self.original_severity_text)?;
        }
        if let Some(v) = self.unique_id.as_ref() {
            struct_ser.serialize_field("unique_id", v)?;
        }
        if let Some(v) = self.file.as_ref() {
            struct_ser.serialize_field("file", v)?;
        }
        if let Some(v) = self.line.as_ref() {
            struct_ser.serialize_field("line", v)?;
        }
        if let Some(v) = self.phase.as_ref() {
            let v = super::phase::ExecutionPhase::try_from(*v)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", *v)))?;
            struct_ser.serialize_field("phase", &v)?;
        }
        if let Some(v) = self.package_name.as_ref() {
            struct_ser.serialize_field("package_name", v)?;
        }
        if let Some(v) = self.relative_path.as_ref() {
            struct_ser.serialize_field("relative_path", v)?;
        }
        if let Some(v) = self.code_line.as_ref() {
            struct_ser.serialize_field("code_line", v)?;
        }
        if let Some(v) = self.code_column.as_ref() {
            struct_ser.serialize_field("code_column", v)?;
        }
        if let Some(v) = self.expanded_relative_path.as_ref() {
            struct_ser.serialize_field("expanded_relative_path", v)?;
        }
        if let Some(v) = self.expanded_line.as_ref() {
            struct_ser.serialize_field("expanded_line", v)?;
        }
        if let Some(v) = self.expanded_column.as_ref() {
            struct_ser.serialize_field("expanded_column", v)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for LogMessage {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "code",
            "code_name",
            "codeName",
            "dbt_core_event_code",
            "dbtCoreEventCode",
            "original_severity_number",
            "originalSeverityNumber",
            "original_severity_text",
            "originalSeverityText",
            "unique_id",
            "uniqueId",
            "file",
            "line",
            "phase",
            "package_name",
            "packageName",
            "relative_path",
            "relativePath",
            "code_line",
            "codeLine",
            "code_column",
            "codeColumn",
            "expanded_relative_path",
            "expandedRelativePath",
            "expanded_line",
            "expandedLine",
            "expanded_column",
            "expandedColumn",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            Code,
            CodeName,
            DbtCoreEventCode,
            OriginalSeverityNumber,
            OriginalSeverityText,
            UniqueId,
            File,
            Line,
            Phase,
            PackageName,
            RelativePath,
            CodeLine,
            CodeColumn,
            ExpandedRelativePath,
            ExpandedLine,
            ExpandedColumn,
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
                            "code" => Ok(GeneratedField::Code),
                            "codeName" | "code_name" => Ok(GeneratedField::CodeName),
                            "dbtCoreEventCode" | "dbt_core_event_code" => Ok(GeneratedField::DbtCoreEventCode),
                            "originalSeverityNumber" | "original_severity_number" => Ok(GeneratedField::OriginalSeverityNumber),
                            "originalSeverityText" | "original_severity_text" => Ok(GeneratedField::OriginalSeverityText),
                            "uniqueId" | "unique_id" => Ok(GeneratedField::UniqueId),
                            "file" => Ok(GeneratedField::File),
                            "line" => Ok(GeneratedField::Line),
                            "phase" => Ok(GeneratedField::Phase),
                            "packageName" | "package_name" => Ok(GeneratedField::PackageName),
                            "relativePath" | "relative_path" => Ok(GeneratedField::RelativePath),
                            "codeLine" | "code_line" => Ok(GeneratedField::CodeLine),
                            "codeColumn" | "code_column" => Ok(GeneratedField::CodeColumn),
                            "expandedRelativePath" | "expanded_relative_path" => Ok(GeneratedField::ExpandedRelativePath),
                            "expandedLine" | "expanded_line" => Ok(GeneratedField::ExpandedLine),
                            "expandedColumn" | "expanded_column" => Ok(GeneratedField::ExpandedColumn),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = LogMessage;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.log.LogMessage")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<LogMessage, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut code__ = None;
                let mut code_name__ = None;
                let mut dbt_core_event_code__ = None;
                let mut original_severity_number__ = None;
                let mut original_severity_text__ = None;
                let mut unique_id__ = None;
                let mut file__ = None;
                let mut line__ = None;
                let mut phase__ = None;
                let mut package_name__ = None;
                let mut relative_path__ = None;
                let mut code_line__ = None;
                let mut code_column__ = None;
                let mut expanded_relative_path__ = None;
                let mut expanded_line__ = None;
                let mut expanded_column__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::Code => {
                            if code__.is_some() {
                                return Err(serde::de::Error::duplicate_field("code"));
                            }
                            code__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::CodeName => {
                            if code_name__.is_some() {
                                return Err(serde::de::Error::duplicate_field("codeName"));
                            }
                            code_name__ = map_.next_value()?;
                        }
                        GeneratedField::DbtCoreEventCode => {
                            if dbt_core_event_code__.is_some() {
                                return Err(serde::de::Error::duplicate_field("dbtCoreEventCode"));
                            }
                            dbt_core_event_code__ = map_.next_value()?;
                        }
                        GeneratedField::OriginalSeverityNumber => {
                            if original_severity_number__.is_some() {
                                return Err(serde::de::Error::duplicate_field("originalSeverityNumber"));
                            }
                            original_severity_number__ = Some(map_.next_value::<super::compat::SeverityNumber>()? as i32);
                        }
                        GeneratedField::OriginalSeverityText => {
                            if original_severity_text__.is_some() {
                                return Err(serde::de::Error::duplicate_field("originalSeverityText"));
                            }
                            original_severity_text__ = Some(map_.next_value()?);
                        }
                        GeneratedField::UniqueId => {
                            if unique_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("uniqueId"));
                            }
                            unique_id__ = map_.next_value()?;
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
                        GeneratedField::Phase => {
                            if phase__.is_some() {
                                return Err(serde::de::Error::duplicate_field("phase"));
                            }
                            phase__ = map_.next_value::<::std::option::Option<super::phase::ExecutionPhase>>()?.map(|x| x as i32);
                        }
                        GeneratedField::PackageName => {
                            if package_name__.is_some() {
                                return Err(serde::de::Error::duplicate_field("packageName"));
                            }
                            package_name__ = map_.next_value()?;
                        }
                        GeneratedField::RelativePath => {
                            if relative_path__.is_some() {
                                return Err(serde::de::Error::duplicate_field("relativePath"));
                            }
                            relative_path__ = map_.next_value()?;
                        }
                        GeneratedField::CodeLine => {
                            if code_line__.is_some() {
                                return Err(serde::de::Error::duplicate_field("codeLine"));
                            }
                            code_line__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::CodeColumn => {
                            if code_column__.is_some() {
                                return Err(serde::de::Error::duplicate_field("codeColumn"));
                            }
                            code_column__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::ExpandedRelativePath => {
                            if expanded_relative_path__.is_some() {
                                return Err(serde::de::Error::duplicate_field("expandedRelativePath"));
                            }
                            expanded_relative_path__ = map_.next_value()?;
                        }
                        GeneratedField::ExpandedLine => {
                            if expanded_line__.is_some() {
                                return Err(serde::de::Error::duplicate_field("expandedLine"));
                            }
                            expanded_line__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::ExpandedColumn => {
                            if expanded_column__.is_some() {
                                return Err(serde::de::Error::duplicate_field("expandedColumn"));
                            }
                            expanded_column__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(LogMessage {
                    code: code__,
                    code_name: code_name__,
                    dbt_core_event_code: dbt_core_event_code__,
                    original_severity_number: original_severity_number__.unwrap_or_default(),
                    original_severity_text: original_severity_text__.unwrap_or_default(),
                    unique_id: unique_id__,
                    file: file__,
                    line: line__,
                    phase: phase__,
                    package_name: package_name__,
                    relative_path: relative_path__,
                    code_line: code_line__,
                    code_column: code_column__,
                    expanded_relative_path: expanded_relative_path__,
                    expanded_line: expanded_line__,
                    expanded_column: expanded_column__,
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.log.LogMessage", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for ProgressMessage {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if self.dbt_core_event_code.is_some() {
            len += 1;
        }
        if !self.action.is_empty() {
            len += 1;
        }
        if !self.target.is_empty() {
            len += 1;
        }
        if self.description.is_some() {
            len += 1;
        }
        if self.unique_id.is_some() {
            len += 1;
        }
        if self.file.is_some() {
            len += 1;
        }
        if self.line.is_some() {
            len += 1;
        }
        if self.phase.is_some() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.log.ProgressMessage", len)?;
        if let Some(v) = self.dbt_core_event_code.as_ref() {
            struct_ser.serialize_field("dbt_core_event_code", v)?;
        }
        if !self.action.is_empty() {
            struct_ser.serialize_field("action", &self.action)?;
        }
        if !self.target.is_empty() {
            struct_ser.serialize_field("target", &self.target)?;
        }
        if let Some(v) = self.description.as_ref() {
            struct_ser.serialize_field("description", v)?;
        }
        if let Some(v) = self.unique_id.as_ref() {
            struct_ser.serialize_field("unique_id", v)?;
        }
        if let Some(v) = self.file.as_ref() {
            struct_ser.serialize_field("file", v)?;
        }
        if let Some(v) = self.line.as_ref() {
            struct_ser.serialize_field("line", v)?;
        }
        if let Some(v) = self.phase.as_ref() {
            let v = super::phase::ExecutionPhase::try_from(*v)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", *v)))?;
            struct_ser.serialize_field("phase", &v)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for ProgressMessage {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "dbt_core_event_code",
            "dbtCoreEventCode",
            "action",
            "target",
            "description",
            "unique_id",
            "uniqueId",
            "file",
            "line",
            "phase",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            DbtCoreEventCode,
            Action,
            Target,
            Description,
            UniqueId,
            File,
            Line,
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
                            "dbtCoreEventCode" | "dbt_core_event_code" => Ok(GeneratedField::DbtCoreEventCode),
                            "action" => Ok(GeneratedField::Action),
                            "target" => Ok(GeneratedField::Target),
                            "description" => Ok(GeneratedField::Description),
                            "uniqueId" | "unique_id" => Ok(GeneratedField::UniqueId),
                            "file" => Ok(GeneratedField::File),
                            "line" => Ok(GeneratedField::Line),
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
            type Value = ProgressMessage;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.log.ProgressMessage")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<ProgressMessage, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut dbt_core_event_code__ = None;
                let mut action__ = None;
                let mut target__ = None;
                let mut description__ = None;
                let mut unique_id__ = None;
                let mut file__ = None;
                let mut line__ = None;
                let mut phase__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::DbtCoreEventCode => {
                            if dbt_core_event_code__.is_some() {
                                return Err(serde::de::Error::duplicate_field("dbtCoreEventCode"));
                            }
                            dbt_core_event_code__ = map_.next_value()?;
                        }
                        GeneratedField::Action => {
                            if action__.is_some() {
                                return Err(serde::de::Error::duplicate_field("action"));
                            }
                            action__ = Some(map_.next_value()?);
                        }
                        GeneratedField::Target => {
                            if target__.is_some() {
                                return Err(serde::de::Error::duplicate_field("target"));
                            }
                            target__ = Some(map_.next_value()?);
                        }
                        GeneratedField::Description => {
                            if description__.is_some() {
                                return Err(serde::de::Error::duplicate_field("description"));
                            }
                            description__ = map_.next_value()?;
                        }
                        GeneratedField::UniqueId => {
                            if unique_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("uniqueId"));
                            }
                            unique_id__ = map_.next_value()?;
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
                        GeneratedField::Phase => {
                            if phase__.is_some() {
                                return Err(serde::de::Error::duplicate_field("phase"));
                            }
                            phase__ = map_.next_value::<::std::option::Option<super::phase::ExecutionPhase>>()?.map(|x| x as i32);
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(ProgressMessage {
                    dbt_core_event_code: dbt_core_event_code__,
                    action: action__.unwrap_or_default(),
                    target: target__.unwrap_or_default(),
                    description: description__,
                    unique_id: unique_id__,
                    file: file__,
                    line: line__,
                    phase: phase__,
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.log.ProgressMessage", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for ShowDataOutput {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if self.output_format != 0 {
            len += 1;
        }
        if !self.content.is_empty() {
            len += 1;
        }
        if !self.node_name.is_empty() {
            len += 1;
        }
        if self.is_inline {
            len += 1;
        }
        if self.unique_id.is_some() {
            len += 1;
        }
        if !self.columns.is_empty() {
            len += 1;
        }
        if !self.dbt_core_event_code.is_empty() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.log.ShowDataOutput", len)?;
        if self.output_format != 0 {
            let v = ShowDataOutputFormat::try_from(self.output_format)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.output_format)))?;
            struct_ser.serialize_field("output_format", &v)?;
        }
        if !self.content.is_empty() {
            struct_ser.serialize_field("content", &self.content)?;
        }
        if !self.node_name.is_empty() {
            struct_ser.serialize_field("node_name", &self.node_name)?;
        }
        if self.is_inline {
            struct_ser.serialize_field("is_inline", &self.is_inline)?;
        }
        if let Some(v) = self.unique_id.as_ref() {
            struct_ser.serialize_field("unique_id", v)?;
        }
        if !self.columns.is_empty() {
            struct_ser.serialize_field("columns", &self.columns)?;
        }
        if !self.dbt_core_event_code.is_empty() {
            struct_ser.serialize_field("dbt_core_event_code", &self.dbt_core_event_code)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for ShowDataOutput {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "output_format",
            "outputFormat",
            "content",
            "node_name",
            "nodeName",
            "is_inline",
            "isInline",
            "unique_id",
            "uniqueId",
            "columns",
            "dbt_core_event_code",
            "dbtCoreEventCode",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            OutputFormat,
            Content,
            NodeName,
            IsInline,
            UniqueId,
            Columns,
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
                            "outputFormat" | "output_format" => Ok(GeneratedField::OutputFormat),
                            "content" => Ok(GeneratedField::Content),
                            "nodeName" | "node_name" => Ok(GeneratedField::NodeName),
                            "isInline" | "is_inline" => Ok(GeneratedField::IsInline),
                            "uniqueId" | "unique_id" => Ok(GeneratedField::UniqueId),
                            "columns" => Ok(GeneratedField::Columns),
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
            type Value = ShowDataOutput;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.log.ShowDataOutput")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<ShowDataOutput, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut output_format__ = None;
                let mut content__ = None;
                let mut node_name__ = None;
                let mut is_inline__ = None;
                let mut unique_id__ = None;
                let mut columns__ = None;
                let mut dbt_core_event_code__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::OutputFormat => {
                            if output_format__.is_some() {
                                return Err(serde::de::Error::duplicate_field("outputFormat"));
                            }
                            output_format__ = Some(map_.next_value::<ShowDataOutputFormat>()? as i32);
                        }
                        GeneratedField::Content => {
                            if content__.is_some() {
                                return Err(serde::de::Error::duplicate_field("content"));
                            }
                            content__ = Some(map_.next_value()?);
                        }
                        GeneratedField::NodeName => {
                            if node_name__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeName"));
                            }
                            node_name__ = Some(map_.next_value()?);
                        }
                        GeneratedField::IsInline => {
                            if is_inline__.is_some() {
                                return Err(serde::de::Error::duplicate_field("isInline"));
                            }
                            is_inline__ = Some(map_.next_value()?);
                        }
                        GeneratedField::UniqueId => {
                            if unique_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("uniqueId"));
                            }
                            unique_id__ = map_.next_value()?;
                        }
                        GeneratedField::Columns => {
                            if columns__.is_some() {
                                return Err(serde::de::Error::duplicate_field("columns"));
                            }
                            columns__ = Some(map_.next_value()?);
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
                Ok(ShowDataOutput {
                    output_format: output_format__.unwrap_or_default(),
                    content: content__.unwrap_or_default(),
                    node_name: node_name__.unwrap_or_default(),
                    is_inline: is_inline__.unwrap_or_default(),
                    unique_id: unique_id__,
                    columns: columns__.unwrap_or_default(),
                    dbt_core_event_code: dbt_core_event_code__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.log.ShowDataOutput", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for ShowDataOutputFormat {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::Unspecified => "SHOW_DATA_OUTPUT_FORMAT_UNSPECIFIED",
            Self::Text => "SHOW_DATA_OUTPUT_FORMAT_TEXT",
            Self::Csv => "SHOW_DATA_OUTPUT_FORMAT_CSV",
            Self::Tsv => "SHOW_DATA_OUTPUT_FORMAT_TSV",
            Self::Json => "SHOW_DATA_OUTPUT_FORMAT_JSON",
            Self::Ndjson => "SHOW_DATA_OUTPUT_FORMAT_NDJSON",
            Self::Yml => "SHOW_DATA_OUTPUT_FORMAT_YML",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for ShowDataOutputFormat {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "SHOW_DATA_OUTPUT_FORMAT_UNSPECIFIED",
            "SHOW_DATA_OUTPUT_FORMAT_TEXT",
            "SHOW_DATA_OUTPUT_FORMAT_CSV",
            "SHOW_DATA_OUTPUT_FORMAT_TSV",
            "SHOW_DATA_OUTPUT_FORMAT_JSON",
            "SHOW_DATA_OUTPUT_FORMAT_NDJSON",
            "SHOW_DATA_OUTPUT_FORMAT_YML",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = ShowDataOutputFormat;

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
                    "SHOW_DATA_OUTPUT_FORMAT_UNSPECIFIED" => Ok(ShowDataOutputFormat::Unspecified),
                    "SHOW_DATA_OUTPUT_FORMAT_TEXT" => Ok(ShowDataOutputFormat::Text),
                    "SHOW_DATA_OUTPUT_FORMAT_CSV" => Ok(ShowDataOutputFormat::Csv),
                    "SHOW_DATA_OUTPUT_FORMAT_TSV" => Ok(ShowDataOutputFormat::Tsv),
                    "SHOW_DATA_OUTPUT_FORMAT_JSON" => Ok(ShowDataOutputFormat::Json),
                    "SHOW_DATA_OUTPUT_FORMAT_NDJSON" => Ok(ShowDataOutputFormat::Ndjson),
                    "SHOW_DATA_OUTPUT_FORMAT_YML" => Ok(ShowDataOutputFormat::Yml),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
impl serde::Serialize for ShowResult {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if self.output_format != 0 {
            len += 1;
        }
        if !self.content.is_empty() {
            len += 1;
        }
        if !self.result_type.is_empty() {
            len += 1;
        }
        if !self.title.is_empty() {
            len += 1;
        }
        if self.unique_id.is_some() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.log.ShowResult", len)?;
        if self.output_format != 0 {
            let v = ShowResultOutputFormat::try_from(self.output_format)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.output_format)))?;
            struct_ser.serialize_field("output_format", &v)?;
        }
        if !self.content.is_empty() {
            struct_ser.serialize_field("content", &self.content)?;
        }
        if !self.result_type.is_empty() {
            struct_ser.serialize_field("result_type", &self.result_type)?;
        }
        if !self.title.is_empty() {
            struct_ser.serialize_field("title", &self.title)?;
        }
        if let Some(v) = self.unique_id.as_ref() {
            struct_ser.serialize_field("unique_id", v)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for ShowResult {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "output_format",
            "outputFormat",
            "content",
            "result_type",
            "resultType",
            "title",
            "unique_id",
            "uniqueId",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            OutputFormat,
            Content,
            ResultType,
            Title,
            UniqueId,
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
                            "outputFormat" | "output_format" => Ok(GeneratedField::OutputFormat),
                            "content" => Ok(GeneratedField::Content),
                            "resultType" | "result_type" => Ok(GeneratedField::ResultType),
                            "title" => Ok(GeneratedField::Title),
                            "uniqueId" | "unique_id" => Ok(GeneratedField::UniqueId),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = ShowResult;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.log.ShowResult")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<ShowResult, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut output_format__ = None;
                let mut content__ = None;
                let mut result_type__ = None;
                let mut title__ = None;
                let mut unique_id__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::OutputFormat => {
                            if output_format__.is_some() {
                                return Err(serde::de::Error::duplicate_field("outputFormat"));
                            }
                            output_format__ = Some(map_.next_value::<ShowResultOutputFormat>()? as i32);
                        }
                        GeneratedField::Content => {
                            if content__.is_some() {
                                return Err(serde::de::Error::duplicate_field("content"));
                            }
                            content__ = Some(map_.next_value()?);
                        }
                        GeneratedField::ResultType => {
                            if result_type__.is_some() {
                                return Err(serde::de::Error::duplicate_field("resultType"));
                            }
                            result_type__ = Some(map_.next_value()?);
                        }
                        GeneratedField::Title => {
                            if title__.is_some() {
                                return Err(serde::de::Error::duplicate_field("title"));
                            }
                            title__ = Some(map_.next_value()?);
                        }
                        GeneratedField::UniqueId => {
                            if unique_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("uniqueId"));
                            }
                            unique_id__ = map_.next_value()?;
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(ShowResult {
                    output_format: output_format__.unwrap_or_default(),
                    content: content__.unwrap_or_default(),
                    result_type: result_type__.unwrap_or_default(),
                    title: title__.unwrap_or_default(),
                    unique_id: unique_id__,
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.log.ShowResult", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for ShowResultOutputFormat {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::Unspecified => "SHOW_RESULT_OUTPUT_FORMAT_UNSPECIFIED",
            Self::Text => "SHOW_RESULT_OUTPUT_FORMAT_TEXT",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for ShowResultOutputFormat {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "SHOW_RESULT_OUTPUT_FORMAT_UNSPECIFIED",
            "SHOW_RESULT_OUTPUT_FORMAT_TEXT",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = ShowResultOutputFormat;

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
                    "SHOW_RESULT_OUTPUT_FORMAT_UNSPECIFIED" => Ok(ShowResultOutputFormat::Unspecified),
                    "SHOW_RESULT_OUTPUT_FORMAT_TEXT" => Ok(ShowResultOutputFormat::Text),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
impl serde::Serialize for StateModifiedDiff {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if self.unique_id.is_some() {
            len += 1;
        }
        if !self.node_type_or_category.is_empty() {
            len += 1;
        }
        if !self.check.is_empty() {
            len += 1;
        }
        if self.self_value.is_some() {
            len += 1;
        }
        if self.other_value.is_some() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.log.StateModifiedDiff", len)?;
        if let Some(v) = self.unique_id.as_ref() {
            struct_ser.serialize_field("unique_id", v)?;
        }
        if !self.node_type_or_category.is_empty() {
            struct_ser.serialize_field("node_type_or_category", &self.node_type_or_category)?;
        }
        if !self.check.is_empty() {
            struct_ser.serialize_field("check", &self.check)?;
        }
        if let Some(v) = self.self_value.as_ref() {
            struct_ser.serialize_field("self_value", v)?;
        }
        if let Some(v) = self.other_value.as_ref() {
            struct_ser.serialize_field("other_value", v)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for StateModifiedDiff {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "unique_id",
            "uniqueId",
            "node_type_or_category",
            "nodeTypeOrCategory",
            "check",
            "self_value",
            "selfValue",
            "other_value",
            "otherValue",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            UniqueId,
            NodeTypeOrCategory,
            Check,
            SelfValue,
            OtherValue,
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
                            "nodeTypeOrCategory" | "node_type_or_category" => Ok(GeneratedField::NodeTypeOrCategory),
                            "check" => Ok(GeneratedField::Check),
                            "selfValue" | "self_value" => Ok(GeneratedField::SelfValue),
                            "otherValue" | "other_value" => Ok(GeneratedField::OtherValue),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = StateModifiedDiff;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.log.StateModifiedDiff")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<StateModifiedDiff, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut unique_id__ = None;
                let mut node_type_or_category__ = None;
                let mut check__ = None;
                let mut self_value__ = None;
                let mut other_value__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::UniqueId => {
                            if unique_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("uniqueId"));
                            }
                            unique_id__ = map_.next_value()?;
                        }
                        GeneratedField::NodeTypeOrCategory => {
                            if node_type_or_category__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeTypeOrCategory"));
                            }
                            node_type_or_category__ = Some(map_.next_value()?);
                        }
                        GeneratedField::Check => {
                            if check__.is_some() {
                                return Err(serde::de::Error::duplicate_field("check"));
                            }
                            check__ = Some(map_.next_value()?);
                        }
                        GeneratedField::SelfValue => {
                            if self_value__.is_some() {
                                return Err(serde::de::Error::duplicate_field("selfValue"));
                            }
                            self_value__ = map_.next_value()?;
                        }
                        GeneratedField::OtherValue => {
                            if other_value__.is_some() {
                                return Err(serde::de::Error::duplicate_field("otherValue"));
                            }
                            other_value__ = map_.next_value()?;
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(StateModifiedDiff {
                    unique_id: unique_id__,
                    node_type_or_category: node_type_or_category__.unwrap_or_default(),
                    check: check__.unwrap_or_default(),
                    self_value: self_value__,
                    other_value: other_value__,
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.log.StateModifiedDiff", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for UserLogMessage {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if self.is_print {
            len += 1;
        }
        if !self.dbt_core_event_code.is_empty() {
            len += 1;
        }
        if self.unique_id.is_some() {
            len += 1;
        }
        if self.phase.is_some() {
            len += 1;
        }
        if self.package_name.is_some() {
            len += 1;
        }
        if self.line.is_some() {
            len += 1;
        }
        if self.column.is_some() {
            len += 1;
        }
        if self.relative_path.is_some() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.log.UserLogMessage", len)?;
        if self.is_print {
            struct_ser.serialize_field("is_print", &self.is_print)?;
        }
        if !self.dbt_core_event_code.is_empty() {
            struct_ser.serialize_field("dbt_core_event_code", &self.dbt_core_event_code)?;
        }
        if let Some(v) = self.unique_id.as_ref() {
            struct_ser.serialize_field("unique_id", v)?;
        }
        if let Some(v) = self.phase.as_ref() {
            let v = super::phase::ExecutionPhase::try_from(*v)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", *v)))?;
            struct_ser.serialize_field("phase", &v)?;
        }
        if let Some(v) = self.package_name.as_ref() {
            struct_ser.serialize_field("package_name", v)?;
        }
        if let Some(v) = self.line.as_ref() {
            struct_ser.serialize_field("line", v)?;
        }
        if let Some(v) = self.column.as_ref() {
            struct_ser.serialize_field("column", v)?;
        }
        if let Some(v) = self.relative_path.as_ref() {
            struct_ser.serialize_field("relative_path", v)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for UserLogMessage {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "is_print",
            "isPrint",
            "dbt_core_event_code",
            "dbtCoreEventCode",
            "unique_id",
            "uniqueId",
            "phase",
            "package_name",
            "packageName",
            "line",
            "column",
            "relative_path",
            "relativePath",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            IsPrint,
            DbtCoreEventCode,
            UniqueId,
            Phase,
            PackageName,
            Line,
            Column,
            RelativePath,
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
                            "isPrint" | "is_print" => Ok(GeneratedField::IsPrint),
                            "dbtCoreEventCode" | "dbt_core_event_code" => Ok(GeneratedField::DbtCoreEventCode),
                            "uniqueId" | "unique_id" => Ok(GeneratedField::UniqueId),
                            "phase" => Ok(GeneratedField::Phase),
                            "packageName" | "package_name" => Ok(GeneratedField::PackageName),
                            "line" => Ok(GeneratedField::Line),
                            "column" => Ok(GeneratedField::Column),
                            "relativePath" | "relative_path" => Ok(GeneratedField::RelativePath),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = UserLogMessage;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.log.UserLogMessage")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<UserLogMessage, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut is_print__ = None;
                let mut dbt_core_event_code__ = None;
                let mut unique_id__ = None;
                let mut phase__ = None;
                let mut package_name__ = None;
                let mut line__ = None;
                let mut column__ = None;
                let mut relative_path__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::IsPrint => {
                            if is_print__.is_some() {
                                return Err(serde::de::Error::duplicate_field("isPrint"));
                            }
                            is_print__ = Some(map_.next_value()?);
                        }
                        GeneratedField::DbtCoreEventCode => {
                            if dbt_core_event_code__.is_some() {
                                return Err(serde::de::Error::duplicate_field("dbtCoreEventCode"));
                            }
                            dbt_core_event_code__ = Some(map_.next_value()?);
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
                            phase__ = map_.next_value::<::std::option::Option<super::phase::ExecutionPhase>>()?.map(|x| x as i32);
                        }
                        GeneratedField::PackageName => {
                            if package_name__.is_some() {
                                return Err(serde::de::Error::duplicate_field("packageName"));
                            }
                            package_name__ = map_.next_value()?;
                        }
                        GeneratedField::Line => {
                            if line__.is_some() {
                                return Err(serde::de::Error::duplicate_field("line"));
                            }
                            line__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::Column => {
                            if column__.is_some() {
                                return Err(serde::de::Error::duplicate_field("column"));
                            }
                            column__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::RelativePath => {
                            if relative_path__.is_some() {
                                return Err(serde::de::Error::duplicate_field("relativePath"));
                            }
                            relative_path__ = map_.next_value()?;
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(UserLogMessage {
                    is_print: is_print__.unwrap_or_default(),
                    dbt_core_event_code: dbt_core_event_code__.unwrap_or_default(),
                    unique_id: unique_id__,
                    phase: phase__,
                    package_name: package_name__,
                    line: line__,
                    column: column__,
                    relative_path: relative_path__,
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.log.UserLogMessage", FIELDS, GeneratedVisitor)
    }
}
