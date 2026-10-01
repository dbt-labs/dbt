//! A small, dependency-light toolkit for coordinating access to files shared
//! between independent OS processes.
//!
//! This crate provides two independently useful pieces:
//!
//! - [`lease`]: a file-based lease (lock with a TTL, not a hold-forever mutex)
//!   so a crashed or hung process can never wedge other processes out of a
//!   shared resource forever.
//! - [`cache_dir`]: OS-appropriate resolution of a local, per-user cache
//!   directory (XDG on Linux, `~/Library/Caches` on macOS, `%LocalAppData%`
//!   on Windows) for whatever file(s) a lease ends up guarding.
//!
//! Ported from the `lease.go` algorithm in dbt Labs' Snowflake Go driver
//! fork, where it has been in production use gating a shared local
//! credentials-cache file.

pub mod cache_dir;
pub mod lease;

pub use lease::{Lease, LeaseError, LeaseHandler};
