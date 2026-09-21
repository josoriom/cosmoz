use crate::error::DecodeError;

#[cfg(feature = "compression")]
use crate::encode_error::EncodeError;

#[cfg(feature = "compression")]
use super::options::CompressOptions;

use super::options::DecompressOptions;

#[cfg(feature = "compression")]
use super::workspace::Encoder;

use super::workspace::Decoder;

#[cfg(feature = "compression")]
pub fn max_compressed_size(input_length: usize, options: &CompressOptions) -> usize {
    crate::encoder::get_max_compressed_size(input_length, &options.to_internal())
}

pub fn decompressed_size(input: &[u8]) -> Result<Option<u64>, DecodeError> {
    crate::decoder::get_decompressed_size(input)
}

pub fn compressed_size(input: &[u8]) -> Result<usize, DecodeError> {
    crate::decoder::get_frame_compressed_size(input)
}

#[cfg(feature = "compression")]
pub fn compress_with(input: &[u8], options: &CompressOptions) -> Result<alloc::vec::Vec<u8>, EncodeError> {
    let mut encoder = Encoder::new(options)?;
    let mut output = alloc::vec![0u8; encoder.max_compressed_size(input.len())];
    let written = encoder.compress_into(input, &mut output)?;
    output.truncate(written);
    Ok(output)
}

#[cfg(feature = "compression")]
pub fn compress(input: &[u8]) -> Result<alloc::vec::Vec<u8>, EncodeError> {
    compress_with(input, &CompressOptions::default())
}

pub(crate) fn allocate_and_decode(
    input: &[u8],
    decoder: &mut Decoder,
) -> Result<alloc::vec::Vec<u8>, DecodeError> {
    let max_output_size = decoder.max_output_size();
    let max_bound = crate::decoder::get_max_output_size(input.len());

    if let Some(size) = decompressed_size(input)? {
        let size = usize::try_from(size).map_err(|_| DecodeError::OutputTooLarge)?;
        if size > max_bound {
            return Err(DecodeError::OutputTooLarge);
        }
        if let Some(max_output_size) = max_output_size
            && size > max_output_size
        {
            return Err(DecodeError::OutputTooLarge);
        }
        let mut output = alloc::vec![0u8; size];
        let written = decoder.decompress_into(input, &mut output)?;
        output.truncate(written);
        return Ok(output);
    }

    let mut capacity = (input.len() * 4).max(4096).min(max_bound);
    if let Some(max_output_size) = max_output_size {
        capacity = capacity.min(max_output_size);
    }

    loop {
        let mut output = alloc::vec![0u8; capacity];
        match decoder.decompress_into(input, &mut output) {
            Ok(written) => {
                output.truncate(written);
                return Ok(output);
            }
            Err(DecodeError::OutputTooSmall) => {
                if capacity >= max_bound {
                    return Err(DecodeError::OutputTooLarge);
                }
                if let Some(max_output_size) = max_output_size
                    && capacity >= max_output_size
                {
                    return Err(DecodeError::OutputTooLarge);
                }
                capacity = match capacity.checked_mul(2) {
                    Some(next) => next,
                    None => return Err(DecodeError::OutputTooLarge),
                };
                capacity = capacity.min(max_bound);
                if let Some(max_output_size) = max_output_size {
                    capacity = capacity.min(max_output_size);
                }
            }
            Err(error) => return Err(error),
        }
    }
}

pub fn decompress_with(
    input: &[u8],
    options: &DecompressOptions,
) -> Result<alloc::vec::Vec<u8>, DecodeError> {
    let mut decoder = Decoder::new(options);
    allocate_and_decode(input, &mut decoder)
}

pub fn decompress(input: &[u8]) -> Result<alloc::vec::Vec<u8>, DecodeError> {
    decompress_with(input, &DecompressOptions::default())
}
