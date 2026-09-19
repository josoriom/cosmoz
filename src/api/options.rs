#[cfg(feature = "compression")]
pub const DEFAULT_LEVEL: u8 = crate::encoder::DEFAULT_COMPRESSION_LEVEL;

#[cfg(feature = "compression")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompressOptions {
    pub level: u8,
    pub checksum: bool,
}

#[cfg(feature = "compression")]
impl Default for CompressOptions {
    fn default() -> Self {
        Self {
            level: DEFAULT_LEVEL,
            checksum: true,
        }
    }
}

#[cfg(feature = "compression")]
impl CompressOptions {
    pub(crate) fn to_internal(self) -> crate::encoder::CompressOptions {
        crate::encoder::CompressOptions {
            with_checksum: self.checksum,
            level: self.level,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecompressOptions {
    pub verify_checksum: bool,
    pub max_output_size: Option<usize>,
}

impl Default for DecompressOptions {
    fn default() -> Self {
        Self {
            verify_checksum: true,
            max_output_size: None,
        }
    }
}
