use crate::{
    bits::{backward_bit_reader::BackwardBitReader, forward_bit_reader::ForwardBitReader},
    error::DecodeError,
};

pub const MAX_ACCURACY_LOG: usize = 9;
pub const MAX_TABLE_SIZE: usize = 1 << MAX_ACCURACY_LOG;
pub const MAX_SYMBOL_COUNT: usize = 256;

#[derive(Clone, Copy, Default)]
pub struct FseDecodeEntry {
    pub symbol: u8,
    pub bit_count: u8,
    pub next_state_base: u16,
}

pub struct FseDecodeTable {
    pub entries: [FseDecodeEntry; MAX_TABLE_SIZE],
    pub accuracy_log: u8,
}

impl FseDecodeTable {
    pub const fn new() -> Self {
        Self {
            entries: [FseDecodeEntry {
                symbol: 0,
                bit_count: 0,
                next_state_base: 0,
            }; MAX_TABLE_SIZE],
            accuracy_log: 0,
        }
    }

    pub fn table_size(&self) -> usize {
        1usize << self.accuracy_log
    }
}

impl Default for FseDecodeTable {
    fn default() -> Self {
        Self::new()
    }
}

pub struct FseDecodeState {
    pub state: usize,
}

impl FseDecodeState {
    pub fn new(reader: &mut BackwardBitReader, table: &FseDecodeTable) -> Self {
        let state = reader.read_bits(table.accuracy_log as usize) as usize;
        Self { state }
    }

    #[inline(always)]
    pub fn get_symbol(&self, table: &FseDecodeTable) -> u8 {
        debug_assert!(self.state < table.entries.len());
        unsafe { get_entry_unchecked(table, self.state).symbol }
    }

    #[inline(always)]
    pub fn update(&mut self, reader: &mut BackwardBitReader, table: &FseDecodeTable) {
        debug_assert!(self.state < table.entries.len());
        let entry = unsafe { get_entry_unchecked(table, self.state) };
        let read_bits = reader.read_bits(entry.bit_count as usize) as usize;
        self.state = entry.next_state_base as usize + read_bits;
    }
}

#[inline(always)]
unsafe fn get_entry_unchecked(table: &FseDecodeTable, state: usize) -> FseDecodeEntry {
    debug_assert!(state < table.entries.len());
    unsafe { *table.entries.get_unchecked(state) }
}

pub fn read_fse_table_description(
    input: &[u8],
    max_accuracy_log: usize,
    max_symbol: usize,
    table: &mut FseDecodeTable,
) -> Result<usize, DecodeError> {
    if input.is_empty() {
        return Err(DecodeError::InputTooShort);
    }
    let mut reader = ForwardBitReader::new(input);
    let accuracy_log = reader.read_bits(4) as usize + 5;
    if accuracy_log > max_accuracy_log || accuracy_log > MAX_ACCURACY_LOG {
        return Err(DecodeError::BadFseTable);
    }

    let table_size = 1usize << accuracy_log;
    let mut normalized_counts = [0i16; MAX_SYMBOL_COUNT];
    let mut symbol_count = 0usize;
    let mut remaining = table_size + 1;

    while remaining > 1 {
        let bits_needed = count_bits_needed(remaining);
        let low = reader.read_bits(bits_needed - 1) as usize;
        let threshold = (1usize << bits_needed) - 1 - remaining;
        let value = if low < threshold {
            low
        } else {
            let extra_bit = reader.read_bits(1) as usize;
            if extra_bit != 0 {
                low + (1usize << (bits_needed - 1)) - threshold
            } else {
                low
            }
        };

        push_normalized_count(&mut normalized_counts, &mut symbol_count, max_symbol, value)?;

        if value == 0 {
            remaining -= 1;
        } else {
            remaining -= value - 1;
        }

        if value == 1 {
            loop {
                let repeat = reader.read_bits(2) as usize;
                for _ in 0..repeat {
                    push_normalized_count(
                        &mut normalized_counts,
                        &mut symbol_count,
                        max_symbol,
                        1,
                    )?;
                }
                if repeat != 3 {
                    break;
                }
            }
        }
    }

    if remaining != 1 || symbol_count == 0 {
        return Err(DecodeError::BadFseTable);
    }

    build_fse_decode_table(&normalized_counts[..symbol_count], accuracy_log, table)?;

    // Reading past the end of `input` yields zero bits instead of failing, and a run of
    // zeroes walks `remaining` down to one just as a real description would. A truncated
    // description can therefore build a table and report consuming more bytes than were
    // ever supplied. Callers slice with this count, so reject it here rather than hand
    // back an offset that points outside the input.
    let bytes_used = reader.bytes_used();
    if bytes_used > input.len() {
        return Err(DecodeError::InputTooShort);
    }
    Ok(bytes_used)
}

