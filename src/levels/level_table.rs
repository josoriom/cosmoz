#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Strategy {
    Fast,
    Lazy2,
    #[cfg(feature = "compression")]
    Ultra2,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct LevelParameters {
    pub window_log: u8,
    pub chain_log: u8,
    pub hash_log: u8,
    pub search_log: u8,
    pub min_match: u8,
    pub target_length: u32,
    pub strategy: Strategy,
}

#[cfg(feature = "compression")]
pub(crate) const SUPPORTED_LEVELS: [u8; 4] = [1, 9, 12, 22];
#[cfg(not(feature = "compression"))]
pub(crate) const SUPPORTED_LEVELS: [u8; 1] = [1];
pub(crate) const DEFAULT_LEVEL: u8 = SUPPORTED_LEVELS[0];

pub(crate) const fn level_one_parameters() -> LevelParameters {
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

#[cfg(feature = "compression")]
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

#[cfg(feature = "compression")]
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

#[cfg(feature = "compression")]
const fn level_twenty_two_parameters() -> LevelParameters {
    LevelParameters {
        window_log: 27,
        chain_log: 27,
        hash_log: 25,
        search_log: 9,
        min_match: 3,
        target_length: 999,
        strategy: Strategy::Ultra2,
    }
}

#[cfg(feature = "compression")]
const fn level_twenty_two_parameters_for_input_length(input_length: usize) -> LevelParameters {
    let (window_log, chain_log, hash_log, search_log) = if input_length <= SMALL_INPUT_LENGTH {
        (14, 15, 15, 10)
    } else if input_length <= MEDIUM_INPUT_LENGTH {
        (17, 18, 17, 11)
    } else if input_length <= LARGE_INPUT_LENGTH {
        (18, 19, 19, 13)
    } else {
        return level_twenty_two_parameters();
    };
    LevelParameters {
        window_log,
        chain_log,
        hash_log,
        search_log,
        min_match: 3,
        target_length: 999,
        strategy: Strategy::Ultra2,
    }
}

#[cfg(feature = "compression")]
const SMALL_INPUT_LENGTH: usize = 16 * 1024;
#[cfg(feature = "compression")]
const MEDIUM_INPUT_LENGTH: usize = 128 * 1024;
#[cfg(feature = "compression")]
const LARGE_INPUT_LENGTH: usize = 256 * 1024;
#[cfg(feature = "compression")]
const MIN_HASH_LOG: u8 = 6;
#[cfg(feature = "compression")]
const MIN_WINDOW_LOG: u8 = 10;
#[cfg(feature = "compression")]
const MAX_RESIZED_INPUT_LENGTH: usize = 1 << 30;

pub(crate) const fn get_level_parameters(level: u8) -> Option<LevelParameters> {
    match level {
        1 => Some(level_one_parameters()),
        #[cfg(feature = "compression")]
        9 => Some(level_nine_parameters()),
        #[cfg(feature = "compression")]
        12 => Some(level_twelve_parameters()),
        #[cfg(feature = "compression")]
        22 => Some(level_twenty_two_parameters()),
        _ => None,
    }
}

#[cfg(feature = "compression")]
pub(crate) fn get_level_parameters_for_input_length(
    level: u8,
    input_length: usize,
) -> Option<LevelParameters> {
    let parameters = match level {
        22 => level_twenty_two_parameters_for_input_length(input_length),
        _ => get_level_parameters(level)?,
    };
    Some(shrink_parameters_to_input_length(parameters, input_length))
}

#[cfg(feature = "compression")]
fn shrink_parameters_to_input_length(
    parameters: LevelParameters,
    input_length: usize,
) -> LevelParameters {
    let mut shrunk = parameters;
    if input_length <= MAX_RESIZED_INPUT_LENGTH {
        let input_log = if input_length < 1 << MIN_HASH_LOG {
            MIN_HASH_LOG
        } else {
            (usize::BITS - (input_length - 1).leading_zeros()) as u8
        };
        shrunk.window_log = shrunk.window_log.min(input_log);
    }
    shrunk.hash_log = shrunk.hash_log.min(shrunk.window_log + 1);
    let cycle_log = match shrunk.strategy {
        Strategy::Ultra2 => shrunk.chain_log - 1,
        Strategy::Fast | Strategy::Lazy2 => shrunk.chain_log,
    };
    if cycle_log > shrunk.window_log {
        shrunk.chain_log -= cycle_log - shrunk.window_log;
    }
    shrunk.window_log = shrunk.window_log.max(MIN_WINDOW_LOG);
    shrunk
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

    #[cfg(feature = "compression")]
    #[test]
    fn level_twenty_two_shrinks_like_libzstd_for_a_five_megabyte_input() {
        let parameters = get_level_parameters_for_input_length(22, 5_103_183).unwrap();
        assert_eq!(parameters.window_log, 23);
        assert_eq!(parameters.hash_log, 24);
        assert_eq!(parameters.chain_log, 24);
        assert_eq!(parameters.search_log, 9);
        assert_eq!(parameters.strategy, Strategy::Ultra2);
        let small = get_level_parameters_for_input_length(22, 100 * 1024).unwrap();
        assert_eq!(small.window_log, 17);
        assert_eq!(small.search_log, 11);
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
