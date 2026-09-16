use crate::{
    bits::backward_bit_writer::BackwardBitWriter,
    block::{
        sequence_codes::{
            LITERAL_LENGTH_CODE_COUNT, MATCH_LENGTH_CODE_COUNT, OFFSET_CODE_COUNT,
            get_literal_length_code, get_literal_length_extra_bits, get_match_length_code,
            get_match_length_extra_bits, get_offset_code,
        },
        sequence_record::SequenceRecord,
        sequences::TableMode,
    },
    encode_error::EncodeError,
    entropy::{
        fse_decode_table::MAX_SYMBOL_COUNT,
        fse_encode_table::{
            FseEncodeState, FseEncodeTable, FseSymbolTransform, build_fse_encode_table,
            normalize_counts, pick_accuracy_log, write_fse_table_description,
        },
        fse_predefined::{
            LITERAL_LENGTH_ACCURACY_LOG, LITERAL_LENGTH_DEFAULT_COUNTS, MATCH_LENGTH_ACCURACY_LOG,
            MATCH_LENGTH_DEFAULT_COUNTS, OFFSET_ACCURACY_LOG, OFFSET_DEFAULT_COUNTS,
        },
    },
    frame::frame_header::FrameFormat,
};

pub struct SequenceEncodeTables {
    pub literal_length: FseEncodeTable,
    pub offset: FseEncodeTable,
    pub match_length: FseEncodeTable,
    pub literal_length_mode: TableMode,
    pub offset_mode: TableMode,
    pub match_length_mode: TableMode,
}

impl SequenceEncodeTables {
    pub const fn new() -> Self {
        Self {
            literal_length: FseEncodeTable::new(),
            offset: FseEncodeTable::new(),
            match_length: FseEncodeTable::new(),
            literal_length_mode: TableMode::Predefined,
            offset_mode: TableMode::Predefined,
            match_length_mode: TableMode::Predefined,
        }
    }
}

impl Default for SequenceEncodeTables {
    fn default() -> Self {
        Self::new()
    }
}

impl SequenceEncodeTables {
    pub fn snapshot(&self) -> Self {
        Self {
            literal_length: clone_fse_encode_table(&self.literal_length),
            offset: clone_fse_encode_table(&self.offset),
            match_length: clone_fse_encode_table(&self.match_length),
            literal_length_mode: self.literal_length_mode,
            offset_mode: self.offset_mode,
            match_length_mode: self.match_length_mode,
        }
    }
}

fn clone_fse_encode_table(table: &FseEncodeTable) -> FseEncodeTable {
    FseEncodeTable {
        next_state: table.next_state,
        transforms: table.transforms,
        normalized_counts: table.normalized_counts,
        symbol_count: table.symbol_count,
        accuracy_log: table.accuracy_log,
    }
}

