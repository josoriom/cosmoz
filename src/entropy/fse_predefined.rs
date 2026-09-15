use crate::entropy::fse_decode_table::{FseDecodeTable, build_fse_decode_table};

pub const LITERAL_LENGTH_ACCURACY_LOG: usize = 6;
pub const MATCH_LENGTH_ACCURACY_LOG: usize = 6;
pub const OFFSET_ACCURACY_LOG: usize = 5;

pub const LITERAL_LENGTH_DEFAULT_COUNTS: [i16; 36] = [
    4, 3, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 2, 1, 1, 1, 1, 1,
    -1, -1, -1, -1,
];

pub const MATCH_LENGTH_DEFAULT_COUNTS: [i16; 53] = [
    1, 4, 3, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1, -1, -1, -1,
];

pub const OFFSET_DEFAULT_COUNTS: [i16; 29] = [
    1, 1, 1, 1, 1, 1, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1, -1,
];

pub fn build_predefined_literal_length_table(table: &mut FseDecodeTable) {
    build_fse_decode_table(
        &LITERAL_LENGTH_DEFAULT_COUNTS,
        LITERAL_LENGTH_ACCURACY_LOG,
        table,
    )
    .expect("literal length default distribution always builds a valid table");
}

pub fn build_predefined_match_length_table(table: &mut FseDecodeTable) {
    build_fse_decode_table(
        &MATCH_LENGTH_DEFAULT_COUNTS,
        MATCH_LENGTH_ACCURACY_LOG,
        table,
    )
    .expect("match length default distribution always builds a valid table");
}

pub fn build_predefined_offset_table(table: &mut FseDecodeTable) {
    build_fse_decode_table(&OFFSET_DEFAULT_COUNTS, OFFSET_ACCURACY_LOG, table)
        .expect("offset default distribution always builds a valid table");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_length_default_counts_sum_to_the_table_size() {
        let effective_sum: i32 = LITERAL_LENGTH_DEFAULT_COUNTS
            .iter()
            .map(|&count| if count == -1 { 1 } else { count as i32 })
            .sum();
        assert_eq!(effective_sum, 1 << LITERAL_LENGTH_ACCURACY_LOG);
    }

    #[test]
    fn match_length_default_counts_sum_to_the_table_size() {
        let effective_sum: i32 = MATCH_LENGTH_DEFAULT_COUNTS
            .iter()
            .map(|&count| if count == -1 { 1 } else { count as i32 })
            .sum();
        assert_eq!(effective_sum, 1 << MATCH_LENGTH_ACCURACY_LOG);
    }

    #[test]
    fn offset_default_counts_sum_to_the_table_size() {
        let effective_sum: i32 = OFFSET_DEFAULT_COUNTS
            .iter()
            .map(|&count| if count == -1 { 1 } else { count as i32 })
            .sum();
        assert_eq!(effective_sum, 1 << OFFSET_ACCURACY_LOG);
    }

    #[test]
    fn literal_length_table_matches_the_rfc_8878_appendix_a_1_table() {
        let mut table = FseDecodeTable::new();
        build_predefined_literal_length_table(&mut table);

        let expected_first_ten_states: [(u8, u8, u16); 10] = [
            (0, 4, 0),
            (0, 4, 16),
            (1, 5, 32),
            (3, 5, 0),
            (4, 5, 0),
            (6, 5, 0),
            (7, 5, 0),
            (9, 5, 0),
            (10, 5, 0),
            (12, 5, 0),
        ];

        for (state, (symbol, bit_count, next_state_base)) in
            expected_first_ten_states.into_iter().enumerate()
        {
            let entry = table.entries[state];
            assert_eq!(entry.symbol, symbol, "state {state} symbol");
            assert_eq!(entry.bit_count, bit_count, "state {state} bit_count");
            assert_eq!(
                entry.next_state_base, next_state_base,
                "state {state} next_state_base"
            );
        }
    }
}
