#[cfg(feature = "levels")]
pub mod double_fast_finder;
pub mod finder_tables;
#[cfg(feature = "levels")]
pub mod hash_chain_finder;
pub mod hash_table_finder;
pub mod level_table;
#[cfg(feature = "levels")]
pub mod row_hash_finder;

use crate::block::{repeat_offsets::RepeatOffsets, sequence_record::SequenceRecord};
#[cfg(feature = "levels")]
use double_fast_finder::DoubleFastFinder;
use finder_tables::TableStorage;
#[cfg(feature = "levels")]
use hash_chain_finder::{HashChainFinder, SearchMethod};
use hash_table_finder::HashTableFinder;
use level_table::LevelParameters;
#[cfg(feature = "levels")]
use level_table::Strategy;
#[cfg(feature = "levels")]
use row_hash_finder::RowHashFinder;

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
    Fast(HashTableFinder),
    #[cfg(feature = "levels")]
    DoubleFast(DoubleFastFinder<'tables>),
    #[cfg(feature = "levels")]
    Chain(HashChainFinder<'tables>),
    #[cfg(feature = "levels")]
    Row(RowHashFinder<'tables>),
    #[cfg(not(feature = "levels"))]
    Unused(core::marker::PhantomData<&'tables ()>),
}

impl AnyFinder<'static> {
    pub const fn for_level(level_parameters: LevelParameters) -> Self {
        AnyFinder::Fast(HashTableFinder::new(level_parameters.window_log))
    }
}

#[cfg(feature = "levels")]
fn search_method_for_strategy(strategy: Strategy) -> SearchMethod {
    match strategy {
        Strategy::Lazy => SearchMethod::Lazy,
        Strategy::Lazy2 => SearchMethod::Lazy2,
        Strategy::Greedy | Strategy::Fast | Strategy::DoubleFast => SearchMethod::Greedy,
    }
}

impl<'tables> AnyFinder<'tables> {
    #[cfg(feature = "levels")]
    pub fn for_level_with_storage(
        level_parameters: LevelParameters,
        storage: TableStorage<'tables>,
    ) -> Self {
        match level_parameters.strategy {
            Strategy::DoubleFast => AnyFinder::DoubleFast(DoubleFastFinder::new(
                storage.hash_table,
                storage.long_hash_table,
                level_parameters,
            )),
            Strategy::Greedy | Strategy::Lazy | Strategy::Lazy2 => {
                let search_method = search_method_for_strategy(level_parameters.strategy);
                AnyFinder::Chain(HashChainFinder::new(
                    storage.hash_table,
                    storage.chain_table,
                    level_parameters,
                    search_method,
                ))
            }
            Strategy::Fast => AnyFinder::Fast(HashTableFinder::new(level_parameters.window_log)),
        }
    }

    #[cfg(not(feature = "levels"))]
    pub fn for_level_with_storage(
        level_parameters: LevelParameters,
        _storage: TableStorage<'tables>,
    ) -> Self {
        AnyFinder::Fast(HashTableFinder::new(level_parameters.window_log))
    }

    #[cfg(feature = "levels")]
    pub fn for_level_with_storage_at(
        level: u8,
        level_parameters: LevelParameters,
        storage: TableStorage<'tables>,
    ) -> Self {
        if (5..=12).contains(&level)
            && matches!(
                level_parameters.strategy,
                Strategy::Greedy | Strategy::Lazy | Strategy::Lazy2
            )
        {
            let search_method = search_method_for_strategy(level_parameters.strategy);
            return AnyFinder::Row(RowHashFinder::new(
                storage.hash_table,
                storage.chain_table,
                level_parameters,
                search_method,
            ));
        }
        Self::for_level_with_storage(level_parameters, storage)
    }

    #[cfg(not(feature = "levels"))]
    pub fn for_level_with_storage_at(
        _level: u8,
        level_parameters: LevelParameters,
        storage: TableStorage<'tables>,
    ) -> Self {
        Self::for_level_with_storage(level_parameters, storage)
    }
}

impl MatchFinder for AnyFinder<'_> {
    fn reset(&mut self) {
        match self {
            AnyFinder::Fast(finder) => finder.reset(),
            #[cfg(feature = "levels")]
            AnyFinder::DoubleFast(finder) => finder.reset(),
            #[cfg(feature = "levels")]
            AnyFinder::Chain(finder) => finder.reset(),
            #[cfg(feature = "levels")]
            AnyFinder::Row(finder) => finder.reset(),
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
            AnyFinder::DoubleFast(finder) => {
                finder.find_sequences(input, block_start, sequences, repeat_offsets)
            }
            #[cfg(feature = "levels")]
            AnyFinder::Chain(finder) => {
                finder.find_sequences(input, block_start, sequences, repeat_offsets)
            }
            #[cfg(feature = "levels")]
            AnyFinder::Row(finder) => {
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
            AnyFinder::DoubleFast(finder) => finder.window_log(),
            #[cfg(feature = "levels")]
            AnyFinder::Chain(finder) => finder.window_log(),
            #[cfg(feature = "levels")]
            AnyFinder::Row(finder) => finder.window_log(),
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
