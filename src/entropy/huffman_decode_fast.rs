use crate::{
    bits::fast_bit_reader::FastBitReader,
    entropy::huffman_decode_table::{
        HuffmanDecodeEntry, HuffmanDecodeTable, MAX_HUFFMAN_TABLE_SIZE,
    },
    error::DecodeError,
};

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

#[derive(Clone, Copy)]
struct StreamCursor<'table> {
    reader: FastBitReader,
    bits: u64,
    window: *const u8,
    start: *const u8,
    output: *mut u8,
    remaining: usize,
    entries: &'table [HuffmanDecodeEntry; MAX_HUFFMAN_TABLE_SIZE],
    index_shift: u32,
    window_moved: bool,
}

impl<'table> StreamCursor<'table> {
    unsafe fn new_unchecked(
        stream: &[u8],
        padding_buffer: &mut [u8; 16],
        table: &'table HuffmanDecodeTable,
        output: *mut u8,
        length: usize,
    ) -> Result<Self, DecodeError> {
        debug_assert!(table.is_ready);
        let reader = unsafe { make_reader_unchecked(stream, padding_buffer)? };
        let (window, start, bits_consumed) = reader.window();
        let bits = if bits_consumed < 64 {
            (unsafe { window.cast::<u64>().read_unaligned() }.to_le() | 1) << bits_consumed
        } else {
            1u64 << 63
        };
        Ok(Self {
            reader,
            bits,
            window,
            start,
            output,
            remaining: length,
            entries: &table.entries,
            index_shift: 64 - (table.max_bits as u32).max(1),
            window_moved: false,
        })
    }

    fn safe_rounds(&self) -> usize {
        let input_room = unsafe { self.window.offset_from(self.start) } as usize;
        let consumed_bytes = (self.bits.trailing_zeros() >> 3) as usize;
        (input_room.saturating_sub(consumed_bytes) / 7).min(self.remaining / 5)
    }

    #[inline(always)]
    unsafe fn decode_round_unchecked(&mut self) {
        debug_assert!(self.remaining >= 5);
        let mut bits = self.bits;
        for symbol_index in 0..5 {
            let entry = unsafe {
                *self
                    .entries
                    .get_unchecked((bits >> self.index_shift) as usize)
            };
            unsafe {
                *self.output.add(symbol_index) = entry.symbol;
            }
            bits <<= entry.bit_count;
        }
        self.output = unsafe { self.output.add(5) };
        self.remaining -= 5;
        let trailing = bits.trailing_zeros();
        self.window = unsafe { self.window.sub((trailing >> 3) as usize) };
        self.bits =
            (unsafe { self.window.cast::<u64>().read_unaligned() }.to_le() | 1) << (trailing & 7);
        self.window_moved = true;
    }

    unsafe fn finish_unchecked(mut self) -> bool {
        debug_assert!(self.window >= self.start);
        if self.window_moved {
            unsafe {
                self.reader
                    .move_window_unchecked(self.window, self.bits.trailing_zeros());
            }
        }
        let width = 64 - self.index_shift;
        while self.remaining > 0 {
            unsafe {
                let _ = self.reader.refill_unchecked();
            }
            let index = peek_window_padded(&self.reader, width) as usize;
            let entry = unsafe { *self.entries.get_unchecked(index) };
            unsafe {
                *self.output = entry.symbol;
                self.output = self.output.add(1);
            }
            self.reader.skip(entry.bit_count as u32);
            self.remaining -= 1;
        }
        self.reader.is_finished()
    }
}

