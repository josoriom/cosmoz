pub const LITERAL_LENGTH_CODE_COUNT: usize = 36;
pub const MATCH_LENGTH_CODE_COUNT: usize = 53;
pub const OFFSET_CODE_COUNT: usize = 32;

const LITERAL_LENGTH_EXTRA_BASE: [(u32, u8); 20] = [
    (16, 1),
    (18, 1),
    (20, 1),
    (22, 1),
    (24, 2),
    (28, 2),
    (32, 3),
    (40, 3),
    (48, 4),
    (64, 6),
    (128, 7),
    (256, 8),
    (512, 9),
    (1024, 10),
    (2048, 11),
    (4096, 12),
    (8192, 13),
    (16384, 14),
    (32768, 15),
    (65536, 16),
];

const MATCH_LENGTH_EXTRA_BASE: [(u32, u8); 21] = [
    (35, 1),
    (37, 1),
    (39, 1),
    (41, 1),
    (43, 2),
    (47, 2),
    (51, 3),
    (59, 3),
    (67, 4),
    (83, 4),
    (99, 5),
    (131, 7),
    (259, 8),
    (515, 9),
    (1027, 10),
    (2051, 11),
    (4099, 12),
    (8195, 13),
    (16387, 14),
    (32771, 15),
    (65539, 16),
];

const fn build_literal_length_base_table() -> [u32; LITERAL_LENGTH_CODE_COUNT] {
    let mut table = [0u32; LITERAL_LENGTH_CODE_COUNT];
    let mut code = 0usize;
    while code < LITERAL_LENGTH_CODE_COUNT {
        table[code] = if code <= 15 {
            code as u32
        } else {
            LITERAL_LENGTH_EXTRA_BASE[code - 16].0
        };
        code += 1;
    }
    table
}

const fn build_literal_length_extra_bits_table() -> [u8; LITERAL_LENGTH_CODE_COUNT] {
    let mut table = [0u8; LITERAL_LENGTH_CODE_COUNT];
    let mut code = 0usize;
    while code < LITERAL_LENGTH_CODE_COUNT {
        table[code] = if code <= 15 {
            0
        } else {
            LITERAL_LENGTH_EXTRA_BASE[code - 16].1
        };
        code += 1;
    }
    table
}

const fn build_match_length_base_table() -> [u32; MATCH_LENGTH_CODE_COUNT] {
    let mut table = [0u32; MATCH_LENGTH_CODE_COUNT];
    let mut code = 0usize;
    while code < MATCH_LENGTH_CODE_COUNT {
        table[code] = if code <= 31 {
            code as u32 + 3
        } else {
            MATCH_LENGTH_EXTRA_BASE[code - 32].0
        };
        code += 1;
    }
    table
}

const fn build_match_length_extra_bits_table() -> [u8; MATCH_LENGTH_CODE_COUNT] {
    let mut table = [0u8; MATCH_LENGTH_CODE_COUNT];
    let mut code = 0usize;
    while code < MATCH_LENGTH_CODE_COUNT {
        table[code] = if code <= 31 {
            0
        } else {
            MATCH_LENGTH_EXTRA_BASE[code - 32].1
        };
        code += 1;
    }
    table
}

const LITERAL_LENGTH_BASE_TABLE: [u32; LITERAL_LENGTH_CODE_COUNT] =
    build_literal_length_base_table();
const LITERAL_LENGTH_EXTRA_BITS_TABLE: [u8; LITERAL_LENGTH_CODE_COUNT] =
    build_literal_length_extra_bits_table();
const MATCH_LENGTH_BASE_TABLE: [u32; MATCH_LENGTH_CODE_COUNT] = build_match_length_base_table();
const MATCH_LENGTH_EXTRA_BITS_TABLE: [u8; MATCH_LENGTH_CODE_COUNT] =
    build_match_length_extra_bits_table();

