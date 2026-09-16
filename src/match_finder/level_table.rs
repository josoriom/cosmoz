#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Strategy {
    Fast,
    DoubleFast,
    Greedy,
    Lazy,
    Lazy2,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LevelParameters {
    pub window_log: u8,
    pub chain_log: u8,
    pub hash_log: u8,
    pub search_log: u8,
    pub min_match: u8,
    pub target_length: u32,
    pub strategy: Strategy,
}

pub const MIN_LEVEL: u8 = 1;
#[cfg(feature = "levels")]
pub const MAX_LEVEL: u8 = 12;
#[cfg(not(feature = "levels"))]
pub const MAX_LEVEL: u8 = 1;

pub const fn level_one_parameters() -> LevelParameters {
    LevelParameters {
        window_log: 20,
        chain_log: 13,
        hash_log: 16,
        search_log: 1,
        min_match: 4,
        target_length: 0,
        strategy: Strategy::Fast,
    }
}

pub const fn get_level_parameters(level: u8) -> Option<LevelParameters> {
    match level {
        1 => Some(level_one_parameters()),
        #[cfg(feature = "levels")]
        2 => Some(LevelParameters {
            window_log: 20,
            chain_log: 15,
            hash_log: 16,
            search_log: 1,
            min_match: 6,
            target_length: 0,
            strategy: Strategy::Fast,
        }),
        #[cfg(feature = "levels")]
        3 => Some(LevelParameters {
            window_log: 21,
            chain_log: 16,
            hash_log: 17,
            search_log: 1,
            min_match: 5,
            target_length: 0,
            strategy: Strategy::DoubleFast,
        }),
        #[cfg(feature = "levels")]
        4 => Some(LevelParameters {
            window_log: 21,
            chain_log: 18,
            hash_log: 18,
            search_log: 1,
            min_match: 5,
            target_length: 0,
            strategy: Strategy::DoubleFast,
        }),
        #[cfg(feature = "levels")]
        5 => Some(LevelParameters {
            window_log: 21,
            chain_log: 18,
            hash_log: 19,
            search_log: 3,
            min_match: 5,
            target_length: 2,
            strategy: Strategy::Greedy,
        }),
        #[cfg(feature = "levels")]
        6 => Some(LevelParameters {
            window_log: 21,
            chain_log: 18,
            hash_log: 19,
            search_log: 3,
            min_match: 5,
            target_length: 4,
            strategy: Strategy::Lazy,
        }),
        #[cfg(feature = "levels")]
        7 => Some(LevelParameters {
            window_log: 21,
            chain_log: 19,
            hash_log: 20,
            search_log: 4,
            min_match: 5,
            target_length: 8,
            strategy: Strategy::Lazy,
        }),
        #[cfg(feature = "levels")]
        8 => Some(LevelParameters {
            window_log: 21,
            chain_log: 19,
            hash_log: 20,
            search_log: 4,
            min_match: 5,
            target_length: 16,
            strategy: Strategy::Lazy2,
        }),
        #[cfg(feature = "levels")]
        9 => Some(LevelParameters {
            window_log: 22,
            chain_log: 20,
            hash_log: 21,
            search_log: 4,
            min_match: 5,
            target_length: 16,
            strategy: Strategy::Lazy2,
        }),
        #[cfg(feature = "levels")]
        10 => Some(LevelParameters {
            window_log: 22,
            chain_log: 21,
            hash_log: 22,
            search_log: 5,
            min_match: 5,
            target_length: 16,
            strategy: Strategy::Lazy2,
        }),
        #[cfg(feature = "levels")]
        11 => Some(LevelParameters {
            window_log: 22,
            chain_log: 21,
            hash_log: 22,
            search_log: 6,
            min_match: 5,
            target_length: 16,
            strategy: Strategy::Lazy2,
        }),
        #[cfg(feature = "levels")]
        12 => Some(LevelParameters {
            window_log: 22,
            chain_log: 22,
            hash_log: 23,
            search_log: 6,
            min_match: 5,
            target_length: 32,
            strategy: Strategy::Lazy2,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ClevelsRow {
        level: u8,
        window_log: u8,
        chain_log: u8,
        hash_log: u8,
        search_log: u8,
        min_match: u8,
        target_length: u32,
        strategy: Strategy,
    }

    #[test]
    fn rows_match_clevels() {
        let clevels_rows = [
            ClevelsRow {
                level: 1,
                window_log: 19,
                chain_log: 13,
                hash_log: 14,
                search_log: 1,
                min_match: 7,
                target_length: 0,
                strategy: Strategy::Fast,
            },
            ClevelsRow {
                level: 2,
                window_log: 20,
                chain_log: 15,
                hash_log: 16,
                search_log: 1,
                min_match: 6,
                target_length: 0,
                strategy: Strategy::Fast,
            },
            ClevelsRow {
                level: 3,
                window_log: 21,
                chain_log: 16,
                hash_log: 17,
                search_log: 1,
                min_match: 5,
                target_length: 0,
                strategy: Strategy::DoubleFast,
            },
            ClevelsRow {
                level: 4,
                window_log: 21,
                chain_log: 18,
                hash_log: 18,
                search_log: 1,
                min_match: 5,
                target_length: 0,
                strategy: Strategy::DoubleFast,
            },
            ClevelsRow {
                level: 5,
                window_log: 21,
                chain_log: 18,
                hash_log: 19,
                search_log: 3,
                min_match: 5,
                target_length: 2,
                strategy: Strategy::Greedy,
            },
            ClevelsRow {
                level: 6,
                window_log: 21,
                chain_log: 18,
                hash_log: 19,
                search_log: 3,
                min_match: 5,
                target_length: 4,
                strategy: Strategy::Lazy,
            },
            ClevelsRow {
                level: 7,
                window_log: 21,
                chain_log: 19,
                hash_log: 20,
                search_log: 4,
                min_match: 5,
                target_length: 8,
                strategy: Strategy::Lazy,
            },
            ClevelsRow {
                level: 8,
                window_log: 21,
                chain_log: 19,
                hash_log: 20,
                search_log: 4,
                min_match: 5,
                target_length: 16,
                strategy: Strategy::Lazy2,
            },
            ClevelsRow {
                level: 9,
                window_log: 22,
                chain_log: 20,
                hash_log: 21,
                search_log: 4,
                min_match: 5,
                target_length: 16,
                strategy: Strategy::Lazy2,
            },
            ClevelsRow {
                level: 10,
                window_log: 22,
                chain_log: 21,
                hash_log: 22,
                search_log: 5,
                min_match: 5,
                target_length: 16,
                strategy: Strategy::Lazy2,
            },
            ClevelsRow {
                level: 11,
                window_log: 22,
                chain_log: 21,
                hash_log: 22,
                search_log: 6,
                min_match: 5,
                target_length: 16,
                strategy: Strategy::Lazy2,
            },
            ClevelsRow {
                level: 12,
                window_log: 22,
                chain_log: 22,
                hash_log: 23,
                search_log: 6,
                min_match: 5,
                target_length: 32,
                strategy: Strategy::Lazy2,
            },
        ];

        for row in clevels_rows {
            let parameters = get_level_parameters(row.level).unwrap();

            if row.level == 1 {
                assert_eq!(parameters.window_log, 20);
                assert_eq!(parameters.hash_log, 16);
                assert_eq!(parameters.min_match, 4);
                assert_eq!(parameters.strategy, Strategy::Fast);
                continue;
            }

            assert_eq!(
                parameters.window_log, row.window_log,
                "level {} window_log",
                row.level
            );
            assert_eq!(
                parameters.chain_log, row.chain_log,
                "level {} chain_log",
                row.level
            );
            assert_eq!(
                parameters.hash_log, row.hash_log,
                "level {} hash_log",
                row.level
            );
            assert_eq!(
                parameters.search_log, row.search_log,
                "level {} search_log",
                row.level
            );
            assert_eq!(
                parameters.min_match, row.min_match,
                "level {} min_match",
                row.level
            );
            assert_eq!(
                parameters.target_length, row.target_length,
                "level {} target_length",
                row.level
            );
            assert_eq!(
                parameters.strategy, row.strategy,
                "level {} strategy",
                row.level
            );
        }
    }

    #[test]
    fn rejects_levels_outside_the_supported_range() {
        assert_eq!(get_level_parameters(0), None);
        assert_eq!(get_level_parameters(13), None);
        assert_eq!(get_level_parameters(255), None);
    }

    #[test]
    fn every_supported_level_returns_parameters() {
        for level in MIN_LEVEL..=MAX_LEVEL {
            assert!(get_level_parameters(level).is_some());
        }
    }
}
