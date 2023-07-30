#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]

#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    feature = "alloc",
    not(feature = "std")
))]
extern crate alloc as alloc_crate;
#[cfg(all(any(target_os = "linux", target_os = "android"), feature = "std"))]
extern crate std as alloc_crate;

#[cfg_attr(any(target_os = "linux", target_os = "android"), path = "linux/mod.rs")]
#[cfg_attr(
    any(
        target_os = "macos",
        target_os = "ios",
        target_os = "watchos",
        target_os = "tvos"
    ),
    path = "bsd/apple.rs"
)]
#[cfg_attr(
    any(target_os = "freebsd", target_os = "dragonfly"),
    path = "bsd/freebsd.rs"
)]
#[cfg_attr(
    any(target_os = "netbsd", target_os = "openbsd"),
    path = "bsd/netbsd.rs"
)]
mod imp;

pub use imp::*;
