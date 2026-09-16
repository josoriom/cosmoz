use crate::{
    bits::fast_bit_reader::{FastBitReader, ReloadStatus},
    block::{
        repeat_offsets::RepeatOffsets,
        sequence_tables_fast::{FastSequenceEntry, FastSequenceTable, FastSequenceTables},
    },
    error::DecodeError,
    frame::{block_header::MAX_BLOCK_SIZE, frame_header::FrameFormat},
    simd::copy_bytes::{copy_bytes_overshoot_unchecked, fill_pattern},
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

#[allow(clippy::missing_safety_doc)]
unsafe fn decode_one_sequence_unchecked(
    stream: &mut SequenceStreamState,
    tables: &FastSequenceTables,
    repeat_offsets: &mut RepeatOffsets,
) -> Result<DecodedSequence, DecodeError> {
    debug_assert!(stream.sequences_left > 0);

    if unsafe { stream.reader.refill_unchecked() } == ReloadStatus::Overflow {
        return Err(DecodeError::CorruptBitstream);
    }

    let offset_entry = unsafe { table_entry_unchecked(&tables.offset, stream.offset_state) };
    if offset_entry.extra_bits > 31 {
        return Err(DecodeError::BadOffset);
    }
    let offset_extra = stream.reader.read(offset_entry.extra_bits as u32) as u32;
    let offset_value = offset_entry.base_value + offset_extra;

    let match_entry =
        unsafe { table_entry_unchecked(&tables.match_length, stream.match_length_state) };
    let match_extra = stream.reader.read(match_entry.extra_bits as u32) as u32;
    let match_length = match_entry.base_value + match_extra;

    if unsafe { stream.reader.refill_unchecked() } == ReloadStatus::Overflow {
        return Err(DecodeError::CorruptBitstream);
    }

    let literal_length_entry =
        unsafe { table_entry_unchecked(&tables.literal_length, stream.literal_length_state) };
    let literal_extra = stream.reader.read(literal_length_entry.extra_bits as u32) as u32;
    let literal_length = literal_length_entry.base_value + literal_extra;

    let offset = repeat_offsets.get_offset(offset_value, literal_length);
    if offset == 0 {
        return Err(DecodeError::BadOffset);
    }

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
) {
    unsafe {
        copy_bytes_overshoot_unchecked(literals.add(literal_cursor), output_cursor, length);
    }
}

#[inline(always)]
unsafe fn copy_match_unchecked(
    output_base: *mut u8,
    source_start: usize,
    output_position: usize,
    length: usize,
) {
    let mut written = 0usize;
    while written < length {
        unsafe {
            let source_pointer = output_base.add(source_start + written) as *const u8;
            let destination_pointer = output_base.add(output_position + written);
            copy_bytes_overshoot_unchecked(source_pointer, destination_pointer, 16);
        }
        written += 16;
    }
}

const SEQUENCE_LOOKAHEAD: usize = 4;

#[inline(always)]
fn prefetch_read(pointer: *const u8) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::x86_64::_mm_prefetch(pointer as *const i8, core::arch::x86_64::_MM_HINT_T0);
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!(
            "prfm pldl1keep, [{0}]",
            in(reg) pointer,
            options(nostack, preserves_flags, readonly),
        );
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = pointer;
    }
}

#[derive(Clone, Copy, Default)]
struct PlannedSequence {
    literal_source_cursor: usize,
    literal_length: usize,
    destination_position: usize,
    match_source_start: usize,
    match_destination: usize,
    match_length: usize,
    offset: usize,
}

