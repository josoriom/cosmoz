use crate::{
    bits::{backward_bit_writer::BackwardBitWriter, forward_bit_writer::ForwardBitWriter},
    encode_error::EncodeError,
    entropy::fse_decode_table::{MAX_ACCURACY_LOG, MAX_SYMBOL_COUNT, MAX_TABLE_SIZE},
};

#[derive(Clone, Copy, Default)]
pub(crate) struct FseSymbolTransform {
    pub bits_delta: u32,
    pub find_state_delta: i32,
}

pub(crate) struct FseEncodeTable {
    pub next_state: [u16; MAX_TABLE_SIZE],
    pub transforms: [FseSymbolTransform; MAX_SYMBOL_COUNT],
    pub normalized_counts: [i16; MAX_SYMBOL_COUNT],
    pub symbol_count: usize,
    pub accuracy_log: u8,
}

impl FseEncodeTable {
    pub(crate) const fn new() -> Self {
        Self {
            next_state: [0u16; MAX_TABLE_SIZE],
            transforms: [FseSymbolTransform {
                bits_delta: 0,
                find_state_delta: 0,
            }; MAX_SYMBOL_COUNT],
            normalized_counts: [0i16; MAX_SYMBOL_COUNT],
            symbol_count: 0,
            accuracy_log: 0,
        }
    }
}

impl Default for FseEncodeTable {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) struct FseEncodeState {
    pub state: usize,
}

impl FseEncodeState {
    pub(crate) fn new(table: &FseEncodeTable, first_symbol: u8) -> Self {
        let transform = &table.transforms[first_symbol as usize];
        let bits_out = ((transform.bits_delta.wrapping_add(1u32 << 15)) >> 16) as usize;
        let shifted = ((bits_out as u32) << 16).wrapping_sub(transform.bits_delta);
        let index = ((shifted as usize) >> bits_out) as i64 + transform.find_state_delta as i64;
        let state = table.next_state[index as usize] as usize;
        Self { state }
    }

