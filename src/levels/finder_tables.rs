use super::level_table::{LevelParameters, Strategy};

pub(crate) struct TableStorage<'tables> {
    pub hash_table: &'tables mut [u32],
    pub chain_table: &'tables mut [u32],
    pub optimal_table: &'tables mut [u32],
}

pub(crate) fn hash_table_length(level_parameters: LevelParameters) -> usize {
    1usize << level_parameters.hash_log
}

pub(crate) fn chain_table_length(level_parameters: LevelParameters) -> usize {
    1usize << level_parameters.chain_log
}

pub(crate) fn optimal_table_length(level_parameters: LevelParameters) -> usize {
    match level_parameters.strategy {
        #[cfg(feature = "compression")]
        Strategy::Ultra2 => crate::algorithms::ultra2::get_optimal_table_length(level_parameters),
        _ => 0,
    }
}

#[cfg(feature = "compression")]
pub(crate) fn get_table_parameters_for_input(
    level: u8,
    level_parameters: LevelParameters,
    input_length: usize,
) -> LevelParameters {
    match level_parameters.strategy {
        Strategy::Ultra2 => {
            super::level_table::get_level_parameters_for_input_length(level, input_length)
                .unwrap_or(level_parameters)
        }
        Strategy::Fast | Strategy::Lazy2 => level_parameters,
    }
}

#[cfg(not(feature = "compression"))]
pub(crate) fn get_table_parameters_for_input(
    _level: u8,
    level_parameters: LevelParameters,
    _input_length: usize,
) -> LevelParameters {
    level_parameters
}

pub(crate) fn table_memory_length(level_parameters: LevelParameters) -> usize {
    hash_table_length(level_parameters)
        + chain_table_length(level_parameters)
        + optimal_table_length(level_parameters)
}

pub(crate) fn split_table_storage(
    memory: &mut [u32],
    level_parameters: LevelParameters,
) -> TableStorage<'_> {
    let hash_length = hash_table_length(level_parameters).min(memory.len());
    let (hash_table, remaining) = memory.split_at_mut(hash_length);

    let chain_length = chain_table_length(level_parameters).min(remaining.len());
    let (chain_region, remaining) = remaining.split_at_mut(chain_length);

    let optimal_length = optimal_table_length(level_parameters).min(remaining.len());
    let optimal_table = &mut remaining[..optimal_length];

    match level_parameters.strategy {
        Strategy::Fast => TableStorage {
            hash_table,
            chain_table: &mut [],
            optimal_table,
        },
        Strategy::Lazy2 => TableStorage {
            hash_table,
            chain_table: chain_region,
            optimal_table,
        },
        #[cfg(feature = "compression")]
        Strategy::Ultra2 => TableStorage {
            hash_table,
            chain_table: chain_region,
            optimal_table,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::levels::level_table::get_level_parameters;

    #[test]
    fn table_memory_length_matches_the_forty_eight_megabyte_budget_at_level_twelve() {
        let level_parameters = get_level_parameters(12).unwrap();
        let length = table_memory_length(level_parameters);
        assert_eq!(length * core::mem::size_of::<u32>(), 48 * 1024 * 1024);
    }

    #[test]
    fn split_table_storage_gives_lazy2_finders_their_two_tables() {
        let level_parameters = get_level_parameters(9).unwrap();
        let mut memory = vec![0u32; table_memory_length(level_parameters)];
        let storage = split_table_storage(&mut memory, level_parameters);
        assert_eq!(
            storage.hash_table.len(),
            hash_table_length(level_parameters)
        );
        assert_eq!(
            storage.chain_table.len(),
            chain_table_length(level_parameters)
        );
    }

    #[test]
    fn split_table_storage_gives_the_fast_finder_only_a_hash_table() {
        let level_parameters = get_level_parameters(1).unwrap();
        let mut memory = vec![0u32; table_memory_length(level_parameters)];
        let storage = split_table_storage(&mut memory, level_parameters);
        assert_eq!(
            storage.hash_table.len(),
            hash_table_length(level_parameters)
        );
        assert_eq!(storage.chain_table.len(), 0);
    }
}
