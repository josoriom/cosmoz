use alloc::boxed::Box;

#[cfg(feature = "compression")]
use crate::encode_error::EncodeError;

#[cfg(feature = "compression")]
use super::options::CompressOptions;

#[cfg(feature = "compression")]
#[repr(transparent)]
pub struct Encoder(pub(crate) crate::encoder::EncodeWorkspace);

#[cfg(feature = "compression")]
impl Encoder {
    pub fn new(options: &CompressOptions) -> Result<Box<Self>, EncodeError> {
        let workspace = crate::encoder::EncodeWorkspace::new_boxed_for_level(options.level)?;
        let raw = Box::into_raw(workspace) as *mut Self;
        Ok(unsafe { Box::from_raw(raw) })
    }
}

#[repr(transparent)]
pub struct Decoder(pub(crate) crate::decoder::DecodeWorkspace);

impl Decoder {
    pub fn new() -> Box<Self> {
        let workspace = crate::decoder::DecodeWorkspace::new_boxed();
        let raw = Box::into_raw(workspace) as *mut Self;
        unsafe { Box::from_raw(raw) }
    }
}
