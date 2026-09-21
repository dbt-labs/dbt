//! Small platform primitives for creating files and directories with a
//! protected owner-only discretionary access control list.

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::{ensure_owner_only_dir, open_owner_only, owner_only_tempfile_in};
