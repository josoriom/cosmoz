#![cfg_attr(not(any(feature = "std", test)), no_std)]

extern crate alloc;

mod api;

#[cfg(feature = "compression")]
mod algorithms;
mod bits;
mod block;
mod decoder;
mod encode_error;
#[cfg(feature = "compression")]
mod encoder;
mod entropy;
mod error;
mod frame;
mod hash;
#[cfg(feature = "compression")]
mod levels;
#[cfg(feature = "parallel")]
mod parallel_decoder;
#[cfg(all(feature = "parallel", feature = "compression"))]
mod parallel_encoder;
mod simd;
#[cfg(feature = "compression")]
mod stream_encoder;
#[cfg(all(target_arch = "wasm32", feature = "wasm-exports"))]
mod wasm;

pub use api::*;
pub use encode_error::EncodeError;
pub use error::DecodeError;
