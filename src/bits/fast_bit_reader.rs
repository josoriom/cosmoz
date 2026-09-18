use crate::error::DecodeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReloadStatus {
    Unfinished,
    EndOfBuffer,
    Completed,
    Overflow,
}

#[derive(Clone, Copy)]
pub(crate) struct FastBitReader {
    container: u64,
    bits_consumed: u32,
    position: *const u8,
    start: *const u8,
    limit: *const u8,
}

impl FastBitReader {
    pub(crate) unsafe fn new_unchecked(input: &[u8]) -> Result<Self, DecodeError> {
        debug_assert!(input.len() >= 8);
        let last_byte = input[input.len() - 1];
        if last_byte == 0 {
            return Err(DecodeError::CorruptBitstream);
        }
        let padding_bit_position = 7 - last_byte.leading_zeros();
        let bits_consumed = 8 - padding_bit_position;
        let start = input.as_ptr();
        let position = unsafe { start.add(input.len() - 8) };
        let limit = unsafe { start.add(8) };
        let container = unsafe { read_unaligned_le_u64_unchecked(position) };
        Ok(Self {
            container,
            bits_consumed,
            position,
            start,
            limit,
        })
    }

    pub(crate) unsafe fn new_padded_unchecked(
        input: &[u8],
        buffer: &mut [u8; 16],
    ) -> Result<Self, DecodeError> {
        debug_assert!(!input.is_empty() && input.len() < 8);
        let last_byte = input[input.len() - 1];
        if last_byte == 0 {
            return Err(DecodeError::CorruptBitstream);
        }
        let padding_bit_position = 7 - last_byte.leading_zeros();
        for byte in buffer.iter_mut() {
            *byte = 0;
        }
        buffer[..input.len()].copy_from_slice(input);
        let start = buffer.as_ptr();
        let position = start;
        let limit = unsafe { start.add(8) };
        let container = unsafe { read_unaligned_le_u64_unchecked(position) };
        let phantom_bits = (8 - input.len() as u32) * 8;
        let bits_consumed = 8 - padding_bit_position + phantom_bits;
        Ok(Self {
            container,
            bits_consumed,
            position,
            start,
            limit,
        })
    }

    #[inline(always)]
    pub(crate) fn peek(&self, count: u32) -> u64 {
        debug_assert!(count <= 56);
        let shift = 64u32.wrapping_sub(self.bits_consumed).wrapping_sub(count) & 63;
        let mask = (1u64 << count).wrapping_sub(1);
        (self.container >> shift) & mask
    }

    #[inline(always)]
    pub(crate) fn skip(&mut self, count: u32) {
        self.bits_consumed += count;
    }

    #[inline(always)]
    pub(crate) fn read(&mut self, count: u32) -> u64 {
        let value = self.peek(count);
        self.skip(count);
        value
    }

    #[inline(always)]
    pub(crate) unsafe fn refill_unchecked(&mut self) -> ReloadStatus {
        if self.bits_consumed > 64 {
            return ReloadStatus::Overflow;
        }
        if self.position >= self.limit {
            let consumed_bytes = self.bits_consumed >> 3;
            self.position = unsafe { self.position.sub(consumed_bytes as usize) };
            self.container = unsafe { read_unaligned_le_u64_unchecked(self.position) };
            self.bits_consumed &= 7;
            return ReloadStatus::Unfinished;
        }
        if self.position == self.start {
            return if self.bits_consumed < 64 {
                ReloadStatus::EndOfBuffer
            } else {
                ReloadStatus::Completed
            };
        }
        let available = unsafe { self.position.offset_from(self.start) } as u32;
        let mut consumed_bytes = self.bits_consumed >> 3;
        let mut status = ReloadStatus::Unfinished;
        if consumed_bytes > available {
            consumed_bytes = available;
            status = ReloadStatus::EndOfBuffer;
        }
        self.position = unsafe { self.position.sub(consumed_bytes as usize) };
        self.bits_consumed -= consumed_bytes * 8;
        self.container = unsafe { read_unaligned_le_u64_unchecked(self.position) };
        status
    }

    pub(crate) fn window(&self) -> (*const u8, *const u8, u32) {
        (self.position, self.start, self.bits_consumed)
    }

