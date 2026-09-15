use crate::simd::histogram::count_symbols as count_symbols_kernel;

pub fn count_symbols(input: &[u8], counts: &mut [u32; 256]) {
    count_symbols_kernel(input, counts);
}

pub fn count_used_symbols(counts: &[u32; 256]) -> usize {
    counts.iter().filter(|&&count| count > 0).count()
}

pub fn find_largest_symbol(counts: &[u32; 256]) -> usize {
    counts
        .iter()
        .enumerate()
        .rev()
        .find(|&(_, &count)| count > 0)
        .map(|(symbol, _)| symbol)
        .unwrap_or(0)
}

pub fn run_self_tests() -> Option<u32> {
    crate::simd::histogram::run_self_tests()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_every_byte_value_including_repeats() {
        let input = [0u8, 255, 255, 3, 3, 3];
        let mut counts = [7u32; 256];

        count_symbols(&input, &mut counts);

        assert_eq!(counts[0], 1);
        assert_eq!(counts[3], 3);
        assert_eq!(counts[255], 2);
        assert_eq!(counts[1], 0);
        assert_eq!(counts.iter().sum::<u32>(), input.len() as u32);
    }

    #[test]
    fn counts_used_symbols_and_finds_the_largest() {
        let mut counts = [0u32; 256];
        counts[5] = 2;
        counts[9] = 1;
        counts[200] = 4;

        assert_eq!(count_used_symbols(&counts), 3);
        assert_eq!(find_largest_symbol(&counts), 200);
    }
}
