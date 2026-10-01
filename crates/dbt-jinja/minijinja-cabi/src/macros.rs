/// Declares an ffi function:
///
/// ```rust
/// ffi_fn! {
///     fn my_func() {}
/// }
/// ```
macro_rules! ffi_fn {
    ($(#[$doc:meta])* unsafe fn $name:ident($scope:ident $(,$arg:ident: $arg_ty:ty)* $(,)?) -> $ret:ty $body:block) => {
        $(#[$doc])*
        #[no_mangle]
        pub unsafe extern "C" fn $name($($arg: $arg_ty),*) -> $ret {
            use std::panic::{self, AssertUnwindSafe};

            match panic::catch_unwind(AssertUnwindSafe(move || crate::utils::catch(|$scope| Ok({ $body })))) {
                Ok(v) => v,
                Err(_) => {
                    // TODO: set panic as error
                    crate::utils::AbiResult::err_value()
                }
            }
        }
    };

    ($(#[$doc:meta])* unsafe fn $name:ident($scope:ident $(,$arg:ident: $arg_ty:ty)* $(,)?) $body:block) => {
        ffi_fn!($(#[$doc])* unsafe fn $name($scope $(, $arg: $arg_ty)*) -> () $body);
    };
}
