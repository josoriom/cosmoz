use crate::{
    bits::fast_bit_reader::{FastBitReader, ReloadStatus},
    block::{
        block_decoder::{FAST_PATH_SLACK, copy_match},
        repeat_offsets::RepeatOffsets,
        sequence_tables_fast::{FastSequenceEntry, FastSequenceTable, FastSequenceTables},
    },
    error::DecodeError,
    frame::block_header::MAX_BLOCK_SIZE,
    simd::copy_bytes::{copy_bytes_overshoot_unchecked, copy_match_overshoot_unchecked},
};

struct SequenceStreamState {
    reader: FastBitReader,
    literal_length_state: usize,
    match_length_state: usize,
    offset_state: usize,
    sequences_left: usize,
}

unsafe fn make_reader_unchecked(
    input: &[u8],
    padding_buffer: &mut [u8; 16],
) -> Result<FastBitReader, DecodeError> {
    debug_assert!(!input.is_empty());
    if input.len() >= 8 {
        unsafe { FastBitReader::new_unchecked(input) }
    } else {
        unsafe { FastBitReader::new_padded_unchecked(input, padding_buffer) }
    }
}

impl SequenceStreamState {
    unsafe fn new_unchecked(
        input: &[u8],
        padding_buffer: &mut [u8; 16],
        tables: &FastSequenceTables,
        sequence_count: usize,
    ) -> Result<Self, DecodeError> {
        let mut reader = unsafe { make_reader_unchecked(input, padding_buffer)? };
        let literal_length_state = reader.read(tables.literal_length.accuracy_log as u32) as usize;
        let offset_state = reader.read(tables.offset.accuracy_log as u32) as usize;
        let match_length_state = reader.read(tables.match_length.accuracy_log as u32) as usize;
        Ok(Self {
            reader,
            literal_length_state,
            match_length_state,
            offset_state,
            sequences_left: sequence_count,
        })
    }

    fn is_finished(&self) -> bool {
        self.sequences_left == 0
    }
}

#[inline(always)]
unsafe fn table_entry_unchecked(table: &FastSequenceTable, state: usize) -> FastSequenceEntry {
    debug_assert!(state < table.entries.len());
    unsafe { *table.entries.get_unchecked(state) }
}

struct DecodedSequence {
    literal_length: u32,
    match_length: u32,
    offset: u32,
}

#[inline(always)]
unsafe fn read_repeat_offset_unchecked(
    stream: &mut SequenceStreamState,
    offset_code: u32,
    literal_length_is_zero: bool,
    repeat_offsets: &mut RepeatOffsets,
) -> u32 {
    if offset_code == 0 {
        if literal_length_is_zero {
            core::mem::swap(&mut repeat_offsets.first, &mut repeat_offsets.second);
        }
        return repeat_offsets.first;
    }
    let repeat_index = 1 + literal_length_is_zero as u32 + stream.reader.read(1) as u32;
    let offset = match repeat_index {
        1 => repeat_offsets.second,
        2 => repeat_offsets.third,
        _ => repeat_offsets.first.wrapping_sub(1),
    };
    if repeat_index != 1 {
        repeat_offsets.third = repeat_offsets.second;
    }
    repeat_offsets.second = repeat_offsets.first;
    repeat_offsets.first = offset;
    offset
}

#[allow(clippy::missing_safety_doc)]
#[inline(always)]
unsafe fn decode_one_sequence_unchecked(
    stream: &mut SequenceStreamState,
    tables: &FastSequenceTables,
    repeat_offsets: &mut RepeatOffsets,
) -> Result<DecodedSequence, DecodeError> {
    debug_assert!(stream.sequences_left > 0);

    if unsafe { stream.reader.refill_unchecked() } == ReloadStatus::Overflow {
        return Err(DecodeError::CorruptBitstream);
    }

    let literal_length_entry =
        unsafe { table_entry_unchecked(&tables.literal_length, stream.literal_length_state) };
    let match_entry =
        unsafe { table_entry_unchecked(&tables.match_length, stream.match_length_state) };
    let offset_entry = unsafe { table_entry_unchecked(&tables.offset, stream.offset_state) };
    let offset_code = offset_entry.extra_bits as u32;
    debug_assert!(offset_code <= 31);

    let offset = if offset_code > 1 {
        let offset = offset_entry.base_value + stream.reader.read(offset_code) as u32 - 3;
        repeat_offsets.third = repeat_offsets.second;
        repeat_offsets.second = repeat_offsets.first;
        repeat_offsets.first = offset;
        offset
    } else {
        let offset = unsafe {
            read_repeat_offset_unchecked(
                stream,
                offset_code,
                literal_length_entry.base_value == 0,
                repeat_offsets,
            )
        };
        if offset == 0 {
            return Err(DecodeError::BadOffset);
        }
        offset
    };

    let match_length =
        match_entry.base_value + stream.reader.read(match_entry.extra_bits as u32) as u32;

    if offset_code + match_entry.extra_bits as u32 + literal_length_entry.extra_bits as u32 > 30
        && unsafe { stream.reader.refill_unchecked() } == ReloadStatus::Overflow
    {
        return Err(DecodeError::CorruptBitstream);
    }

    let literal_length = literal_length_entry.base_value
        + stream.reader.read(literal_length_entry.extra_bits as u32) as u32;

    if stream.sequences_left > 1 {
        stream.literal_length_state = literal_length_entry.next_state_base as usize
            + stream.reader.read(literal_length_entry.state_bits as u32) as usize;
        stream.match_length_state = match_entry.next_state_base as usize
            + stream.reader.read(match_entry.state_bits as u32) as usize;
        stream.offset_state = offset_entry.next_state_base as usize
            + stream.reader.read(offset_entry.state_bits as u32) as usize;
    }

    stream.sequences_left -= 1;

    Ok(DecodedSequence {
        literal_length,
        match_length,
        offset,
    })
}

