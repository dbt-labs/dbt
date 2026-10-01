impl serde::Serialize for Invocation {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if !self.invocation_id.is_empty() {
            len += 1;
        }
        if !self.raw_command.is_empty() {
            len += 1;
        }
        if self.eval_args.is_some() {
            len += 1;
        }
        if self.process_info.is_some() {
            len += 1;
        }
        if self.metrics.is_some() {
            len += 1;
        }
        if self.parent_span_id.is_some() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.invocation.Invocation", len)?;
        if !self.invocation_id.is_empty() {
            struct_ser.serialize_field("invocation_id", &self.invocation_id)?;
        }
        if !self.raw_command.is_empty() {
            struct_ser.serialize_field("raw_command", &self.raw_command)?;
        }
        if let Some(v) = self.eval_args.as_ref() {
            struct_ser.serialize_field("eval_args", v)?;
        }
        if let Some(v) = self.process_info.as_ref() {
            struct_ser.serialize_field("process_info", v)?;
        }
        if let Some(v) = self.metrics.as_ref() {
            struct_ser.serialize_field("metrics", v)?;
        }
        if let Some(v) = self.parent_span_id.as_ref() {
            #[allow(clippy::needless_borrow)]
            #[allow(clippy::needless_borrows_for_generic_args)]
            struct_ser.serialize_field("parent_span_id", ToString::to_string(&v).as_str())?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for Invocation {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "invocation_id",
            "invocationId",
            "raw_command",
            "rawCommand",
            "eval_args",
            "evalArgs",
            "process_info",
            "processInfo",
            "metrics",
            "parent_span_id",
            "parentSpanId",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            InvocationId,
            RawCommand,
            EvalArgs,
            ProcessInfo,
            Metrics,
            ParentSpanId,
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
                            "invocationId" | "invocation_id" => Ok(GeneratedField::InvocationId),
                            "rawCommand" | "raw_command" => Ok(GeneratedField::RawCommand),
                            "evalArgs" | "eval_args" => Ok(GeneratedField::EvalArgs),
                            "processInfo" | "process_info" => Ok(GeneratedField::ProcessInfo),
                            "metrics" => Ok(GeneratedField::Metrics),
                            "parentSpanId" | "parent_span_id" => Ok(GeneratedField::ParentSpanId),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = Invocation;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.invocation.Invocation")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<Invocation, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut invocation_id__ = None;
                let mut raw_command__ = None;
                let mut eval_args__ = None;
                let mut process_info__ = None;
                let mut metrics__ = None;
                let mut parent_span_id__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::InvocationId => {
                            if invocation_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("invocationId"));
                            }
                            invocation_id__ = Some(map_.next_value()?);
                        }
                        GeneratedField::RawCommand => {
                            if raw_command__.is_some() {
                                return Err(serde::de::Error::duplicate_field("rawCommand"));
                            }
                            raw_command__ = Some(map_.next_value()?);
                        }
                        GeneratedField::EvalArgs => {
                            if eval_args__.is_some() {
                                return Err(serde::de::Error::duplicate_field("evalArgs"));
                            }
                            eval_args__ = map_.next_value()?;
                        }
                        GeneratedField::ProcessInfo => {
                            if process_info__.is_some() {
                                return Err(serde::de::Error::duplicate_field("processInfo"));
                            }
                            process_info__ = map_.next_value()?;
                        }
                        GeneratedField::Metrics => {
                            if metrics__.is_some() {
                                return Err(serde::de::Error::duplicate_field("metrics"));
                            }
                            metrics__ = map_.next_value()?;
                        }
                        GeneratedField::ParentSpanId => {
                            if parent_span_id__.is_some() {
                                return Err(serde::de::Error::duplicate_field("parentSpanId"));
                            }
                            parent_span_id__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(Invocation {
                    invocation_id: invocation_id__.unwrap_or_default(),
                    raw_command: raw_command__.unwrap_or_default(),
                    eval_args: eval_args__,
                    process_info: process_info__,
                    metrics: metrics__,
                    parent_span_id: parent_span_id__,
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.invocation.Invocation", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for InvocationEvalArgs {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if !self.command.is_empty() {
            len += 1;
        }
        if self.profiles_dir.is_some() {
            len += 1;
        }
        if self.packages_install_path.is_some() {
            len += 1;
        }
        if self.target.is_some() {
            len += 1;
        }
        if self.profile.is_some() {
            len += 1;
        }
        if !self.vars.is_empty() {
            len += 1;
        }
        if self.limit.is_some() {
            len += 1;
        }
        if self.num_threads.is_some() {
            len += 1;
        }
        if self.selector.is_some() {
            len += 1;
        }
        if !self.select.is_empty() {
            len += 1;
        }
        if !self.exclude.is_empty() {
            len += 1;
        }
        if self.indirect_selection.is_some() {
            len += 1;
        }
        if !self.output_keys.is_empty() {
            len += 1;
        }
        if !self.resource_types.is_empty() {
            len += 1;
        }
        if !self.exclude_resource_types.is_empty() {
            len += 1;
        }
        if self.debug.is_some() {
            len += 1;
        }
        if self.log_format.is_some() {
            len += 1;
        }
        if self.log_level.is_some() {
            len += 1;
        }
        if self.log_path.is_some() {
            len += 1;
        }
        if self.target_path.is_some() {
            len += 1;
        }
        if self.project_dir.is_some() {
            len += 1;
        }
        if self.quiet.is_some() {
            len += 1;
        }
        if self.write_json.is_some() {
            len += 1;
        }
        if self.write_catalog.is_some() {
            len += 1;
        }
        if self.manage_state.is_some() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.invocation.InvocationEvalArgs", len)?;
        if !self.command.is_empty() {
            struct_ser.serialize_field("command", &self.command)?;
        }
        if let Some(v) = self.profiles_dir.as_ref() {
            struct_ser.serialize_field("profiles_dir", v)?;
        }
        if let Some(v) = self.packages_install_path.as_ref() {
            struct_ser.serialize_field("packages_install_path", v)?;
        }
        if let Some(v) = self.target.as_ref() {
            struct_ser.serialize_field("target", v)?;
        }
        if let Some(v) = self.profile.as_ref() {
            struct_ser.serialize_field("profile", v)?;
        }
        if !self.vars.is_empty() {
            struct_ser.serialize_field("vars", &self.vars)?;
        }
        if let Some(v) = self.limit.as_ref() {
            #[allow(clippy::needless_borrow)]
            #[allow(clippy::needless_borrows_for_generic_args)]
            struct_ser.serialize_field("limit", ToString::to_string(&v).as_str())?;
        }
        if let Some(v) = self.num_threads.as_ref() {
            #[allow(clippy::needless_borrow)]
            #[allow(clippy::needless_borrows_for_generic_args)]
            struct_ser.serialize_field("num_threads", ToString::to_string(&v).as_str())?;
        }
        if let Some(v) = self.selector.as_ref() {
            struct_ser.serialize_field("selector", v)?;
        }
        if !self.select.is_empty() {
            struct_ser.serialize_field("select", &self.select)?;
        }
        if !self.exclude.is_empty() {
            struct_ser.serialize_field("exclude", &self.exclude)?;
        }
        if let Some(v) = self.indirect_selection.as_ref() {
            struct_ser.serialize_field("indirect_selection", v)?;
        }
        if !self.output_keys.is_empty() {
            struct_ser.serialize_field("output_keys", &self.output_keys)?;
        }
        if !self.resource_types.is_empty() {
            struct_ser.serialize_field("resource_types", &self.resource_types)?;
        }
        if !self.exclude_resource_types.is_empty() {
            struct_ser.serialize_field("exclude_resource_types", &self.exclude_resource_types)?;
        }
        if let Some(v) = self.debug.as_ref() {
            struct_ser.serialize_field("debug", v)?;
        }
        if let Some(v) = self.log_format.as_ref() {
            struct_ser.serialize_field("log_format", v)?;
        }
        if let Some(v) = self.log_level.as_ref() {
            struct_ser.serialize_field("log_level", v)?;
        }
        if let Some(v) = self.log_path.as_ref() {
            struct_ser.serialize_field("log_path", v)?;
        }
        if let Some(v) = self.target_path.as_ref() {
            struct_ser.serialize_field("target_path", v)?;
        }
        if let Some(v) = self.project_dir.as_ref() {
            struct_ser.serialize_field("project_dir", v)?;
        }
        if let Some(v) = self.quiet.as_ref() {
            struct_ser.serialize_field("quiet", v)?;
        }
        if let Some(v) = self.write_json.as_ref() {
            struct_ser.serialize_field("write_json", v)?;
        }
        if let Some(v) = self.write_catalog.as_ref() {
            struct_ser.serialize_field("write_catalog", v)?;
        }
        if let Some(v) = self.manage_state.as_ref() {
            struct_ser.serialize_field("manage_state", v)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for InvocationEvalArgs {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "command",
            "profiles_dir",
            "profilesDir",
            "packages_install_path",
            "packagesInstallPath",
            "target",
            "profile",
            "vars",
            "limit",
            "num_threads",
            "numThreads",
            "selector",
            "select",
            "exclude",
            "indirect_selection",
            "indirectSelection",
            "output_keys",
            "outputKeys",
            "resource_types",
            "resourceTypes",
            "exclude_resource_types",
            "excludeResourceTypes",
            "debug",
            "log_format",
            "logFormat",
            "log_level",
            "logLevel",
            "log_path",
            "logPath",
            "target_path",
            "targetPath",
            "project_dir",
            "projectDir",
            "quiet",
            "write_json",
            "writeJson",
            "write_catalog",
            "writeCatalog",
            "manage_state",
            "manageState",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            Command,
            ProfilesDir,
            PackagesInstallPath,
            Target,
            Profile,
            Vars,
            Limit,
            NumThreads,
            Selector,
            Select,
            Exclude,
            IndirectSelection,
            OutputKeys,
            ResourceTypes,
            ExcludeResourceTypes,
            Debug,
            LogFormat,
            LogLevel,
            LogPath,
            TargetPath,
            ProjectDir,
            Quiet,
            WriteJson,
            WriteCatalog,
            ManageState,
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
                            "command" => Ok(GeneratedField::Command),
                            "profilesDir" | "profiles_dir" => Ok(GeneratedField::ProfilesDir),
                            "packagesInstallPath" | "packages_install_path" => Ok(GeneratedField::PackagesInstallPath),
                            "target" => Ok(GeneratedField::Target),
                            "profile" => Ok(GeneratedField::Profile),
                            "vars" => Ok(GeneratedField::Vars),
                            "limit" => Ok(GeneratedField::Limit),
                            "numThreads" | "num_threads" => Ok(GeneratedField::NumThreads),
                            "selector" => Ok(GeneratedField::Selector),
                            "select" => Ok(GeneratedField::Select),
                            "exclude" => Ok(GeneratedField::Exclude),
                            "indirectSelection" | "indirect_selection" => Ok(GeneratedField::IndirectSelection),
                            "outputKeys" | "output_keys" => Ok(GeneratedField::OutputKeys),
                            "resourceTypes" | "resource_types" => Ok(GeneratedField::ResourceTypes),
                            "excludeResourceTypes" | "exclude_resource_types" => Ok(GeneratedField::ExcludeResourceTypes),
                            "debug" => Ok(GeneratedField::Debug),
                            "logFormat" | "log_format" => Ok(GeneratedField::LogFormat),
                            "logLevel" | "log_level" => Ok(GeneratedField::LogLevel),
                            "logPath" | "log_path" => Ok(GeneratedField::LogPath),
                            "targetPath" | "target_path" => Ok(GeneratedField::TargetPath),
                            "projectDir" | "project_dir" => Ok(GeneratedField::ProjectDir),
                            "quiet" => Ok(GeneratedField::Quiet),
                            "writeJson" | "write_json" => Ok(GeneratedField::WriteJson),
                            "writeCatalog" | "write_catalog" => Ok(GeneratedField::WriteCatalog),
                            "manageState" | "manage_state" => Ok(GeneratedField::ManageState),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = InvocationEvalArgs;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.invocation.InvocationEvalArgs")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<InvocationEvalArgs, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut command__ = None;
                let mut profiles_dir__ = None;
                let mut packages_install_path__ = None;
                let mut target__ = None;
                let mut profile__ = None;
                let mut vars__ = None;
                let mut limit__ = None;
                let mut num_threads__ = None;
                let mut selector__ = None;
                let mut select__ = None;
                let mut exclude__ = None;
                let mut indirect_selection__ = None;
                let mut output_keys__ = None;
                let mut resource_types__ = None;
                let mut exclude_resource_types__ = None;
                let mut debug__ = None;
                let mut log_format__ = None;
                let mut log_level__ = None;
                let mut log_path__ = None;
                let mut target_path__ = None;
                let mut project_dir__ = None;
                let mut quiet__ = None;
                let mut write_json__ = None;
                let mut write_catalog__ = None;
                let mut manage_state__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::Command => {
                            if command__.is_some() {
                                return Err(serde::de::Error::duplicate_field("command"));
                            }
                            command__ = Some(map_.next_value()?);
                        }
                        GeneratedField::ProfilesDir => {
                            if profiles_dir__.is_some() {
                                return Err(serde::de::Error::duplicate_field("profilesDir"));
                            }
                            profiles_dir__ = map_.next_value()?;
                        }
                        GeneratedField::PackagesInstallPath => {
                            if packages_install_path__.is_some() {
                                return Err(serde::de::Error::duplicate_field("packagesInstallPath"));
                            }
                            packages_install_path__ = map_.next_value()?;
                        }
                        GeneratedField::Target => {
                            if target__.is_some() {
                                return Err(serde::de::Error::duplicate_field("target"));
                            }
                            target__ = map_.next_value()?;
                        }
                        GeneratedField::Profile => {
                            if profile__.is_some() {
                                return Err(serde::de::Error::duplicate_field("profile"));
                            }
                            profile__ = map_.next_value()?;
                        }
                        GeneratedField::Vars => {
                            if vars__.is_some() {
                                return Err(serde::de::Error::duplicate_field("vars"));
                            }
                            vars__ = Some(map_.next_value()?);
                        }
                        GeneratedField::Limit => {
                            if limit__.is_some() {
                                return Err(serde::de::Error::duplicate_field("limit"));
                            }
                            limit__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::NumThreads => {
                            if num_threads__.is_some() {
                                return Err(serde::de::Error::duplicate_field("numThreads"));
                            }
                            num_threads__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::Selector => {
                            if selector__.is_some() {
                                return Err(serde::de::Error::duplicate_field("selector"));
                            }
                            selector__ = map_.next_value()?;
                        }
                        GeneratedField::Select => {
                            if select__.is_some() {
                                return Err(serde::de::Error::duplicate_field("select"));
                            }
                            select__ = Some(map_.next_value()?);
                        }
                        GeneratedField::Exclude => {
                            if exclude__.is_some() {
                                return Err(serde::de::Error::duplicate_field("exclude"));
                            }
                            exclude__ = Some(map_.next_value()?);
                        }
                        GeneratedField::IndirectSelection => {
                            if indirect_selection__.is_some() {
                                return Err(serde::de::Error::duplicate_field("indirectSelection"));
                            }
                            indirect_selection__ = map_.next_value()?;
                        }
                        GeneratedField::OutputKeys => {
                            if output_keys__.is_some() {
                                return Err(serde::de::Error::duplicate_field("outputKeys"));
                            }
                            output_keys__ = Some(map_.next_value()?);
                        }
                        GeneratedField::ResourceTypes => {
                            if resource_types__.is_some() {
                                return Err(serde::de::Error::duplicate_field("resourceTypes"));
                            }
                            resource_types__ = Some(map_.next_value()?);
                        }
                        GeneratedField::ExcludeResourceTypes => {
                            if exclude_resource_types__.is_some() {
                                return Err(serde::de::Error::duplicate_field("excludeResourceTypes"));
                            }
                            exclude_resource_types__ = Some(map_.next_value()?);
                        }
                        GeneratedField::Debug => {
                            if debug__.is_some() {
                                return Err(serde::de::Error::duplicate_field("debug"));
                            }
                            debug__ = map_.next_value()?;
                        }
                        GeneratedField::LogFormat => {
                            if log_format__.is_some() {
                                return Err(serde::de::Error::duplicate_field("logFormat"));
                            }
                            log_format__ = map_.next_value()?;
                        }
                        GeneratedField::LogLevel => {
                            if log_level__.is_some() {
                                return Err(serde::de::Error::duplicate_field("logLevel"));
                            }
                            log_level__ = map_.next_value()?;
                        }
                        GeneratedField::LogPath => {
                            if log_path__.is_some() {
                                return Err(serde::de::Error::duplicate_field("logPath"));
                            }
                            log_path__ = map_.next_value()?;
                        }
                        GeneratedField::TargetPath => {
                            if target_path__.is_some() {
                                return Err(serde::de::Error::duplicate_field("targetPath"));
                            }
                            target_path__ = map_.next_value()?;
                        }
                        GeneratedField::ProjectDir => {
                            if project_dir__.is_some() {
                                return Err(serde::de::Error::duplicate_field("projectDir"));
                            }
                            project_dir__ = map_.next_value()?;
                        }
                        GeneratedField::Quiet => {
                            if quiet__.is_some() {
                                return Err(serde::de::Error::duplicate_field("quiet"));
                            }
                            quiet__ = map_.next_value()?;
                        }
                        GeneratedField::WriteJson => {
                            if write_json__.is_some() {
                                return Err(serde::de::Error::duplicate_field("writeJson"));
                            }
                            write_json__ = map_.next_value()?;
                        }
                        GeneratedField::WriteCatalog => {
                            if write_catalog__.is_some() {
                                return Err(serde::de::Error::duplicate_field("writeCatalog"));
                            }
                            write_catalog__ = map_.next_value()?;
                        }
                        GeneratedField::ManageState => {
                            if manage_state__.is_some() {
                                return Err(serde::de::Error::duplicate_field("manageState"));
                            }
                            manage_state__ = map_.next_value()?;
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(InvocationEvalArgs {
                    command: command__.unwrap_or_default(),
                    profiles_dir: profiles_dir__,
                    packages_install_path: packages_install_path__,
                    target: target__,
                    profile: profile__,
                    vars: vars__.unwrap_or_default(),
                    limit: limit__,
                    num_threads: num_threads__,
                    selector: selector__,
                    select: select__.unwrap_or_default(),
                    exclude: exclude__.unwrap_or_default(),
                    indirect_selection: indirect_selection__,
                    output_keys: output_keys__.unwrap_or_default(),
                    resource_types: resource_types__.unwrap_or_default(),
                    exclude_resource_types: exclude_resource_types__.unwrap_or_default(),
                    debug: debug__,
                    log_format: log_format__,
                    log_level: log_level__,
                    log_path: log_path__,
                    target_path: target_path__,
                    project_dir: project_dir__,
                    quiet: quiet__,
                    write_json: write_json__,
                    write_catalog: write_catalog__,
                    manage_state: manage_state__,
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.invocation.InvocationEvalArgs", FIELDS, GeneratedVisitor)
    }
}
impl serde::Serialize for InvocationMetrics {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if self.total_errors.is_some() {
            len += 1;
        }
        if self.total_warnings.is_some() {
            len += 1;
        }
        if self.autofix_suggestions.is_some() {
            len += 1;
        }
        if !self.node_type_counts.is_empty() {
            len += 1;
        }
        if !self.status_counts.is_empty() {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.invocation.InvocationMetrics", len)?;
        if let Some(v) = self.total_errors.as_ref() {
            #[allow(clippy::needless_borrow)]
            #[allow(clippy::needless_borrows_for_generic_args)]
            struct_ser.serialize_field("total_errors", ToString::to_string(&v).as_str())?;
        }
        if let Some(v) = self.total_warnings.as_ref() {
            #[allow(clippy::needless_borrow)]
            #[allow(clippy::needless_borrows_for_generic_args)]
            struct_ser.serialize_field("total_warnings", ToString::to_string(&v).as_str())?;
        }
        if let Some(v) = self.autofix_suggestions.as_ref() {
            #[allow(clippy::needless_borrow)]
            #[allow(clippy::needless_borrows_for_generic_args)]
            struct_ser.serialize_field("autofix_suggestions", ToString::to_string(&v).as_str())?;
        }
        if !self.node_type_counts.is_empty() {
            let v: std::collections::HashMap<_, _> = self.node_type_counts.iter()
                .map(|(k, v)| (k, v.to_string())).collect();
            struct_ser.serialize_field("node_type_counts", &v)?;
        }
        if !self.status_counts.is_empty() {
            let v: std::collections::HashMap<_, _> = self.status_counts.iter()
                .map(|(k, v)| (k, v.to_string())).collect();
            struct_ser.serialize_field("status_counts", &v)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for InvocationMetrics {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "total_errors",
            "totalErrors",
            "total_warnings",
            "totalWarnings",
            "autofix_suggestions",
            "autofixSuggestions",
            "node_type_counts",
            "nodeTypeCounts",
            "status_counts",
            "statusCounts",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            TotalErrors,
            TotalWarnings,
            AutofixSuggestions,
            NodeTypeCounts,
            StatusCounts,
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
                            "totalErrors" | "total_errors" => Ok(GeneratedField::TotalErrors),
                            "totalWarnings" | "total_warnings" => Ok(GeneratedField::TotalWarnings),
                            "autofixSuggestions" | "autofix_suggestions" => Ok(GeneratedField::AutofixSuggestions),
                            "nodeTypeCounts" | "node_type_counts" => Ok(GeneratedField::NodeTypeCounts),
                            "statusCounts" | "status_counts" => Ok(GeneratedField::StatusCounts),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = InvocationMetrics;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.invocation.InvocationMetrics")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<InvocationMetrics, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut total_errors__ = None;
                let mut total_warnings__ = None;
                let mut autofix_suggestions__ = None;
                let mut node_type_counts__ = None;
                let mut status_counts__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::TotalErrors => {
                            if total_errors__.is_some() {
                                return Err(serde::de::Error::duplicate_field("totalErrors"));
                            }
                            total_errors__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::TotalWarnings => {
                            if total_warnings__.is_some() {
                                return Err(serde::de::Error::duplicate_field("totalWarnings"));
                            }
                            total_warnings__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::AutofixSuggestions => {
                            if autofix_suggestions__.is_some() {
                                return Err(serde::de::Error::duplicate_field("autofixSuggestions"));
                            }
                            autofix_suggestions__ = 
                                map_.next_value::<::std::option::Option<::pbjson::private::NumberDeserialize<_>>>()?.map(|x| x.0)
                            ;
                        }
                        GeneratedField::NodeTypeCounts => {
                            if node_type_counts__.is_some() {
                                return Err(serde::de::Error::duplicate_field("nodeTypeCounts"));
                            }
                            node_type_counts__ = Some(
                                map_.next_value::<std::collections::HashMap<_, ::pbjson::private::NumberDeserialize<u64>>>()?
                                    .into_iter().map(|(k,v)| (k, v.0)).collect()
                            );
                        }
                        GeneratedField::StatusCounts => {
                            if status_counts__.is_some() {
                                return Err(serde::de::Error::duplicate_field("statusCounts"));
                            }
                            status_counts__ = Some(
                                map_.next_value::<std::collections::HashMap<_, ::pbjson::private::NumberDeserialize<u64>>>()?
                                    .into_iter().map(|(k,v)| (k, v.0)).collect()
                            );
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(InvocationMetrics {
                    total_errors: total_errors__,
                    total_warnings: total_warnings__,
                    autofix_suggestions: autofix_suggestions__,
                    node_type_counts: node_type_counts__.unwrap_or_default(),
                    status_counts: status_counts__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.invocation.InvocationMetrics", FIELDS, GeneratedVisitor)
    }
}
