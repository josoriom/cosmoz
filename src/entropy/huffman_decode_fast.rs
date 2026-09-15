use crate::bits::fast_bit_reader::{FastBitReader, ReloadStatus};
use crate::entropy::huffman_decode_table::HuffmanDecodeTable;
use crate::error::DecodeError;

const FAST_LOOKUPS_PER_REFILL: usize = 4;
const FAST_ITERATION_SLACK: usize = FAST_LOOKUPS_PER_REFILL * 2;
const WINDOW_BITS: u32 = 12;

unsafe fn make_reader_unchecked(
    stream: &[u8],
    padding_buffer: &mut [u8; 16],
) -> Result<FastBitReader, DecodeError> {
    debug_assert!(!stream.is_empty());
    if stream.len() >= 8 {
        unsafe { FastBitReader::new_unchecked(stream) }
    } else {
        unsafe { FastBitReader::new_padded_unchecked(stream, padding_buffer) }
    }
}

unsafe fn decode_stream_double_table_unchecked(
    reader: &mut FastBitReader,
    table: &HuffmanDecodeTable,
    output: *mut u8,
    segment_length: usize,
) {
    debug_assert!(table.is_ready);
    let double_entries = &table.double_entries;
    let mut position = 0usize;

    loop {
        if segment_length - position < FAST_ITERATION_SLACK {
            break;
        }
        let status = unsafe { reader.refill_unchecked() };
        if status != ReloadStatus::Unfinished {
            break;
        }
        let mut lookup_index = 0usize;
        while lookup_index < FAST_LOOKUPS_PER_REFILL {
            let window = reader.peek(WINDOW_BITS) as usize;
            debug_assert!(window < double_entries.len());
            let entry = unsafe { *double_entries.get_unchecked(window) };
            unsafe {
                output
                    .add(position)
                    .cast::<[u8; 2]>()
                    .write_unaligned(entry.symbols);
            }
            reader.skip(entry.bit_count as u32);
            position += entry.symbol_count as usize;
            lookup_index += 1;
        }
    }

    let max_bits = table.max_bits as u32;
    let single_entries = &table.entries;
    while position < segment_length {
        unsafe {
            let _ = reader.refill_unchecked();
        }
        let index = peek_window_padded(reader, max_bits) as usize;
        debug_assert!(index < single_entries.len());
        let entry = unsafe { *single_entries.get_unchecked(index) };
        unsafe {
            *output.add(position) = entry.symbol;
        }
        reader.skip(entry.bit_count as u32);
        position += 1;
    }
}

#[inline(always)]
fn peek_window_padded(reader: &FastBitReader, width: u32) -> u64 {
    let available = (reader.bits_left() as u32).min(width);
    if available == width {
        reader.peek(width)
    } else {
        reader.peek(available) << (width - available)
    }
}

