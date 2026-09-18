#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    OutputTooSmall,
    InputTooLarge,
    BadOptions,
    TableNotUsable,
    OutOfMemory,
}

impl EncodeError {
    const fn message(self) -> &'static str {
        match self {
            EncodeError::OutputTooSmall => "the output buffer is too small to hold the compressed data",
            EncodeError::InputTooLarge => "the input is too large to compress with these options",
            EncodeError::BadOptions => {
                "the compression options are invalid, such as a level cosmoz does not support"
            }
            EncodeError::TableNotUsable => {
                "an internal encoding table could not be built for this input"
            }
            EncodeError::OutOfMemory => "allocating memory for compression failed",
        }
    }
}

impl core::fmt::Display for EncodeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.message())
    }
}

#[cfg(feature = "std")]
impl std::error::Error for EncodeError {}
