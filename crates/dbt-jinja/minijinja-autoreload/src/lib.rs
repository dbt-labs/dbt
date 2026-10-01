//! This crate adds a convenient auto reloader for MiniJinja.
//!
//! The [`AutoReloader`] is an utility type that can be passed around or placed
//! in a global variable using something like
//! [`once_cell`](https://docs.rs/once_cell/latest/once_cell/).  It accepts a
//! closure which is used to create an environment which is passed a notifier.
//! This notifier can automatically watch file system paths or it can be manually
//! instructed to invalidate the environment.
//!
//! Every time [`acquire_env`](AutoReloader::acquire_env) is called the reloader
//! checks if a reload is scheduled in which case it will automatically re-create
//! the environment.  While the [guard](EnvironmentGuard) is retained, the environment
//! won't perform further reloads.
//!
//! ## Example
//!
//! This is an example that uses the `source` feature of MiniJinja to automatically
//! load templates from the file system:
//!
//! ```
//! # fn test() -> Result<(), minijinja::Error> {
//! use minijinja_autoreload::AutoReloader;
//! use minijinja::{Environment, path_loader};
//!
//! let reloader = AutoReloader::new(|notifier| {
//!     let template_path = "path/to/templates";
//!     let mut env = Environment::new();
//!     env.set_loader(path_loader(template_path));
//!     notifier.watch_path(template_path, true);
//!     Ok(env)
//! });
//!
//! let env = reloader.acquire_env()?;
//! let tmpl = env.get_template("index.html")?;
//! # Ok(()) } fn main() { test().unwrap_err(); }
//! ```
#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]
use std::ops::Deref;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

#[cfg(feature = "watch-fs")]
use std::path::Path;

use minijinja::{Environment, Error};

type EnvCreator = dyn Fn(Notifier) -> Result<Environment<'static>, Error> + Send + Sync + 'static;

/// An auto reloader for MiniJinja [`Environment`]s.
pub struct AutoReloader {
    env_creator: Box<EnvCreator>,
    notifier: Notifier,
    cached_env: Mutex<Option<Environment<'static>>>,
}

impl AutoReloader {
    /// Creates a new auto reloader.
    ///
    /// The given closure is invoked to create a new environment whenever the auto-reloader
    /// detects that it should reload.  It is passed a [`Notifier`] which can be used to
    /// signal back to the auto-reloader when the environment should be re-created.
    pub fn new<F>(f: F) -> AutoReloader
    where
        F: Fn(Notifier) -> Result<Environment<'static>, Error> + Send + Sync + 'static,
    {
        AutoReloader {
            env_creator: Box::new(f),
            notifier: Notifier::new(),
            cached_env: Default::default(),
        }
    }

    /// Returns a handle to the notifier.
    ///
    /// This handle can be cloned and used for instance to trigger reloads from
    /// a background thread.
    pub fn notifier(&self) -> Notifier {
        self.notifier.weak()
    }

    /// Acquires a new environment, potentially reloading it if needed.
    ///
    /// The acquired environment is protected by a guard.  Until the guard is
    /// dropped the environment won't be reloaded.  Crucially the environment
    /// returned is also behind a shared reference which means that it won't
    /// be possible to mutate it.
    ///
    /// If the creator function passed to the constructor fails, the error is
    /// returned from this method.
    pub fn acquire_env(&self) -> Result<EnvironmentGuard<'_>, Error> {
        let mut mutex_guard = self.cached_env.lock().unwrap();
        if mutex_guard.is_none() || self.notifier.should_reload() {
            let weak_notifier = self.notifier.prepare_and_mark_reload()?;
            if mutex_guard.is_none() || !self.notifier.fast_reload() {
                *mutex_guard = Some((self.env_creator)(weak_notifier)?);
            } else {
                mutex_guard.as_mut().unwrap().clear_templates();
            }
        }
        Ok(EnvironmentGuard { mutex_guard })
    }
}

/// A guard that de-references into an [`Environment`].
///
/// While the guard is in scope, auto reloads are temporarily paused until the
/// guard is dropped.
pub struct EnvironmentGuard<'reloader> {
    mutex_guard: MutexGuard<'reloader, Option<Environment<'static>>>,
}

impl Deref for EnvironmentGuard<'_> {
    type Target = Environment<'static>;

    fn deref(&self) -> &Self::Target {
        self.mutex_guard.as_ref().unwrap()
    }
}

