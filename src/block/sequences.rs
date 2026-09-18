use crate::{
    bits::backward_bit_reader::BackwardBitReader,
    block::{
        repeat_offsets::RepeatOffsets,
        sequence_codes::{
            get_literal_length_base, get_literal_length_extra_bits, get_match_length_base,
            get_match_length_extra_bits,
        },
    },
    entropy::{
        fse_decode_table::{
            FseDecodeState, FseDecodeTable, build_rle_table, read_fse_table_description,
        },
        fse_predefined::{
            build_predefined_literal_length_table, build_predefined_match_length_table,
            build_predefined_offset_table,
        },
    },
    error::DecodeError,
    frame::frame_header::FrameFormat,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TableMode {
    Predefined,
    Rle,
    Compressed,
    Repeat,
}

pub(crate) struct SequencesHeader {
    pub sequence_count: usize,
    pub literal_length_mode: TableMode,
    pub offset_mode: TableMode,
    pub match_length_mode: TableMode,
    pub header_length: usize,
}

pub(crate) struct Sequence {
    pub literal_length: u32,
    pub match_length: u32,
    pub offset: u32,
}

pub(crate) struct SequenceTables {
    pub literal_length: FseDecodeTable,
    pub offset: FseDecodeTable,
    pub match_length: FseDecodeTable,
    pub literal_length_ready: bool,
    pub offset_ready: bool,
    pub match_length_ready: bool,
}

impl SequenceTables {
    pub(crate) const fn new() -> Self {
        Self {
            literal_length: FseDecodeTable::new(),
            offset: FseDecodeTable::new(),
            match_length: FseDecodeTable::new(),
            literal_length_ready: false,
            offset_ready: false,
            match_length_ready: false,
        }
    }
}

impl Default for SequenceTables {
    fn default() -> Self {
        Self::new()
    }
}

fn table_mode_from_bits(bits: u8) -> TableMode {
    match bits {
        0 => TableMode::Predefined,
        1 => TableMode::Rle,
        2 => TableMode::Compressed,
        _ => TableMode::Repeat,
    }
}

pub(crate) fn read_sequences_header(input: &[u8]) -> Result<SequencesHeader, DecodeError> {
    let byte0 = *input.first().ok_or(DecodeError::InputTooShort)?;

    let (sequence_count, count_length) = if byte0 < 128 {
        (byte0 as usize, 1usize)
    } else if byte0 < 255 {
        let byte1 = *input.get(1).ok_or(DecodeError::InputTooShort)?;
        ((((byte0 - 128) as usize) << 8) + byte1 as usize, 2usize)
    } else {
        let byte1 = *input.get(1).ok_or(DecodeError::InputTooShort)?;
        let byte2 = *input.get(2).ok_or(DecodeError::InputTooShort)?;
        (byte1 as usize + ((byte2 as usize) << 8) + 0x7F00, 3usize)
    };

    if sequence_count == 0 {
        return Ok(SequencesHeader {
            sequence_count: 0,
            literal_length_mode: TableMode::Predefined,
            offset_mode: TableMode::Predefined,
            match_length_mode: TableMode::Predefined,
            header_length: count_length,
        });
    }

    let modes_byte = *input.get(count_length).ok_or(DecodeError::InputTooShort)?;
    if modes_byte & 0b11 != 0 {
        return Err(DecodeError::BadSequencesHeader);
    }

    let literal_length_mode = table_mode_from_bits((modes_byte >> 6) & 0b11);
    let offset_mode = table_mode_from_bits((modes_byte >> 4) & 0b11);
    let match_length_mode = table_mode_from_bits((modes_byte >> 2) & 0b11);

    Ok(SequencesHeader {
        sequence_count,
        literal_length_mode,
        offset_mode,
        match_length_mode,
        header_length: count_length + 1,
    })
}

fn read_table_for_mode(
    input: &[u8],
    mode: TableMode,
    max_accuracy_log: usize,
    max_symbol: usize,
    predefined: fn(&mut FseDecodeTable),
    table: &mut FseDecodeTable,
    ready: &mut bool,
) -> Result<usize, DecodeError> {
    match mode {
        TableMode::Predefined => {
            predefined(table);
            *ready = true;
            Ok(0)
        }
        TableMode::Rle => {
            let symbol = *input.first().ok_or(DecodeError::InputTooShort)?;
            if symbol as usize > max_symbol {
                return Err(DecodeError::BadSequencesHeader);
            }
            build_rle_table(symbol, table);
            *ready = true;
            Ok(1)
        }
        TableMode::Compressed => {
            let bytes_used =
                read_fse_table_description(input, max_accuracy_log, max_symbol, table)?;
            *ready = true;
            Ok(bytes_used)
        }
        TableMode::Repeat => {
            if !*ready {
                return Err(DecodeError::BadSequencesHeader);
            }
            Ok(0)
        }
    }
}

pub(crate) fn read_sequence_tables(
    input: &[u8],
    header: &SequencesHeader,
    tables: &mut SequenceTables,
) -> Result<usize, DecodeError> {
    let mut position = 0usize;

    position += read_table_for_mode(
        input.get(position..).ok_or(DecodeError::InputTooShort)?,
        header.literal_length_mode,
        9,
        35,
        build_predefined_literal_length_table,
        &mut tables.literal_length,
        &mut tables.literal_length_ready,
    )?;

    position += read_table_for_mode(
        input.get(position..).ok_or(DecodeError::InputTooShort)?,
        header.offset_mode,
        8,
        31,
        build_predefined_offset_table,
        &mut tables.offset,
        &mut tables.offset_ready,
    )?;

    position += read_table_for_mode(
        input.get(position..).ok_or(DecodeError::InputTooShort)?,
        header.match_length_mode,
        9,
        52,
        build_predefined_match_length_table,
        &mut tables.match_length,
        &mut tables.match_length_ready,
    )?;

    Ok(position)
}

struct SequenceStream<'input> {
    reader: BackwardBitReader<'input>,
    literal_length_state: FseDecodeState,
    offset_state: FseDecodeState,
    match_length_state: FseDecodeState,
    sequences_left: usize,
}