fn push_normalized_count(
    normalized_counts: &mut [i16; MAX_SYMBOL_COUNT],
    symbol_count: &mut usize,
    max_symbol: usize,
    value: usize,
) -> Result<(), DecodeError> {
    if *symbol_count > max_symbol || *symbol_count >= MAX_SYMBOL_COUNT {
        return Err(DecodeError::BadFseTable);
    }
    normalized_counts[*symbol_count] = if value == 0 { -1 } else { (value - 1) as i16 };
    *symbol_count += 1;
    Ok(())
}

pub fn build_rle_table(symbol: u8, table: &mut FseDecodeTable) {
    table.accuracy_log = 0;
    table.entries[0] = FseDecodeEntry {
        symbol,
        bit_count: 0,
        next_state_base: 0,
    };
}

pub fn build_fse_decode_table(
    normalized_counts: &[i16],
    accuracy_log: usize,
    table: &mut FseDecodeTable,
) -> Result<(), DecodeError> {
    if accuracy_log == 0 || accuracy_log > MAX_ACCURACY_LOG {
        return Err(DecodeError::BadFseTable);
    }
    if normalized_counts.is_empty() || normalized_counts.len() > MAX_SYMBOL_COUNT {
        return Err(DecodeError::BadFseTable);
    }

    let table_size = 1usize << accuracy_log;

    let mut effective_sum = 0usize;
    for &count in normalized_counts {
        if count < -1 {
            return Err(DecodeError::BadFseTable);
        }
        effective_sum += if count == -1 { 1 } else { count as usize };
    }
    if effective_sum != table_size {
        return Err(DecodeError::BadFseTable);
    }

    let mut symbol_of_position = [0u8; MAX_TABLE_SIZE];
    let mut high_threshold = table_size - 1;
    for (symbol, &count) in normalized_counts.iter().enumerate() {
        if count == -1 {
            symbol_of_position[high_threshold] = symbol as u8;
            high_threshold = high_threshold.wrapping_sub(1);
        }
    }

    let step = get_spread_step(table_size);
    let position_mask = table_size - 1;
    let mut position = 0usize;
    for (symbol, &count) in normalized_counts.iter().enumerate() {
        if count <= 0 {
            continue;
        }
        for _ in 0..count {
            symbol_of_position[position] = symbol as u8;
            position = (position + step) & position_mask;
            while position > high_threshold {
                position = (position + step) & position_mask;
            }
        }
    }
    if position != 0 {
        return Err(DecodeError::BadFseTable);
    }

    let mut symbol_next_state = [0u16; MAX_SYMBOL_COUNT];
    for (symbol, &count) in normalized_counts.iter().enumerate() {
        symbol_next_state[symbol] = if count == -1 { 1 } else { count as u16 };
    }

    for (cell_index, &symbol) in symbol_of_position.iter().enumerate().take(table_size) {
        let next_state = symbol_next_state[symbol as usize];
        symbol_next_state[symbol as usize] += 1;
        let bit_count = accuracy_log - (count_bits_needed(next_state as usize) - 1);
        let next_state_base = ((next_state as usize) << bit_count) - table_size;
        table.entries[cell_index] = FseDecodeEntry {
            symbol,
            bit_count: bit_count as u8,
            next_state_base: next_state_base as u16,
        };
    }

    table.accuracy_log = accuracy_log as u8;
    Ok(())
}

