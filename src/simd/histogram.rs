pub fn count_symbols(input: &[u8], counts: &mut [u32; 256]) {
    count_symbols_four_tables(input, counts);
}

fn count_symbols_scalar(input: &[u8], counts: &mut [u32; 256]) {
    for count in counts.iter_mut() {
        *count = 0;
    }
    for &byte in input {
        counts[byte as usize] = counts[byte as usize].saturating_add(1);
    }
}

fn count_symbols_four_tables(input: &[u8], counts: &mut [u32; 256]) {
    let mut table0 = [0u32; 256];
    let mut table1 = [0u32; 256];
    let mut table2 = [0u32; 256];
    let mut table3 = [0u32; 256];

    let block_count = input.len() / 16;
    let unrolled_length = block_count * 16;

    let mut position = 0usize;
    while position < unrolled_length {
        bump(&mut table0, input[position]);
        bump(&mut table1, input[position + 1]);
        bump(&mut table2, input[position + 2]);
        bump(&mut table3, input[position + 3]);
        bump(&mut table0, input[position + 4]);
        bump(&mut table1, input[position + 5]);
        bump(&mut table2, input[position + 6]);
        bump(&mut table3, input[position + 7]);
        bump(&mut table0, input[position + 8]);
        bump(&mut table1, input[position + 9]);
        bump(&mut table2, input[position + 10]);
        bump(&mut table3, input[position + 11]);
        bump(&mut table0, input[position + 12]);
        bump(&mut table1, input[position + 13]);
        bump(&mut table2, input[position + 14]);
        bump(&mut table3, input[position + 15]);
        position += 16;
    }

    accumulate_tail(&input[unrolled_length..], &mut table0);

    for symbol in 0..256 {
        counts[symbol] = table0[symbol]
            .saturating_add(table1[symbol])
            .saturating_add(table2[symbol])
            .saturating_add(table3[symbol]);
    }
}

fn bump(table: &mut [u32; 256], byte: u8) {
    table[byte as usize] = table[byte as usize].saturating_add(1);
}

fn accumulate_tail(input: &[u8], table: &mut [u32; 256]) {
    for &byte in input {
        bump(table, byte);
    }
}

fn xorshift_next(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

fn fill_random(buffer: &mut [u8], seed: u32) {
    let mut state = seed | 1;
    for byte in buffer.iter_mut() {
        *byte = (xorshift_next(&mut state) & 0xff) as u8;
    }
}

const SELF_TEST_LENGTHS: [usize; 13] = [0, 1, 15, 16, 17, 31, 32, 33, 63, 64, 65, 1000, 65536];
const MAX_SELF_TEST_LENGTH: usize = 65536;
const MAX_ALIGNMENT: usize = 15;

pub fn run_self_tests() -> Option<u32> {
    let mut test_index = 0u32;
    let mut buffer = [0u8; MAX_SELF_TEST_LENGTH + MAX_ALIGNMENT];

    for &length in SELF_TEST_LENGTHS.iter() {
        for alignment in 0..=MAX_ALIGNMENT {
            let seed =
                0x9e37_79b9u32 ^ (length as u32).wrapping_mul(2654435761) ^ (alignment as u32);
            fill_random(&mut buffer[..length + alignment], seed);
            let input = &buffer[alignment..alignment + length];

            let mut expected = [0u32; 256];
            let mut actual = [0u32; 256];
            count_symbols_scalar(input, &mut expected);
            count_symbols_four_tables(input, &mut actual);
            if expected != actual {
                return Some(test_index);
            }
            test_index += 1;
        }
    }

    for &length in SELF_TEST_LENGTHS.iter() {
        if length == 0 {
            test_index += 1;
            continue;
        }
        for value in [0u8, 255u8] {
            for byte in buffer[..length].iter_mut() {
                *byte = value;
            }
            let input = &buffer[..length];

            let mut expected = [0u32; 256];
            let mut actual = [0u32; 256];
            count_symbols_scalar(input, &mut expected);
            count_symbols_four_tables(input, &mut actual);
            if expected != actual {
                return Some(test_index);
            }
            test_index += 1;
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const LENGTHS: [usize; 13] = SELF_TEST_LENGTHS;

    fn random_bytes(count: usize, seed: u32) -> std::vec::Vec<u8> {
        let mut buffer = std::vec![0u8; count];
        fill_random(&mut buffer, seed);
        buffer
    }

    #[test]
    fn four_tables_matches_scalar_across_lengths_and_alignments_with_random_contents() {
        for &length in LENGTHS.iter() {
            for alignment in 0..16usize {
                let buffer = random_bytes(length + alignment, 0x1234_5678 ^ length as u32);
                let input = &buffer[alignment..alignment + length];

                let mut expected = [0u32; 256];
                let mut actual = [0u32; 256];
                count_symbols_scalar(input, &mut expected);
                count_symbols_four_tables(input, &mut actual);

                assert_eq!(
                    expected, actual,
                    "length {} alignment {}",
                    length, alignment
                );
            }
        }
    }

    #[test]
    fn four_tables_matches_scalar_on_a_run_of_one_byte_value() {
        for &length in LENGTHS.iter() {
            if length == 0 {
                continue;
            }
            for alignment in 0..16usize {
                let mut buffer = std::vec![7u8; length + alignment];
                buffer[..alignment].fill(9);
                let input = &buffer[alignment..alignment + length];

                let mut expected = [0u32; 256];
                let mut actual = [0u32; 256];
                count_symbols_scalar(input, &mut expected);
                count_symbols_four_tables(input, &mut actual);

                assert_eq!(
                    expected, actual,
                    "length {} alignment {}",
                    length, alignment
                );
                assert_eq!(actual[7], length as u32);
            }
        }
    }

    #[test]
    fn dispatcher_matches_scalar() {
        for &length in LENGTHS.iter() {
            let buffer = random_bytes(length, 0xabcd_ef01);
            let mut expected = [0u32; 256];
            let mut actual = [0u32; 256];
            count_symbols_scalar(&buffer, &mut expected);
            count_symbols(&buffer, &mut actual);
            assert_eq!(expected, actual, "length {}", length);
        }
    }

    #[test]
    fn self_tests_pass() {
        assert_eq!(run_self_tests(), None);
    }
}