unsafe fn decode_rounds_unchecked<const STREAMS: usize>(cursors: &mut [StreamCursor; STREAMS]) {
    debug_assert!(STREAMS > 0);
    loop {
        let rounds = cursors
            .iter()
            .map(|cursor| cursor.safe_rounds())
            .min()
            .unwrap_or(0);
        if rounds == 0 {
            return;
        }
        for _ in 0..rounds {
            for cursor in cursors.iter_mut() {
                unsafe {
                    cursor.decode_round_unchecked();
                }
            }
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

unsafe fn start_section_unchecked<'table>(
    streams: &[&[u8]],
    padding_buffers: &mut [[u8; 16]],
    table: &'table HuffmanDecodeTable,
    output: *mut u8,
    segment_sizes: &[usize],
    cursors: &mut [Option<StreamCursor<'table>>],
) -> Result<(), DecodeError> {
    debug_assert!(streams.len() == segment_sizes.len() && streams.len() <= cursors.len());
    let mut offset = 0usize;
    for stream in 0..streams.len() {
        cursors[stream] = Some(unsafe {
            StreamCursor::new_unchecked(
                streams[stream],
                &mut padding_buffers[stream],
                table,
                output.add(offset),
                segment_sizes[stream],
            )?
        });
        offset += segment_sizes[stream];
    }
    Ok(())
}

unsafe fn finish_cursors_unchecked(cursors: &[StreamCursor]) -> Result<(), DecodeError> {
    debug_assert!(!cursors.is_empty());
    let mut all_finished = true;
    for cursor in cursors {
        all_finished &= unsafe { cursor.finish_unchecked() };
    }
    if all_finished {
        Ok(())
    } else {
        Err(DecodeError::CorruptBitstream)
    }
}

pub(crate) unsafe fn decode_four_streams_unchecked(
    streams: [&[u8]; 4],
    table: &HuffmanDecodeTable,
    output: *mut u8,
    segment_sizes: [usize; 4],
) -> Result<(), DecodeError> {
    let mut padding_buffers = [[0u8; 16]; 4];
    let mut cursors = [None; 4];
    unsafe {
        start_section_unchecked(
            &streams,
            &mut padding_buffers,
            table,
            output,
            &segment_sizes,
            &mut cursors,
        )?;
    }
    let mut cursors = cursors.map(|cursor| cursor.unwrap());
    unsafe {
        decode_rounds_unchecked(&mut cursors);
        finish_cursors_unchecked(&cursors)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn decode_two_sections_unchecked(
    first_streams: [&[u8]; 4],
    first_table: &HuffmanDecodeTable,
    first_output: *mut u8,
    first_segment_sizes: [usize; 4],
    second_streams: [&[u8]; 4],
    second_table: &HuffmanDecodeTable,
    second_output: *mut u8,
    second_segment_sizes: [usize; 4],
) -> Result<(), DecodeError> {
    debug_assert!(first_table.is_ready && second_table.is_ready);
    let mut padding_buffers = [[0u8; 16]; 8];
    let mut cursors = [None; 8];
    let (first_padding, second_padding) = padding_buffers.split_at_mut(4);
    let (first_cursors, second_cursors) = cursors.split_at_mut(4);
    unsafe {
        start_section_unchecked(
            &first_streams,
            first_padding,
            first_table,
            first_output,
            &first_segment_sizes,
            first_cursors,
        )?;
        start_section_unchecked(
            &second_streams,
            second_padding,
            second_table,
            second_output,
            &second_segment_sizes,
            second_cursors,
        )?;
    }
    let mut cursors = cursors.map(|cursor| cursor.unwrap());
    unsafe {
        decode_rounds_unchecked(&mut cursors);
    }
    let mut first_group = [cursors[0], cursors[1], cursors[2], cursors[3]];
    let mut second_group = [cursors[4], cursors[5], cursors[6], cursors[7]];
    unsafe {
        decode_rounds_unchecked(&mut first_group);
        decode_rounds_unchecked(&mut second_group);
        finish_cursors_unchecked(&first_group)?;
        finish_cursors_unchecked(&second_group)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::{
        fse_decode_table::FseDecodeTable,
        histogram::count_symbols,
        huffman_decode::decode_four_streams,
        huffman_encode::encode_four_streams,
        huffman_encode_table::{HuffmanEncodeTable, build_huffman_encode_table},
    };

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
        build_huffman_encode_table(counts, &mut encode_table, crate::entropy::huffman_decode_table::MAX_HUFFMAN_BITS).ok()?;

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
        regenerated_size: usize,
    ) -> Result<Vec<u8>, DecodeError> {
        let mut output = vec![0u8; regenerated_size + 32];
        let (streams, segments) =
            crate::entropy::huffman_decode::split_four_streams(encoded, regenerated_size)?;
        unsafe { decode_four_streams_unchecked(streams, table, output.as_mut_ptr(), segments)? };
        output.truncate(regenerated_size);
        Ok(output)
    }

    #[test]
    fn fast_path_matches_checked_path_on_random_tables_and_inputs() {
        let mut random = XorshiftRandom::new(0x9E3779B97F4A7C15);

        for _ in 0..500 {
            let (encode_table, decode_table) = build_tables(&mut random);
            let length = random.next_range(20000);
            if length < 16 {
                continue;
            }
            let text = random_text(&mut random, &encode_table, length);

            let mut counts = [0u32; 256];
            count_symbols(&text, &mut counts);

            let mut encoded = vec![0u8; length * 2 + 4096];
            let bytes_written = match encode_four_streams(&text, &encode_table, &mut encoded) {
                Ok(bytes_written) => bytes_written,
                Err(_) => continue,
            };
            let encoded = &encoded[..bytes_written];

            let mut checked_output = vec![0u8; length];
            let checked_result = decode_four_streams(encoded, &decode_table, &mut checked_output);

            let fast_result = decode_fast(encoded, &decode_table, length);

            match (checked_result, fast_result) {
                (Ok(()), Ok(fast_output)) => assert_eq!(checked_output, fast_output),
                (Err(checked_error), Err(fast_error)) => assert_eq!(checked_error, fast_error),
                (checked, fast) => panic!(
                    "fast and checked paths disagreed: checked={checked:?} fast={fast:?} length={length}"
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
        let bytes_written = encode_four_streams(&text, &encode_table, &mut encoded).unwrap();
        let truncated = &encoded[..bytes_written - 1];

        let mut checked_output = vec![0u8; length];
        let checked_result = decode_four_streams(truncated, &decode_table, &mut checked_output);
        let fast_result = decode_fast(truncated, &decode_table, length);

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

            for &segment_length in &segment_lengths {
                let length = segment_length * 4;
                let text = random_text(&mut random, &encode_table, length);

                let mut encoded = vec![0u8; length * 2 + 4096];
                let bytes_written = match encode_four_streams(&text, &encode_table, &mut encoded) {
                    Ok(bytes_written) => bytes_written,
                    Err(_) => continue,
                };
                let encoded = &encoded[..bytes_written];

                let mut checked_output = vec![0u8; length];
                let checked_result =
                    decode_four_streams(encoded, &decode_table, &mut checked_output);

                let fast_result = decode_fast(encoded, &decode_table, length);

                match (checked_result, fast_result) {
                    (Ok(()), Ok(fast_output)) => assert_eq!(checked_output, fast_output),
                    (Err(checked_error), Err(fast_error)) => {
                        assert_eq!(checked_error, fast_error)
                    }
                    (checked, fast) => panic!(
                        "fast and checked paths disagreed: checked={checked:?} fast={fast:?} max_bits={wanted_max_bits} segment_length={segment_length}"
                    ),
                }
            }
        }
    }
}
