use alloc::vec::Vec;

use crate::{
    block::{
        sequence_codes::{
            get_literal_length_code, get_literal_length_extra_bits, get_match_length_code,
            get_match_length_extra_bits, get_offset_code,
        },
        sequence_record::SequenceRecord,
    },
    entropy::histogram::count_symbols,
    frame::{block_header::BLOCK_HEADER_LENGTH, frame_header::FrameFormat},
};

pub const MIN_SPLIT_SEQUENCES: usize = 32;
pub const MIN_SPLIT_LITERAL_BYTES: usize = 4 * 1024;
pub const MIN_SPLIT_BLOCK_BYTES: usize = 8 * 1024;
pub const MAX_BLOCK_SPLITS: usize = 196;
const MIN_SEQUENCES_TO_CONSIDER_SPLITTING: usize = 4;

const LITERALS_HEADER_ESTIMATE: usize = 5;
const HUFFMAN_TABLE_BASE_ESTIMATE: usize = 4;
const SEQUENCE_TABLES_BASE_ESTIMATE: usize = 6;

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

fn cross_entropy_bits(counts: &[u32], total: u32) -> u64 {
    if total == 0 {
        return 0;
    }
    let mut cost = 0u64;
    for &count in counts {
        if count == 0 {
            continue;
        }
        let normalized = ((256u64 * count as u64) / total as u64).clamp(1, 255);
        cost += count as u64 * INVERSE_PROBABILITY_LOG256[normalized as usize] as u64;
    }
    cost >> 8
}

struct SplitContext<'a> {
    sequences: &'a [SequenceRecord],
    literals: &'a [u8],
    literal_prefix: &'a [u32],
    byte_prefix: &'a [u32],
}

fn literal_range<'a>(context: &SplitContext<'a>, start: usize, end: usize) -> &'a [u8] {
    let start_byte = context.literal_prefix[start] as usize;
    let end_byte = context.literal_prefix[end] as usize;
    &context.literals[start_byte..end_byte]
}

fn byte_length(context: &SplitContext, start: usize, end: usize) -> usize {
    (context.byte_prefix[end] - context.byte_prefix[start]) as usize
}

fn trial_encoded_size(context: &SplitContext, start: usize, end: usize) -> usize {
    trial_encoded_size_slices(
        &context.sequences[start..end],
        literal_range(context, start, end),
    )
}

fn used_symbol_count(counts: &[u32]) -> usize {
    counts.iter().filter(|&&count| count > 0).count()
}

fn trial_encoded_size_slices(sequences: &[SequenceRecord], literals: &[u8]) -> usize {
    let mut byte_counts = [0u32; 256];
    count_symbols(literals, &mut byte_counts);
    let literal_bits = cross_entropy_bits(&byte_counts, literals.len() as u32);
    let literal_bytes = if literals.is_empty() {
        0
    } else {
        let used_symbols = used_symbol_count(&byte_counts);
        let huffman_table_estimate = HUFFMAN_TABLE_BASE_ESTIMATE + used_symbols.div_ceil(2);
        (literal_bits as usize).div_ceil(8) + huffman_table_estimate
    };

    let mut literal_length_counts = [0u32; 36];
    let mut match_length_counts = [0u32; 53];
    let mut offset_counts = [0u32; 32];
    let mut extra_bits_total = 0u64;

    for record in sequences {
        let (literal_length_code, _) = get_literal_length_code(record.literal_length);
        let (match_length_code, _) = get_match_length_code(record.match_length);
        let (offset_code, _) = get_offset_code(record.offset_value);
        literal_length_counts[literal_length_code as usize] += 1;
        match_length_counts[match_length_code as usize] += 1;
        offset_counts[offset_code as usize] += 1;
        extra_bits_total += get_literal_length_extra_bits(literal_length_code) as u64;
        extra_bits_total += get_match_length_extra_bits(match_length_code) as u64;
        extra_bits_total += offset_code as u64;
    }

    let sequence_bytes = if sequences.is_empty() {
        1
    } else {
        let sequence_count = sequences.len() as u32;
        let sequence_bits = cross_entropy_bits(&literal_length_counts, sequence_count)
            + cross_entropy_bits(&match_length_counts, sequence_count)
            + cross_entropy_bits(&offset_counts, sequence_count)
            + extra_bits_total;
        let distinct_codes = used_symbol_count(&literal_length_counts)
            + used_symbol_count(&match_length_counts)
            + used_symbol_count(&offset_counts);
        let sequence_table_estimate = SEQUENCE_TABLES_BASE_ESTIMATE + distinct_codes.div_ceil(2);
        (sequence_bits as usize).div_ceil(8) + sequence_table_estimate
    };

    LITERALS_HEADER_ESTIMATE + literal_bytes + sequence_bytes
}

