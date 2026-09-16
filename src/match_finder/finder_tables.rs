use super::level_table::{LevelParameters, Strategy};

pub struct TableStorage<'tables> {
    pub hash_table: &'tables mut [u32],
    pub long_hash_table: &'tables mut [u32],
    pub chain_table: &'tables mut [u32],
}

pub fn hash_table_length(level_parameters: LevelParameters) -> usize {
    1usize << level_parameters.hash_log
}

pub fn chain_table_length(level_parameters: LevelParameters) -> usize {
    1usize << level_parameters.chain_log
}

pub fn table_memory_length(level_parameters: LevelParameters) -> usize {
    hash_table_length(level_parameters) + chain_table_length(level_parameters)
}

pub fn split_table_storage(
    memory: &mut [u32],
    level_parameters: LevelParameters,
) -> TableStorage<'_> {
    let hash_length = hash_table_length(level_parameters).min(memory.len());
    let (hash_table, remaining) = memory.split_at_mut(hash_length);

    let chain_length = chain_table_length(level_parameters).min(remaining.len());
    let (chain_region, _unused) = remaining.split_at_mut(chain_length);

    match level_parameters.strategy {
        Strategy::DoubleFast => TableStorage {
            hash_table,
            long_hash_table: chain_region,
            chain_table: &mut [],
        },
        Strategy::Fast => TableStorage {
            hash_table,
            long_hash_table: &mut [],
            chain_table: &mut [],
        },
        Strategy::Greedy | Strategy::Lazy | Strategy::Lazy2 => TableStorage {
            hash_table,
            long_hash_table: &mut [],
            chain_table: chain_region,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::match_finder::level_table::get_level_parameters;

    #[test]
    fn table_memory_length_matches_the_forty_eight_megabyte_budget_at_level_twelve() {
        let level_parameters = get_level_parameters(12).unwrap();
        let length = table_memory_length(level_parameters);
        assert_eq!(length * core::mem::size_of::<u32>(), 48 * 1024 * 1024);
    }

    #[test]
    fn split_table_storage_gives_double_fast_its_two_tables() {
        let level_parameters = get_level_parameters(3).unwrap();
        let mut memory = vec![0u32; table_memory_length(level_parameters)];
        let storage = split_table_storage(&mut memory, level_parameters);
        assert_eq!(
            storage.hash_table.len(),
            hash_table_length(level_parameters)
        );
        assert_eq!(
            storage.long_hash_table.len(),
            chain_table_length(level_parameters)
        );
        assert_eq!(storage.chain_table.len(), 0);
    }

    #[test]
    fn split_table_storage_gives_hash_chain_finders_their_two_tables() {
        let level_parameters = get_level_parameters(9).unwrap();
        let mut memory = vec![0u32; table_memory_length(level_parameters)];
        let storage = split_table_storage(&mut memory, level_parameters);
        assert_eq!(
            storage.hash_table.len(),
            hash_table_length(level_parameters)
        );
        assert_eq!(storage.long_hash_table.len(), 0);
        assert_eq!(
            storage.chain_table.len(),
            chain_table_length(level_parameters)
        );
    }
}
