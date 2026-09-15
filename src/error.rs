#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    InputTooShort,
    OutputTooSmall,
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
}
