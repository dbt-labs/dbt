//! The adapters a run can execute on: one per adapter type the active target
//! declares.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use dbt_adapter_core::AdapterType;
use dbt_common::{ErrorCode, FsResult, fs_err};

use crate::Adapter;

/// How the store builds an adapter it has not built yet.
///
/// Construction is not a pure function of the adapter type -- it depends on the
/// replay mode, the schema store, the execution backend and more, none of which
/// this crate knows about. So the store takes a builder rather than those
/// inputs: its job is identity and memoisation, not construction.
pub type AdapterBuilder = Box<dyn Fn(AdapterType) -> FsResult<Arc<Adapter>> + Send + Sync>;

/// One adapter per adapter type the active target declares, built on first use.
///
/// A run used to hold exactly one `Adapter`, made from the default connection.
/// That is why a node on a non-default adapter could only get part of the way
/// there: its dialect reached macro dispatch through the render context, while the
/// engine and connection stayed the default's. Holding the adapters by type means a
/// caller that knows a node's adapter can obtain the one that actually executes
/// it, rather than the one the run happened to start with.
///
/// **Lazy on purpose.** A target commonly declares more adapters than a given run
/// selects -- that is the whole point of checking availability after scheduling
/// rather than at parse -- so connecting to an adapter no node uses would make
/// declaring one expensive.
pub struct AdapterStore {
    /// The type unannotated nodes run on. Always among [`Self::declared`].
    default_adapter: AdapterType,
    /// Declaration order, as the profile wrote them.
    declared: Vec<AdapterType>,
    build: AdapterBuilder,
    /// Memoised by type. A `Mutex` rather than `OnceLock` per entry because the
    /// key set is known only at construction and the contention is negligible:
    /// each type is built once, and lookups after that are a map hit.
    built: Mutex<HashMap<AdapterType, Arc<Adapter>>>,
}

/// Hand-written because the builder is a closure. Prints what a reader wants
/// anyway: which adapters are declared, which is the default, and which have been
/// built so far.
impl std::fmt::Debug for AdapterStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let built: Vec<&str> = self
            .declared
            .iter()
            .filter(|t| self.is_built(**t))
            .map(|t| t.as_ref())
            .collect();
        f.debug_struct("AdapterStore")
            .field("default_adapter", &self.default_adapter.as_ref())
            .field(
                "declared",
                &self.declared.iter().map(|t| t.as_ref()).collect::<Vec<_>>(),
            )
            .field("built", &built)
            .finish()
    }
}