/// Signalling utility to notify the auto reloader about reloads.
///
/// The notifier can both watch file system paths or be manually instructed
/// to reload.  For file system path watching the `watch-fs` feature must be
/// enabled.
///
/// The notifier can be cloned which allows it to be passed to background
/// threads.  If the [`AutoReloader`] that created the notifier was dropped
/// the notifier itself is marked as dead.  In that case it stops doing anything
/// useful and returns `true` from [`is_dead`](Self::is_dead).
#[derive(Clone)]
pub struct Notifier {
    handle: NotifierImplHandle,
}

#[derive(Clone)]
enum NotifierImplHandle {
    Weak(Weak<Mutex<NotifierImpl>>),
    Strong(Arc<Mutex<NotifierImpl>>),
}

#[derive(Default)]
struct NotifierImpl {
    should_reload: bool,
    should_reload_callback: Option<Box<dyn Fn() -> bool + Send + Sync + 'static>>,
    on_should_reload_callback: Option<Box<dyn Fn() + Send + Sync + 'static>>,
    fast_reload: bool,
    #[cfg(feature = "watch-fs")]
    fs_watcher: Option<notify::RecommendedWatcher>,
    #[cfg(feature = "watch-fs")]
    persistent_fs_watcher: bool,
}

impl Notifier {
    fn new() -> Notifier {
        Notifier {
            handle: NotifierImplHandle::Strong(Arc::new(Default::default())),
        }
    }

    /// Tells the notifier that the environment needs reloading.
    pub fn request_reload(&self) {
        if let Some(handle) = self.handle() {
            handle.lock().unwrap().should_reload = true;

            if let Some(callback) = handle.lock().unwrap().on_should_reload_callback.as_ref() {
                callback();
            }
        }
    }

    /// Enables or disables fast reload.
    ///
    /// By default fast reload is disabled which causes the entire environment to
    /// be recreated.  When fast reload is enabled, then on reload
    /// [`clear_templates`](minijinja::Environment::clear_templates) is called.
    /// This will only work if a loader was added to the environment as the loader
    /// will then cause templates to be loaded again.
    ///
    /// When fast reloading is enabled, the environment creation function is
    /// only called once.
    pub fn set_fast_reload(&self, yes: bool) {
        if let Some(handle) = self.handle() {
            handle.lock().unwrap().fast_reload = yes;
        }
    }

    /// Registers a callback that is invoked to check the freshness of the
    /// environment.
    ///
    /// When the auto reloader checks if it should reload it will optionally
    /// invoke this callback.  Only one callback can be set.  If this is invoked
    /// another time, the old callback is removed.  The function should return
    /// `true` to request a reload, `false` otherwise.
    pub fn set_callback<F>(&self, f: F)
    where
        F: Fn() -> bool + Send + Sync + 'static,
    {
        if let Some(handle) = self.handle() {
            handle.lock().unwrap().should_reload_callback = Some(Box::new(f));
        }
    }

    /// Registers a callback that is invoked when the environment should reload.
    ///
    /// The callback is called in these scenarios:
    ///  - A reload was requested via [`request_reload`](Self::request_reload)
    ///  - The callback set via [`set_callback`](Self::set_callback) returned `true`
    ///
    /// When the feature `watch-fs` is enabled, the callback is also invoked when
    /// a change is detected in a watched file path.
    ///
    /// **NOTE**: This callback is invoked **before** the environment is reloaded.
    pub fn set_on_should_reload_callback<F>(&self, f: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        if let Some(handle) = self.handle() {
            handle.lock().unwrap().on_should_reload_callback = Some(Box::new(f));
        }
    }

    /// Tells the notifier to watch a file system path for changes.
    ///
    /// This can watch both directories and files.  The second parameter controls if
    /// the watcher should be operating recursively in which case `true` must be passed.
    /// When the environment is reloaded the watcher is cleared out which means that
    /// [`watch_path`](Self::watch_path) must be invoked again.  If this is not wanted
    /// [`persistent_watch`](Self::persistent_watch) must be enabled.
    #[cfg(feature = "watch-fs")]
    #[cfg_attr(docsrs, doc(cfg(feature = "watch-fs")))]
    pub fn watch_path<P: AsRef<Path>>(&self, path: P, recursive: bool) {
        use notify::{RecursiveMode, Watcher};
        let path = path.as_ref();
        let mode = if recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        self.with_fs_watcher(|watcher| {
            watcher.watch(path, mode).ok();
        });
    }

