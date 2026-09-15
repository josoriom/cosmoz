#![cfg_attr(not(any(feature = "std", test)), no_std)]

pub mod bits;
pub mod entropy;
pub mod error;
pub mod frame;
pub mod hash;
#[cfg(target_arch = "wasm32")]
pub mod wasm;

pub use error::DecodeError;