#[allow(clippy::too_many_arguments)]
unsafe fn plan_sequence_unchecked(
    stream: &mut SequenceStreamState,
    tables: &FastSequenceTables,
    repeat_offsets: &mut RepeatOffsets,
    literal_cursor: &mut usize,
    literal_count: usize,
    position: &mut usize,
    block_start: usize,
    block_end: usize,
) -> Result<PlannedSequence, DecodeError> {
    let sequence = unsafe { decode_one_sequence_unchecked(stream, tables, repeat_offsets)? };

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

    let literal_source_cursor = *literal_cursor;
    let destination_position = *position;
    *literal_cursor = new_literal_cursor;

    let position_after_literals = destination_position + literal_length;

    if offset == 0 || offset > position_after_literals {
        return Err(DecodeError::BadOffset);
    }
    let match_source_start = position_after_literals - offset;

    *position = after_position;

    Ok(PlannedSequence {
        literal_source_cursor,
        literal_length,
        destination_position,
        match_source_start,
        match_destination: position_after_literals,
        match_length,
        offset,
    })
}

unsafe fn execute_planned_sequence_unchecked(
    output_base: *mut u8,
    literals: *const u8,
    plan: &PlannedSequence,
) {
    if plan.literal_length > 0 {
        unsafe {
            copy_literals_unchecked(
                literals,
                plan.literal_source_cursor,
                plan.literal_length,
                output_base.add(plan.destination_position),
            );
        }
    }

    if plan.match_length > 0 {
        if plan.offset >= 16 {
            unsafe {
                copy_match_unchecked(
                    output_base,
                    plan.match_source_start,
                    plan.match_destination,
                    plan.match_length,
                );
            }
        } else {
            let mut pattern_bytes = [0u8; 16];
            unsafe {
                let source = output_base.add(plan.match_source_start);
                let mut index = 0usize;
                while index < plan.offset {
                    pattern_bytes[index] = *source.add(index);
                    index += 1;
                }
                let destination = core::slice::from_raw_parts_mut(
                    output_base.add(plan.match_destination),
                    plan.match_length,
                );
                fill_pattern(&pattern_bytes[..plan.offset], destination);
            }
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

    if literal_length > 0 {
        unsafe {
            copy_literals_unchecked(
                literals,
                *literal_cursor,
                literal_length,
                output_base.add(*position),
            );
        }
    }
    *literal_cursor = new_literal_cursor;
    *position += literal_length;

    if offset == 0 || offset > *position {
        return Err(DecodeError::BadOffset);
    }
    let source_start = *position - offset;

    if match_length > 0 {
        if offset >= 16 {
            unsafe {
                copy_match_unchecked(output_base, source_start, *position, match_length);
            }
        } else {
            let mut pattern_bytes = [0u8; 16];
            unsafe {
                let source = output_base.add(source_start);
                let mut index = 0usize;
                while index < offset {
                    pattern_bytes[index] = *source.add(index);
                    index += 1;
                }
                let destination =
                    core::slice::from_raw_parts_mut(output_base.add(*position), match_length);
                fill_pattern(&pattern_bytes[..offset], destination);
            }
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
        let mut batch = [PlannedSequence::default(); SEQUENCE_LOOKAHEAD];
        let mut batch_length = 0usize;

        while batch_length < SEQUENCE_LOOKAHEAD && !stream.is_finished() {
            let plan = unsafe {
                plan_sequence_unchecked(
                    &mut stream,
                    tables,
                    repeat_offsets,
                    &mut literal_cursor,
                    literal_count,
                    &mut position,
                    output_position,
                    output_end,
                )?
            };
            if plan.match_length > 0 {
                prefetch_read(unsafe { output_base.add(plan.match_source_start) });
            }
            batch[batch_length] = plan;
            batch_length += 1;
        }

        for plan in &batch[..batch_length] {
            unsafe {
                execute_planned_sequence_unchecked(output_base, literals, plan);
            }
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
            );
        }
        position = after_position;
    }

    Ok(position)
}

#[allow(clippy::missing_safety_doc)]
#[allow(clippy::too_many_arguments)]
unsafe fn decode_sequences_two_streams_unchecked(
    first_input: &[u8],
    second_input: &[u8],
    tables: &FastSequenceTables,
    first_count: usize,
    second_count: usize,
    literals: *const u8,
    literal_count: usize,
    output_base: *mut u8,
    output_position: usize,
    output_end: usize,
    repeat_offsets: &mut RepeatOffsets,
) -> Result<usize, DecodeError> {
    debug_assert!(!first_input.is_empty());
    debug_assert!(!second_input.is_empty());

    let mut first_padding = [0u8; 16];
    let mut second_padding = [0u8; 16];
    let mut first_stream = unsafe {
        SequenceStreamState::new_unchecked(first_input, &mut first_padding, tables, first_count)?
    };
    let mut second_stream = unsafe {
        SequenceStreamState::new_unchecked(second_input, &mut second_padding, tables, second_count)?
    };

    let mut position = output_position;
    let mut literal_cursor = 0usize;
    let mut read_from_second_next = false;

    loop {
        let take_from_second = read_from_second_next && second_stream.sequences_left > 0;
        let take_from_first = !take_from_second && first_stream.sequences_left > 0;

        let sequence = if take_from_first {
            read_from_second_next = true;
            unsafe { decode_one_sequence_unchecked(&mut first_stream, tables, repeat_offsets)? }
        } else if take_from_second {
            read_from_second_next = false;
            unsafe { decode_one_sequence_unchecked(&mut second_stream, tables, repeat_offsets)? }
        } else if second_stream.sequences_left > 0 {
            unsafe { decode_one_sequence_unchecked(&mut second_stream, tables, repeat_offsets)? }
        } else {
            break;
        };

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

    if !first_stream.reader.is_finished() || !second_stream.reader.is_finished() {
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
            );
        }
        position = after_position;
    }

    Ok(position)
}

type OsmoStreamSplit<'input> = (&'input [u8], &'input [u8], usize, usize);

fn split_osmo_streams(
    input: &[u8],
    sequence_count: usize,
) -> Result<OsmoStreamSplit<'_>, DecodeError> {
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
    Ok((
        first_stream_input,
        second_stream_input,
        first_stream_count,
        second_stream_count,
    ))
}

