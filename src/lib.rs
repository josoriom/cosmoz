#![cfg_attr(not(any(feature = "std", test)), no_std)]
#![allow(clippy::missing_safety_doc)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod bits;
pub mod block;
pub mod decoder;
pub mod encode_error;
pub mod encoder;
pub mod entropy;
pub mod error;
pub mod frame;
pub mod hash;
pub mod match_finder;
#[cfg(feature = "parallel")]
pub mod parallel_decoder;
#[cfg(feature = "parallel")]
pub mod parallel_encoder;
pub mod simd;
#[cfg(target_arch = "wasm32")]
pub mod wasm;

pub use decoder::{DecodeWorkspace, decompress, get_decompressed_size};
pub use encode_error::EncodeError;
pub use encoder::{CompressOptions, EncodeWorkspace, compress, get_max_compressed_size};
pub use error::DecodeError;
