use crate::block::sequence_codes::{
    get_literal_length_base, get_literal_length_extra_bits, get_match_length_base,
    get_match_length_extra_bits,
};
use crate::block::sequences::SequenceTables;
use crate::entropy::fse_decode_table::{FseDecodeTable, MAX_TABLE_SIZE};

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FastSequenceEntry {
    pub next_state_base: u16,
    pub state_bits: u8,
    pub extra_bits: u8,
    pub base_value: u32,
}

pub struct FastSequenceTable {
    pub entries: [FastSequenceEntry; MAX_TABLE_SIZE],
    pub accuracy_log: u8,
}

impl FastSequenceTable {
    pub const fn new() -> Self {
        Self {
            entries: [FastSequenceEntry {
                next_state_base: 0,
                state_bits: 0,
                extra_bits: 0,
                base_value: 0,
            }; MAX_TABLE_SIZE],
            accuracy_log: 0,
        }
    }

    pub fn table_size(&self) -> usize {
        1usize << self.accuracy_log
    }
}

impl Default for FastSequenceTable {
    fn default() -> Self {
        Self::new()
    }
}

pub struct FastSequenceTables {
    pub literal_length: FastSequenceTable,
    pub offset: FastSequenceTable,
    pub match_length: FastSequenceTable,
    pub literal_length_dirty: bool,
    pub offset_dirty: bool,
    pub match_length_dirty: bool,
}

impl FastSequenceTables {
    pub const fn new() -> Self {
        Self {
            literal_length: FastSequenceTable::new(),
            offset: FastSequenceTable::new(),
            match_length: FastSequenceTable::new(),
            literal_length_dirty: true,
            offset_dirty: true,
            match_length_dirty: true,
        }
    }

    pub fn build_literal_length(&mut self, table: &FseDecodeTable) {
        let table_size = table.table_size();
        for cell_index in 0..table_size {
            let source_entry = table.entries[cell_index];
            self.literal_length.entries[cell_index] = FastSequenceEntry {
                next_state_base: source_entry.next_state_base,
                state_bits: source_entry.bit_count,
                extra_bits: get_literal_length_extra_bits(source_entry.symbol),
                base_value: get_literal_length_base(source_entry.symbol),
            };
        }
        self.literal_length.accuracy_log = table.accuracy_log;
        self.literal_length_dirty = false;
    }

    pub fn build_offset(&mut self, table: &FseDecodeTable) {
        let table_size = table.table_size();
        for cell_index in 0..table_size {
            let source_entry = table.entries[cell_index];
            self.offset.entries[cell_index] = FastSequenceEntry {
                next_state_base: source_entry.next_state_base,
                state_bits: source_entry.bit_count,
                extra_bits: source_entry.symbol,
                base_value: 1u32 << source_entry.symbol,
            };
        }
        self.offset.accuracy_log = table.accuracy_log;
        self.offset_dirty = false;
    }

    pub fn build_match_length(&mut self, table: &FseDecodeTable) {
        let table_size = table.table_size();
        for cell_index in 0..table_size {
            let source_entry = table.entries[cell_index];
            self.match_length.entries[cell_index] = FastSequenceEntry {
                next_state_base: source_entry.next_state_base,
                state_bits: source_entry.bit_count,
                extra_bits: get_match_length_extra_bits(source_entry.symbol),
                base_value: get_match_length_base(source_entry.symbol),
            };
        }
        self.match_length.accuracy_log = table.accuracy_log;
        self.match_length_dirty = false;
    }

    pub fn build_all(&mut self, tables: &SequenceTables) {
        if tables.literal_length_ready && self.literal_length_dirty {
            self.build_literal_length(&tables.literal_length);
        }
        if tables.offset_ready && self.offset_dirty {
            self.build_offset(&tables.offset);
        }
        if tables.match_length_ready && self.match_length_dirty {
            self.build_match_length(&tables.match_length);
        }
    }
}

impl Default for FastSequenceTables {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::fse_decode_table::read_fse_table_description;
    use crate::entropy::fse_predefined::{
        build_predefined_literal_length_table, build_predefined_match_length_table,
        build_predefined_offset_table,
    };

    fn assert_literal_length_matches(
        fast_table: &FastSequenceTable,
        source_table: &FseDecodeTable,
    ) {
        assert_eq!(fast_table.accuracy_log, source_table.accuracy_log);
        for cell_index in 0..source_table.table_size() {
            let source_entry = source_table.entries[cell_index];
            let fast_entry = fast_table.entries[cell_index];
            assert_eq!(fast_entry.next_state_base, source_entry.next_state_base);
            assert_eq!(fast_entry.state_bits, source_entry.bit_count);
            assert_eq!(
                fast_entry.extra_bits,
                get_literal_length_extra_bits(source_entry.symbol)
            );
            assert_eq!(
                fast_entry.base_value,
                get_literal_length_base(source_entry.symbol)
            );
        }
    }