impl<'input> SequenceStream<'input> {
    fn new(
        input: &'input [u8],
        tables: &SequenceTables,
        sequence_count: usize,
    ) -> Result<Self, DecodeError> {
        let mut reader = BackwardBitReader::new(input)?;
        let literal_length_state = FseDecodeState::new(&mut reader, &tables.literal_length);
        let offset_state = FseDecodeState::new(&mut reader, &tables.offset);
        let match_length_state = FseDecodeState::new(&mut reader, &tables.match_length);
        Ok(Self {
            reader,
            literal_length_state,
            offset_state,
            match_length_state,
            sequences_left: sequence_count,
        })
    }

    fn is_finished(&self) -> bool {
        self.sequences_left == 0 && self.reader.is_finished()
    }

    fn next_sequence(
        &mut self,
        tables: &SequenceTables,
        repeat_offsets: &mut RepeatOffsets,
    ) -> Option<Result<Sequence, DecodeError>> {
        if self.sequences_left == 0 {
            return None;
        }

        let offset_code = self.offset_state.get_symbol(&tables.offset);
        if offset_code > 31 {
            return Some(Err(DecodeError::BadOffset));
        }
        let offset_extra_bits = self.reader.read_bits(offset_code as usize) as u32;
        let offset_value = (1u32 << offset_code) + offset_extra_bits;

        let match_length_code = self.match_length_state.get_symbol(&tables.match_length);
        let match_length_extra_bits = get_match_length_extra_bits(match_length_code);
        let match_length_extra = self.reader.read_bits(match_length_extra_bits as usize) as u32;
        let match_length = get_match_length_base(match_length_code) + match_length_extra;

        let literal_length_code = self.literal_length_state.get_symbol(&tables.literal_length);
        let literal_length_extra_bits = get_literal_length_extra_bits(literal_length_code);
        let literal_length_extra = self.reader.read_bits(literal_length_extra_bits as usize) as u32;
        let literal_length = get_literal_length_base(literal_length_code) + literal_length_extra;

        let offset = repeat_offsets.get_offset(offset_value, literal_length);
        if offset == 0 {
            return Some(Err(DecodeError::BadOffset));
        }

        if self.sequences_left > 1 {
            self.literal_length_state
                .update(&mut self.reader, &tables.literal_length);
            self.match_length_state
                .update(&mut self.reader, &tables.match_length);
            self.offset_state.update(&mut self.reader, &tables.offset);
        }

        if self.reader.has_overflowed() {
            return Some(Err(DecodeError::CorruptBitstream));
        }

        self.sequences_left -= 1;

        Some(Ok(Sequence {
            literal_length,
            match_length,
            offset,
        }))
    }
}

