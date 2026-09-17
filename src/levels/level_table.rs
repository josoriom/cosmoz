#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Strategy {
    Fast,
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

#[cfg(feature = "levels")]
pub const SUPPORTED_LEVELS: [u8; 3] = [1, 9, 12];
#[cfg(not(feature = "levels"))]
pub const SUPPORTED_LEVELS: [u8; 1] = [1];
pub const DEFAULT_LEVEL: u8 = SUPPORTED_LEVELS[0];

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

#[cfg(feature = "levels")]
const fn level_nine_parameters() -> LevelParameters {
    LevelParameters {
        window_log: 22,
        chain_log: 20,
        hash_log: 21,
        search_log: 4,
        min_match: 5,
        target_length: 16,
        strategy: Strategy::Lazy2,
    }
}

#[cfg(feature = "levels")]
const fn level_twelve_parameters() -> LevelParameters {
    LevelParameters {
        window_log: 22,
        chain_log: 22,
        hash_log: 23,
        search_log: 6,
        min_match: 5,
        target_length: 32,
        strategy: Strategy::Lazy2,
    }
}

pub const fn get_level_parameters(level: u8) -> Option<LevelParameters> {
    match level {
        1 => Some(level_one_parameters()),
        #[cfg(feature = "levels")]
        9 => Some(level_nine_parameters()),
        #[cfg(feature = "levels")]
        12 => Some(level_twelve_parameters()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_one_matches_clevels() {
        let parameters = get_level_parameters(1).unwrap();
        assert_eq!(parameters.window_log, 20);
        assert_eq!(parameters.hash_log, 16);
        assert_eq!(parameters.min_match, 4);
        assert_eq!(parameters.strategy, Strategy::Fast);
    }

    #[test]
    fn level_nine_matches_clevels() {
        let parameters = get_level_parameters(9).unwrap();
        assert_eq!(parameters.window_log, 22);
        assert_eq!(parameters.chain_log, 20);
        assert_eq!(parameters.hash_log, 21);
        assert_eq!(parameters.search_log, 4);
        assert_eq!(parameters.min_match, 5);
        assert_eq!(parameters.target_length, 16);
        assert_eq!(parameters.strategy, Strategy::Lazy2);
    }

    #[test]
    fn level_twelve_matches_clevels() {
        let parameters = get_level_parameters(12).unwrap();
        assert_eq!(parameters.window_log, 22);
        assert_eq!(parameters.chain_log, 22);
        assert_eq!(parameters.hash_log, 23);
        assert_eq!(parameters.search_log, 6);
        assert_eq!(parameters.min_match, 5);
        assert_eq!(parameters.target_length, 32);
        assert_eq!(parameters.strategy, Strategy::Lazy2);
    }

    #[test]
    fn rejects_levels_outside_the_supported_set() {
        assert_eq!(get_level_parameters(0), None);
        assert_eq!(get_level_parameters(2), None);
        assert_eq!(get_level_parameters(13), None);
        assert_eq!(get_level_parameters(255), None);
    }

    #[test]
    fn every_supported_level_returns_parameters() {
        for level in SUPPORTED_LEVELS {
            assert!(get_level_parameters(level).is_some());
        }
    }
}
