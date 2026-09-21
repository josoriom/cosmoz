use alloc::boxed::Box;

use crate::error::DecodeError;

#[cfg(feature = "compression")]
use crate::encode_error::EncodeError;

#[cfg(feature = "compression")]
use super::options::CompressOptions;

use super::options::DecompressOptions;

#[cfg(feature = "compression")]
pub struct Encoder {
    options: CompressOptions,
    workspace: Box<crate::encoder::EncodeWorkspace>,
}

#[cfg(feature = "compression")]
impl Encoder {
    pub fn new(options: &CompressOptions) -> Result<Self, EncodeError> {
        let workspace = crate::encoder::EncodeWorkspace::new_boxed_for_level(options.level)?;
        Ok(Self {
            options: *options,
            workspace,
        })
    }

    pub fn max_compressed_size(&self, input_length: usize) -> usize {
        super::one_shot::max_compressed_size(input_length, &self.options)
    }

    pub fn compress_into(&mut self, input: &[u8], output: &mut [u8]) -> Result<usize, EncodeError> {
        crate::encoder::compress(input, output, &self.options.to_internal(), &mut self.workspace)
    }
}

pub struct Decoder {
    options: DecompressOptions,
    workspace: Box<crate::decoder::DecodeWorkspace>,
}

impl Decoder {
    pub fn new(options: &DecompressOptions) -> Self {
        Self {
            options: *options,
            workspace: crate::decoder::DecodeWorkspace::new_boxed(),
        }
    }

    pub(crate) fn max_output_size(&self) -> Option<usize> {
        self.options.max_output_size
    }

    pub fn decompress_into(&mut self, input: &[u8], output: &mut [u8]) -> Result<usize, DecodeError> {
        crate::decoder::decompress_checked(input, output, &mut self.workspace, self.options.verify_checksum)
    }
}