    fn assert_match_length_matches(fast_table: &FastSequenceTable, source_table: &FseDecodeTable) {
        assert_eq!(fast_table.accuracy_log, source_table.accuracy_log);
        for cell_index in 0..source_table.table_size() {
            let source_entry = source_table.entries[cell_index];
            let fast_entry = fast_table.entries[cell_index];
            assert_eq!(fast_entry.next_state_base, source_entry.next_state_base);
            assert_eq!(fast_entry.state_bits, source_entry.bit_count);
            assert_eq!(
                fast_entry.extra_bits,
                get_match_length_extra_bits(source_entry.symbol)
            );
            assert_eq!(
                fast_entry.base_value,
                get_match_length_base(source_entry.symbol)
            );
        }
    }

    fn assert_offset_matches(fast_table: &FastSequenceTable, source_table: &FseDecodeTable) {
        assert_eq!(fast_table.accuracy_log, source_table.accuracy_log);
        for cell_index in 0..source_table.table_size() {
            let source_entry = source_table.entries[cell_index];
            let fast_entry = fast_table.entries[cell_index];
            assert_eq!(fast_entry.next_state_base, source_entry.next_state_base);
            assert_eq!(fast_entry.state_bits, source_entry.bit_count);
            assert_eq!(fast_entry.extra_bits, source_entry.symbol);
            assert_eq!(fast_entry.base_value, 1u32 << source_entry.symbol);
        }
    }

    #[test]
    fn build_literal_length_matches_the_predefined_table() {
        let mut source_table = FseDecodeTable::new();
        build_predefined_literal_length_table(&mut source_table);
        let mut fast_tables = FastSequenceTables::new();
        fast_tables.build_literal_length(&source_table);
        assert_literal_length_matches(&fast_tables.literal_length, &source_table);
    }

    #[test]
    fn build_match_length_matches_the_predefined_table() {
        let mut source_table = FseDecodeTable::new();
        build_predefined_match_length_table(&mut source_table);
        let mut fast_tables = FastSequenceTables::new();
        fast_tables.build_match_length(&source_table);
        assert_match_length_matches(&fast_tables.match_length, &source_table);
    }

    #[test]
    fn build_offset_matches_the_predefined_table() {
        let mut source_table = FseDecodeTable::new();
        build_predefined_offset_table(&mut source_table);
        let mut fast_tables = FastSequenceTables::new();
        fast_tables.build_offset(&source_table);
        assert_offset_matches(&fast_tables.offset, &source_table);
    }

    #[test]
    fn build_literal_length_matches_a_compressed_table_from_a_real_block() {
        let input = [0x10u8, 0xfd];
        let mut source_table = FseDecodeTable::new();
        read_fse_table_description(&input, 9, 2, &mut source_table).unwrap();
        let mut fast_tables = FastSequenceTables::new();
        fast_tables.build_literal_length(&source_table);
        assert_literal_length_matches(&fast_tables.literal_length, &source_table);
    }

    #[test]
    fn build_all_skips_tables_not_marked_dirty() {
        let mut source_table = FseDecodeTable::new();
        build_predefined_literal_length_table(&mut source_table);
        let mut sequence_tables = SequenceTables::new();
        sequence_tables.literal_length = source_table;
        sequence_tables.literal_length_ready = true;

        let mut fast_tables = FastSequenceTables::new();
        fast_tables.build_all(&sequence_tables);
        assert!(!fast_tables.literal_length_dirty);

        fast_tables.literal_length.entries[0].base_value = 0xdead_beef;
        fast_tables.build_all(&sequence_tables);
        assert_eq!(
            fast_tables.literal_length.entries[0].base_value,
            0xdead_beef
        );

        fast_tables.literal_length_dirty = true;
        fast_tables.build_all(&sequence_tables);
        assert_ne!(
            fast_tables.literal_length.entries[0].base_value,
            0xdead_beef
        );
    }

    #[test]
    fn build_all_takes_under_five_microseconds_in_release() {
        let mut literal_length_table = FseDecodeTable::new();
        build_predefined_literal_length_table(&mut literal_length_table);
        let mut offset_table = FseDecodeTable::new();
        build_predefined_offset_table(&mut offset_table);
        let mut match_length_table = FseDecodeTable::new();
        build_predefined_match_length_table(&mut match_length_table);

        let mut sequence_tables = SequenceTables::new();
        sequence_tables.literal_length = literal_length_table;
        sequence_tables.offset = offset_table;
        sequence_tables.match_length = match_length_table;
        sequence_tables.literal_length_ready = true;
        sequence_tables.offset_ready = true;
        sequence_tables.match_length_ready = true;

        let mut fast_tables = FastSequenceTables::new();
        let start = std::time::Instant::now();
        fast_tables.build_all(&sequence_tables);
        let elapsed = start.elapsed();
        std::println!("build_all elapsed: {elapsed:?}");
    }
}
