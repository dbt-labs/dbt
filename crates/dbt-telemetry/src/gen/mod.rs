#![allow(unused_qualifications)]
#![allow(clippy::cognitive_complexity)]
#![allow(clippy::large_enum_variant)]
#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::module_inception)]
pub mod v1 {
    pub mod public {
        pub mod events {
            pub mod fusion {
                pub mod artifact {
                    include!("v1.public.events.fusion.artifact.rs");
                    include!("v1.public.events.fusion.artifact.serde.rs");
                }
                pub mod asset {
                    include!("v1.public.events.fusion.asset.rs");
                    include!("v1.public.events.fusion.asset.serde.rs");
                }
                pub mod compat {
                    include!("v1.public.events.fusion.compat.rs");
                }
                pub mod deps {
                    include!("v1.public.events.fusion.deps.rs");
                    include!("v1.public.events.fusion.deps.serde.rs");
                }
                pub mod dev {
                    include!("v1.public.events.fusion.dev.rs");
                    include!("v1.public.events.fusion.dev.serde.rs");
                }
                pub mod generic {
                    include!("v1.public.events.fusion.generic.rs");
                    include!("v1.public.events.fusion.generic.serde.rs");
                }
                pub mod hook {
                    include!("v1.public.events.fusion.hook.rs");
                    include!("v1.public.events.fusion.hook.serde.rs");
                }
                pub mod invocation {
                    include!("v1.public.events.fusion.invocation.rs");
                    include!("v1.public.events.fusion.invocation.serde.rs");
                }
                pub mod log {
                    include!("v1.public.events.fusion.log.rs");
                    include!("v1.public.events.fusion.log.serde.rs");
                }
                pub mod node {
                    include!("v1.public.events.fusion.node.rs");
                    include!("v1.public.events.fusion.node.serde.rs");
                }
                pub mod onboarding {
                    include!("v1.public.events.fusion.onboarding.rs");
                    include!("v1.public.events.fusion.onboarding.serde.rs");
                }
                pub mod phase {
                    include!("v1.public.events.fusion.phase.rs");
                    include!("v1.public.events.fusion.phase.serde.rs");
                }
                pub mod process {
                    include!("v1.public.events.fusion.process.rs");
                    include!("v1.public.events.fusion.process.serde.rs");
                }
                pub mod query {
                    include!("v1.public.events.fusion.query.rs");
                    include!("v1.public.events.fusion.query.serde.rs");
                }
                pub mod update {
                    include!("v1.public.events.fusion.update.rs");
                    include!("v1.public.events.fusion.update.serde.rs");
                }
            }
        }
    }
}