pub fn write_sequences(
    sequences: &[SequenceRecord],
    format: FrameFormat,
    output: &mut [u8],
    tables: &mut SequenceEncodeTables,
    scratch: &mut [u8],
) -> Result<usize, EncodeError> {
    let sequence_count = sequences.len();
    if sequence_count == 0 {
        *output.first_mut().ok_or(EncodeError::OutputTooSmall)? = 0;
        return Ok(1);
    }

    let mut literal_length_counts = [0u32; LITERAL_LENGTH_CODE_COUNT];
    let mut offset_counts = [0u32; OFFSET_CODE_COUNT];
    let mut match_length_counts = [0u32; MATCH_LENGTH_CODE_COUNT];

    for record in sequences {
        let (literal_length_code, _) = get_literal_length_code(record.literal_length);
        let (match_length_code, _) = get_match_length_code(record.match_length);
        let (offset_code, _) = get_offset_code(record.offset_value);
        literal_length_counts[literal_length_code as usize] += 1;
        offset_counts[offset_code as usize] += 1;
        match_length_counts[match_length_code as usize] += 1;
    }

    let (literal_length_mode, _) = pick_table_mode(
        &literal_length_counts,
        sequence_count,
        LITERAL_LENGTH_CODE_COUNT - 1,
        &LITERAL_LENGTH_DEFAULT_COUNTS,
        LITERAL_LENGTH_ACCURACY_LOG,
        9,
        &mut tables.literal_length,
    )?;
    tables.literal_length_mode = literal_length_mode;

    let (offset_mode, _) = pick_table_mode(
        &offset_counts,
        sequence_count,
        OFFSET_CODE_COUNT - 1,
        &OFFSET_DEFAULT_COUNTS,
        OFFSET_ACCURACY_LOG,
        8,
        &mut tables.offset,
    )?;
    tables.offset_mode = offset_mode;

    let (match_length_mode, _) = pick_table_mode(
        &match_length_counts,
        sequence_count,
        MATCH_LENGTH_CODE_COUNT - 1,
        &MATCH_LENGTH_DEFAULT_COUNTS,
        MATCH_LENGTH_ACCURACY_LOG,
        9,
        &mut tables.match_length,
    )?;
    tables.match_length_mode = match_length_mode;

    let header_length = write_sequences_header(output, sequence_count, tables)?;
    let body_output = output
        .get_mut(header_length..)
        .ok_or(EncodeError::OutputTooSmall)?;

    let body_length = if format == FrameFormat::Osmo && sequence_count >= 2 {
        write_two_streams(sequences, tables, body_output, scratch)?
    } else {
        write_one_stream(sequences, tables, body_output)?
    };

    Ok(header_length + body_length)
}

fn write_sequences_header(
    output: &mut [u8],
    sequence_count: usize,
    tables: &SequenceEncodeTables,
) -> Result<usize, EncodeError> {
    let mut position = write_sequence_count(output, sequence_count)?;

    let modes_byte = (mode_bits(tables.literal_length_mode) << 6)
        | (mode_bits(tables.offset_mode) << 4)
        | (mode_bits(tables.match_length_mode) << 2);
    *output
        .get_mut(position)
        .ok_or(EncodeError::OutputTooSmall)? = modes_byte;
    position += 1;

    position += write_table_description(
        output
            .get_mut(position..)
            .ok_or(EncodeError::OutputTooSmall)?,
        tables.literal_length_mode,
        &tables.literal_length,
    )?;
    position += write_table_description(
        output
            .get_mut(position..)
            .ok_or(EncodeError::OutputTooSmall)?,
        tables.offset_mode,
        &tables.offset,
    )?;
    position += write_table_description(
        output
            .get_mut(position..)
            .ok_or(EncodeError::OutputTooSmall)?,
        tables.match_length_mode,
        &tables.match_length,
    )?;

    Ok(position)
}

fn write_sequence_count(output: &mut [u8], sequence_count: usize) -> Result<usize, EncodeError> {
    if sequence_count < 128 {
        *output.first_mut().ok_or(EncodeError::OutputTooSmall)? = sequence_count as u8;
        return Ok(1);
    }

    if sequence_count <= 126 * 256 + 255 {
        let high = sequence_count >> 8;
        let low = sequence_count & 0xFF;
        let bytes = output.get_mut(0..2).ok_or(EncodeError::OutputTooSmall)?;
        bytes[0] = 128 + high as u8;
        bytes[1] = low as u8;
        return Ok(2);
    }

    let remainder = sequence_count - 0x7F00;
    if remainder > 0xFFFF {
        return Err(EncodeError::BadOptions);
    }
    let bytes = output.get_mut(0..3).ok_or(EncodeError::OutputTooSmall)?;
    bytes[0] = 255;
    bytes[1] = (remainder & 0xFF) as u8;
    bytes[2] = ((remainder >> 8) & 0xFF) as u8;
    Ok(3)
}

fn mode_bits(mode: TableMode) -> u8 {
    match mode {
        TableMode::Predefined => 0,
        TableMode::Rle => 1,
        TableMode::Compressed => 2,
        TableMode::Repeat => 3,
    }
}