    pub(crate) fn encode_symbol(
        &mut self,
        writer: &mut BackwardBitWriter,
        table: &FseEncodeTable,
        symbol: u8,
    ) -> Result<(), EncodeError> {
        let transform = &table.transforms[symbol as usize];
        let bits_out = ((self.state as u32).wrapping_add(transform.bits_delta) >> 16) as usize;
        writer.add_bits(self.state as u64, bits_out)?;
        let index = (self.state >> bits_out) as i64 + transform.find_state_delta as i64;
        self.state = table.next_state[index as usize] as usize;
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn encode_symbol_without_flush(
        &mut self,
        writer: &mut BackwardBitWriter,
        table: &FseEncodeTable,
        symbol: u8,
    ) {
        let transform = &table.transforms[symbol as usize];
        let bits_out = ((self.state as u32).wrapping_add(transform.bits_delta) >> 16) as usize;
        writer.add_bits_without_flush(self.state as u64, bits_out);
        let index = (self.state >> bits_out) as i64 + transform.find_state_delta as i64;
        self.state = table.next_state[index as usize] as usize;
    }

    pub(crate) fn flush(
        &self,
        writer: &mut BackwardBitWriter,
        table: &FseEncodeTable,
    ) -> Result<(), EncodeError> {
        writer.add_bits(self.state as u64, table.accuracy_log as usize)
    }
}

pub(crate) fn pick_accuracy_log(
    total_count: usize,
    symbol_count: usize,
    max_accuracy_log: usize,
) -> usize {
    let max_accuracy_log = max_accuracy_log.min(MAX_ACCURACY_LOG);
    let max_bits_from_total = if total_count <= 1 {
        0
    } else {
        highest_bit(total_count - 1).saturating_sub(2)
    };

    let mut accuracy_log = max_accuracy_log;
    if max_bits_from_total < accuracy_log {
        accuracy_log = max_bits_from_total;
    }

    let min_bits_for_symbols = highest_bit(symbol_count.saturating_sub(1)) + 2;
    if min_bits_for_symbols > accuracy_log {
        accuracy_log = min_bits_for_symbols;
    }

    if accuracy_log < 5 {
        accuracy_log = 5;
    }
    if accuracy_log > max_accuracy_log {
        accuracy_log = max_accuracy_log;
    }
    accuracy_log
}

pub(crate) fn normalize_counts(
    counts: &[u32],
    total_count: usize,
    accuracy_log: usize,
    normalized_counts: &mut [i16],
) -> Result<(), EncodeError> {
    if accuracy_log == 0 || accuracy_log > MAX_ACCURACY_LOG {
        return Err(EncodeError::BadOptions);
    }
    if total_count == 0 || counts.is_empty() || counts.len() > normalized_counts.len() {
        return Err(EncodeError::BadOptions);
    }

    let table_size = 1usize << accuracy_log;
    for slot in normalized_counts[..counts.len()].iter_mut() {
        *slot = 0;
    }

    let present_symbol_count = counts.iter().filter(|&&count| count > 0).count();
    if present_symbol_count == 0 || present_symbol_count > table_size {
        return Err(EncodeError::BadOptions);
    }
    if present_symbol_count == 1 {
        let only_symbol = counts.iter().position(|&count| count > 0).unwrap();
        normalized_counts[only_symbol] = table_size as i16;
        return Ok(());
    }

    let scale = 62 - accuracy_log;
    let step = (1u128 << 62) / total_count as u128;

    distribute_by_scaled_share(counts, step, scale, table_size, normalized_counts);

    if largest_symbol_is_valid(counts, normalized_counts) {
        return Ok(());
    }

    distribute_proportionally(counts, total_count, table_size, normalized_counts);

    if largest_symbol_is_valid(counts, normalized_counts) {
        Ok(())
    } else {
        Err(EncodeError::BadOptions)
    }
}

fn distribute_by_scaled_share(
    counts: &[u32],
    step: u128,
    scale: usize,
    table_size: usize,
    normalized_counts: &mut [i16],
) {
    let mut still_to_distribute: i64 = table_size as i64;
    let largest_symbol = find_largest_count_symbol(counts);

    for (symbol, &count) in counts.iter().enumerate() {
        if count == 0 {
            continue;
        }
        let scaled = ((count as u128 * step) >> scale) as i64;
        let assigned = if scaled == 0 { -1 } else { scaled };
        normalized_counts[symbol] = assigned as i16;
        still_to_distribute -= if assigned == -1 { 1 } else { assigned };
    }

    normalized_counts[largest_symbol] += still_to_distribute as i16;
}

fn distribute_proportionally(
    counts: &[u32],
    total_count: usize,
    table_size: usize,
    normalized_counts: &mut [i16],
) {
    for slot in normalized_counts[..counts.len()].iter_mut() {
        *slot = 0;
    }

    let mut still_to_distribute: i64 = table_size as i64;
    let largest_symbol = find_largest_count_symbol(counts);

    for (symbol, &count) in counts.iter().enumerate() {
        if count == 0 {
            continue;
        }
        let share = ((count as u128 * table_size as u128) / total_count as u128) as i64;
        let assigned = if share == 0 { -1 } else { share };
        normalized_counts[symbol] = assigned as i16;
        still_to_distribute -= if assigned == -1 { 1 } else { assigned };
    }

    normalized_counts[largest_symbol] += still_to_distribute as i16;
}

fn find_largest_count_symbol(counts: &[u32]) -> usize {
    let mut largest_symbol = 0usize;
    let mut largest_count = 0u32;
    for (symbol, &count) in counts.iter().enumerate() {
        if count > largest_count {
            largest_count = count;
            largest_symbol = symbol;
        }
    }
    largest_symbol
}

fn largest_symbol_is_valid(counts: &[u32], normalized_counts: &[i16]) -> bool {
    normalized_counts[find_largest_count_symbol(counts)] > 0
}

pub(crate) fn build_fse_encode_table(
    normalized_counts: &[i16],
    accuracy_log: usize,
    table: &mut FseEncodeTable,
) -> Result<(), EncodeError> {
    if accuracy_log == 0 || accuracy_log > MAX_ACCURACY_LOG {
        return Err(EncodeError::BadOptions);
    }
    if normalized_counts.is_empty() || normalized_counts.len() > MAX_SYMBOL_COUNT {
        return Err(EncodeError::BadOptions);
    }

    let table_size = 1usize << accuracy_log;

    let mut effective_sum = 0usize;
    for &count in normalized_counts {
        if count < -1 {
            return Err(EncodeError::BadOptions);
        }
        effective_sum += if count == -1 { 1 } else { count as usize };
    }
    if effective_sum != table_size {
        return Err(EncodeError::BadOptions);
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
        return Err(EncodeError::BadOptions);
    }

    let mut cumulative = [0u32; MAX_SYMBOL_COUNT];
    let mut running_total = 0u32;
    for (symbol, &count) in normalized_counts.iter().enumerate() {
        cumulative[symbol] = running_total;
        running_total += if count == -1 { 1 } else { count.max(0) as u32 };
    }

    for (cell_index, &symbol) in symbol_of_position.iter().enumerate().take(table_size) {
        let slot = &mut cumulative[symbol as usize];
        table.next_state[*slot as usize] = (table_size + cell_index) as u16;
        *slot += 1;
    }

    let mut running_state_total: i64 = 0;
    for (symbol, &count) in normalized_counts.iter().enumerate() {
        table.transforms[symbol] = if count == 0 {
            FseSymbolTransform {
                bits_delta: ((accuracy_log as u32 + 1) << 16).wrapping_sub(1u32 << accuracy_log),
                find_state_delta: 0,
            }
        } else if count == -1 || count == 1 {
            let transform = FseSymbolTransform {
                bits_delta: ((accuracy_log as u32) << 16).wrapping_sub(1u32 << accuracy_log),
                find_state_delta: (running_state_total - 1) as i32,
            };
            running_state_total += 1;
            transform
        } else {
            let n = count as u32;
            let max_bits_out = accuracy_log as u32 - highest_bit((n - 1) as usize) as u32;
            let min_state_plus = (n as u64) << max_bits_out;
            let bits_delta = (max_bits_out << 16).wrapping_sub(min_state_plus as u32);
            let find_state_delta = running_state_total - n as i64;
            running_state_total += n as i64;
            FseSymbolTransform {
                bits_delta,
                find_state_delta: find_state_delta as i32,
            }
        };
    }

    table.normalized_counts[..normalized_counts.len()].copy_from_slice(normalized_counts);
    for slot in table.normalized_counts[normalized_counts.len()..].iter_mut() {
        *slot = 0;
    }
    table.symbol_count = normalized_counts.len();
    table.accuracy_log = accuracy_log as u8;
    Ok(())
}

fn get_spread_step(table_size: usize) -> usize {
    (table_size >> 1) + (table_size >> 3) + 3
}

fn highest_bit(value: usize) -> usize {
    if value == 0 {
        0
    } else {
        (usize::BITS - 1 - value.leading_zeros()) as usize
    }
}

fn count_bits_needed(value: usize) -> usize {
    (usize::BITS - value.leading_zeros()) as usize
}

pub(crate) fn write_fse_table_description(
    output: &mut [u8],
    table: &FseEncodeTable,
) -> Result<usize, EncodeError> {
    if table.accuracy_log < 5 || table.symbol_count == 0 {
        return Err(EncodeError::BadOptions);
    }

    let mut writer = ForwardBitWriter::new(output);
    writer.add_bits((table.accuracy_log - 5) as u64, 4)?;

    let table_size = 1usize << table.accuracy_log;
    let mut remaining: i64 = table_size as i64 + 1;
    let mut symbol = 0usize;

    while remaining > 1 {
        if symbol >= table.symbol_count {
            return Err(EncodeError::BadOptions);
        }
        let count = table.normalized_counts[symbol];
        let value = if count == -1 {
            0usize
        } else {
            (count + 1) as usize
        };

        let bits_needed = count_bits_needed(remaining as usize);
        let low_range = 1usize << (bits_needed - 1);
        let threshold = (1usize << bits_needed) - 1 - remaining as usize;

        if value < threshold {
            writer.add_bits(value as u64, bits_needed - 1)?;
        } else if value < low_range {
            writer.add_bits(value as u64, bits_needed - 1)?;
            writer.add_bits(0, 1)?;
        } else {
            let low = value + threshold - low_range;
            writer.add_bits(low as u64, bits_needed - 1)?;
            writer.add_bits(1, 1)?;
        }

        if value == 0 {
            remaining -= 1;
        } else {
            remaining -= value as i64 - 1;
        }
        symbol += 1;

        if value == 1 {
            let mut run = 0usize;
            while symbol + run < table.symbol_count && table.normalized_counts[symbol + run] == 0 {
                run += 1;
            }
            let mut remaining_run = run;
            loop {
                if remaining_run >= 3 {
                    writer.add_bits(3, 2)?;
                    remaining_run -= 3;
                } else {
                    writer.add_bits(remaining_run as u64, 2)?;
                    break;
                }
            }
            symbol += run;
        }
    }

    if remaining != 1 {
        return Err(EncodeError::BadOptions);
    }

    writer.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bits::backward_bit_reader::BackwardBitReader,
        entropy::{
            fse_decode_table::{
                FseDecodeState, FseDecodeTable, build_fse_decode_table, read_fse_table_description,
            },
            fse_predefined::{
                LITERAL_LENGTH_ACCURACY_LOG, LITERAL_LENGTH_DEFAULT_COUNTS,
                build_predefined_literal_length_table,
            },
        },
    };