#[inline(always)]
unsafe fn get_literal_length_base_unchecked(code: u8) -> u32 {
    debug_assert!((code as usize) < LITERAL_LENGTH_CODE_COUNT);
    unsafe { *LITERAL_LENGTH_BASE_TABLE.get_unchecked(code as usize) }
}

#[inline(always)]
unsafe fn get_literal_length_extra_bits_unchecked(code: u8) -> u8 {
    debug_assert!((code as usize) < LITERAL_LENGTH_CODE_COUNT);
    unsafe { *LITERAL_LENGTH_EXTRA_BITS_TABLE.get_unchecked(code as usize) }
}

#[inline(always)]
unsafe fn get_match_length_base_unchecked(code: u8) -> u32 {
    debug_assert!((code as usize) < MATCH_LENGTH_CODE_COUNT);
    unsafe { *MATCH_LENGTH_BASE_TABLE.get_unchecked(code as usize) }
}

#[inline(always)]
unsafe fn get_match_length_extra_bits_unchecked(code: u8) -> u8 {
    debug_assert!((code as usize) < MATCH_LENGTH_CODE_COUNT);
    unsafe { *MATCH_LENGTH_EXTRA_BITS_TABLE.get_unchecked(code as usize) }
}

#[inline(always)]
pub fn get_literal_length_base(code: u8) -> u32 {
    debug_assert!((code as usize) < LITERAL_LENGTH_CODE_COUNT);
    unsafe { get_literal_length_base_unchecked(code) }
}

#[inline(always)]
pub fn get_literal_length_extra_bits(code: u8) -> u8 {
    debug_assert!((code as usize) < LITERAL_LENGTH_CODE_COUNT);
    unsafe { get_literal_length_extra_bits_unchecked(code) }
}

#[inline(always)]
pub fn get_match_length_base(code: u8) -> u32 {
    debug_assert!((code as usize) < MATCH_LENGTH_CODE_COUNT);
    unsafe { get_match_length_base_unchecked(code) }
}

#[inline(always)]
pub fn get_match_length_extra_bits(code: u8) -> u8 {
    debug_assert!((code as usize) < MATCH_LENGTH_CODE_COUNT);
    unsafe { get_match_length_extra_bits_unchecked(code) }
}

pub const MAX_LITERAL_LENGTH: u32 = 131071;
pub const MIN_MATCH_LENGTH: u32 = 3;
pub const MAX_MATCH_LENGTH: u32 = 131074;

const fn highest_set_bit(value: u32) -> u32 {
    31 - value.leading_zeros()
}

const LITERAL_LENGTH_DIRECT_CODE_END: u32 = 15;
const LITERAL_LENGTH_FIRST_TABLE_CODE: usize = 16;
const LITERAL_LENGTH_POWER_OF_TWO_CODE_OFFSET: u32 = 19;
const LITERAL_LENGTH_SMALL_CODE_START: u32 = 16;
const LITERAL_LENGTH_SMALL_CODE_END: u32 = 63;
const LITERAL_LENGTH_SMALL_CODE_LENGTH: usize =
    (LITERAL_LENGTH_SMALL_CODE_END - LITERAL_LENGTH_SMALL_CODE_START + 1) as usize;

const fn find_small_literal_length_code(length: u32) -> u8 {
    let mut table_index = 0usize;
    while table_index < LITERAL_LENGTH_EXTRA_BASE.len() {
        let base = LITERAL_LENGTH_EXTRA_BASE[table_index].0;
        let extra_bits = LITERAL_LENGTH_EXTRA_BASE[table_index].1;
        let range_size = 1u32 << extra_bits;
        if length >= base && length - base < range_size {
            return (table_index + LITERAL_LENGTH_FIRST_TABLE_CODE) as u8;
        }
        table_index += 1;
    }
    (LITERAL_LENGTH_CODE_COUNT - 1) as u8
}

