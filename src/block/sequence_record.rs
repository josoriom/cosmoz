use crate::frame::block_header::MAX_BLOCK_SIZE;

pub(crate) const MAX_SEQUENCES_PER_BLOCK: usize = MAX_BLOCK_SIZE / 3 + 1;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct SequenceRecord {
    pub literal_length: u32,
    pub match_length: u32,
    pub offset_value: u32,
}