    /// Tells the notifier to stop watching a file system path for changes.
    ///
    /// This is usually not useful but it can be useful when [persistent
    /// watching](Self::persistent_watch) is enabled.
    #[cfg(feature = "watch-fs")]
    #[cfg_attr(docsrs, doc(cfg(feature = "watch-fs")))]
    pub fn unwatch_path<P: AsRef<Path>>(&self, path: P) {
        use notify::Watcher;
        let path = path.as_ref();
        self.with_fs_watcher(|watcher| {
            watcher.unwatch(path).ok();
        });
    }

    /// Enables the file system watcher to be persistent between reloads.
    #[cfg(feature = "watch-fs")]
    #[cfg_attr(docsrs, doc(cfg(feature = "watch-fs")))]
    pub fn persistent_watch(&self, yes: bool) {
        if let Some(handle) = self.handle() {
            handle.lock().unwrap().persistent_fs_watcher = yes;
        }
    }

    /// Returns `true` if the notifier is dead.
    ///
    /// A notifier is dead when the [`AutoReloader`] that created it was dropped.
    pub fn is_dead(&self) -> bool {
        self.handle().is_none()
    }

    fn handle(&self) -> Option<Arc<Mutex<NotifierImpl>>> {
        match self.handle {
            NotifierImplHandle::Weak(ref weak) => weak.upgrade(),
            NotifierImplHandle::Strong(ref arc) => Some(arc.clone()),
        }
    }

    fn fast_reload(&self) -> bool {
        let handle = match self.handle() {
            Some(handle) => handle,
            None => return false,
        };
        let inner = handle.lock().unwrap();
        inner.fast_reload
    }

    fn should_reload(&self) -> bool {
        let handle = match self.handle() {
            Some(handle) => handle,
            None => return false,
        };
        let inner = handle.lock().unwrap();

        // Early return if we already know we should reload so that
        // `should_reload_callback` isn't polled unnecessarily and
        // `on_should_reload_callback` isn't called twice.
        // (It should've been called already when setting `should_reload`.)
        if inner.should_reload {
            return true;
        }

        let should_reload = inner.should_reload_callback.as_ref().map_or(false, |x| x());

        if should_reload {
            if let Some(callback) = inner.on_should_reload_callback.as_ref() {
                callback();
            }
        }

        should_reload
    }

    #[cfg(feature = "watch-fs")]
    fn with_fs_watcher<F: FnOnce(&mut notify::RecommendedWatcher)>(&self, f: F) {
        use notify::event::{EventKind, ModifyKind};

        let handle = match self.handle() {
            Some(handle) => handle,
            None => return,
        };
        let weak_handle = Arc::downgrade(&handle);
        f(handle
            .lock()
            .unwrap()
            .fs_watcher
            .get_or_insert_with(move || {
                notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                    let kind = match res {
                        Ok(event) => event.kind,
                        Err(_) => return,
                    };
                    if matches!(
                        kind,
                        EventKind::Create(_)
                            | EventKind::Remove(_)
                            | EventKind::Modify(
                                ModifyKind::Data(_) | ModifyKind::Name(_) | ModifyKind::Any
                            )
                    ) {
                        if let Some(inner) = weak_handle.upgrade() {
                            inner.lock().unwrap().should_reload = true;

                            if let Some(callback) =
                                inner.lock().unwrap().on_should_reload_callback.as_ref()
                            {
                                callback();
                            }
                        }
                    }
                })
                .expect("unable to initialize fs watcher")
            }));
    }

    fn prepare_and_mark_reload(&self) -> Result<Notifier, Error> {
        let handle = self.handle().expect("notifier unexpectedly went away");
        #[cfg(feature = "watch-fs")]
        {
            let mut locked_handle = handle.lock().unwrap();
            if !locked_handle.persistent_fs_watcher && !locked_handle.fast_reload {
                locked_handle.fs_watcher.take();
            }
        }
        let weak_notifier = Notifier {
            handle: NotifierImplHandle::Weak(Arc::downgrade(&handle)),
        };
        handle.lock().unwrap().should_reload = false;
        Ok(weak_notifier)
    }

    fn weak(&self) -> Notifier {
        let handle = self.handle().expect("notifier unexpectedly went away");
        Notifier {
            handle: NotifierImplHandle::Weak(Arc::downgrade(&handle)),
        }
    }
}
