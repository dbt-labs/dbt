impl serde::Serialize for GenericOpExecuted {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if !self.operation_id.is_empty() {
            len += 1;
        }
        if !self.display_action.is_empty() {
            len += 1;
        }
        if self.item_count_total.is_some() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.generic.GenericOpExecuted", len)?;
        if !self.operation_id.is_empty() {
            struct_ser.serialize_field("operation_id", &self.operation_id)?;
        }
        if !self.display_action.is_empty() {
            struct_ser.serialize_field("display_action", &self.display_action)?;
        }
        if let Some(v) = self.item_count_total.as_ref() {
            #[allow(clippy::needless_borrow)]
            #[allow(clippy::needless_borrows_for_generic_args)]
            struct_ser.serialize_field("item_count_total", ToString::to_string(&v).as_str())?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for GenericOpExecuted {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "operation_id",
            "operationId",
            "display_action",
            "displayAction",
            "item_count_total",
            "itemCountTotal",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            OperationId,
            DisplayAction,
            ItemCountTotal,
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
                            "operationId" | "operation_id" => Ok(GeneratedField::OperationId),
                            "displayAction" | "display_action" => Ok(GeneratedField::DisplayAction),
                            "itemCountTotal" | "item_count_total" => Ok(GeneratedField::ItemCountTotal),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = GenericOpExecuted;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.generic.GenericOpExecuted")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<GenericOpExecuted, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut operation_id__ = None;
                let mut display_action__ = None;
                let mut item_count_total__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::OperationId => {
                            if operation_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("operationId"));
                            }
                            operation_id__ = Some(map_.next_value()?);
                        }
                        GeneratedField::DisplayAction => {
                            if display_action__.is_some() {
                                return Err(serde::de::Error::duplicate_field("displayAction"));
                            }
                            display_action__ = Some(map_.next_value()?);
                        }
                        GeneratedField::ItemCountTotal => {
                            if item_count_total__.is_some() {
                                return Err(serde::de::Error::duplicate_field("itemCountTotal"));
                            }
                            item_count_total__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(GenericOpExecuted {
                    operation_id: operation_id__.unwrap_or_default(),
                    display_action: display_action__.unwrap_or_default(),
                    item_count_total: item_count_total__,
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.generic.GenericOpExecuted", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for GenericOpItemProcessed {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if !self.operation_id.is_empty() {
            len += 1;
        }
        if !self.display_in_progress_action.is_empty() {
            len += 1;
        }
        if !self.display_on_success_action.is_empty() {
            len += 1;
        }
        if !self.target.is_empty() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.generic.GenericOpItemProcessed", len)?;
        if !self.operation_id.is_empty() {
            struct_ser.serialize_field("operation_id", &self.operation_id)?;
        }
        if !self.display_in_progress_action.is_empty() {
            struct_ser.serialize_field("display_in_progress_action", &self.display_in_progress_action)?;
        }
        if !self.display_on_success_action.is_empty() {
            struct_ser.serialize_field("display_on_success_action", &self.display_on_success_action)?;
        }
        if !self.target.is_empty() {
            struct_ser.serialize_field("target", &self.target)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for GenericOpItemProcessed {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "operation_id",
            "operationId",
            "display_in_progress_action",
            "displayInProgressAction",
            "display_on_success_action",
            "displayOnSuccessAction",
            "target",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            OperationId,
            DisplayInProgressAction,
            DisplayOnSuccessAction,
            Target,
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
                            "operationId" | "operation_id" => Ok(GeneratedField::OperationId),
                            "displayInProgressAction" | "display_in_progress_action" => Ok(GeneratedField::DisplayInProgressAction),
                            "displayOnSuccessAction" | "display_on_success_action" => Ok(GeneratedField::DisplayOnSuccessAction),
                            "target" => Ok(GeneratedField::Target),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = GenericOpItemProcessed;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.generic.GenericOpItemProcessed")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<GenericOpItemProcessed, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut operation_id__ = None;
                let mut display_in_progress_action__ = None;
                let mut display_on_success_action__ = None;
                let mut target__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::OperationId => {
                            if operation_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("operationId"));
                            }
                            operation_id__ = Some(map_.next_value()?);
                        }
                        GeneratedField::DisplayInProgressAction => {
                            if display_in_progress_action__.is_some() {
                                return Err(serde::de::Error::duplicate_field("displayInProgressAction"));
                            }
                            display_in_progress_action__ = Some(map_.next_value()?);
                        }
                        GeneratedField::DisplayOnSuccessAction => {
                            if display_on_success_action__.is_some() {
                                return Err(serde::de::Error::duplicate_field("displayOnSuccessAction"));
                            }
                            display_on_success_action__ = Some(map_.next_value()?);
                        }
                        GeneratedField::Target => {
                            if target__.is_some() {
                                return Err(serde::de::Error::duplicate_field("target"));
                            }
                            target__ = Some(map_.next_value()?);
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(GenericOpItemProcessed {
                    operation_id: operation_id__.unwrap_or_default(),
                    display_in_progress_action: display_in_progress_action__.unwrap_or_default(),
                    display_on_success_action: display_on_success_action__.unwrap_or_default(),
                    target: target__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.generic.GenericOpItemProcessed", FIELDS, GeneratedVisitor)
    }
}