fn get_spread_step(table_size: usize) -> usize {
    (table_size >> 1) + (table_size >> 3) + 3
}

fn count_bits_needed(value: usize) -> usize {
    (usize::BITS - value.leading_zeros()) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_description_that_reads_past_the_end_of_its_input() {
        // A single zero byte sets an accuracy log of five and then runs out. The zeroes the
        // reader invents past the end walk `remaining` down to one, so the description looks
        // complete while reporting far more bytes consumed than were supplied.
        let mut table = FseDecodeTable::new();
        assert_eq!(
            read_fse_table_description(&[0x00], 9, 35, &mut table),
            Err(DecodeError::InputTooShort)
        );
    }

    #[test]
    fn build_fse_decode_table_matches_a_hand_computed_table() {
        let counts: [i16; 4] = [8, 4, 2, 2];
        let mut table = FseDecodeTable::new();
        build_fse_decode_table(&counts, 4, &mut table).unwrap();

        let expected: [(u8, u8, u16); 16] = [
            (0, 1, 0),
            (0, 1, 2),
            (1, 2, 0),
            (3, 3, 0),
            (0, 1, 4),
            (1, 2, 4),
            (3, 3, 8),
            (0, 1, 6),
            (1, 2, 8),
            (2, 3, 0),
            (0, 1, 8),
            (0, 1, 10),
            (2, 3, 8),
            (0, 1, 12),
            (0, 1, 14),
            (1, 2, 12),
        ];

        for (cell_index, (symbol, bit_count, next_state_base)) in expected.into_iter().enumerate() {
            let entry = table.entries[cell_index];
            assert_eq!(entry.symbol, symbol, "cell {cell_index} symbol");
            assert_eq!(entry.bit_count, bit_count, "cell {cell_index} bit_count");
            assert_eq!(
                entry.next_state_base, next_state_base,
                "cell {cell_index} next_state_base"
            );
        }
        assert_eq!(table.accuracy_log, 4);
        assert_eq!(table.table_size(), 16);
    }

    #[test]
    fn a_less_than_one_symbol_lands_at_the_last_cell_with_full_bit_count() {
        let counts: [i16; 5] = [7, 4, 2, 2, -1];
        let mut table = FseDecodeTable::new();
        build_fse_decode_table(&counts, 4, &mut table).unwrap();

        let last_cell = table.entries[table.table_size() - 1];
        assert_eq!(last_cell.symbol, 4);
        assert_eq!(last_cell.bit_count, table.accuracy_log);
        assert_eq!(last_cell.next_state_base, 0);
    }

    #[test]
    fn reads_a_hand_encoded_table_description() {
        let input = [0x10u8, 0xfd];
        let mut table = FseDecodeTable::new();

        let bytes_consumed =
            read_fse_table_description(&input, MAX_ACCURACY_LOG, 2, &mut table).unwrap();

        assert_eq!(bytes_consumed, 2);
        assert_eq!(table.accuracy_log, 5);
        assert_eq!(table.table_size(), 32);
    }

    #[test]
    fn rejects_an_accuracy_log_above_the_caller_supplied_maximum() {
        let input = [0x00u8, 0x00];
        let mut table = FseDecodeTable::new();

        let result = read_fse_table_description(&input, 4, 2, &mut table);

        assert_eq!(result, Err(DecodeError::BadFseTable));
    }

    #[test]
    fn rejects_more_symbols_than_the_caller_expects() {
        let input = [0x10u8, 0xfd];
        let mut table = FseDecodeTable::new();

        let result = read_fse_table_description(&input, MAX_ACCURACY_LOG, 0, &mut table);

        assert_eq!(result, Err(DecodeError::BadFseTable));
    }

    #[test]
    fn build_rle_table_produces_a_single_zero_bit_cell() {
        let mut table = FseDecodeTable::new();
        build_rle_table(7, &mut table);

        assert_eq!(table.accuracy_log, 0);
        assert_eq!(table.table_size(), 1);
        assert_eq!(table.entries[0].symbol, 7);
        assert_eq!(table.entries[0].bit_count, 0);
        assert_eq!(table.entries[0].next_state_base, 0);
    }
}
