/// fs_err!(code,msg) construct a user-facing [FsError], to be used for further
/// processing, e.g. typically used in `.map_err(|_| fs_err!(..))`, etc
#[macro_export]
macro_rules! fs_err {
    ($code:expr, $($arg:tt)*) => {
        Box::new($crate::FsError::new(
            $code,
            format!($($arg)*),
        ))
    };
    (code => $code:expr, loc => $location:expr, $($arg:tt)*) => {
        Box::new($crate::FsError::new(
            $code,
            format!($($arg)*),
        ).with_location($location))
    };
    (code => $code:expr, hacky_yml_loc => $location:expr, $($arg:tt)*) => {
        Box::new($crate::FsError::new(
            $code,
            format!($($arg)*),
        ).with_hacky_yml_location($location))
    }
}

/// err! constructs a user-facing [FsError] and immediately wrap it in an `Err`
/// variant of a `Result`, typically used in `return err!(...)`, etc
#[macro_export]
macro_rules! err {
    ($code:expr, $($arg:tt)*) => {
        Err($crate::fs_err!($code, $($arg)*))
    };
    (code => $code:expr, loc => $location:expr, $($arg:tt)*) => {
        Err($crate::fs_err!(code => $code, loc => $location, $($arg)*))
    };
    (code => $code:expr, hacky_yml_loc => $location:expr, $($arg:tt)*) => {
        Err($crate::fs_err!(code => $code, hacky_yml_loc => $location, $($arg)*))
    }
}

#[macro_export]
macro_rules! unexpected_err {
    ($($arg:tt)*) => {
        Err($crate::unexpected_fs_err!($($arg)*))
    }
}

#[macro_export]
macro_rules! unexpected_fs_err {
    ($($arg:tt)*) => {
        Box::new($crate::FsError::new_with_forced_backtrace(
            $crate::ErrorCode::Unexpected,
            format!($($arg)*),
        ))
    }
}

#[macro_export]
macro_rules! not_implemented_err {
    ($($arg:tt)*) => {
        Err($crate::not_implemented_fs_err!($($arg)*))
    }
}

#[macro_export]
macro_rules! not_implemented_fs_err {
    ($($arg:tt)*) => {
        Box::new($crate::FsError::new(
            $crate::ErrorCode::NotImplemented,
            format!($($arg)*),
        ))
    }
}

#[macro_export]
macro_rules! ectx {
    (code => $code:expr, loc => $location:expr $(,)? ) => {
        || $crate::ErrContext {
            code: Some($code),
            location: Some($location),
            context: None,
        }
    };
    (code => $code:expr, loc => $location:expr, $($arg:tt)*) => {
        || $crate::ErrContext {
            code: Some($code),
            location: Some($location),
            context: Some(format!($($arg)*)),
        }
    };
    (code => $code:expr, $($arg:tt)*) => {
        || $crate::ErrContext {
            code: Some($code),
            location: None,
            context: Some(format!($($arg)*)),
        }
    };
    (loc => $location:expr) => {
        || $crate::ErrContext {
            code: None,
            location: Some($location),
            context: None,
        }
    };
    (loc => $location:expr, $($arg:tt)*) => {
        || $crate::ErrContext {
            code: None,
            location: Some($location),
            context: Some(format!($($arg)*)),
        }
    };
    (code => $code:expr) => {
        || $crate::ErrContext {
            code: Some($code),
            location: None,
            context: None,
        }
    };
    ($($arg:tt)*) => {
        || $crate::ErrContext {
            code: None,
            location: None,
            context: Some(format!($($arg)*)),
        }
    };
}
