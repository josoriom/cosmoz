use core::cell::Cell;

use crate::error::DecodeError;

pub struct BackwardBitReader<'input> {
    input: &'input [u8],
    container: Cell<u64>,
    bits_in_container: Cell<usize>,
    byte_position: Cell<usize>,
    has_overflowed: Cell<bool>,
}

impl<'input> BackwardBitReader<'input> {
    pub fn new(input: &'input [u8]) -> Result<Self, DecodeError> {
        let last_byte = *input.last().ok_or(DecodeError::InputTooShort)?;
        let padding_bit_position =
            find_padding_bit_position(last_byte).ok_or(DecodeError::CorruptBitstream)?;
        let total_length = input.len();
        let bytes_before_last_byte = total_length - 1;
        let bits_before_last_byte = bytes_before_last_byte
            .checked_mul(8)
            .ok_or(DecodeError::InputTooShort)?;
        let total_bits_left = bits_before_last_byte
            .checked_add(padding_bit_position)
            .ok_or(DecodeError::InputTooShort)?;

        let bytes_to_load = core::cmp::min(total_length, 8);
        let start_byte = total_length - bytes_to_load;
        let container = read_little_endian_word_from(input, start_byte);
        let bits_in_container = total_bits_left - start_byte * 8;

        Ok(Self {
            input,
            container: Cell::new(container),
            bits_in_container: Cell::new(bits_in_container),
            byte_position: Cell::new(start_byte),
            has_overflowed: Cell::new(false),
        })
    }

    pub fn read_bits(&mut self, count: usize) -> u64 {
        let value = self.peek_bits(count);
        self.skip_bits(count);
        value
    }

    pub fn peek_bits(&self, count: usize) -> u64 {
        debug_assert!(count <= 56);
        if count == 0 {
            return 0;
        }
        if count > self.bits_in_container.get() {
            self.load_more_bytes();
        }
        let bits_in_container = self.bits_in_container.get();
        let container = self.container.get();
        if count <= bits_in_container {
            let shift = bits_in_container - count;
            (container >> shift) & low_bits_mask(count)
        } else {
            let value = container & low_bits_mask(bits_in_container);
            value << (count - bits_in_container)
        }
    }

    pub fn skip_bits(&mut self, count: usize) {
        if count > self.bits_in_container.get() {
            self.load_more_bytes();
        }
        let bits_in_container = self.bits_in_container.get();
        if count <= bits_in_container {
            self.bits_in_container.set(bits_in_container - count);
        } else {
            self.bits_in_container.set(0);
            self.has_overflowed.set(true);
        }
    }

    pub fn refill(&mut self) {
        self.load_more_bytes();
    }

    pub fn bits_left(&self) -> usize {
        self.bits_in_container.get() + self.byte_position.get() * 8
    }

    pub fn is_finished(&self) -> bool {
        self.bits_left() == 0 && !self.has_overflowed.get()
    }

    pub fn has_overflowed(&self) -> bool {
        self.has_overflowed.get()
    }

    fn load_more_bytes(&self) {
        let byte_position = self.byte_position.get();
        if byte_position == 0 {
            return;
        }
        let bits_in_container = self.bits_in_container.get();
        if bits_in_container > 56 {
            return;
        }
        let capacity_bits = 64 - bits_in_container;
        let bytes_to_add = core::cmp::min(byte_position, capacity_bits / 8);
        if bytes_to_add == 0 {
            return;
        }
        let new_byte_position = byte_position - bytes_to_add;
        self.container
            .set(read_little_endian_word_from(self.input, new_byte_position));
        self.bits_in_container
            .set(bits_in_container + bytes_to_add * 8);
        self.byte_position.set(new_byte_position);
    }
}

fn find_padding_bit_position(last_byte: u8) -> Option<usize> {
    if last_byte == 0 {
        return None;
    }
    Some(7 - last_byte.leading_zeros() as usize)
}

fn read_little_endian_word_from(input: &[u8], start_byte: usize) -> u64 {
    let available_bytes = input.get(start_byte..).unwrap_or(&[]);
    if let Some(full_word) = available_bytes.first_chunk::<8>() {
        return u64::from_le_bytes(*full_word);
    }
    let mut word_bytes = [0u8; 8];
    word_bytes[..available_bytes.len()].copy_from_slice(available_bytes);
    u64::from_le_bytes(word_bytes)
}