    fn skewed_counts(symbol_count: usize) -> Vec<u32> {
        let mut counts = Vec::with_capacity(symbol_count);
        let mut weight = 1000u32;
        for _ in 0..symbol_count {
            counts.push(weight);
            weight = (weight / 2).max(1);
        }
        counts
    }

    fn build_encode_and_decode_tables(
        counts: &[u32],
        accuracy_log: usize,
    ) -> (FseEncodeTable, FseDecodeTable, Vec<i16>) {
        let total_count: usize = counts.iter().map(|&count| count as usize).sum();
        let mut normalized_counts = vec![0i16; counts.len()];
        normalize_counts(counts, total_count, accuracy_log, &mut normalized_counts).unwrap();

        let mut encode_table = FseEncodeTable::new();
        build_fse_encode_table(&normalized_counts, accuracy_log, &mut encode_table).unwrap();

        let mut decode_table = FseDecodeTable::new();
        build_fse_decode_table(&normalized_counts, accuracy_log, &mut decode_table).unwrap();

        (encode_table, decode_table, normalized_counts)
    }

    #[test]
    fn pick_accuracy_log_uses_the_symbol_bound_as_a_lower_limit() {
        assert_eq!(pick_accuracy_log(10_000, 36, 9), 9);
        assert_eq!(pick_accuracy_log(100, 3, 9), 5);
    }