pub(crate) unsafe fn decode_four_streams_unchecked(
    streams: [&[u8]; 4],
    table: &HuffmanDecodeTable,
    output: *mut u8,
    segment_sizes: [usize; 4],
) -> Result<(), DecodeError> {
    debug_assert!(table.is_ready);
    debug_assert!(streams.iter().all(|stream| !stream.is_empty()));

    let mut padding_buffers = [[0u8; 16]; 4];
    let [padding_0, padding_1, padding_2, padding_3] = &mut padding_buffers;
    let mut reader_0 = unsafe { make_reader_unchecked(streams[0], padding_0)? };
    let mut reader_1 = unsafe { make_reader_unchecked(streams[1], padding_1)? };
    let mut reader_2 = unsafe { make_reader_unchecked(streams[2], padding_2)? };
    let mut reader_3 = unsafe { make_reader_unchecked(streams[3], padding_3)? };

    let offset_0 = 0usize;
    let offset_1 = offset_0 + segment_sizes[0];
    let offset_2 = offset_1 + segment_sizes[1];
    let offset_3 = offset_2 + segment_sizes[2];

    unsafe {
        decode_stream_double_table_unchecked(
            &mut reader_0,
            table,
            output.add(offset_0),
            segment_sizes[0],
        );
        decode_stream_double_table_unchecked(
            &mut reader_1,
            table,
            output.add(offset_1),
            segment_sizes[1],
        );
        decode_stream_double_table_unchecked(
            &mut reader_2,
            table,
            output.add(offset_2),
            segment_sizes[2],
        );
        decode_stream_double_table_unchecked(
            &mut reader_3,
            table,
            output.add(offset_3),
            segment_sizes[3],
        );
    }

    let all_finished = reader_0.is_finished()
        && reader_1.is_finished()
        && reader_2.is_finished()
        && reader_3.is_finished();
    if !all_finished {
        return Err(DecodeError::CorruptBitstream);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn decode_eight_streams_unchecked(
    streams: [&[u8]; 8],
    table: &HuffmanDecodeTable,
    output: *mut u8,
    segment_sizes: [usize; 8],
) -> Result<(), DecodeError> {
    debug_assert!(table.is_ready);
    debug_assert!(streams.iter().all(|stream| !stream.is_empty()));

    let mut padding_buffers = [[0u8; 16]; 8];
    let [
        padding_0,
        padding_1,
        padding_2,
        padding_3,
        padding_4,
        padding_5,
        padding_6,
        padding_7,
    ] = &mut padding_buffers;
    let mut reader_0 = unsafe { make_reader_unchecked(streams[0], padding_0)? };
    let mut reader_1 = unsafe { make_reader_unchecked(streams[1], padding_1)? };
    let mut reader_2 = unsafe { make_reader_unchecked(streams[2], padding_2)? };
    let mut reader_3 = unsafe { make_reader_unchecked(streams[3], padding_3)? };
    let mut reader_4 = unsafe { make_reader_unchecked(streams[4], padding_4)? };
    let mut reader_5 = unsafe { make_reader_unchecked(streams[5], padding_5)? };
    let mut reader_6 = unsafe { make_reader_unchecked(streams[6], padding_6)? };
    let mut reader_7 = unsafe { make_reader_unchecked(streams[7], padding_7)? };

    let offset_0 = 0usize;
    let offset_1 = offset_0 + segment_sizes[0];
    let offset_2 = offset_1 + segment_sizes[1];
    let offset_3 = offset_2 + segment_sizes[2];
    let offset_4 = offset_3 + segment_sizes[3];
    let offset_5 = offset_4 + segment_sizes[4];
    let offset_6 = offset_5 + segment_sizes[5];
    let offset_7 = offset_6 + segment_sizes[6];

    unsafe {
        decode_stream_double_table_unchecked(
            &mut reader_0,
            table,
            output.add(offset_0),
            segment_sizes[0],
        );
        decode_stream_double_table_unchecked(
            &mut reader_1,
            table,
            output.add(offset_1),
            segment_sizes[1],
        );
        decode_stream_double_table_unchecked(
            &mut reader_2,
            table,
            output.add(offset_2),
            segment_sizes[2],
        );
        decode_stream_double_table_unchecked(
            &mut reader_3,
            table,
            output.add(offset_3),
            segment_sizes[3],
        );
        decode_stream_double_table_unchecked(
            &mut reader_4,
            table,
            output.add(offset_4),
            segment_sizes[4],
        );
        decode_stream_double_table_unchecked(
            &mut reader_5,
            table,
            output.add(offset_5),
            segment_sizes[5],
        );
        decode_stream_double_table_unchecked(
            &mut reader_6,
            table,
            output.add(offset_6),
            segment_sizes[6],
        );
        decode_stream_double_table_unchecked(
            &mut reader_7,
            table,
            output.add(offset_7),
            segment_sizes[7],
        );
    }

    let all_finished = reader_0.is_finished()
        && reader_1.is_finished()
        && reader_2.is_finished()
        && reader_3.is_finished()
        && reader_4.is_finished()
        && reader_5.is_finished()
        && reader_6.is_finished()
        && reader_7.is_finished();
    if !all_finished {
        return Err(DecodeError::CorruptBitstream);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::fse_decode_table::FseDecodeTable;
    use crate::entropy::histogram::count_symbols;
    use crate::entropy::huffman_decode::decode_many_streams;
    use crate::entropy::huffman_encode::encode_many_streams;
    use crate::entropy::huffman_encode_table::{HuffmanEncodeTable, build_huffman_encode_table};

    struct XorshiftRandom {
        state: u64,
    }

    impl XorshiftRandom {
        fn new(seed: u64) -> Self {
            Self { state: seed | 1 }
        }

        fn next_u64(&mut self) -> u64 {
            let mut value = self.state;
            value ^= value << 13;
            value ^= value >> 7;
            value ^= value << 17;
            self.state = value;
            value
        }

        fn next_range(&mut self, bound: usize) -> usize {
            (self.next_u64() % bound as u64) as usize
        }
    }

    fn build_random_histogram(random: &mut XorshiftRandom) -> [u32; 256] {
        let mut counts = [0u32; 256];
        let symbol_count = 2 + random.next_range(60);
        for _ in 0..4000 {
            let symbol = random.next_range(symbol_count);
            counts[symbol] += 1;
        }
        counts
    }

    fn build_tables(random: &mut XorshiftRandom) -> (HuffmanEncodeTable, HuffmanDecodeTable) {
        loop {
            let counts = build_random_histogram(random);
            let mut encode_table = HuffmanEncodeTable::new();
            if build_huffman_encode_table(&counts, &mut encode_table).is_err() {
                continue;
            }

            let mut weights_output = [0u8; 256];
            let bytes_written = crate::entropy::huffman_encode_table::write_direct_weights(
                &mut weights_output,
                &encode_table,
            )
            .unwrap();
            let mut decode_table = HuffmanDecodeTable::new();
            let mut weight_fse_table = FseDecodeTable::new();
            crate::entropy::huffman_decode_table::read_huffman_table(
                &weights_output[..bytes_written],
                &mut decode_table,
                &mut weight_fse_table,
            )
            .unwrap();
            return (encode_table, decode_table);
        }
    }

    fn random_text(
        random: &mut XorshiftRandom,
        encode_table: &HuffmanEncodeTable,
        length: usize,
    ) -> Vec<u8> {
        let usable_symbols: Vec<u8> = (0..=255u8)
            .filter(|&symbol| encode_table.codes[symbol as usize].bit_count > 0)
            .collect();
        let mut text = Vec::with_capacity(length);
        for _ in 0..length {
            let symbol = usable_symbols[random.next_range(usable_symbols.len())];
            text.push(symbol);
        }
        text
    }

    fn decode_fast(
        encoded: &[u8],
        table: &HuffmanDecodeTable,
        stream_count: usize,
        regenerated_size: usize,
    ) -> Result<Vec<u8>, DecodeError> {
        let mut output = vec![0u8; regenerated_size + 32];
        let jump_table_size = (stream_count - 1) * 2;
        let after_jump_table = &encoded[jump_table_size..];
        let segment_size = regenerated_size.div_ceil(stream_count);

        let mut stream_slices: Vec<&[u8]> = Vec::with_capacity(stream_count);
        let mut segment_lengths: Vec<usize> = Vec::with_capacity(stream_count);
        let mut remaining_input = after_jump_table;
        let mut remaining_output = regenerated_size;
        for stream_index in 0..stream_count {
            let is_last = stream_index == stream_count - 1;
            let stream = if is_last {
                remaining_input
            } else {
                let stream_size =
                    u16::from_le_bytes([encoded[stream_index * 2], encoded[stream_index * 2 + 1]])
                        as usize;
                let stream = &remaining_input[..stream_size];
                remaining_input = &remaining_input[stream_size..];
                stream
            };
            let segment_length = if is_last {
                remaining_output
            } else {
                segment_size
            };
            remaining_output -= segment_length;
            stream_slices.push(stream);
            segment_lengths.push(segment_length);
        }

        let output_pointer = output.as_mut_ptr();
        if stream_count == 4 {
            let streams = [
                stream_slices[0],
                stream_slices[1],
                stream_slices[2],
                stream_slices[3],
            ];
            let segments = [
                segment_lengths[0],
                segment_lengths[1],
                segment_lengths[2],
                segment_lengths[3],
            ];
            unsafe { decode_four_streams_unchecked(streams, table, output_pointer, segments)? };
        } else {
            let streams = [
                stream_slices[0],
                stream_slices[1],
                stream_slices[2],
                stream_slices[3],
                stream_slices[4],
                stream_slices[5],
                stream_slices[6],
                stream_slices[7],
            ];
            let segments = [
                segment_lengths[0],
                segment_lengths[1],
                segment_lengths[2],
                segment_lengths[3],
                segment_lengths[4],
                segment_lengths[5],
                segment_lengths[6],
                segment_lengths[7],
            ];
            unsafe { decode_eight_streams_unchecked(streams, table, output_pointer, segments)? };
        }
        output.truncate(regenerated_size);
        Ok(output)
    }

    #[test]
    fn fast_path_matches_checked_path_on_random_tables_and_inputs() {
        let mut random = XorshiftRandom::new(0x9E3779B97F4A7C15);

        for case_index in 0..500 {
            let (encode_table, decode_table) = build_tables(&mut random);
            let stream_count = if case_index % 2 == 0 { 4 } else { 8 };
            let length = random.next_range(20000);
            if length < stream_count * 4 {
                continue;
            }
            let text = random_text(&mut random, &encode_table, length);

            let mut counts = [0u32; 256];
            count_symbols(&text, &mut counts);

            let mut encoded = vec![0u8; length * 2 + 4096];
            let bytes_written =
                match encode_many_streams(&text, &encode_table, stream_count, &mut encoded) {
                    Ok(bytes_written) => bytes_written,
                    Err(_) => continue,
                };
            let encoded = &encoded[..bytes_written];

            let mut checked_output = vec![0u8; length];
            let checked_result =
                decode_many_streams(encoded, &decode_table, stream_count, &mut checked_output);

            let fast_result = decode_fast(encoded, &decode_table, stream_count, length);

            match (checked_result, fast_result) {
                (Ok(()), Ok(fast_output)) => assert_eq!(checked_output, fast_output),
                (Err(checked_error), Err(fast_error)) => assert_eq!(checked_error, fast_error),
                (checked, fast) => panic!(
                    "fast and checked paths disagreed: checked={checked:?} fast={fast:?} length={length} stream_count={stream_count}"
                ),
            }
        }
    }

    #[test]
    fn truncated_streams_yield_corrupt_bitstream_from_both_paths() {
        let mut random = XorshiftRandom::new(0xD1B54A32D192ED03);
        let (encode_table, decode_table) = build_tables(&mut random);
        let length = 4000;
        let text = random_text(&mut random, &encode_table, length);

        let mut encoded = vec![0u8; length * 2 + 4096];
        let bytes_written = encode_many_streams(&text, &encode_table, 4, &mut encoded).unwrap();
        let truncated = &encoded[..bytes_written - 1];

        let mut checked_output = vec![0u8; length];
        let checked_result = decode_many_streams(truncated, &decode_table, 4, &mut checked_output);
        let fast_result = decode_fast(truncated, &decode_table, 4, length);

        assert!(checked_result.is_err());
        assert!(fast_result.is_err());
    }
}