fn low_bits_mask(count: usize) -> u64 {
    (1u64 << count) - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_byte_with_padding_at_top_bit_gives_seven_bits_left() {
        let mut reader = BackwardBitReader::new(&[0b1010_0000]).unwrap();
        assert_eq!(reader.bits_left(), 7);
        assert_eq!(reader.read_bits(3), 0b010);
    }

    #[test]
    fn two_bytes_with_padding_at_bottom_of_last_byte_gives_full_byte() {
        let mut reader = BackwardBitReader::new(&[0xFF, 0x01]).unwrap();
        assert_eq!(reader.bits_left(), 8);
        assert_eq!(reader.read_bits(8), 0xFF);
        assert!(reader.is_finished());
    }

    #[test]
    fn reads_crossing_byte_boundaries_and_a_fifty_six_bit_read_match_hand_computed_values() {
        let mut reader =
            BackwardBitReader::new(&[0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF1]).unwrap();
        assert_eq!(reader.bits_left(), 63);
        assert_eq!(reader.read_bits(3), 0b111);
        assert_eq!(reader.bits_left(), 60);
        assert_eq!(reader.read_bits(56), 0x1debc9a7856341);
        assert_eq!(reader.bits_left(), 4);
    }

    #[test]
    fn last_byte_of_zero_is_rejected_as_corrupt_bitstream() {
        let result = BackwardBitReader::new(&[0x01, 0x00]);
        assert!(matches!(result, Err(DecodeError::CorruptBitstream)));
    }

    #[test]
    fn empty_input_is_rejected_as_too_short() {
        let result = BackwardBitReader::new(&[]);
        assert!(matches!(result, Err(DecodeError::InputTooShort)));
    }

    #[test]
    fn reading_past_the_start_returns_zero_padded_low_bits_and_sets_overflow() {
        let mut reader = BackwardBitReader::new(&[0b0001_1010]).unwrap();
        assert_eq!(reader.bits_left(), 4);
        assert_eq!(reader.read_bits(6), 0b101000);
        assert!(reader.has_overflowed());
        assert!(!reader.is_finished());
    }

    #[test]
    fn peeking_past_the_start_does_not_set_overflow() {
        let reader = BackwardBitReader::new(&[0b0001_1010]).unwrap();
        assert_eq!(reader.peek_bits(6), 0b101000);
        assert!(!reader.has_overflowed());
    }

    #[test]
    fn refill_can_be_called_directly_without_changing_the_read_values() {
        let mut reader =
            BackwardBitReader::new(&[0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF1, 0x03])
                .unwrap();
        reader.refill();
        let bits_left_before = reader.bits_left();
        reader.read_bits(9);
        assert_eq!(bits_left_before - 9, reader.bits_left());
    }

    struct ReferenceReader<'input> {
        input: &'input [u8],
        bits_left: usize,
        has_overflowed: bool,
    }

    impl<'input> ReferenceReader<'input> {
        fn new(input: &'input [u8]) -> Result<Self, DecodeError> {
            let last_byte = *input.last().ok_or(DecodeError::InputTooShort)?;
            let padding_bit_position =
                find_padding_bit_position(last_byte).ok_or(DecodeError::CorruptBitstream)?;
            let bytes_before_last_byte = input.len() - 1;
            let bits_before_last_byte = bytes_before_last_byte
                .checked_mul(8)
                .ok_or(DecodeError::InputTooShort)?;
            let bits_left = bits_before_last_byte
                .checked_add(padding_bit_position)
                .ok_or(DecodeError::InputTooShort)?;
            Ok(Self {
                input,
                bits_left,
                has_overflowed: false,
            })
        }

        fn read_bits(&mut self, count: usize) -> u64 {
            let value = self.peek_bits(count);
            self.skip_bits(count);
            value
        }

        fn peek_bits(&self, count: usize) -> u64 {
            if count == 0 {
                return 0;
            }
            if count <= self.bits_left {
                extract_bits_ending_at(self.input, self.bits_left, count)
            } else {
                let available = self.bits_left;
                let value = extract_bits_ending_at(self.input, available, available);
                value << (count - available)
            }
        }

        fn skip_bits(&mut self, count: usize) {
            if count <= self.bits_left {
                self.bits_left -= count;
            } else {
                self.bits_left = 0;
                self.has_overflowed = true;
            }
        }

        fn bits_left(&self) -> usize {
            self.bits_left
        }

        fn has_overflowed(&self) -> bool {
            self.has_overflowed
        }
    }

    fn extract_bits_ending_at(input: &[u8], end_bit_exclusive: usize, count: usize) -> u64 {
        if count == 0 {
            return 0;
        }
        let start_bit = end_bit_exclusive - count;
        let start_byte = start_bit / 8;
        let bit_offset = start_bit % 8;
        let word = read_little_endian_word_from(input, start_byte);
        let shifted = word >> bit_offset;
        shifted & low_bits_mask(count)
    }

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
    fn differential_test_matches_reference_reader_over_random_operation_sequences() {
        let mut random = XorshiftRandom::new(0x9E3779B97F4A7C15);

        for _ in 0..10_000 {
            let length = 1 + random.next_range(40);
            let mut input = vec![0u8; length];
            for byte in input.iter_mut() {
                *byte = random.next_range(256) as u8;
            }
            let last_index = length - 1;
            if input[last_index] == 0 {
                input[last_index] = 1;
            }

            let new_reader = BackwardBitReader::new(&input);
            let reference_reader = ReferenceReader::new(&input);
            let (mut new_reader, mut reference_reader) = match (new_reader, reference_reader) {
                (Ok(new_reader), Ok(reference_reader)) => (new_reader, reference_reader),
                (Err(_), Err(_)) => continue,
                _ => panic!("new reader and reference reader disagreed on construction"),
            };

            assert_eq!(new_reader.bits_left(), reference_reader.bits_left());

            let operation_count = 1 + random.next_range(20);
            for _ in 0..operation_count {
                let count = random.next_range(33);
                let operation = random.next_range(3);
                match operation {
                    0 => {
                        let new_value = new_reader.read_bits(count);
                        let reference_value = reference_reader.read_bits(count);
                        assert_eq!(new_value, reference_value);
                    }
                    1 => {
                        let new_value = new_reader.peek_bits(count);
                        let reference_value = reference_reader.peek_bits(count);
                        assert_eq!(new_value, reference_value);
                    }
                    _ => {
                        new_reader.skip_bits(count);
                        reference_reader.skip_bits(count);
                    }
                }
                assert_eq!(new_reader.bits_left(), reference_reader.bits_left());
                assert_eq!(
                    new_reader.has_overflowed(),
                    reference_reader.has_overflowed()
                );
            }
        }
    }
}
