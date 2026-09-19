pub(crate) mod finder_tables;
pub(crate) mod level_table;

use crate::algorithms::fast::FastFinder;
#[cfg(feature = "compression")]
use crate::algorithms::lazy2::Lazy2Finder;
#[cfg(feature = "compression")]
use crate::algorithms::ultra2::Ultra2Finder;
use crate::block::{repeat_offsets::RepeatOffsets, sequence_record::SequenceRecord};
use finder_tables::TableStorage;
use level_table::LevelParameters;
#[cfg(feature = "compression")]
use level_table::Strategy;

pub(crate) const MAX_OFFSET_LOG: u8 = 22;

pub(crate) trait MatchFinder {
    fn reset(&mut self, input_length: usize);
    fn find_sequences(
        &mut self,
        input: &[u8],
        block_start: usize,
        sequences: &mut [SequenceRecord],
        repeat_offsets: &mut RepeatOffsets,
    ) -> (usize, usize);
    #[allow(dead_code)]
    fn window_log(&self) -> u8;
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum AnyFinder<'tables> {
    Fast(FastFinder),
    #[cfg(feature = "compression")]
    Lazy2(Lazy2Finder<'tables>),
    #[cfg(feature = "compression")]
    Ultra2(Ultra2Finder<'tables>),
    #[cfg(not(feature = "compression"))]
    Unused(core::marker::PhantomData<&'tables ()>),
}

impl AnyFinder<'static> {
    pub(crate) const fn for_level(level_parameters: LevelParameters) -> Self {
        AnyFinder::Fast(FastFinder::new(level_parameters))
    }
}

impl<'tables> AnyFinder<'tables> {
    #[cfg(feature = "compression")]
    pub(crate) fn for_level_with_storage(
        level: u8,
        level_parameters: LevelParameters,
        storage: TableStorage<'tables>,
    ) -> Self {
        match level_parameters.strategy {
            Strategy::Ultra2 => AnyFinder::Ultra2(Ultra2Finder::new_over_zeroed_tables(
                storage.hash_table,
                storage.chain_table,
                storage.optimal_table,
                level,
                level_parameters,
            )),
            Strategy::Lazy2 => AnyFinder::Lazy2(Lazy2Finder::new(
                storage.hash_table,
                storage.chain_table,
                level_parameters,
            )),
            Strategy::Fast => AnyFinder::Fast(FastFinder::new(level_parameters)),
        }
    }

    #[cfg(not(feature = "compression"))]
    pub(crate) fn for_level_with_storage(
        _level: u8,
        level_parameters: LevelParameters,
        _storage: TableStorage<'tables>,
    ) -> Self {
        AnyFinder::Fast(FastFinder::new(level_parameters))
    }
}

impl MatchFinder for AnyFinder<'_> {
    fn reset(&mut self, input_length: usize) {
        match self {
            AnyFinder::Fast(finder) => finder.reset(input_length),
            #[cfg(feature = "compression")]
            AnyFinder::Lazy2(finder) => finder.reset(input_length),
            #[cfg(feature = "compression")]
            AnyFinder::Ultra2(finder) => finder.reset(input_length),
            #[cfg(not(feature = "compression"))]
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
            #[cfg(feature = "compression")]
            AnyFinder::Lazy2(finder) => {
                finder.find_sequences(input, block_start, sequences, repeat_offsets)
            }
            #[cfg(feature = "compression")]
            AnyFinder::Ultra2(finder) => {
                finder.find_sequences(input, block_start, sequences, repeat_offsets)
            }
            #[cfg(not(feature = "compression"))]
            AnyFinder::Unused(_) => (0, 0),
        }
    }

    fn window_log(&self) -> u8 {
        match self {
            AnyFinder::Fast(finder) => finder.window_log(),
            #[cfg(feature = "compression")]
            AnyFinder::Lazy2(finder) => finder.window_log(),
            #[cfg(feature = "compression")]
            AnyFinder::Ultra2(finder) => finder.window_log(),
            #[cfg(not(feature = "compression"))]
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
