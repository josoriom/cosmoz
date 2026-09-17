use crate::block::sequence_codes::{
    LITERAL_LENGTH_CODE_COUNT, MATCH_LENGTH_CODE_COUNT, OFFSET_CODE_COUNT, get_literal_length_code,
    get_literal_length_extra_bits, get_match_length_code, get_match_length_extra_bits,
};
use crate::frame::block_header::MAX_BLOCK_SIZE;

const BIT_COST_MULTIPLIER: u32 = 1 << BIT_COST_ACCURACY;
const BIT_COST_ACCURACY: u32 = 8;
const LITERAL_SYMBOL_COUNT: usize = 256;
const LITERAL_FREQUENCY_STEP: u32 = 2;
const PREDEFINED_STATISTICS_THRESHOLD: usize = 8;
const PREDEFINED_LITERAL_BITS: u32 = 6;
const PREDEFINED_OFFSET_BITS: u32 = 16;
const MATCH_PRICE_PENALTY: u32 = BIT_COST_MULTIPLIER / 5;
const FIRST_BLOCK_LITERAL_SHIFT: u32 = 8;
const LITERAL_SCALE_LOG: u32 = 12;
const SEQUENCE_SCALE_LOG: u32 = 11;
const MIN_MATCH_LENGTH: u32 = 3;

const BASE_LITERAL_LENGTH_FREQUENCIES: [u32; LITERAL_LENGTH_CODE_COUNT] = [
    4, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1,
];

