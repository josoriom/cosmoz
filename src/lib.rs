#![cfg_attr(not(any(feature = "std", test)), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "encoder")]
pub mod algorithms;
pub mod bits;
pub mod block;
pub mod decoder;
pub mod encode_error;
#[cfg(feature = "encoder")]
pub mod encoder;
pub mod entropy;
pub mod error;
pub mod frame;
pub mod hash;
#[cfg(feature = "encoder")]
pub mod levels;
#[cfg(feature = "parallel")]
pub mod parallel_decoder;
#[cfg(all(feature = "parallel", feature = "encoder"))]
pub mod parallel_encoder;
pub mod simd;
#[cfg(all(feature = "encoder", feature = "alloc"))]
pub mod stream_encoder;
#[cfg(all(feature = "encoder", feature = "std"))]
pub mod stream_writer;
#[cfg(all(target_arch = "wasm32", feature = "wasm-exports"))]
pub mod wasm;

pub use decoder::{DecodeWorkspace, decompress, get_decompressed_size, get_frame_compressed_size};
pub use encode_error::EncodeError;
#[cfg(feature = "encoder")]
pub use encoder::{CompressFormat, CompressOptions, EncodeWorkspace, compress, get_max_compressed_size};
pub use error::DecodeError;
#[cfg(all(feature = "encoder", feature = "alloc"))]
pub use stream_encoder::StreamEncoder;
#[cfg(all(feature = "encoder", feature = "std"))]
pub use stream_writer::StreamWriter;