fn write_table_description(
    output: &mut [u8],
    mode: TableMode,
    table: &FseEncodeTable,
) -> Result<usize, EncodeError> {
    match mode {
        TableMode::Predefined => Ok(0),
        TableMode::Rle => {
            let symbol = table
                .normalized_counts
                .iter()
                .position(|&count| count > 0)
                .ok_or(EncodeError::TableNotUsable)? as u8;
            *output.first_mut().ok_or(EncodeError::OutputTooSmall)? = symbol;
            Ok(1)
        }
        TableMode::Compressed => write_fse_table_description(output, table),
        TableMode::Repeat => Err(EncodeError::TableNotUsable),
    }
}

fn pick_table_mode(
    counts: &[u32],
    total: usize,
    max_symbol: usize,
    predefined_counts: &[i16],
    predefined_log: usize,
    max_log: usize,
    table: &mut FseEncodeTable,
) -> Result<(TableMode, usize), EncodeError> {
    debug_assert_eq!(counts.len(), max_symbol + 1);

    if let Some(symbol) = single_distinct_symbol(counts) {
        build_rle_encode_table(symbol, table);
        return Ok((TableMode::Rle, 1));
    }

    let max_used_symbol = counts.iter().rposition(|&count| count > 0).unwrap_or(0);
    let predefined_supports_this = max_used_symbol < predefined_counts.len();

    if total < 64 && predefined_supports_this {
        build_fse_encode_table(predefined_counts, predefined_log, table)?;
        return Ok((TableMode::Predefined, 0));
    }

    let accuracy_log = pick_accuracy_log(total, max_used_symbol + 1, max_log);
    let mut normalized_counts = [0i16; MAX_SYMBOL_COUNT];
    normalize_counts(
        counts,
        total,
        accuracy_log,
        &mut normalized_counts[..counts.len()],
    )?;

    let mut custom_table = FseEncodeTable::new();
    build_fse_encode_table(
        &normalized_counts[..counts.len()],
        accuracy_log,
        &mut custom_table,
    )?;

    let mut description_scratch = [0u8; 320];
    let description_bytes = write_fse_table_description(&mut description_scratch, &custom_table)?;
    let custom_cost = estimate_bit_cost(counts, &normalized_counts[..counts.len()], accuracy_log)
        + description_bytes * 8;

    if predefined_supports_this {
        let predefined_cost = estimate_bit_cost(counts, predefined_counts, predefined_log);
        if custom_cost >= predefined_cost {
            build_fse_encode_table(predefined_counts, predefined_log, table)?;
            return Ok((TableMode::Predefined, 0));
        }
    }

    *table = custom_table;
    Ok((TableMode::Compressed, description_bytes))
}

fn single_distinct_symbol(counts: &[u32]) -> Option<u8> {
    let mut found_symbol: Option<usize> = None;
    for (symbol, &count) in counts.iter().enumerate() {
        if count == 0 {
            continue;
        }
        if found_symbol.is_some() {
            return None;
        }
        found_symbol = Some(symbol);
    }
    found_symbol.map(|symbol| symbol as u8)
}

fn build_rle_encode_table(symbol: u8, table: &mut FseEncodeTable) {
    *table = FseEncodeTable::new();
    table.accuracy_log = 0;
    table.symbol_count = symbol as usize + 1;
    table.normalized_counts[symbol as usize] = 1;
    table.transforms[symbol as usize] = FseSymbolTransform {
        bits_delta: 0,
        find_state_delta: 0,
    };
    table.next_state[0] = 0;
}

fn effective_probability_count(normalized_count: i16) -> u32 {
    if normalized_count <= 0 {
        1
    } else {
        normalized_count as u32
    }
}

fn floor_log2(value: u32) -> u32 {
    31 - value.leading_zeros()
}

fn estimate_bit_cost(counts: &[u32], normalized_counts: &[i16], accuracy_log: usize) -> usize {
    let mut total_bits = 0usize;
    for (symbol, &count) in counts.iter().enumerate() {
        if count == 0 {
            continue;
        }
        let effective =
            effective_probability_count(normalized_counts.get(symbol).copied().unwrap_or(0));
        let bits = accuracy_log as u32 - floor_log2(effective);
        total_bits += count as usize * bits as usize;
    }
    total_bits
}

