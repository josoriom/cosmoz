#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    InputTooShort,
    OutputTooSmall,
    OutputTooLarge,
    TruncatedInput,
    BadMagicNumber,
    BadFrameHeader,
    DictionaryNotSupported,
    WindowTooLarge,
    ReservedBlockType,
    BlockTooLarge,
    BadLiteralsHeader,
    BadHuffmanWeights,
    BadFseTable,
    BadSequencesHeader,
    BadOffset,
    CorruptBitstream,
    ChecksumMismatch,
    ChecksumNotSupported,
}

impl DecodeError {
    const fn message(self) -> &'static str {
        match self {
            DecodeError::InputTooShort => "the input ends before a complete frame could be read",
            DecodeError::OutputTooSmall => "the output buffer is too small to hold the decompressed data",
            DecodeError::OutputTooLarge => {
                "the decompressed data would exceed the configured maximum output size"
            }
            DecodeError::TruncatedInput => "the input stream ended without a complete frame",
            DecodeError::BadMagicNumber => "the input does not start with a zstd frame signature",
            DecodeError::BadFrameHeader => "the frame header is invalid or inconsistent with its content",
            DecodeError::DictionaryNotSupported => {
                "the frame uses a dictionary, which cosmoz does not support"
            }
            DecodeError::WindowTooLarge => "the frame declares a window larger than cosmoz can allocate",
            DecodeError::ReservedBlockType => "the frame uses a reserved block type",
            DecodeError::BlockTooLarge => "a block is larger than the maximum allowed block size",
            DecodeError::BadLiteralsHeader => "the literals section header is invalid",
            DecodeError::BadHuffmanWeights => "the Huffman weight table is invalid",
            DecodeError::BadFseTable => "the FSE table description is invalid",
            DecodeError::BadSequencesHeader => "the sequences section header is invalid",
            DecodeError::BadOffset => "a sequence references an offset outside the decoded history",
            DecodeError::CorruptBitstream => "the compressed bitstream is corrupt",
            DecodeError::ChecksumMismatch => "the decompressed content does not match its checksum",
            DecodeError::ChecksumNotSupported => {
                "the frame has a checksum but cosmoz was built without the checksum feature"
            }
        }
    }
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.message())
    }
}

#[cfg(feature = "std")]
impl std::error::Error for DecodeError {}