fn derive_splits(context: &SplitContext, start: usize, end: usize, splits: &mut Vec<usize>) {
    if splits.len() >= MAX_BLOCK_SPLITS {
        return;
    }
    let sequence_count = end - start;
    if sequence_count < MIN_SPLIT_SEQUENCES {
        return;
    }
    let literal_bytes = (context.literal_prefix[end] - context.literal_prefix[start]) as usize;
    if literal_bytes < MIN_SPLIT_LITERAL_BYTES {
        return;
    }

    let mid = start + sequence_count / 2;

    let first_bytes = byte_length(context, start, mid);
    let second_bytes = byte_length(context, mid, end);
    if first_bytes < MIN_SPLIT_BLOCK_BYTES || second_bytes < MIN_SPLIT_BLOCK_BYTES {
        return;
    }

    let whole_size = trial_encoded_size(context, start, end);
    let first_size = trial_encoded_size(context, start, mid);
    let second_size = trial_encoded_size(context, mid, end);

    let split_size = first_size
        .saturating_add(second_size)
        .saturating_add(BLOCK_HEADER_LENGTH);
    if split_size < whole_size {
        derive_splits(context, start, mid, splits);
        splits.push(mid);
        derive_splits(context, mid, end, splits);
    }
}

pub fn split_block(
    sequences: &[SequenceRecord],
    literals: &[u8],
    _format: FrameFormat,
) -> Vec<usize> {
    let sequence_count = sequences.len();
    if sequence_count <= MIN_SEQUENCES_TO_CONSIDER_SPLITTING {
        return Vec::new();
    }

    let mut literal_prefix = Vec::with_capacity(sequence_count + 1);
    let mut byte_prefix = Vec::with_capacity(sequence_count + 1);
    literal_prefix.push(0u32);
    byte_prefix.push(0u32);
    let mut literal_accumulator = 0u32;
    let mut byte_accumulator = 0u32;
    for sequence in sequences {
        literal_accumulator += sequence.literal_length;
        byte_accumulator += sequence.literal_length + sequence.match_length;
        literal_prefix.push(literal_accumulator);
        byte_prefix.push(byte_accumulator);
    }

    let context = SplitContext {
        sequences,
        literals,
        literal_prefix: &literal_prefix,
        byte_prefix: &byte_prefix,
    };

    let mut splits = Vec::new();
    derive_splits(&context, 0, sequence_count, &mut splits);
    splits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sequence(literal_length: u32, match_length: u32, offset_value: u32) -> SequenceRecord {
        SequenceRecord {
            literal_length,
            match_length,
            offset_value,
        }
    }

    #[test]
    fn returns_no_splits_for_few_sequences() {
        let sequences = [make_sequence(1, 4, 1); 4];
        let literals = [0u8; 4];
        assert!(split_block(&sequences, &literals, FrameFormat::Zstd).is_empty());
    }

    #[test]
    fn returns_no_splits_when_literals_are_too_short() {
        let mut sequences = Vec::new();
        for _ in 0..400 {
            sequences.push(make_sequence(1, 4, 1));
        }
        let literals = [0u8; 400];
        assert!(split_block(&sequences, &literals, FrameFormat::Zstd).is_empty());
    }

    #[test]
    fn splits_a_block_with_two_distinct_halves() {
        let mut sequences = Vec::new();
        let mut literals = Vec::new();
        for index in 0..4000u32 {
            sequences.push(make_sequence(4, 4, 1));
            let byte = if index < 2000 { b'a' } else { b'z' };
            literals.extend_from_slice(&[byte; 4]);
        }
        for index in 0..4000u32 {
            let target = 1 + (index % 3);
            sequences[index as usize].offset_value =
                if index < 2000 { target } else { target + 40 };
        }

        let splits = split_block(&sequences, &literals, FrameFormat::Zstd);
        assert!(!splits.is_empty());
        for &split in &splits {
            assert!(split > 0 && split < sequences.len());
        }
    }

    #[test]
    fn does_not_split_uniform_data() {
        let mut sequences = Vec::new();
        let mut literals = Vec::new();
        for _ in 0..2000u32 {
            sequences.push(make_sequence(4, 4, 1));
            literals.extend_from_slice(b"abcd");
        }
        assert!(split_block(&sequences, &literals, FrameFormat::Zstd).is_empty());
    }
}
