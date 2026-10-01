#![allow(unused_qualifications)]

pub mod serde_timestamp_micros;

#[allow(
    clippy::cognitive_complexity,
    clippy::large_enum_variant,
    clippy::doc_lazy_continuation,
    clippy::module_inception
)]
pub mod v1 {
    pub mod events {
        pub mod vortex {
            include!("gen/v1.events.vortex.rs");
        }
    }
    pub mod public {
        pub mod common {
            include!("gen/v1.public.common.rs");
            pub mod vortex_telemetry_contexts {
                include!("gen/v1.public.common.vortex_telemetry_contexts.rs");
            }
        }
        pub mod events {
            pub mod fusion {
                include!("gen/v1.public.events.fusion.rs");
            }
        }
        pub mod fields {
            pub mod adapter_types {
                include!("gen/v1.public.fields.adapter_types.rs");
            }
            pub mod core_types {
                include!("gen/v1.public.fields.core_types.rs");
            }
        }
    }
}
