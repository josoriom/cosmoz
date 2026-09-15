use crate::bits::fast_bit_reader::{FastBitReader, ReloadStatus};
use crate::entropy::huffman_decode_table::HuffmanDecodeTable;
use crate::error::DecodeError;

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

#[allow(clippy::needless_range_loop)]
unsafe fn decode_streams_interleaved_unchecked<const STREAMS: usize>(
    readers: &mut [FastBitReader; STREAMS],
    table: &HuffmanDecodeTable,
    outputs: [*mut u8; STREAMS],
    segment_lengths: [usize; STREAMS],
) {
    debug_assert!(table.is_ready);
    let max_bits = table.max_bits as u32;
    let lookups_per_refill = (57 / max_bits.max(1)) as usize;
    let iteration_slack = lookups_per_refill * 2;
    let single_entries = &table.entries;
    let mut positions = [0usize; STREAMS];

    'batches: loop {
        for stream in 0..STREAMS {
            if segment_lengths[stream] - positions[stream] < iteration_slack {
                break 'batches;
            }
        }
        for stream in 0..STREAMS {
            let status = unsafe { readers[stream].refill_unchecked() };
            if status != ReloadStatus::Unfinished {
                break 'batches;
            }
        }
        for _ in 0..lookups_per_refill {
            for stream in 0..STREAMS {
                let index = readers[stream].peek(max_bits) as usize;
                debug_assert!(index < single_entries.len());
                let entry = unsafe { *single_entries.get_unchecked(index) };
                unsafe {
                    *outputs[stream].add(positions[stream]) = entry.symbol;
                }
                readers[stream].skip(entry.bit_count as u32);
                positions[stream] += 1;
            }
        }
    }

    for stream in 0..STREAMS {
        while positions[stream] < segment_lengths[stream] {
            unsafe {
                let _ = readers[stream].refill_unchecked();
            }
            let index = peek_window_padded(&readers[stream], max_bits) as usize;
            debug_assert!(index < single_entries.len());
            let entry = unsafe { *single_entries.get_unchecked(index) };
            unsafe {
                *outputs[stream].add(positions[stream]) = entry.symbol;
            }
            readers[stream].skip(entry.bit_count as u32);
            positions[stream] += 1;
        }
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
    let reader_0 = unsafe { make_reader_unchecked(streams[0], padding_0)? };
    let reader_1 = unsafe { make_reader_unchecked(streams[1], padding_1)? };
    let reader_2 = unsafe { make_reader_unchecked(streams[2], padding_2)? };
    let reader_3 = unsafe { make_reader_unchecked(streams[3], padding_3)? };

    let offset_0 = 0usize;
    let offset_1 = offset_0 + segment_sizes[0];
    let offset_2 = offset_1 + segment_sizes[1];
    let offset_3 = offset_2 + segment_sizes[2];

    let mut readers = [reader_0, reader_1, reader_2, reader_3];
    let outputs = unsafe {
        [
            output.add(offset_0),
            output.add(offset_1),
            output.add(offset_2),
            output.add(offset_3),
        ]
    };
    unsafe {
        decode_streams_interleaved_unchecked(&mut readers, table, outputs, segment_sizes);
    }

    let all_finished = readers.iter().all(|reader| reader.is_finished());
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
    let reader_0 = unsafe { make_reader_unchecked(streams[0], padding_0)? };
    let reader_1 = unsafe { make_reader_unchecked(streams[1], padding_1)? };
    let reader_2 = unsafe { make_reader_unchecked(streams[2], padding_2)? };
    let reader_3 = unsafe { make_reader_unchecked(streams[3], padding_3)? };
    let reader_4 = unsafe { make_reader_unchecked(streams[4], padding_4)? };
    let reader_5 = unsafe { make_reader_unchecked(streams[5], padding_5)? };
    let reader_6 = unsafe { make_reader_unchecked(streams[6], padding_6)? };
    let reader_7 = unsafe { make_reader_unchecked(streams[7], padding_7)? };

    let offset_0 = 0usize;
    let offset_1 = offset_0 + segment_sizes[0];
    let offset_2 = offset_1 + segment_sizes[1];
    let offset_3 = offset_2 + segment_sizes[2];
    let offset_4 = offset_3 + segment_sizes[3];
    let offset_5 = offset_4 + segment_sizes[4];
    let offset_6 = offset_5 + segment_sizes[5];
    let offset_7 = offset_6 + segment_sizes[6];

    let mut readers = [
        reader_0, reader_1, reader_2, reader_3, reader_4, reader_5, reader_6, reader_7,
    ];
    let outputs = unsafe {
        [
            output.add(offset_0),
            output.add(offset_1),
            output.add(offset_2),
            output.add(offset_3),
            output.add(offset_4),
            output.add(offset_5),
            output.add(offset_6),
            output.add(offset_7),
        ]
    };
    unsafe {
        decode_streams_interleaved_unchecked(&mut readers, table, outputs, segment_sizes);
    }

    let all_finished = readers.iter().all(|reader| reader.is_finished());
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
            if let Some(tables) = build_tables_from_counts(&counts) {
                return tables;
            }
        }
    }

    fn build_tables_from_counts(
        counts: &[u32; 256],
    ) -> Option<(HuffmanEncodeTable, HuffmanDecodeTable)> {
        let mut encode_table = HuffmanEncodeTable::new();
        build_huffman_encode_table(counts, &mut encode_table).ok()?;

        let mut weights_output = [0u8; 256];
        let bytes_written = crate::entropy::huffman_encode_table::write_direct_weights(
            &mut weights_output,
            &encode_table,
        )
        .ok()?;
        let mut decode_table = HuffmanDecodeTable::new();
        let mut weight_fse_table = FseDecodeTable::new();
        crate::entropy::huffman_decode_table::read_huffman_table(
            &weights_output[..bytes_written],
            &mut decode_table,
            &mut weight_fse_table,
        )
        .ok()?;
        Some((encode_table, decode_table))
    }

    fn build_table_with_max_bits(
        random: &mut XorshiftRandom,
        wanted_max_bits: u8,
    ) -> (HuffmanEncodeTable, HuffmanDecodeTable) {
        for _ in 0..20_000 {
            let counts = build_random_histogram(random);
            if let Some((encode_table, decode_table)) = build_tables_from_counts(&counts)
                && decode_table.max_bits == wanted_max_bits
            {
                return (encode_table, decode_table);
            }
        }
        if wanted_max_bits == 11 {
            let mut counts = [0u32; 256];
            let mut previous = 1u32;
            let mut current = 1u32;
            for symbol_count in counts.iter_mut().take(24) {
                *symbol_count = current;
                let next = previous + current;
                previous = current;
                current = next;
            }
            if let Some((encode_table, decode_table)) = build_tables_from_counts(&counts)
                && decode_table.max_bits == wanted_max_bits
            {
                return (encode_table, decode_table);
            }
        }
        panic!("could not find a random table with max_bits = {wanted_max_bits}");
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

    #[test]
    fn fast_path_matches_checked_path_at_segment_length_boundaries_and_bit_width_extremes() {
        let mut random = XorshiftRandom::new(0x51ED270B39537A2F);
        let segment_lengths = [1usize, 7, 8, 9, 63, 64, 65, 5000];

        for &wanted_max_bits in &[11u8, 1u8, 2u8] {
            let (encode_table, decode_table) =
                build_table_with_max_bits(&mut random, wanted_max_bits);
            assert_eq!(decode_table.max_bits, wanted_max_bits);

            for stream_count in [4usize, 8usize] {
                for &segment_length in &segment_lengths {
                    let length = segment_length * stream_count;
                    let text = random_text(&mut random, &encode_table, length);

                    let mut encoded = vec![0u8; length * 2 + 4096];
                    let bytes_written =
                        match encode_many_streams(&text, &encode_table, stream_count, &mut encoded)
                        {
                            Ok(bytes_written) => bytes_written,
                            Err(_) => continue,
                        };
                    let encoded = &encoded[..bytes_written];

                    let mut checked_output = vec![0u8; length];
                    let checked_result = decode_many_streams(
                        encoded,
                        &decode_table,
                        stream_count,
                        &mut checked_output,
                    );

                    let fast_result = decode_fast(encoded, &decode_table, stream_count, length);

                    match (checked_result, fast_result) {
                        (Ok(()), Ok(fast_output)) => assert_eq!(checked_output, fast_output),
                        (Err(checked_error), Err(fast_error)) => {
                            assert_eq!(checked_error, fast_error)
                        }
                        (checked, fast) => panic!(
                            "fast and checked paths disagreed: checked={checked:?} fast={fast:?} max_bits={wanted_max_bits} segment_length={segment_length} stream_count={stream_count}"
                        ),
                    }
                }
            }
        }
    }
}
