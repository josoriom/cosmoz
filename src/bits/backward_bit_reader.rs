use crate::error::DecodeError;

pub struct BackwardBitReader<'input> {
    input: &'input [u8],
    bits_left: usize,
    has_overflowed: bool,
}

impl<'input> BackwardBitReader<'input> {
    pub fn new(input: &'input [u8]) -> Result<Self, DecodeError> {
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
        if count <= self.bits_left {
            extract_bits_ending_at(self.input, self.bits_left, count)
        } else {
            let available = self.bits_left;
            let value = extract_bits_ending_at(self.input, available, available);
            value << (count - available)
        }
    }

    pub fn skip_bits(&mut self, count: usize) {
        if count <= self.bits_left {
            self.bits_left -= count;
        } else {
            self.bits_left = 0;
            self.has_overflowed = true;
        }
    }

    pub fn bits_left(&self) -> usize {
        self.bits_left
    }

    pub fn is_finished(&self) -> bool {
        self.bits_left == 0 && !self.has_overflowed
    }

    pub fn has_overflowed(&self) -> bool {
        self.has_overflowed
    }
}

fn find_padding_bit_position(last_byte: u8) -> Option<usize> {
    if last_byte == 0 {
        return None;
    }
    Some(7 - last_byte.leading_zeros() as usize)
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
}
