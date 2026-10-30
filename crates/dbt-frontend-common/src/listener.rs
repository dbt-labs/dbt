//! The callbacks a host implements to hear about a bind.

use std::any::Any;
use std::fmt::Debug;
use std::rc::Rc;

use crate::error::{CodeLocation, ErrorCode, FrontendError};
use crate::fix::{Fix, Noqas};

/// What a host listener hears about a bind: warnings, frontend errors, and
/// whether to drop a warning at a location. Every default does nothing.
pub trait FrontendListener: Debug {
    fn as_any(&self) -> &dyn Any;

    /// The listener's name, for debugging.
    fn name(&self) -> &str;

    fn on_warning(
        &self,
        _code: ErrorCode,
        _location: CodeLocation,
        _reason: &str,
        _fixes: Option<Vec<Fix>>,
        _noqas: Option<&Noqas>,
    ) {
    }

    fn should_ignore_location(&self, _location: CodeLocation) -> bool {
        false
    }

    fn on_frontend_error(&self, _err: FrontendError, _should_warn: bool) {}
}

/// A shared listener is a listener: the host keeps one handle and the binder
/// calls through another.
impl<L: FrontendListener + ?Sized> FrontendListener for Rc<L> {
    fn as_any(&self) -> &dyn Any {
        (**self).as_any()
    }

    fn name(&self) -> &str {
        (**self).name()
    }

    fn on_warning(
        &self,
        code: ErrorCode,
        location: CodeLocation,
        reason: &str,
        fixes: Option<Vec<Fix>>,
        noqas: Option<&Noqas>,
    ) {
        (**self).on_warning(code, location, reason, fixes, noqas);
    }

    fn should_ignore_location(&self, location: CodeLocation) -> bool {
        (**self).should_ignore_location(location)
    }

    fn on_frontend_error(&self, err: FrontendError, should_warn: bool) {
        (**self).on_frontend_error(err, should_warn);
    }
}
