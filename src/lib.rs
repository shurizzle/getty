#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]

#[cfg_attr(target_os = "linux", path = "linux/mod.rs")]
#[cfg_attr(any(target_os = "macos", target_os = "ios"), path = "bsd/apple.rs")]
mod imp;

pub use imp::*;