    pub(crate) unsafe fn move_window_unchecked(&mut self, position: *const u8, bits_consumed: u32) {
        debug_assert!(position >= self.start && bits_consumed <= 64);
        self.position = position;
        self.container = unsafe { read_unaligned_le_u64_unchecked(position) };
        self.bits_consumed = bits_consumed;
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.position == self.start && self.bits_consumed == 64
    }

    pub(crate) fn bits_left(&self) -> usize {
        let bytes_available = unsafe { self.position.offset_from(self.start) } as usize;
        let container_bits = 64u32.saturating_sub(self.bits_consumed) as usize;
        bytes_available * 8 + container_bits
    }
}

#[inline(always)]
unsafe fn read_unaligned_le_u64_unchecked(pointer: *const u8) -> u64 {
    debug_assert!(!pointer.is_null());
    let bytes = unsafe { pointer.cast::<[u8; 8]>().read_unaligned() };
    u64::from_le_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::backward_bit_reader::BackwardBitReader;

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

    #[test]
    fn eight_byte_input_with_padding_at_top_bit_matches_backward_reader() {
        let input = [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF1];
        let mut checked = BackwardBitReader::new(&input).unwrap();
        let fast = unsafe { FastBitReader::new_unchecked(&input).unwrap() };
        assert_eq!(fast.bits_left(), checked.bits_left());
        assert_eq!(fast.peek(3), checked.peek_bits(3));
        assert_eq!(fast.peek(56), checked.peek_bits(56));
        let _ = checked.read_bits(3);
    }

    #[test]
    fn short_input_through_new_padded_unchecked_matches_backward_reader() {
        let input = [0b1010_0000u8];
        let mut buffer = [0u8; 16];
        let checked = BackwardBitReader::new(&input).unwrap();
        let fast = unsafe { FastBitReader::new_padded_unchecked(&input, &mut buffer).unwrap() };
        assert_eq!(fast.bits_left(), checked.bits_left());
        assert_eq!(fast.peek(3), checked.peek_bits(3));
    }

    #[test]
    fn last_byte_of_zero_is_rejected_as_corrupt_bitstream() {
        let input = [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let result = unsafe { FastBitReader::new_unchecked(&input) };
        assert!(matches!(result, Err(DecodeError::CorruptBitstream)));

        let short_input = [0x00u8];
        let mut buffer = [0u8; 16];
        let short_result =
            unsafe { FastBitReader::new_padded_unchecked(&short_input, &mut buffer) };
        assert!(matches!(short_result, Err(DecodeError::CorruptBitstream)));
    }

    #[test]
    fn is_finished_agrees_with_backward_reader_after_consuming_everything() {
        let input = [0xFFu8, 0x01];
        let mut buffer = [0u8; 16];
        let mut checked = BackwardBitReader::new(&input).unwrap();
        let mut fast = unsafe { FastBitReader::new_padded_unchecked(&input, &mut buffer).unwrap() };
        let _ = fast.read(8);
        let _ = checked.read_bits(8);
        assert_eq!(fast.is_finished(), checked.is_finished());
        assert!(fast.is_finished());
    }

    fn build_random_input(random: &mut XorshiftRandom, length: usize) -> Vec<u8> {
        let mut input = vec![0u8; length];
        for byte in input.iter_mut() {
            *byte = random.next_range(256) as u8;
        }
        let last_index = length - 1;
        if input[last_index] == 0 {
            input[last_index] = 1;
        }
        input
    }

    #[test]
    fn differential_test_matches_backward_bit_reader_over_random_operation_sequences() {
        let mut random = XorshiftRandom::new(0xD1B54A32D192ED03);
        let mut buffer = [0u8; 16];

        for _ in 0..100_000 {
            let length = 1 + random.next_range(64);
            let input = build_random_input(&mut random, length);

            let checked_result = BackwardBitReader::new(&input);
            let fast_result = if length >= 8 {
                unsafe { FastBitReader::new_unchecked(&input) }
            } else {
                unsafe { FastBitReader::new_padded_unchecked(&input, &mut buffer) }
            };

            let (mut checked, mut fast) = match (checked_result, fast_result) {
                (Ok(checked), Ok(fast)) => (checked, fast),
                (Err(_), Err(_)) => continue,
                _ => panic!("checked reader and fast reader disagreed on construction"),
            };

            assert_eq!(fast.bits_left(), checked.bits_left());

            let operation_count = 1 + random.next_range(30);
            for _ in 0..operation_count {
                if checked.has_overflowed() {
                    break;
                }
                let status = unsafe { fast.refill_unchecked() };
                checked.refill();
                match status {
                    ReloadStatus::Overflow => {
                        assert!(checked.has_overflowed());
                        break;
                    }
                    ReloadStatus::Completed => assert!(checked.is_finished()),
                    ReloadStatus::EndOfBuffer | ReloadStatus::Unfinished => {
                        assert!(!checked.is_finished())
                    }
                }
                assert_eq!(fast.bits_left(), checked.bits_left());

                let operation = random.next_range(2);
                let count = random.next_range(33) as u32;
                let bits_left = fast.bits_left();
                match operation {
                    0 => {
                        if (count as usize) <= bits_left {
                            let fast_value = fast.peek(count);
                            let checked_value = checked.peek_bits(count as usize);
                            assert_eq!(fast_value, checked_value);
                        }
                    }
                    _ => {
                        fast.skip(count);
                        checked.skip_bits(count as usize);
                    }
                }
                if checked.has_overflowed() {
                    break;
                }
                assert_eq!(fast.bits_left(), checked.bits_left());
            }
        }
    }

    struct TinyFseEntry {
        symbol: u8,
        bit_count: u32,
        next_state_base: u32,
    }

    fn tiny_fse_table() -> [TinyFseEntry; 4] {
        [
            TinyFseEntry {
                symbol: 0,
                bit_count: 1,
                next_state_base: 0,
            },
            TinyFseEntry {
                symbol: 1,
                bit_count: 1,
                next_state_base: 2,
            },
            TinyFseEntry {
                symbol: 0,
                bit_count: 2,
                next_state_base: 0,
            },
            TinyFseEntry {
                symbol: 1,
                bit_count: 0,
                next_state_base: 3,
            },
        ]
    }

    fn decode_with_backward_reader(
        input: &[u8],
        table: &[TinyFseEntry; 4],
        symbol_count: usize,
    ) -> Vec<u8> {
        let mut reader = BackwardBitReader::new(input).unwrap();
        let mut state = reader.read_bits(2) as usize;
        let mut symbols = Vec::with_capacity(symbol_count);
        for _ in 0..symbol_count {
            symbols.push(table[state].symbol);
            let bits = reader.read_bits(table[state].bit_count as usize) as usize;
            state = table[state].next_state_base as usize + bits;
        }
        symbols
    }

    fn decode_with_fast_reader(
        input: &[u8],
        table: &[TinyFseEntry; 4],
        symbol_count: usize,
        buffer: &mut [u8; 16],
    ) -> Vec<u8> {
        let mut reader = if input.len() >= 8 {
            unsafe { FastBitReader::new_unchecked(input).unwrap() }
        } else {
            unsafe { FastBitReader::new_padded_unchecked(input, buffer).unwrap() }
        };
        let mut state = reader.read(2) as usize;
        let mut symbols = Vec::with_capacity(symbol_count);
        for i in 0..symbol_count {
            symbols.push(table[state].symbol);
            let bit_count = table[state].bit_count;
            let bits = reader.read(bit_count) as usize;
            state = table[state].next_state_base as usize + bits;
            if i % 4 == 3 {
                unsafe {
                    let _ = reader.refill_unchecked();
                }
            }
        }
        symbols
    }

    #[test]
    fn tiny_fse_state_loop_matches_between_fast_and_backward_readers() {
        let table = tiny_fse_table();
        let mut random = XorshiftRandom::new(0xA5A5A5A5A5A5A5A5);

        for _ in 0..200 {
            let length = 8 + random.next_range(56);
            let input = build_random_input(&mut random, length);
            let mut buffer = [0u8; 16];

            let expected = decode_with_backward_reader(&input, &table, 32);
            let actual = decode_with_fast_reader(&input, &table, 32, &mut buffer);
            assert_eq!(expected, actual);
        }
    }
}
