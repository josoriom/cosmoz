use crate::frame::block_header::MAX_BLOCK_SIZE;

pub const MAX_SEQUENCES_PER_BLOCK: usize = MAX_BLOCK_SIZE / 4 + 1;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct SequenceRecord {
    pub literal_length: u32,
    pub match_length: u32,
    pub offset_value: u32,
}