const fn build_literal_length_small_code_table() -> [u8; LITERAL_LENGTH_SMALL_CODE_LENGTH] {
    let mut table = [0u8; LITERAL_LENGTH_SMALL_CODE_LENGTH];
    let mut table_index = 0usize;
    while table_index < LITERAL_LENGTH_SMALL_CODE_LENGTH {
        let length = LITERAL_LENGTH_SMALL_CODE_START + table_index as u32;
        table[table_index] = find_small_literal_length_code(length);
        table_index += 1;
    }
    table
}

const LITERAL_LENGTH_SMALL_CODE_TABLE: [u8; LITERAL_LENGTH_SMALL_CODE_LENGTH] =
    build_literal_length_small_code_table();

const MATCH_LENGTH_DIRECT_CODE_END: u32 = MATCH_LENGTH_SMALL_CODE_START - 1;
const MATCH_LENGTH_SMALL_CODE_START: u32 = 35;
const MATCH_LENGTH_FIRST_TABLE_CODE: usize = 32;
const MATCH_LENGTH_POWER_OF_TWO_CODE_OFFSET: u32 = 36;
const MATCH_LENGTH_SMALL_CODE_END: u32 = 130;
const MATCH_LENGTH_SMALL_CODE_LENGTH: usize =
    (MATCH_LENGTH_SMALL_CODE_END - MATCH_LENGTH_SMALL_CODE_START + 1) as usize;

const fn find_small_match_length_code(length: u32) -> u8 {
    let mut table_index = 0usize;
    while table_index < MATCH_LENGTH_EXTRA_BASE.len() {
        let base = MATCH_LENGTH_EXTRA_BASE[table_index].0;
        let extra_bits = MATCH_LENGTH_EXTRA_BASE[table_index].1;
        let range_size = 1u32 << extra_bits;
        if length >= base && length - base < range_size {
            return (table_index + MATCH_LENGTH_FIRST_TABLE_CODE) as u8;
        }
        table_index += 1;
    }
    (MATCH_LENGTH_CODE_COUNT - 1) as u8
}

const fn build_match_length_small_code_table() -> [u8; MATCH_LENGTH_SMALL_CODE_LENGTH] {
    let mut table = [0u8; MATCH_LENGTH_SMALL_CODE_LENGTH];
    let mut table_index = 0usize;
    while table_index < MATCH_LENGTH_SMALL_CODE_LENGTH {
        let length = MATCH_LENGTH_SMALL_CODE_START + table_index as u32;
        table[table_index] = find_small_match_length_code(length);
        table_index += 1;
    }
    table
}

const MATCH_LENGTH_SMALL_CODE_TABLE: [u8; MATCH_LENGTH_SMALL_CODE_LENGTH] =
    build_match_length_small_code_table();

pub fn get_literal_length_code(length: u32) -> (u8, u32) {
    debug_assert!(length <= MAX_LITERAL_LENGTH);
    let length = length.min(MAX_LITERAL_LENGTH);
    if length <= LITERAL_LENGTH_DIRECT_CODE_END {
        return (length as u8, 0);
    }
    if length <= LITERAL_LENGTH_SMALL_CODE_END {
        let code =
            LITERAL_LENGTH_SMALL_CODE_TABLE[(length - LITERAL_LENGTH_SMALL_CODE_START) as usize];
        return (code, length - get_literal_length_base(code));
    }
    let highest_bit = highest_set_bit(length);
    let code = (highest_bit + LITERAL_LENGTH_POWER_OF_TWO_CODE_OFFSET) as u8;
    (code, length - (1u32 << highest_bit))
}

pub fn get_match_length_code(length: u32) -> (u8, u32) {
    debug_assert!((MIN_MATCH_LENGTH..=MAX_MATCH_LENGTH).contains(&length));
    let length = length.clamp(MIN_MATCH_LENGTH, MAX_MATCH_LENGTH);
    if length <= MATCH_LENGTH_DIRECT_CODE_END {
        return ((length - MIN_MATCH_LENGTH) as u8, 0);
    }
    if length <= MATCH_LENGTH_SMALL_CODE_END {
        let code = MATCH_LENGTH_SMALL_CODE_TABLE[(length - MATCH_LENGTH_SMALL_CODE_START) as usize];
        return (code, length - get_match_length_base(code));
    }
    let highest_bit = highest_set_bit(length - MIN_MATCH_LENGTH);
    let code = (highest_bit + MATCH_LENGTH_POWER_OF_TWO_CODE_OFFSET) as u8;
    (code, length - ((1u32 << highest_bit) + MIN_MATCH_LENGTH))
}