pub(crate) struct SequenceDecoder<'input, 'tables> {
    tables: &'tables SequenceTables,
    first_stream: SequenceStream<'input>,
    second_stream: Option<SequenceStream<'input>>,
    read_from_second_next: bool,
}

impl<'input, 'tables> SequenceDecoder<'input, 'tables> {
    pub(crate) fn new(
        input: &'input [u8],
        tables: &'tables SequenceTables,
        sequence_count: usize,
        format: FrameFormat,
    ) -> Result<Self, DecodeError> {
        if format == FrameFormat::Cosmoz && sequence_count >= 2 {
            let length_bytes = input.get(0..4).ok_or(DecodeError::BadSequencesHeader)?;
            let first_stream_length = u32::from_le_bytes([
                length_bytes[0],
                length_bytes[1],
                length_bytes[2],
                length_bytes[3],
            ]) as usize;
            let remaining = input.get(4..).ok_or(DecodeError::BadSequencesHeader)?;
            let first_stream_input = remaining
                .get(..first_stream_length)
                .ok_or(DecodeError::BadSequencesHeader)?;
            let second_stream_input = remaining
                .get(first_stream_length..)
                .ok_or(DecodeError::BadSequencesHeader)?;

            let first_stream_count = sequence_count.div_ceil(2);
            let second_stream_count = sequence_count / 2;

            let first_stream = SequenceStream::new(first_stream_input, tables, first_stream_count)?;
            let second_stream =
                SequenceStream::new(second_stream_input, tables, second_stream_count)?;

            return Ok(Self {
                tables,
                first_stream,
                second_stream: Some(second_stream),
                read_from_second_next: false,
            });
        }

        let first_stream = SequenceStream::new(input, tables, sequence_count)?;
        Ok(Self {
            tables,
            first_stream,
            second_stream: None,
            read_from_second_next: false,
        })
    }

    pub(crate) fn is_finished(&self) -> bool {
        let second_finished = match &self.second_stream {
            Some(second_stream) => second_stream.is_finished(),
            None => true,
        };
        self.first_stream.is_finished() && second_finished
    }

