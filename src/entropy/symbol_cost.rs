use crate::entropy::fse_encode_table::FseEncodeTable;

const COST_ACCURACY_LOG: usize = 8;
const COST_SCALE: usize = 1 << COST_ACCURACY_LOG;

const INVERSE_PROBABILITY_LOG256: [u32; 256] = [
    0, 2048, 1792, 1642, 1536, 1454, 1386, 1329, 1280, 1236, 1198, 1162, 1130, 1101, 1073, 1048,
    1024, 1002, 980, 961, 942, 924, 906, 890, 874, 859, 845, 831, 817, 804, 792, 780, 768, 757,
    746, 735, 724, 714, 705, 695, 686, 676, 668, 659, 650, 642, 634, 626, 618, 611, 603, 596, 589,
    582, 575, 568, 561, 555, 548, 542, 536, 530, 524, 518, 512, 506, 501, 495, 490, 484, 479, 474,
    468, 463, 458, 453, 449, 444, 439, 434, 430, 425, 420, 416, 412, 407, 403, 399, 394, 390, 386,
    382, 378, 374, 370, 366, 362, 358, 355, 351, 347, 343, 340, 336, 333, 329, 326, 322, 319, 315,
    312, 309, 305, 302, 299, 296, 292, 289, 286, 283, 280, 277, 274, 271, 268, 265, 262, 259, 256,
    253, 250, 247, 245, 242, 239, 236, 234, 231, 228, 226, 223, 220, 218, 215, 212, 210, 207, 205,
    202, 200, 197, 195, 193, 190, 188, 185, 183, 181, 178, 176, 174, 171, 169, 167, 164, 162, 160,
    158, 156, 153, 151, 149, 147, 145, 143, 140, 138, 136, 134, 132, 130, 128, 126, 124, 122, 120,
    118, 116, 114, 112, 110, 108, 106, 104, 102, 101, 99, 97, 95, 93, 91, 89, 87, 86, 84, 82, 80,
    78, 77, 75, 73, 71, 70, 68, 66, 64, 63, 61, 59, 58, 56, 54, 53, 51, 49, 48, 46, 44, 43, 41, 40,
    38, 36, 35, 33, 32, 30, 28, 27, 25, 24, 22, 21, 19, 18, 16, 15, 13, 12, 10, 9, 7, 6, 4, 3, 1,
];

pub(crate) fn get_entropy_bits(counts: &[u32], total: usize) -> usize {
    if total == 0 {
        return 0;
    }
    let mut cost = 0usize;
    for &count in counts {
        if count == 0 {
            continue;
        }
        let share = ((COST_SCALE * count as usize) / total).clamp(1, COST_SCALE - 1);
        cost += count as usize * INVERSE_PROBABILITY_LOG256[share] as usize;
    }
    cost >> COST_ACCURACY_LOG
}

pub(crate) fn get_shared_table_bits(
    normalized_counts: &[i16],
    accuracy_log: usize,
    counts: &[u32],
) -> usize {
    debug_assert!(accuracy_log <= COST_ACCURACY_LOG);
    let shift = COST_ACCURACY_LOG - accuracy_log;
    let mut cost = 0usize;
    for (symbol, &count) in counts.iter().enumerate() {
        if count == 0 {
            continue;
        }
        let normalized = match normalized_counts.get(symbol) {
            Some(&normalized) if normalized != -1 => normalized.max(0) as usize,
            _ => 1,
        };
        let share = (normalized << shift).clamp(1, COST_SCALE - 1);
        cost += count as usize * INVERSE_PROBABILITY_LOG256[share] as usize;
    }
    cost >> COST_ACCURACY_LOG
}

pub(crate) fn get_encode_table_bits(table: &FseEncodeTable, counts: &[u32]) -> Option<usize> {
    let table_log = table.accuracy_log as usize;
    if table_log == 0 || table.symbol_count < counts.len() {
        return None;
    }
    let too_expensive = (table_log + 1) * COST_SCALE;
    let table_size = 1usize << table_log;
    let mut cost = 0usize;
    for (symbol, &count) in counts.iter().enumerate() {
        if count == 0 {
            continue;
        }
        let bits_delta = table.transforms.get(symbol)?.bits_delta as usize;
        let shortest_bits = bits_delta >> 16;
        let threshold = (shortest_bits + 1) << 16;
        let distance_to_threshold = threshold.checked_sub(bits_delta + table_size)?;
        let extra_bits = (distance_to_threshold << COST_ACCURACY_LOG) >> table_log;
        let symbol_bits = ((shortest_bits + 1) * COST_SCALE).checked_sub(extra_bits)?;
        if symbol_bits >= too_expensive {
            return None;
        }
        cost += count as usize * symbol_bits;
    }
    Some(cost >> COST_ACCURACY_LOG)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::fse_encode_table::{build_fse_encode_table, normalize_counts};

    fn build_table(counts: &[u32], accuracy_log: usize) -> FseEncodeTable {
        let total = counts.iter().sum::<u32>() as usize;
        let mut normalized_counts = [0i16; 64];
        normalize_counts(
            counts,
            total,
            accuracy_log,
            &mut normalized_counts[..counts.len()],
        )
        .unwrap();
        let mut table = FseEncodeTable::new();
        build_fse_encode_table(&normalized_counts[..counts.len()], accuracy_log, &mut table).unwrap();
        table
    }

    #[test]
    fn entropy_bits_grow_when_symbols_are_spread_out() {
        let concentrated = [100u32, 1, 1, 1];
        let spread = [26u32, 26, 26, 25];
        let total = 103usize;
        assert!(get_entropy_bits(&concentrated, total) < get_entropy_bits(&spread, total));
    }

    #[test]
    fn a_table_costs_least_on_the_counts_it_was_built_from() {
        let counts = [40u32, 30, 20, 10];
        let table = build_table(&counts, 6);
        let matching_cost = get_encode_table_bits(&table, &counts).unwrap();

        let other_counts = [10u32, 20, 30, 40];
        let other_cost = get_encode_table_bits(&table, &other_counts).unwrap();
        assert!(matching_cost < other_cost);
    }

    #[test]
    fn a_table_cannot_encode_a_symbol_it_never_saw() {
        let counts = [40u32, 30, 20, 10, 0, 0];
        let table = build_table(&counts, 6);
        let with_new_symbol = [40u32, 30, 20, 10, 0, 5];
        assert_eq!(get_encode_table_bits(&table, &with_new_symbol), None);
    }

    #[test]
    fn shared_table_bits_match_entropy_bits_for_a_matching_share() {
        let counts = [64u32, 64, 64, 64];
        let normalized = [16i16, 16, 16, 16];
        assert_eq!(
            get_shared_table_bits(&normalized, 6, &counts),
            get_entropy_bits(&counts, 256)
        );
    }
}