impl AdapterStore {
    /// `declared` is every adapter type the target declares, in declaration order;
    /// `default_adapter` must be one of them.
    pub fn new(
        declared: Vec<AdapterType>,
        default_adapter: AdapterType,
        build: AdapterBuilder,
    ) -> FsResult<Self> {
        if !declared.contains(&default_adapter) {
            return Err(fs_err!(
                ErrorCode::InvalidConfig,
                "the target's default adapter '{default_adapter}' is not among the adapters it \
                 declares ({}); this is a bug in profile resolution rather than a user error",
                declared
                    .iter()
                    .map(|t| t.as_ref())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Ok(Self {
            default_adapter,
            declared,
            build,
            built: Mutex::new(HashMap::new()),
        })
    }

    /// The adapter for `adapter_type`, building it if this is its first use.
    ///
    /// Errors when the target does not declare the type. Reaching here with an
    /// undeclared type means something bypassed the post-schedule availability
    /// check, so the message says so rather than blaming the user's profile.
    pub fn get(&self, adapter_type: AdapterType) -> FsResult<Arc<Adapter>> {
        if !self.declared.contains(&adapter_type) {
            return Err(fs_err!(
                ErrorCode::InvalidConfig,
                "no adapter is configured for '{adapter_type}'; the target declares {}",
                self.declared
                    .iter()
                    .map(|t| t.as_ref())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        // Built outside the lock would let two callers race to construct the same
        // adapter and open two connections. Holding it across the build serialises
        // that; the build is once per type per run.
        let mut built = self
            .built
            .lock()
            .map_err(|_| fs_err!(ErrorCode::Unexpected, "adapter store lock was poisoned"))?;
        if let Some(adapter) = built.get(&adapter_type) {
            return Ok(Arc::clone(adapter));
        }
        let adapter = (self.build)(adapter_type)?;
        built.insert(adapter_type, Arc::clone(&adapter));
        Ok(adapter)
    }

    /// The adapter unannotated nodes run on.
    pub fn default_adapter(&self) -> FsResult<Arc<Adapter>> {
        self.get(self.default_adapter)
    }

    /// The type unannotated nodes run on.
    pub fn default_adapter_type(&self) -> AdapterType {
        self.default_adapter
    }

    /// Every type the target declares, in declaration order.
    pub fn declared(&self) -> &[AdapterType] {
        &self.declared
    }

    /// Whether `adapter_type` has been built yet. For tests and diagnostics: the
    /// laziness is a property worth being able to assert.
    pub fn is_built(&self, adapter_type: AdapterType) -> bool {
        self.built
            .lock()
            .map(|built| built.contains_key(&adapter_type))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A builder that records which types it was asked for and always fails.
    ///
    /// Failing is deliberate: constructing a real `Adapter` needs a `TypeOps` impl
    /// and a connection config, and none of the properties below are about a built
    /// adapter. They are about *when* the store builds -- which the call log shows
    /// directly, and which is the part that costs a connection if it is wrong.
    fn counting_builder() -> (Arc<Mutex<Vec<AdapterType>>>, AdapterBuilder) {
        let calls: Arc<Mutex<Vec<AdapterType>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&calls);
        let build: AdapterBuilder = Box::new(move |adapter_type| {
            recorded.lock().unwrap().push(adapter_type);
            Err(fs_err!(ErrorCode::Unexpected, "not built in tests"))
        });
        (calls, build)
    }

    fn store(declared: &[AdapterType], default_adapter: AdapterType) -> AdapterStore {
        let (_, build) = counting_builder();
        AdapterStore::new(declared.to_vec(), default_adapter, build).expect("valid store")
    }

    /// A default outside the declared set means profile resolution produced
    /// something incoherent; better to say so at construction than to fail later
    /// on a lookup that looks like a user error.
    #[test]
    fn a_default_outside_the_declared_set_is_rejected() {
        let (_, build) = counting_builder();
        let err = AdapterStore::new(vec![AdapterType::Snowflake], AdapterType::Bigquery, build)
            .expect_err("bigquery is not declared");
        assert!(
            err.to_string()
                .contains("not among the adapters it declares"),
            "unexpected message: {err}"
        );
    }

    /// Nothing is built until something asks. A target may declare adapters a given
    /// run never selects, and each build is a connection.
    #[test]
    fn nothing_is_built_until_requested() {
        let (calls, build) = counting_builder();
        let store = AdapterStore::new(
            vec![AdapterType::Snowflake, AdapterType::Bigquery],
            AdapterType::Snowflake,
            build,
        )
        .unwrap();

        assert!(
            calls.lock().unwrap().is_empty(),
            "constructing built nothing"
        );
        assert!(!store.is_built(AdapterType::Snowflake));
        assert!(!store.is_built(AdapterType::Bigquery));
    }

    /// Asking for one adapter must not build the others.
    #[test]
    fn requesting_one_adapter_does_not_build_the_rest() {
        let (calls, build) = counting_builder();
        let store = AdapterStore::new(
            vec![
                AdapterType::Snowflake,
                AdapterType::Bigquery,
                AdapterType::LakeCompute,
            ],
            AdapterType::Snowflake,
            build,
        )
        .unwrap();

        let _ = store.get(AdapterType::Bigquery);
        assert_eq!(*calls.lock().unwrap(), vec![AdapterType::Bigquery]);
    }

    /// An undeclared type is rejected without reaching the builder -- the error is
    /// about configuration, and building would be a wasted connection attempt.
    #[test]
    fn an_undeclared_adapter_is_rejected_without_building() {
        let (calls, build) = counting_builder();
        let store =
            AdapterStore::new(vec![AdapterType::Snowflake], AdapterType::Snowflake, build).unwrap();

        let err = store
            .get(AdapterType::Bigquery)
            .expect_err("bigquery is not declared");
        assert!(
            err.to_string().contains("no adapter is configured for"),
            "unexpected message: {err}"
        );
        assert!(
            calls.lock().unwrap().is_empty(),
            "must not attempt to build an undeclared adapter"
        );
    }

    #[test]
    fn the_default_adapter_is_the_one_the_target_marked() {
        let store = store(
            &[AdapterType::Bigquery, AdapterType::Snowflake],
            AdapterType::Snowflake,
        );
        assert_eq!(store.default_adapter_type(), AdapterType::Snowflake);
    }

    /// Declaration order is the profile's, so diagnostics list adapters the way the
    /// user wrote them.
    #[test]
    fn declared_order_is_preserved() {
        let store = store(
            &[
                AdapterType::LakeCompute,
                AdapterType::Snowflake,
                AdapterType::Bigquery,
            ],
            AdapterType::Snowflake,
        );
        assert_eq!(
            store.declared(),
            &[
                AdapterType::LakeCompute,
                AdapterType::Snowflake,
                AdapterType::Bigquery
            ]
        );
    }

    /// A failed build is not cached: a transient connection failure should not
    /// poison the adapter for the rest of the run.
    #[test]
    fn a_failed_build_is_retried_rather_than_cached() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&attempts);
        let build: AdapterBuilder = Box::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            Err(fs_err!(ErrorCode::Unexpected, "transient"))
        });
        let store =
            AdapterStore::new(vec![AdapterType::Snowflake], AdapterType::Snowflake, build).unwrap();

        assert!(store.get(AdapterType::Snowflake).is_err());
        assert!(store.get(AdapterType::Snowflake).is_err());
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            2,
            "should retry, not cache"
        );
        assert!(!store.is_built(AdapterType::Snowflake));
    }
}