fn write_one_stream(
    sequences: &[SequenceRecord],
    tables: &SequenceEncodeTables,
    output: &mut [u8],
) -> Result<usize, EncodeError> {
    write_stream_over_indices(sequences, 0, 1, sequences.len(), tables, output)
}

fn write_two_streams(
    sequences: &[SequenceRecord],
    tables: &SequenceEncodeTables,
    output: &mut [u8],
    scratch: &mut [u8],
) -> Result<usize, EncodeError> {
    let sequence_count = sequences.len();
    let first_stream_count = sequence_count.div_ceil(2);
    let second_stream_count = sequence_count / 2;

    let first_stream_length =
        write_stream_over_indices(sequences, 0, 2, first_stream_count, tables, scratch)?;

    let length_bytes = (first_stream_length as u32).to_le_bytes();
    output
        .get_mut(0..4)
        .ok_or(EncodeError::OutputTooSmall)?
        .copy_from_slice(&length_bytes);

    let after_length = output.get_mut(4..).ok_or(EncodeError::OutputTooSmall)?;
    let first_stream_destination = after_length
        .get_mut(..first_stream_length)
        .ok_or(EncodeError::OutputTooSmall)?;
    first_stream_destination.copy_from_slice(
        scratch
            .get(..first_stream_length)
            .ok_or(EncodeError::OutputTooSmall)?,
    );

    let second_stream_output = after_length
        .get_mut(first_stream_length..)
        .ok_or(EncodeError::OutputTooSmall)?;
    let second_stream_length = write_stream_over_indices(
        sequences,
        1,
        2,
        second_stream_count,
        tables,
        second_stream_output,
    )?;

    Ok(4 + first_stream_length + second_stream_length)
}