const BASE_OFFSET_CODE_FREQUENCIES: [u32; OFFSET_CODE_COUNT] = [
    6, 2, 1, 1, 2, 3, 4, 4, 4, 3, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum PriceMode {
    Dynamic,
    Predefined,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ZeroCountBase {
    ZeroAllowed,
    OneGuaranteed,
}

pub struct SymbolPrices {
    literal_frequencies: [u32; LITERAL_SYMBOL_COUNT],
    literal_length_frequencies: [u32; LITERAL_LENGTH_CODE_COUNT],
    match_length_frequencies: [u32; MATCH_LENGTH_CODE_COUNT],
    offset_code_frequencies: [u32; OFFSET_CODE_COUNT],
    literal_sum: u32,
    literal_length_sum: u32,
    match_length_sum: u32,
    offset_code_sum: u32,
    literal_sum_price: u32,
    literal_length_sum_price: u32,
    match_length_sum_price: u32,
    offset_code_sum_price: u32,
    mode: PriceMode,
}

#[inline(always)]
fn highest_bit(value: u32) -> u32 {
    31 - value.leading_zeros()
}

#[inline(always)]
fn get_weight(frequency: u32) -> u32 {
    let stat = frequency + 1;
    let highest = highest_bit(stat);
    highest * BIT_COST_MULTIPLIER + ((stat << BIT_COST_ACCURACY) >> highest)
}

fn scale_down(table: &mut [u32], shift: u32, zero_count_base: ZeroCountBase) -> u32 {
    let mut sum = 0u32;
    for frequency in table.iter_mut() {
        let base = match zero_count_base {
            ZeroCountBase::OneGuaranteed => 1,
            ZeroCountBase::ZeroAllowed => (*frequency > 0) as u32,
        };
        *frequency = base + (*frequency >> shift);
        sum += *frequency;
    }
    sum
}

fn scale_to_target(table: &mut [u32], target_log: u32) -> u32 {
    let previous_sum: u32 = table.iter().sum();
    let factor = previous_sum >> target_log;
    if factor <= 1 {
        return previous_sum;
    }
    scale_down(table, highest_bit(factor), ZeroCountBase::OneGuaranteed)
}

impl Default for SymbolPrices {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolPrices {
    pub const fn new() -> Self {
        SymbolPrices {
            literal_frequencies: [0; LITERAL_SYMBOL_COUNT],
            literal_length_frequencies: [0; LITERAL_LENGTH_CODE_COUNT],
            match_length_frequencies: [0; MATCH_LENGTH_CODE_COUNT],
            offset_code_frequencies: [0; OFFSET_CODE_COUNT],
            literal_sum: 0,
            literal_length_sum: 0,
            match_length_sum: 0,
            offset_code_sum: 0,
            literal_sum_price: 0,
            literal_length_sum_price: 0,
            match_length_sum_price: 0,
            offset_code_sum_price: 0,
            mode: PriceMode::Dynamic,
        }
    }

    pub fn has_statistics(&self) -> bool {
        self.literal_length_sum != 0
    }

    pub fn prepare_for_block(&mut self, block: &[u8]) {
        self.mode = PriceMode::Dynamic;
        if self.has_statistics() {
            self.literal_sum = scale_to_target(&mut self.literal_frequencies, LITERAL_SCALE_LOG);
            self.literal_length_sum =
                scale_to_target(&mut self.literal_length_frequencies, SEQUENCE_SCALE_LOG);
            self.match_length_sum =
                scale_to_target(&mut self.match_length_frequencies, SEQUENCE_SCALE_LOG);
            self.offset_code_sum =
                scale_to_target(&mut self.offset_code_frequencies, SEQUENCE_SCALE_LOG);
        } else {
            if block.len() <= PREDEFINED_STATISTICS_THRESHOLD {
                self.mode = PriceMode::Predefined;
            }
            self.literal_frequencies = [0; LITERAL_SYMBOL_COUNT];
            crate::entropy::histogram::count_symbols(block, &mut self.literal_frequencies);
            self.literal_sum = scale_down(
                &mut self.literal_frequencies,
                FIRST_BLOCK_LITERAL_SHIFT,
                ZeroCountBase::ZeroAllowed,
            );
            self.literal_length_frequencies = BASE_LITERAL_LENGTH_FREQUENCIES;
            self.literal_length_sum = BASE_LITERAL_LENGTH_FREQUENCIES.iter().sum();
            self.match_length_frequencies = [1; MATCH_LENGTH_CODE_COUNT];
            self.match_length_sum = MATCH_LENGTH_CODE_COUNT as u32;
            self.offset_code_frequencies = BASE_OFFSET_CODE_FREQUENCIES;
            self.offset_code_sum = BASE_OFFSET_CODE_FREQUENCIES.iter().sum();
        }
        self.update_sum_prices();
    }

    pub fn update_sum_prices(&mut self) {
        self.literal_sum_price = get_weight(self.literal_sum);
        self.literal_length_sum_price = get_weight(self.literal_length_sum);
        self.match_length_sum_price = get_weight(self.match_length_sum);
        self.offset_code_sum_price = get_weight(self.offset_code_sum);
    }

    #[inline(always)]
    pub fn get_literal_price(&self, literal: u8) -> i32 {
        if self.mode == PriceMode::Predefined {
            return (PREDEFINED_LITERAL_BITS * BIT_COST_MULTIPLIER) as i32;
        }
        let max_literal_price = self.literal_sum_price - BIT_COST_MULTIPLIER;
        let literal_price =
            get_weight(self.literal_frequencies[literal as usize]).min(max_literal_price);
        (self.literal_sum_price - literal_price) as i32
    }

    #[inline(always)]
    pub fn get_literal_length_price(&self, literal_length: u32) -> i32 {
        if self.mode == PriceMode::Predefined {
            return get_weight(literal_length) as i32;
        }
        if literal_length as usize == MAX_BLOCK_SIZE {
            return BIT_COST_MULTIPLIER as i32 + self.get_literal_length_price(literal_length - 1);
        }
        let (code, _) = get_literal_length_code(literal_length);
        (get_literal_length_extra_bits(code) as u32 * BIT_COST_MULTIPLIER
            + self.literal_length_sum_price
            - get_weight(self.literal_length_frequencies[code as usize])) as i32
    }

    #[inline(always)]
    pub fn get_literal_length_increase_price(&self, literal_length: u32) -> i32 {
        self.get_literal_length_price(literal_length)
            - self.get_literal_length_price(literal_length - 1)
    }

    #[inline(always)]
    pub fn get_match_price(&self, offset_base: u32, match_length: u32) -> i32 {
        let offset_code = highest_bit(offset_base);
        if self.mode == PriceMode::Predefined {
            return (get_weight(match_length - MIN_MATCH_LENGTH)
                + (PREDEFINED_OFFSET_BITS + offset_code) * BIT_COST_MULTIPLIER)
                as i32;
        }
        let offset_price = offset_code * BIT_COST_MULTIPLIER + self.offset_code_sum_price
            - get_weight(self.offset_code_frequencies[offset_code as usize]);
        let (match_length_code, _) = get_match_length_code(match_length);
        let match_length_price = get_match_length_extra_bits(match_length_code) as u32
            * BIT_COST_MULTIPLIER
            + self.match_length_sum_price
            - get_weight(self.match_length_frequencies[match_length_code as usize]);
        (offset_price + match_length_price + MATCH_PRICE_PENALTY) as i32
    }

    pub fn record_sequence(&mut self, literals: &[u8], offset_base: u32, match_length: u32) {
        for &literal in literals {
            self.literal_frequencies[literal as usize] += LITERAL_FREQUENCY_STEP;
        }
        self.literal_sum += literals.len() as u32 * LITERAL_FREQUENCY_STEP;

        let (literal_length_code, _) = get_literal_length_code(literals.len() as u32);
        self.literal_length_frequencies[literal_length_code as usize] += 1;
        self.literal_length_sum += 1;

        self.offset_code_frequencies[highest_bit(offset_base) as usize] += 1;
        self.offset_code_sum += 1;

        let (match_length_code, _) = get_match_length_code(match_length);
        self.match_length_frequencies[match_length_code as usize] += 1;
        self.match_length_sum += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weight_matches_libzstd_fractional_weight() {
        assert_eq!(get_weight(0), 256);
        assert_eq!(get_weight(1), 256 + 256);
        assert_eq!(get_weight(2), 256 + 384);
        assert_eq!(get_weight(254), 7 * 256 + 510);
    }

    #[test]
    fn frequent_literals_cost_less_than_rare_literals() {
        let mut block = vec![b'a'; 4000];
        block.extend_from_slice(b"xyz");
        let mut prices = SymbolPrices::new();
        prices.prepare_for_block(&block);
        assert!(prices.get_literal_price(b'a') < prices.get_literal_price(b'x'));
        assert!(prices.get_literal_price(b'x') < prices.get_literal_price(b'q'));
        assert!(prices.has_statistics());
    }
}
