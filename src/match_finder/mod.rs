pub mod hash_table_finder;

use crate::block::{repeat_offsets::RepeatOffsets, sequence_record::SequenceRecord};

pub const MIN_MATCH: usize = 4;
pub const MAX_OFFSET_LOG: u8 = 22;

pub trait MatchFinder {
    fn reset(&mut self);
    fn find_sequences(
        &mut self,
        input: &[u8],
        block_start: usize,
        sequences: &mut [SequenceRecord],
        repeat_offsets: &mut RepeatOffsets,
    ) -> (usize, usize);
}