/// Copies `length` literals in whole 16-byte vectors, so it reads up to 15 bytes past
/// `literals + literal_cursor + length`. Callers must keep that overshoot inside a live
/// allocation; see `raw_literals_have_read_slack` in the parent module.
#[inline(always)]
unsafe fn copy_literals_unchecked(
    literals: *const u8,
    literal_cursor: usize,
    length: usize,
    output_cursor: *mut u8,
    exact: bool,
) {
    unsafe {
        let source = literals.add(literal_cursor);
        if exact {
            core::ptr::copy_nonoverlapping(source, output_cursor, length);
            return;
        }
        copy_bytes_overshoot_unchecked(source, output_cursor, 16);
        if length > 16 {
            copy_bytes_overshoot_unchecked(source.add(16), output_cursor.add(16), length - 16);
        }
    }
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
unsafe fn execute_sequence_unchecked(
    output_base: *mut u8,
    position: &mut usize,
    literals: *const u8,
    literal_cursor: &mut usize,
    literal_count: usize,
    sequence: &DecodedSequence,
    block_start: usize,
    block_end: usize,
) -> Result<(), DecodeError> {
    let literal_length = sequence.literal_length as usize;
    let match_length = sequence.match_length as usize;
    let offset = sequence.offset as usize;

    let new_literal_cursor = literal_cursor
        .checked_add(literal_length)
        .ok_or(DecodeError::CorruptBitstream)?;
    if new_literal_cursor > literal_count {
        return Err(DecodeError::CorruptBitstream);
    }

    let after_position = position
        .checked_add(literal_length)
        .and_then(|value| value.checked_add(match_length))
        .ok_or(DecodeError::BlockTooLarge)?;
    if after_position - block_start > MAX_BLOCK_SIZE {
        return Err(DecodeError::BlockTooLarge);
    }
    if after_position > block_end {
        return Err(DecodeError::OutputTooSmall);
    }
    let exact = block_end - after_position < FAST_PATH_SLACK;

    if literal_length > 0 {
        unsafe {
            copy_literals_unchecked(
                literals,
                *literal_cursor,
                literal_length,
                output_base.add(*position),
                exact,
            );
        }
    }
    *literal_cursor = new_literal_cursor;
    *position += literal_length;

    if offset == 0 || offset > *position {
        return Err(DecodeError::BadOffset);
    }

    if exact {
        let output = unsafe { core::slice::from_raw_parts_mut(output_base, block_end) };
        copy_match(output, *position, offset, match_length)?;
    } else if match_length > 0 {
        unsafe {
            copy_match_overshoot_unchecked(output_base.add(*position), offset, match_length);
        }
    }
    *position = after_position;

    Ok(())
}

#[allow(clippy::missing_safety_doc)]
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn decode_sequences_unchecked(
    bitstream: &[u8],
    tables: &FastSequenceTables,
    sequence_count: usize,
    literals: *const u8,
    literal_count: usize,
    output_base: *mut u8,
    output_position: usize,
    output_end: usize,
    repeat_offsets: &mut RepeatOffsets,
) -> Result<usize, DecodeError> {
    debug_assert!(sequence_count > 0);
    debug_assert!(!bitstream.is_empty());

    let mut padding_buffer = [0u8; 16];
    let mut stream = unsafe {
        SequenceStreamState::new_unchecked(bitstream, &mut padding_buffer, tables, sequence_count)?
    };

    let mut position = output_position;
    let mut literal_cursor = 0usize;

    while !stream.is_finished() {
        let sequence =
            unsafe { decode_one_sequence_unchecked(&mut stream, tables, repeat_offsets)? };
        unsafe {
            execute_sequence_unchecked(
                output_base,
                &mut position,
                literals,
                &mut literal_cursor,
                literal_count,
                &sequence,
                output_position,
                output_end,
            )?;
        }
    }

    if !stream.reader.is_finished() {
        return Err(DecodeError::CorruptBitstream);
    }

    if literal_cursor < literal_count {
        let remaining = literal_count - literal_cursor;
        let after_position = position
            .checked_add(remaining)
            .ok_or(DecodeError::BlockTooLarge)?;
        if after_position - output_position > MAX_BLOCK_SIZE {
            return Err(DecodeError::BlockTooLarge);
        }
        if after_position > output_end {
            return Err(DecodeError::OutputTooSmall);
        }
        unsafe {
            copy_literals_unchecked(
                literals,
                literal_cursor,
                remaining,
                output_base.add(position),
                output_end - after_position < FAST_PATH_SLACK,
            );
        }
        position = after_position;
    }

    Ok(position)
}

/// # Safety
///
/// `literals` must be followed by at least 16 readable bytes within its own
/// allocation, because literal copies overshoot to a 16-byte boundary.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn decode_sequences_fast_path_unchecked(
    bitstream: &[u8],
    tables: &FastSequenceTables,
    sequence_count: usize,
    literals: *const u8,
    literal_count: usize,
    output: &mut [u8],
    output_position: usize,
    repeat_offsets: &mut RepeatOffsets,
) -> Result<usize, DecodeError> {
    debug_assert!(sequence_count > 0);

    let output_end = output.len();
    let output_base = output.as_mut_ptr();

    unsafe {
        decode_sequences_unchecked(
            bitstream,
            tables,
            sequence_count,
            literals,
            literal_count,
            output_base,
            output_position,
            output_end,
            repeat_offsets,
        )
    }
}