    pub(crate) fn next_sequence(
        &mut self,
        repeat_offsets: &mut RepeatOffsets,
    ) -> Option<Result<Sequence, DecodeError>> {
        let Some(second_stream) = self.second_stream.as_mut() else {
            return self.first_stream.next_sequence(self.tables, repeat_offsets);
        };

        let take_from_second = self.read_from_second_next && second_stream.sequences_left > 0;
        let take_from_first = !take_from_second && self.first_stream.sequences_left > 0;

        if take_from_first {
            self.read_from_second_next = true;
            return self.first_stream.next_sequence(self.tables, repeat_offsets);
        }
        if take_from_second {
            self.read_from_second_next = false;
            return second_stream.next_sequence(self.tables, repeat_offsets);
        }
        if second_stream.sequences_left > 0 {
            return second_stream.next_sequence(self.tables, repeat_offsets);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_with_unread_sequence_bits_is_rejected_like_libzstd() {
        let frame = [
            0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x68, 0x4d, 0x00, 0x00, 0x08, 0x61, 0x01, 0x00, 0x93,
            0x5b, 0x0d, 0x08, 0x01,
        ];
        let mut workspace = crate::decoder::DecodeWorkspace::new_boxed();
        let mut output = [0u8; 64];
        assert!(crate::decoder::decompress(&frame, &mut output, &mut workspace).is_err());
    }

    #[test]
    fn header_with_two_sequences_and_all_predefined_modes() {
        let header = read_sequences_header(&[0x02, 0x00]).unwrap();
        assert_eq!(header.sequence_count, 2);
        assert_eq!(header.literal_length_mode, TableMode::Predefined);
        assert_eq!(header.offset_mode, TableMode::Predefined);
        assert_eq!(header.match_length_mode, TableMode::Predefined);
        assert_eq!(header.header_length, 2);
    }

    #[test]
    fn header_with_zero_sequences_has_no_modes_byte() {
        let header = read_sequences_header(&[0x00]).unwrap();
        assert_eq!(header.sequence_count, 0);
        assert_eq!(header.literal_length_mode, TableMode::Predefined);
        assert_eq!(header.offset_mode, TableMode::Predefined);
        assert_eq!(header.match_length_mode, TableMode::Predefined);
        assert_eq!(header.header_length, 1);
    }

    #[test]
    fn header_with_three_byte_count_form() {
        let byte1 = 0x40u8;
        let byte2 = 0x01u8;
        let expected_count = byte1 as usize + ((byte2 as usize) << 8) + 0x7F00;
        let header = read_sequences_header(&[0xFF, byte1, byte2, 0x00]).unwrap();
        assert_eq!(header.sequence_count, expected_count);
        assert_eq!(header.header_length, 4);
    }

    #[test]
    fn reserved_bits_set_is_rejected() {
        let result = read_sequences_header(&[0x02, 0x01]);
        assert!(matches!(result, Err(DecodeError::BadSequencesHeader)));
    }

    #[test]
    fn all_predefined_tables_consume_no_bytes_and_become_ready() {
        let header = SequencesHeader {
            sequence_count: 5,
            literal_length_mode: TableMode::Predefined,
            offset_mode: TableMode::Predefined,
            match_length_mode: TableMode::Predefined,
            header_length: 2,
        };
        let mut tables = SequenceTables::new();
        let bytes_consumed = read_sequence_tables(&[], &header, &mut tables).unwrap();
        assert_eq!(bytes_consumed, 0);
        assert!(tables.literal_length_ready);
        assert!(tables.offset_ready);
        assert!(tables.match_length_ready);
    }

    #[test]
    fn repeat_mode_on_a_fresh_table_is_an_error() {
        let header = SequencesHeader {
            sequence_count: 5,
            literal_length_mode: TableMode::Repeat,
            offset_mode: TableMode::Predefined,
            match_length_mode: TableMode::Predefined,
            header_length: 2,
        };
        let mut tables = SequenceTables::new();
        let result = read_sequence_tables(&[], &header, &mut tables);
        assert!(matches!(result, Err(DecodeError::BadSequencesHeader)));
    }

    #[test]
    fn decodes_sequences_from_a_real_zstd_level_one_block() {
        let literal_pool = *b"abcxyzdef\n";
        let sequences_section: [u8; 14] = [
            0x05, 0x00, 0x40, 0x01, 0x3a, 0x36, 0x75, 0x10, 0x00, 0x60, 0x23, 0x99, 0xba, 0x21,
        ];

        let header = read_sequences_header(&sequences_section).unwrap();
        assert_eq!(header.sequence_count, 5);
        assert_eq!(header.literal_length_mode, TableMode::Predefined);
        assert_eq!(header.offset_mode, TableMode::Predefined);
        assert_eq!(header.match_length_mode, TableMode::Predefined);

        let mut tables = SequenceTables::new();
        let bitstream_start = header.header_length
            + read_sequence_tables(
                &sequences_section[header.header_length..],
                &header,
                &mut tables,
            )
            .unwrap();

        let mut decoder = SequenceDecoder::new(
            &sequences_section[bitstream_start..],
            &tables,
            header.sequence_count,
            FrameFormat::Zstd,
        )
        .unwrap();

        let mut repeat_offsets = RepeatOffsets::new();
        let mut output: [u8; 199] = [0; 199];
        let mut output_position = 0usize;
        let mut literal_position = 0usize;
        let mut decoded_count = 0usize;

        while let Some(sequence) = decoder.next_sequence(&mut repeat_offsets) {
            let sequence = sequence.unwrap();
            decoded_count += 1;

            for _ in 0..sequence.literal_length {
                output[output_position] = literal_pool[literal_position];
                output_position += 1;
                literal_position += 1;
            }

            let offset = sequence.offset as usize;
            for _ in 0..sequence.match_length {
                output[output_position] = output[output_position - offset];
                output_position += 1;
            }
        }

        while literal_position < literal_pool.len() && output_position < output.len() {
            output[output_position] = literal_pool[literal_position];
            output_position += 1;
            literal_position += 1;
        }

        assert!(decoder.is_finished());
        assert_eq!(decoded_count, 5);
        assert_eq!(output_position, 199);

        let expected = b"abcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcxyzxyzxyzabcabcabcabcabcabcabcabcabcabcdefdefdefabcabcabcabcabcabcabcabcabcabcabcabcabc\n";
        assert_eq!(&output[..], &expected[..199]);
    }
}
