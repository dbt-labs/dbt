impl serde::Serialize for OnboardingScreen {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let variant = match self {
            Self::Unspecified => "ONBOARDING_SCREEN_UNSPECIFIED",
            Self::Welcome => "ONBOARDING_SCREEN_WELCOME",
            Self::ProfileCheck => "ONBOARDING_SCREEN_PROFILE_CHECK",
            Self::ProfileFound => "ONBOARDING_SCREEN_PROFILE_FOUND",
            Self::ProfileSetup => "ONBOARDING_SCREEN_PROFILE_SETUP",
            Self::LinkAccount => "ONBOARDING_SCREEN_LINK_ACCOUNT",
            Self::DbtParse => "ONBOARDING_SCREEN_DBT_PARSE",
            Self::ParseErrorAutofix => "ONBOARDING_SCREEN_PARSE_ERROR_AUTOFIX",
            Self::DbtParseRetry => "ONBOARDING_SCREEN_DBT_PARSE_RETRY",
            Self::ParseErrorFail => "ONBOARDING_SCREEN_PARSE_ERROR_FAIL",
            Self::CompileNoSa => "ONBOARDING_SCREEN_COMPILE_NO_SA",
            Self::CompileNoSaFail => "ONBOARDING_SCREEN_COMPILE_NO_SA_FAIL",
            Self::Compile => "ONBOARDING_SCREEN_COMPILE",
            Self::CompileFail => "ONBOARDING_SCREEN_COMPILE_FAIL",
            Self::Success => "ONBOARDING_SCREEN_SUCCESS",
            Self::AgenticAutofix => "ONBOARDING_SCREEN_AGENTIC_AUTOFIX",
            Self::TryAgenticAutofix => "ONBOARDING_SCREEN_TRY_AGENTIC_AUTOFIX",
            Self::DownloadAgentsMd => "ONBOARDING_SCREEN_DOWNLOAD_AGENTS_MD",
            Self::CompileSaBaseline => "ONBOARDING_SCREEN_COMPILE_SA_BASELINE",
            Self::CompileSaBaselineSuccess => "ONBOARDING_SCREEN_COMPILE_SA_BASELINE_SUCCESS",
            Self::Login => "ONBOARDING_SCREEN_LOGIN",
        };
        serializer.serialize_str(variant)
    }
}
impl<'de> serde::Deserialize<'de> for OnboardingScreen {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "ONBOARDING_SCREEN_UNSPECIFIED",
            "ONBOARDING_SCREEN_WELCOME",
            "ONBOARDING_SCREEN_PROFILE_CHECK",
            "ONBOARDING_SCREEN_PROFILE_FOUND",
            "ONBOARDING_SCREEN_PROFILE_SETUP",
            "ONBOARDING_SCREEN_LINK_ACCOUNT",
            "ONBOARDING_SCREEN_DBT_PARSE",
            "ONBOARDING_SCREEN_PARSE_ERROR_AUTOFIX",
            "ONBOARDING_SCREEN_DBT_PARSE_RETRY",
            "ONBOARDING_SCREEN_PARSE_ERROR_FAIL",
            "ONBOARDING_SCREEN_COMPILE_NO_SA",
            "ONBOARDING_SCREEN_COMPILE_NO_SA_FAIL",
            "ONBOARDING_SCREEN_COMPILE",
            "ONBOARDING_SCREEN_COMPILE_FAIL",
            "ONBOARDING_SCREEN_SUCCESS",
            "ONBOARDING_SCREEN_AGENTIC_AUTOFIX",
            "ONBOARDING_SCREEN_TRY_AGENTIC_AUTOFIX",
            "ONBOARDING_SCREEN_DOWNLOAD_AGENTS_MD",
            "ONBOARDING_SCREEN_COMPILE_SA_BASELINE",
            "ONBOARDING_SCREEN_COMPILE_SA_BASELINE_SUCCESS",
            "ONBOARDING_SCREEN_LOGIN",
        ];

        struct GeneratedVisitor;

        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = OnboardingScreen;

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
                    "ONBOARDING_SCREEN_UNSPECIFIED" => Ok(OnboardingScreen::Unspecified),
                    "ONBOARDING_SCREEN_WELCOME" => Ok(OnboardingScreen::Welcome),
                    "ONBOARDING_SCREEN_PROFILE_CHECK" => Ok(OnboardingScreen::ProfileCheck),
                    "ONBOARDING_SCREEN_PROFILE_FOUND" => Ok(OnboardingScreen::ProfileFound),
                    "ONBOARDING_SCREEN_PROFILE_SETUP" => Ok(OnboardingScreen::ProfileSetup),
                    "ONBOARDING_SCREEN_LINK_ACCOUNT" => Ok(OnboardingScreen::LinkAccount),
                    "ONBOARDING_SCREEN_DBT_PARSE" => Ok(OnboardingScreen::DbtParse),
                    "ONBOARDING_SCREEN_PARSE_ERROR_AUTOFIX" => Ok(OnboardingScreen::ParseErrorAutofix),
                    "ONBOARDING_SCREEN_DBT_PARSE_RETRY" => Ok(OnboardingScreen::DbtParseRetry),
                    "ONBOARDING_SCREEN_PARSE_ERROR_FAIL" => Ok(OnboardingScreen::ParseErrorFail),
                    "ONBOARDING_SCREEN_COMPILE_NO_SA" => Ok(OnboardingScreen::CompileNoSa),
                    "ONBOARDING_SCREEN_COMPILE_NO_SA_FAIL" => Ok(OnboardingScreen::CompileNoSaFail),
                    "ONBOARDING_SCREEN_COMPILE" => Ok(OnboardingScreen::Compile),
                    "ONBOARDING_SCREEN_COMPILE_FAIL" => Ok(OnboardingScreen::CompileFail),
                    "ONBOARDING_SCREEN_SUCCESS" => Ok(OnboardingScreen::Success),
                    "ONBOARDING_SCREEN_AGENTIC_AUTOFIX" => Ok(OnboardingScreen::AgenticAutofix),
                    "ONBOARDING_SCREEN_TRY_AGENTIC_AUTOFIX" => Ok(OnboardingScreen::TryAgenticAutofix),
                    "ONBOARDING_SCREEN_DOWNLOAD_AGENTS_MD" => Ok(OnboardingScreen::DownloadAgentsMd),
                    "ONBOARDING_SCREEN_COMPILE_SA_BASELINE" => Ok(OnboardingScreen::CompileSaBaseline),
                    "ONBOARDING_SCREEN_COMPILE_SA_BASELINE_SUCCESS" => Ok(OnboardingScreen::CompileSaBaselineSuccess),
                    "ONBOARDING_SCREEN_LOGIN" => Ok(OnboardingScreen::Login),
                    _ => Err(serde::de::Error::unknown_variant(value, FIELDS)),
                }
            }
        }
        deserializer.deserialize_any(GeneratedVisitor)
    }
}
impl serde::Serialize for OnboardingScreenShown {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut len = 0;
        if self.screen != 0 {
            len += 1;
        }
        let mut struct_ser = serializer.serialize_struct("v1.public.events.fusion.onboarding.OnboardingScreenShown", len)?;
        if self.screen != 0 {
            let v = OnboardingScreen::try_from(self.screen)
                .map_err(|_| serde::ser::Error::custom(format!("Invalid variant {}", self.screen)))?;
            struct_ser.serialize_field("screen", &v)?;
        }
        struct_ser.end()
    }
}
impl<'de> serde::Deserialize<'de> for OnboardingScreenShown {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "screen",
        ];

        #[allow(clippy::enum_variant_names)]
        enum GeneratedField {
            Screen,
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
                            "screen" => Ok(GeneratedField::Screen),
                            _ => Ok(GeneratedField::__SkipField__),
                        }
                    }
                }
                deserializer.deserialize_identifier(GeneratedVisitor)
            }
        }
        struct GeneratedVisitor;
        impl<'de> serde::de::Visitor<'de> for GeneratedVisitor {
            type Value = OnboardingScreenShown;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct v1.public.events.fusion.onboarding.OnboardingScreenShown")
            }

            fn visit_map<V>(self, mut map_: V) -> std::result::Result<OnboardingScreenShown, V::Error>
                where
                    V: serde::de::MapAccess<'de>,
            {
                let mut screen__ = None;
                while let Some(k) = map_.next_key()? {
                    match k {
                        GeneratedField::Screen => {
                            if screen__.is_some() {
                                return Err(serde::de::Error::duplicate_field("screen"));
                            }
                            screen__ = Some(map_.next_value::<OnboardingScreen>()? as i32);
                        }
                        GeneratedField::__SkipField__ => {
                            let _ = map_.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(OnboardingScreenShown {
                    screen: screen__.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_struct("v1.public.events.fusion.onboarding.OnboardingScreenShown", FIELDS, GeneratedVisitor)
    }
}