pub fn get_offset_code(offset_value: u32) -> (u8, u32) {
    debug_assert!(offset_value >= 1);
    let offset_value = offset_value.max(1);
    let code = highest_set_bit(offset_value) as u8;
    (code, offset_value - (1u32 << code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_length_code_fifteen() {
        assert_eq!(get_literal_length_base(15), 15);
        assert_eq!(get_literal_length_extra_bits(15), 0);
    }

    #[test]
    fn literal_length_code_sixteen() {
        assert_eq!(get_literal_length_base(16), 16);
        assert_eq!(get_literal_length_extra_bits(16), 1);
    }

    #[test]
    fn literal_length_code_thirty_five() {
        assert_eq!(get_literal_length_base(35), 65536);
        assert_eq!(get_literal_length_extra_bits(35), 16);
    }

    #[test]
    fn match_length_code_zero() {
        assert_eq!(get_match_length_base(0), 3);
        assert_eq!(get_match_length_extra_bits(0), 0);
    }

    #[test]
    fn match_length_code_thirty_one() {
        assert_eq!(get_match_length_base(31), 34);
        assert_eq!(get_match_length_extra_bits(31), 0);
    }

    #[test]
    fn match_length_code_thirty_two() {
        assert_eq!(get_match_length_base(32), 35);
        assert_eq!(get_match_length_extra_bits(32), 1);
    }

    #[test]
    fn match_length_code_fifty_two() {
        assert_eq!(get_match_length_base(52), 65539);
        assert_eq!(get_match_length_extra_bits(52), 16);
    }

    #[test]
    fn literal_length_code_inverts_base_table_for_every_length() {
        let mut length = 0u32;
        loop {
            let (code, extra_value) = get_literal_length_code(length);
            let base = get_literal_length_base(code);
            let extra_bits = get_literal_length_extra_bits(code);
            assert_eq!(base + extra_value, length);
            assert!(extra_value < 1u32 << extra_bits);
            if length == MAX_LITERAL_LENGTH {
                break;
            }
            length += 1;
        }
    }

    #[test]
    fn match_length_code_inverts_base_table_for_every_length() {
        let mut length = MIN_MATCH_LENGTH;
        loop {
            let (code, extra_value) = get_match_length_code(length);
            let base = get_match_length_base(code);
            let extra_bits = get_match_length_extra_bits(code);
            assert_eq!(base + extra_value, length);
            assert!(extra_value < 1u32 << extra_bits);
            if length == MAX_MATCH_LENGTH {
                break;
            }
            length += 1;
        }
    }

    fn next_pseudo_random_offset(state: &mut u32) -> u32 {
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        (*state).max(1)
    }

    #[test]
    fn offset_code_inverts_for_boundaries_and_samples() {
        let mut offsets_to_check: Vec<u32> = vec![1];
        let mut power_of_two = 1u32;
        loop {
            offsets_to_check.push(power_of_two);
            if power_of_two > 1 {
                offsets_to_check.push(power_of_two - 1);
            }
            if power_of_two == 1u32 << 31 {
                break;
            }
            power_of_two <<= 1;
        }
        offsets_to_check.push(u32::MAX);

        let mut random_state = 2463534242u32;
        for _ in 0..1000 {
            offsets_to_check.push(next_pseudo_random_offset(&mut random_state));
        }

        for offset_value in offsets_to_check {
            let (code, extra_value) = get_offset_code(offset_value);
            let base = 1u32 << code;
            assert_eq!(base + extra_value, offset_value);
            assert!(extra_value < 1u32 << code);
        }
    }
}