/// # Safety
///
/// `output` must have at least `MAX_BLOCK_SIZE + 32` bytes free from `output_position`,
/// and `literals` must be followed by at least 16 readable bytes within its own
/// allocation, because literal copies overshoot to a 16-byte boundary.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn decode_sequences_fast_path_unchecked(
    bitstream: &[u8],
    tables: &FastSequenceTables,
    sequence_count: usize,
    literals: &[u8],
    literal_count: usize,
    output: &mut [u8],
    output_position: usize,
    format: FrameFormat,
    repeat_offsets: &mut RepeatOffsets,
) -> Result<usize, DecodeError> {
    debug_assert!(sequence_count > 0);
    debug_assert!(literal_count <= literals.len());
    debug_assert!(output.len() >= output_position + MAX_BLOCK_SIZE + 32);

    let output_end = output.len();
    let output_base = output.as_mut_ptr();
    let literals_pointer = literals.as_ptr();

    if format == FrameFormat::Osmo && sequence_count >= 2 {
        let (first_input, second_input, first_count, second_count) =
            split_osmo_streams(bitstream, sequence_count)?;
        if first_input.is_empty() || second_input.is_empty() {
            return Err(DecodeError::BadSequencesHeader);
        }
        return unsafe {
            decode_sequences_two_streams_unchecked(
                first_input,
                second_input,
                tables,
                first_count,
                second_count,
                literals_pointer,
                literal_count,
                output_base,
                output_position,
                output_end,
                repeat_offsets,
            )
        };
    }

    unsafe {
        decode_sequences_unchecked(
            bitstream,
            tables,
            sequence_count,
            literals_pointer,
            literal_count,
            output_base,
            output_position,
            output_end,
            repeat_offsets,
        )
    }
}