    #[test]
    fn description_round_trips_for_several_table_sizes_and_count_shapes() {
        let cases: [(usize, Vec<i16>); 6] = [
            (5, vec![16, 8, 4, 2, 1, 1]),
            (6, vec![32, 16, 8, 4, 2, 1, 1]),
            (8, vec![200, 30, 20, 4, -1, -1]),
            (9, vec![400, 98, 6, 4, -1, -1, -1, -1]),
            (5, vec![29, -1, -1, -1]),
            (6, vec![40, 0, 0, 0, 0, 0, 10, -1, -1, -1, -1, -1, 9]),
        ];

        for (accuracy_log, normalized_counts) in cases {
            let table_size = 1i64 << accuracy_log;
            let sum: i64 = normalized_counts
                .iter()
                .map(|&count| if count == -1 { 1 } else { count as i64 })
                .sum();
            assert_eq!(sum, table_size);

            let mut table = FseEncodeTable::new();
            build_fse_encode_table(&normalized_counts, accuracy_log, &mut table).unwrap();

            let mut output = [0u8; 64];
            let bytes_written = write_fse_table_description(&mut output, &table).unwrap();

            let mut decode_table = FseDecodeTable::new();
            let bytes_read = read_fse_table_description(
                &output[..bytes_written],
                MAX_ACCURACY_LOG,
                normalized_counts.len() - 1,
                &mut decode_table,
            )
            .unwrap();

            assert_eq!(bytes_read, bytes_written);
            assert_eq!(decode_table.accuracy_log as usize, accuracy_log);
        }
    }

