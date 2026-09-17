pub mod finder_tables;
pub mod level_table;

use crate::algorithms::fast::FastFinder;
#[cfg(feature = "levels")]
use crate::algorithms::lazy2::Lazy2Finder;
use crate::block::{repeat_offsets::RepeatOffsets, sequence_record::SequenceRecord};
use finder_tables::TableStorage;
use level_table::LevelParameters;
#[cfg(feature = "levels")]
use level_table::Strategy;

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
    fn window_log(&self) -> u8;
}

#[allow(clippy::large_enum_variant)]
pub enum AnyFinder<'tables> {
    Fast(FastFinder),
    #[cfg(feature = "levels")]
    Lazy2(Lazy2Finder<'tables>),
    #[cfg(not(feature = "levels"))]
    Unused(core::marker::PhantomData<&'tables ()>),
}

impl AnyFinder<'static> {
    pub const fn for_level(level_parameters: LevelParameters) -> Self {
        AnyFinder::Fast(FastFinder::new(level_parameters.window_log))
    }
}

impl<'tables> AnyFinder<'tables> {
    #[cfg(feature = "levels")]
    pub fn for_level_with_storage(
        level_parameters: LevelParameters,
        storage: TableStorage<'tables>,
    ) -> Self {
        match level_parameters.strategy {
            Strategy::Lazy2 => AnyFinder::Lazy2(Lazy2Finder::new(
                storage.hash_table,
                storage.chain_table,
                level_parameters,
            )),
            Strategy::Fast => AnyFinder::Fast(FastFinder::new(level_parameters.window_log)),
        }
    }

    #[cfg(not(feature = "levels"))]
    pub fn for_level_with_storage(
        level_parameters: LevelParameters,
        _storage: TableStorage<'tables>,
    ) -> Self {
        AnyFinder::Fast(FastFinder::new(level_parameters.window_log))
    }
}

impl MatchFinder for AnyFinder<'_> {
    fn reset(&mut self) {
        match self {
            AnyFinder::Fast(finder) => finder.reset(),
            #[cfg(feature = "levels")]
            AnyFinder::Lazy2(finder) => finder.reset(),
            #[cfg(not(feature = "levels"))]
            AnyFinder::Unused(_) => {}
        }
    }

    fn find_sequences(
        &mut self,
        input: &[u8],
        block_start: usize,
        sequences: &mut [SequenceRecord],
        repeat_offsets: &mut RepeatOffsets,
    ) -> (usize, usize) {
        match self {
            AnyFinder::Fast(finder) => {
                finder.find_sequences(input, block_start, sequences, repeat_offsets)
            }
            #[cfg(feature = "levels")]
            AnyFinder::Lazy2(finder) => {
                finder.find_sequences(input, block_start, sequences, repeat_offsets)
            }
            #[cfg(not(feature = "levels"))]
            AnyFinder::Unused(_) => (0, 0),
        }
    }

    fn window_log(&self) -> u8 {
        match self {
            AnyFinder::Fast(finder) => finder.window_log(),
            #[cfg(feature = "levels")]
            AnyFinder::Lazy2(finder) => finder.window_log(),
            #[cfg(not(feature = "levels"))]
            AnyFinder::Unused(_) => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_finder_for_level_carries_the_level_window_log() {
        let level_parameters = level_table::get_level_parameters(9).unwrap();
        let finder = AnyFinder::for_level(level_parameters);
        assert_eq!(finder.window_log(), level_parameters.window_log);
    }
}