fn write_stream_over_indices(
    sequences: &[SequenceRecord],
    start_index: usize,
    stride: usize,
    count: usize,
    tables: &SequenceEncodeTables,
    output: &mut [u8],
) -> Result<usize, EncodeError> {
    let record_at = |position: usize| sequences[start_index + position * stride];

    let mut writer = BackwardBitWriter::new(output);

    let last = record_at(count - 1);
    let (last_literal_length_code, last_literal_length_extra) =
        get_literal_length_code(last.literal_length);
    let (last_match_length_code, last_match_length_extra) =
        get_match_length_code(last.match_length);
    let (last_offset_code, last_offset_extra) = get_offset_code(last.offset_value);

    let mut match_length_state = FseEncodeState::new(&tables.match_length, last_match_length_code);
    let mut offset_state = FseEncodeState::new(&tables.offset, last_offset_code);
    let mut literal_length_state =
        FseEncodeState::new(&tables.literal_length, last_literal_length_code);

    writer.add_bits(
        last_literal_length_extra as u64,
        get_literal_length_extra_bits(last_literal_length_code) as usize,
    )?;
    writer.add_bits(
        last_match_length_extra as u64,
        get_match_length_extra_bits(last_match_length_code) as usize,
    )?;
    writer.add_bits(last_offset_extra as u64, last_offset_code as usize)?;

    for position in (0..count - 1).rev() {
        let record = record_at(position);
        let (literal_length_code, literal_length_extra) =
            get_literal_length_code(record.literal_length);
        let (match_length_code, match_length_extra) = get_match_length_code(record.match_length);
        let (offset_code, offset_extra) = get_offset_code(record.offset_value);

        offset_state.encode_symbol(&mut writer, &tables.offset, offset_code)?;
        match_length_state.encode_symbol(&mut writer, &tables.match_length, match_length_code)?;
        literal_length_state.encode_symbol(
            &mut writer,
            &tables.literal_length,
            literal_length_code,
        )?;

        writer.add_bits(
            literal_length_extra as u64,
            get_literal_length_extra_bits(literal_length_code) as usize,
        )?;
        writer.add_bits(
            match_length_extra as u64,
            get_match_length_extra_bits(match_length_code) as usize,
        )?;
        writer.add_bits(offset_extra as u64, offset_code as usize)?;
    }

    match_length_state.flush(&mut writer, &tables.match_length)?;
    offset_state.flush(&mut writer, &tables.offset)?;
    literal_length_state.flush(&mut writer, &tables.literal_length)?;

    writer.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{
        repeat_offsets::RepeatOffsets,
        sequences::{SequenceDecoder, SequenceTables, read_sequence_tables, read_sequences_header},
    };

    fn resolve_offset_values(records: &[SequenceRecord]) -> Vec<(u32, u32, u32)> {
        let mut repeat_offsets = RepeatOffsets::new();
        records
            .iter()
            .map(|record| {
                let offset = repeat_offsets.get_offset(record.offset_value, record.literal_length);
                (record.literal_length, record.match_length, offset)
            })
            .collect()
    }

    fn build_test_records() -> Vec<SequenceRecord> {
        let mut records = Vec::new();
        let mut repeat_offsets = RepeatOffsets::new();

        let literal_lengths: [u32; 10] = [0, 3, 15, 16, 20, 63, 64, 300, 40000, 70000];
        let match_lengths: [u32; 10] = [3, 4, 34, 35, 60, 130, 200, 1000, 40000, 70000];
        let fresh_offsets: [u32; 4] = [10, 500, 70000, 5_000_000];

        for index in 0..200usize {
            let literal_length = literal_lengths[index % literal_lengths.len()];
            let match_length = match_lengths[index % match_lengths.len()];

            let repeat_choice = index % 7;
            let real_offset = match repeat_choice {
                0 => repeat_offsets.first,
                1 => repeat_offsets.second,
                2 => repeat_offsets.third,
                3 => {
                    if literal_length == 0 && repeat_offsets.first > 1 {
                        repeat_offsets.first - 1
                    } else {
                        repeat_offsets.first
                    }
                }
                _ => fresh_offsets[index % fresh_offsets.len()] + index as u32,
            };

            let offset_value = repeat_offsets.get_offset_value(real_offset, literal_length);
            records.push(SequenceRecord {
                literal_length,
                match_length,
                offset_value,
            });
        }

        records
    }

    fn decode_all(
        bitstream: &[u8],
        sequence_count: usize,
        tables: &SequenceTables,
        format: FrameFormat,
    ) -> Vec<(u32, u32, u32)> {
        let mut decoder = SequenceDecoder::new(bitstream, tables, sequence_count, format).unwrap();
        let mut repeat_offsets = RepeatOffsets::new();
        let mut decoded = Vec::new();
        while let Some(sequence) = decoder.next_sequence(&mut repeat_offsets) {
            let sequence = sequence.unwrap();
            decoded.push((
                sequence.literal_length,
                sequence.match_length,
                sequence.offset,
            ));
        }
        assert!(decoder.is_finished());
        decoded
    }

    #[test]
    fn round_trips_as_a_single_zstd_stream() {
        let records = build_test_records();
        let expected = resolve_offset_values(&records);

        let mut output = [0u8; 8192];
        let mut scratch = [0u8; 8192];
        let mut tables = SequenceEncodeTables::new();

        let written = write_sequences(
            &records,
            FrameFormat::Zstd,
            &mut output,
            &mut tables,
            &mut scratch,
        )
        .unwrap();

        let header = read_sequences_header(&output[..written]).unwrap();
        assert_eq!(header.sequence_count, records.len());

        let mut decode_tables = SequenceTables::new();
        let table_bytes = read_sequence_tables(
            &output[header.header_length..written],
            &header,
            &mut decode_tables,
        )
        .unwrap();

        let bitstream_start = header.header_length + table_bytes;
        let decoded = decode_all(
            &output[bitstream_start..written],
            header.sequence_count,
            &decode_tables,
            FrameFormat::Zstd,
        );

        assert_eq!(decoded, expected);
    }

    #[test]
    fn round_trips_as_two_osmo_streams() {
        let records = build_test_records();
        let expected = resolve_offset_values(&records);

        let mut output = [0u8; 8192];
        let mut scratch = [0u8; 8192];
        let mut tables = SequenceEncodeTables::new();

        let written = write_sequences(
            &records,
            FrameFormat::Osmo,
            &mut output,
            &mut tables,
            &mut scratch,
        )
        .unwrap();

        let header = read_sequences_header(&output[..written]).unwrap();
        assert_eq!(header.sequence_count, records.len());

        let mut decode_tables = SequenceTables::new();
        let table_bytes = read_sequence_tables(
            &output[header.header_length..written],
            &header,
            &mut decode_tables,
        )
        .unwrap();

        let bitstream_start = header.header_length + table_bytes;
        assert!(written - bitstream_start >= 4);

        let first_stream_length = u32::from_le_bytes([
            output[bitstream_start],
            output[bitstream_start + 1],
            output[bitstream_start + 2],
            output[bitstream_start + 3],
        ]) as usize;
        assert!(first_stream_length > 0);
        assert!(first_stream_length < written - bitstream_start - 4);

        let decoded = decode_all(
            &output[bitstream_start..written],
            header.sequence_count,
            &decode_tables,
            FrameFormat::Osmo,
        );

        assert_eq!(decoded, expected);
    }

    #[test]
    fn osmo_with_one_sequence_has_no_length_field() {
        let records = vec![SequenceRecord {
            literal_length: 5,
            match_length: 10,
            offset_value: 4,
        }];
        let expected = resolve_offset_values(&records);

        let mut output = [0u8; 256];
        let mut scratch = [0u8; 256];
        let mut tables = SequenceEncodeTables::new();

        let written = write_sequences(
            &records,
            FrameFormat::Osmo,
            &mut output,
            &mut tables,
            &mut scratch,
        )
        .unwrap();

        let header = read_sequences_header(&output[..written]).unwrap();
        assert_eq!(header.sequence_count, 1);

        let mut decode_tables = SequenceTables::new();
        let table_bytes = read_sequence_tables(
            &output[header.header_length..written],
            &header,
            &mut decode_tables,
        )
        .unwrap();

        let bitstream_start = header.header_length + table_bytes;
        let decoded = decode_all(
            &output[bitstream_start..written],
            header.sequence_count,
            &decode_tables,
            FrameFormat::Osmo,
        );

        assert_eq!(decoded, expected);
    }

    #[test]
    fn skewed_distribution_forces_compressed_literal_length_mode() {
        let mut records = Vec::new();
        for index in 0..2000usize {
            let literal_length = if index % 20 == 0 {
                500 + index as u32
            } else {
                0
            };
            records.push(SequenceRecord {
                literal_length,
                match_length: 4,
                offset_value: 1 + (index as u32 % 3),
            });
        }
        let mut repeat_offsets = RepeatOffsets::new();
        for record in records.iter_mut() {
            record.offset_value =
                repeat_offsets.get_offset_value(record.offset_value + 3, record.literal_length);
        }
        let expected = resolve_offset_values(&records);

        let mut output = [0u8; 16384];
        let mut scratch = [0u8; 16384];
        let mut tables = SequenceEncodeTables::new();

        let written = write_sequences(
            &records,
            FrameFormat::Zstd,
            &mut output,
            &mut tables,
            &mut scratch,
        )
        .unwrap();

        assert_eq!(tables.literal_length_mode, TableMode::Compressed);

        let header = read_sequences_header(&output[..written]).unwrap();
        assert_eq!(header.literal_length_mode, TableMode::Compressed);

        let mut decode_tables = SequenceTables::new();
        let table_bytes = read_sequence_tables(
            &output[header.header_length..written],
            &header,
            &mut decode_tables,
        )
        .unwrap();

        let bitstream_start = header.header_length + table_bytes;
        let decoded = decode_all(
            &output[bitstream_start..written],
            header.sequence_count,
            &decode_tables,
            FrameFormat::Zstd,
        );

        assert_eq!(decoded, expected);
    }
}