    #[test]
    fn encode_round_trip_over_ten_thousand_symbols_from_a_skewed_distribution() {
        let counts = skewed_counts(20);
        let (encode_table, decode_table, _normalized_counts) =
            build_encode_and_decode_tables(&counts, 9);

        let total_count: usize = counts.iter().map(|&count| count as usize).sum();
        let mut random_state = 0x2545F4914F6CDD1Du64;
        let mut next_random = || {
            random_state ^= random_state << 13;
            random_state ^= random_state >> 7;
            random_state ^= random_state << 17;
            random_state
        };

        let mut symbols = Vec::with_capacity(10_000);
        for _ in 0..10_000 {
            let mut pick = (next_random() as usize) % total_count;
            let mut symbol = 0u8;
            for (index, &count) in counts.iter().enumerate() {
                if pick < count as usize {
                    symbol = index as u8;
                    break;
                }
                pick -= count as usize;
            }
            symbols.push(symbol);
        }

        let mut output = [0u8; 20_000];
        let mut state = FseEncodeState::new(&encode_table, *symbols.last().unwrap());
        {
            let mut writer = BackwardBitWriter::new(&mut output);
            for &symbol in symbols[..symbols.len() - 1].iter().rev() {
                state
                    .encode_symbol(&mut writer, &encode_table, symbol)
                    .unwrap();
            }
            state.flush(&mut writer, &encode_table).unwrap();
            let bytes_written = writer.finish().unwrap();

            let mut reader = BackwardBitReader::new(&output[..bytes_written]).unwrap();
            let mut decode_state = FseDecodeState::new(&mut reader, &decode_table);
            let mut decoded = Vec::with_capacity(symbols.len());
            decoded.push(decode_state.get_symbol(&decode_table));
            for _ in 1..symbols.len() {
                decode_state.update(&mut reader, &decode_table);
                decoded.push(decode_state.get_symbol(&decode_table));
            }
            assert_eq!(decoded, symbols);
        }
    }

    #[test]
    fn predefined_literal_length_table_encodes_and_decodes_a_symbol_run() {
        let mut encode_table = FseEncodeTable::new();
        build_fse_encode_table(
            &LITERAL_LENGTH_DEFAULT_COUNTS,
            LITERAL_LENGTH_ACCURACY_LOG,
            &mut encode_table,
        )
        .unwrap();

        let mut decode_table = FseDecodeTable::new();
        build_predefined_literal_length_table(&mut decode_table);

        let symbols: [u8; 6] = [0, 3, 7, 12, 5, 1];

        let mut output = [0u8; 64];
        let bytes_written;
        {
            let mut writer = BackwardBitWriter::new(&mut output);
            let mut state = FseEncodeState::new(&encode_table, *symbols.last().unwrap());
            for &symbol in symbols[..symbols.len() - 1].iter().rev() {
                state
                    .encode_symbol(&mut writer, &encode_table, symbol)
                    .unwrap();
            }
            state.flush(&mut writer, &encode_table).unwrap();
            bytes_written = writer.finish().unwrap();
        }

        let mut reader = BackwardBitReader::new(&output[..bytes_written]).unwrap();
        let mut decode_state = FseDecodeState::new(&mut reader, &decode_table);
        let mut decoded = Vec::with_capacity(symbols.len());
        decoded.push(decode_state.get_symbol(&decode_table));
        for _ in 1..symbols.len() {
            decode_state.update(&mut reader, &decode_table);
            decoded.push(decode_state.get_symbol(&decode_table));
        }
        assert_eq!(decoded, symbols);
    }
}
