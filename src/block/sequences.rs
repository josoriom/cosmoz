use crate::bits::backward_bit_reader::BackwardBitReader;
use crate::block::repeat_offsets::RepeatOffsets;
use crate::block::sequence_codes::{
    get_literal_length_base, get_literal_length_extra_bits, get_match_length_base,
    get_match_length_extra_bits,
};
use crate::entropy::fse_decode_table::{
    FseDecodeState, FseDecodeTable, build_rle_table, read_fse_table_description,
};
use crate::entropy::fse_predefined::{
    build_predefined_literal_length_table, build_predefined_match_length_table,
    build_predefined_offset_table,
};
use crate::error::DecodeError;
use crate::frame::frame_header::FrameFormat;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TableMode {
    Predefined,
    Rle,
    Compressed,
    Repeat,
}

pub struct SequencesHeader {
    pub sequence_count: usize,
    pub literal_length_mode: TableMode,
    pub offset_mode: TableMode,
    pub match_length_mode: TableMode,
    pub header_length: usize,
}

pub struct Sequence {
    pub literal_length: u32,
    pub match_length: u32,
    pub offset: u32,
}

pub struct SequenceTables {
    pub literal_length: FseDecodeTable,
    pub offset: FseDecodeTable,
    pub match_length: FseDecodeTable,
    pub literal_length_ready: bool,
    pub offset_ready: bool,
    pub match_length_ready: bool,
}

impl SequenceTables {
    pub const fn new() -> Self {
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

pub fn read_sequences_header(input: &[u8]) -> Result<SequencesHeader, DecodeError> {
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

pub fn read_sequence_tables(
    input: &[u8],
    header: &SequencesHeader,
    tables: &mut SequenceTables,
) -> Result<usize, DecodeError> {
    let mut position = 0usize;

    position += read_table_for_mode(
        &input[position..],
        header.literal_length_mode,
        9,
        35,
        build_predefined_literal_length_table,
        &mut tables.literal_length,
        &mut tables.literal_length_ready,
    )?;

    position += read_table_for_mode(
        &input[position..],
        header.offset_mode,
        8,
        31,
        build_predefined_offset_table,
        &mut tables.offset,
        &mut tables.offset_ready,
    )?;

    position += read_table_for_mode(
        &input[position..],
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
        self.sequences_left == 0
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

pub struct SequenceDecoder<'input, 'tables> {
    tables: &'tables SequenceTables,
    first_stream: SequenceStream<'input>,
    second_stream: Option<SequenceStream<'input>>,
    read_from_second_next: bool,
}

impl<'input, 'tables> SequenceDecoder<'input, 'tables> {
    pub fn new(
        input: &'input [u8],
        tables: &'tables SequenceTables,
        sequence_count: usize,
        format: FrameFormat,
    ) -> Result<Self, DecodeError> {
        if format == FrameFormat::Osmo && sequence_count >= 2 {
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

    pub fn is_finished(&self) -> bool {
        let second_finished = match &self.second_stream {
            Some(second_stream) => second_stream.is_finished(),
            None => true,
        };
        self.first_stream.is_finished() && second_finished
    }

    pub fn next_sequence(
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
