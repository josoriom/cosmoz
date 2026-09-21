use crate::block::sequences::TableMode;
use crate::{
    block::{
        block_writer::write_block,
        repeat_offsets::RepeatOffsets,
        sequence_record::{MAX_SEQUENCES_PER_BLOCK, SequenceRecord},
        sequence_writer::SequenceEncodeTables,
    },
    encode_error::EncodeError,
    entropy::{fse_encode_table::FseEncodeTable, huffman_encode_table::HuffmanEncodeTable},
    frame::{
        block_header::{BLOCK_HEADER_LENGTH, MAX_BLOCK_SIZE},
        frame_header::ZSTD_CHECKSUM_LENGTH,
        frame_writer::{MAX_FRAME_HEADER_LENGTH, write_frame_header},
    },
    levels::{
        AnyFinder, MatchFinder,
        level_table::{self, LevelParameters},
    },
};
#[cfg(feature = "checksum")]
use crate::{frame::frame_writer::write_checksum, hash::xxhash64};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CompressOptions {
    pub with_checksum: bool,
    pub level: u8,
}

impl CompressOptions {
    pub(crate) const fn zstd() -> Self {
        Self {
            with_checksum: cfg!(feature = "checksum"),
            level: 1,
        }
    }
}

pub(crate) const DEFAULT_COMPRESSION_LEVEL: u8 = 12;

impl Default for CompressOptions {
    fn default() -> Self {
        Self {
            level: DEFAULT_COMPRESSION_LEVEL,
            ..Self::zstd()
        }
    }
}

pub(crate) struct EncodeWorkspace {
    pub(crate) level: u8,
    pub(crate) level_parameters: LevelParameters,
    pub(crate) match_finder: AnyFinder<'static>,
    pub(crate) table_memory: alloc::boxed::Box<[u32]>,
    pub(crate) repeat_offsets: RepeatOffsets,
    pub(crate) sequences: [SequenceRecord; MAX_SEQUENCES_PER_BLOCK],
    pub(crate) literals: [u8; MAX_BLOCK_SIZE],
    pub(crate) block_scratch: [u8; MAX_BLOCK_SIZE + 1024],
    pub(crate) huffman_table: HuffmanEncodeTable,
    pub(crate) weight_fse_table: FseEncodeTable,
    pub(crate) sequence_tables: SequenceEncodeTables,
}

impl EncodeWorkspace {
    pub(crate) fn new() -> Self {
        let level_parameters = level_table::level_one_parameters();
        Self {
            level: level_table::DEFAULT_LEVEL,
            level_parameters,
            match_finder: AnyFinder::for_level(level_parameters),
            table_memory: alloc::vec::Vec::new().into_boxed_slice(),
            repeat_offsets: RepeatOffsets {
                first: 1,
                second: 4,
                third: 8,
            },
            sequences: [SequenceRecord {
                literal_length: 0,
                match_length: 0,
                offset_value: 0,
            }; MAX_SEQUENCES_PER_BLOCK],
            literals: [0u8; MAX_BLOCK_SIZE],
            block_scratch: [0u8; MAX_BLOCK_SIZE + 1024],
            huffman_table: HuffmanEncodeTable::new(),
            weight_fse_table: FseEncodeTable::new(),
            sequence_tables: SequenceEncodeTables::new(),
        }
    }
}

impl Default for EncodeWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

impl EncodeWorkspace {
    #[cfg(any(test, feature = "wasm-exports"))]
    pub(crate) fn new_boxed() -> alloc::boxed::Box<Self> {
        build_boxed_workspace(
            level_table::DEFAULT_LEVEL,
            level_table::level_one_parameters(),
        )
    }

    pub(crate) fn new_boxed_for_level(level: u8) -> Result<alloc::boxed::Box<Self>, EncodeError> {
        let level_parameters =
            level_table::get_level_parameters(level).ok_or(EncodeError::BadOptions)?;
        Ok(build_boxed_workspace(level, level_parameters))
    }
}

fn build_boxed_workspace(
    level: u8,
    level_parameters: LevelParameters,
) -> alloc::boxed::Box<EncodeWorkspace> {
    let layout = core::alloc::Layout::new::<EncodeWorkspace>();
    unsafe {
        let raw = alloc::alloc::alloc_zeroed(layout) as *mut EncodeWorkspace;
        if raw.is_null() {
            alloc::alloc::handle_alloc_error(layout);
        }
        write_initial_values_unchecked(raw, level, level_parameters);
        alloc::boxed::Box::from_raw(raw)
    }
}

pub(crate) unsafe fn write_initial_values_unchecked(
    target: *mut EncodeWorkspace,
    level: u8,
    level_parameters: LevelParameters,
) {
    unsafe {
        core::ptr::write(core::ptr::addr_of_mut!((*target).level), level);
        core::ptr::write(
            core::ptr::addr_of_mut!((*target).level_parameters),
            level_parameters,
        );

        let table_parameters = crate::levels::finder_tables::get_table_parameters_for_input(
            level,
            level_parameters,
            0,
        );
        let table_memory_length =
            crate::levels::finder_tables::table_memory_length(table_parameters);
        let mut table_memory: alloc::boxed::Box<[u32]> =
            alloc::vec![0u32; table_memory_length].into_boxed_slice();
        let match_finder =
            build_finder_over_tables(&mut table_memory, level, level_parameters, table_parameters);

        core::ptr::write(
            core::ptr::addr_of_mut!((*target).match_finder),
            match_finder,
        );

        core::ptr::write(
            core::ptr::addr_of_mut!((*target).table_memory),
            table_memory,
        );

        core::ptr::write(
            core::ptr::addr_of_mut!((*target).repeat_offsets),
            RepeatOffsets {
                first: 1,
                second: 4,
                third: 8,
            },
        );

        let sequence_tables = core::ptr::addr_of_mut!((*target).sequence_tables);
        core::ptr::write(
            core::ptr::addr_of_mut!((*sequence_tables).literal_length_mode),
            TableMode::Predefined,
        );
        core::ptr::write(
            core::ptr::addr_of_mut!((*sequence_tables).offset_mode),
            TableMode::Predefined,
        );
        core::ptr::write(
            core::ptr::addr_of_mut!((*sequence_tables).match_length_mode),
            TableMode::Predefined,
        );
    }
}

fn build_finder_over_tables(
    table_memory: &mut alloc::boxed::Box<[u32]>,
    level: u8,
    level_parameters: LevelParameters,
    table_parameters: LevelParameters,
) -> AnyFinder<'static> {
    let borrowed_memory: &'static mut [u32] =
        unsafe { core::slice::from_raw_parts_mut(table_memory.as_mut_ptr(), table_memory.len()) };
    let storage =
        crate::levels::finder_tables::split_table_storage(borrowed_memory, table_parameters);
    AnyFinder::for_level_with_storage(level, level_parameters, storage)
}

impl EncodeWorkspace {
    pub(crate) fn prepare_tables_for_input(
        &mut self,
        input_length: usize,
    ) -> Result<(), EncodeError> {
        let table_parameters = crate::levels::finder_tables::get_table_parameters_for_input(
            self.level,
            self.level_parameters,
            input_length,
        );
        let needed_length = crate::levels::finder_tables::table_memory_length(table_parameters);
        if self.table_memory.len() >= needed_length {
            return Ok(());
        }
        let mut memory = alloc::vec::Vec::new();
        memory
            .try_reserve_exact(needed_length)
            .map_err(|_| EncodeError::OutOfMemory)?;
        memory.resize(needed_length, 0u32);
        let mut table_memory = memory.into_boxed_slice();
        self.match_finder = build_finder_over_tables(
            &mut table_memory,
            self.level,
            self.level_parameters,
            table_parameters,
        );
        self.table_memory = table_memory;
        Ok(())
    }
}

#[cfg(test)]
mod new_boxed_tests {
    use super::*;
    use crate::algorithms::fast::HASH_TABLE_SIZE;

    #[test]
    fn new_boxed_matches_new_field_by_field() {
        let boxed_workspace = EncodeWorkspace::new_boxed();

        let large_stack_thread = std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(move || {
                let stack_workspace = EncodeWorkspace::new();

                assert_eq!(boxed_workspace.level, stack_workspace.level);
                assert_eq!(
                    boxed_workspace.level_parameters,
                    stack_workspace.level_parameters
                );
                assert_eq!(
                    boxed_workspace.match_finder.window_log(),
                    stack_workspace.match_finder.window_log()
                );
                assert_eq!(
                    boxed_workspace.table_memory.len(),
                    crate::levels::finder_tables::table_memory_length(
                        boxed_workspace.level_parameters
                    )
                );

                let boxed_finder = match &boxed_workspace.match_finder {
                    AnyFinder::Fast(finder) => finder,
                    _ => panic!("level 1 workspace must select Fast"),
                };
                let stack_finder = match &stack_workspace.match_finder {
                    AnyFinder::Fast(finder) => finder,
                    _ => panic!("level 1 workspace must select Fast"),
                };
                assert_eq!(boxed_finder.positions[0], stack_finder.positions[0]);
                assert_eq!(
                    boxed_finder.positions[HASH_TABLE_SIZE - 1],
                    stack_finder.positions[HASH_TABLE_SIZE - 1]
                );

                assert_eq!(
                    boxed_workspace.repeat_offsets.first,
                    stack_workspace.repeat_offsets.first
                );
                assert_eq!(
                    boxed_workspace.repeat_offsets.second,
                    stack_workspace.repeat_offsets.second
                );
                assert_eq!(
                    boxed_workspace.repeat_offsets.third,
                    stack_workspace.repeat_offsets.third
                );

                assert_eq!(
                    boxed_workspace.sequence_tables.literal_length_mode,
                    stack_workspace.sequence_tables.literal_length_mode
                );
                assert_eq!(
                    boxed_workspace.sequence_tables.offset_mode,
                    stack_workspace.sequence_tables.offset_mode
                );
                assert_eq!(
                    boxed_workspace.sequence_tables.match_length_mode,
                    stack_workspace.sequence_tables.match_length_mode
                );
            })
            .expect("failed to spawn large stack thread");

        large_stack_thread
            .join()
            .expect("comparison thread panicked");
    }
}

pub(crate) fn get_max_compressed_size(input_length: usize, options: &CompressOptions) -> usize {
    let checksum_size = if options.with_checksum {
        ZSTD_CHECKSUM_LENGTH
    } else {
        0
    };
    let block_count = if input_length == 0 {
        1
    } else {
        input_length.div_ceil(MAX_BLOCK_SIZE)
    };
    let blocks_size = input_length + block_count * BLOCK_HEADER_LENGTH;
    MAX_FRAME_HEADER_LENGTH + blocks_size + checksum_size
}

pub(crate) fn compress(
    input: &[u8],
    output: &mut [u8],
    options: &CompressOptions,
    workspace: &mut EncodeWorkspace,
) -> Result<usize, EncodeError> {
    if !level_table::SUPPORTED_LEVELS.contains(&options.level) {
        return Err(EncodeError::BadOptions);
    }
    if options.level != workspace.level {
        return Err(EncodeError::BadOptions);
    }
    #[cfg(not(feature = "checksum"))]
    if options.with_checksum {
        return Err(EncodeError::BadOptions);
    }

    let frame_parameters = crate::levels::finder_tables::get_table_parameters_for_input(
        workspace.level,
        workspace.level_parameters,
        input.len(),
    );
    let mut position = write_frame_header(
        output,
        Some(input.len() as u64),
        frame_parameters.window_log,
        options.with_checksum,
    )?;

    workspace.prepare_tables_for_input(input.len())?;
    workspace.match_finder.reset(input.len());
    workspace.repeat_offsets = RepeatOffsets::new();

    let blocks_output = output
        .get_mut(position..)
        .ok_or(EncodeError::OutputTooSmall)?;
    position += compress_blocks(input, blocks_output, workspace)?;

    #[cfg(feature = "checksum")]
    if options.with_checksum {
        let hash = xxhash64::hash_bytes(input, 0);
        let checksum_output = output
            .get_mut(position..)
            .ok_or(EncodeError::OutputTooSmall)?;
        position += write_checksum(checksum_output, hash)?;
    }

    Ok(position)
}

fn compress_blocks(
    input: &[u8],
    output: &mut [u8],
    workspace: &mut EncodeWorkspace,
) -> Result<usize, EncodeError> {
    if input.is_empty() {
        return write_block(input, 0, true, output, workspace);
    }

    let mut position = 0usize;
    let mut block_start = 0usize;
    let mut savings = 0i64;
    while block_start < input.len() {
        let block_length =
            find_next_block_length(&input[block_start..], savings, workspace.level_parameters);
        let block_end = block_start + block_length;
        let is_last = block_end == input.len();
        let block_output = output
            .get_mut(position..)
            .ok_or(EncodeError::OutputTooSmall)?;
        let written = write_block(
            &input[..block_end],
            block_start,
            is_last,
            block_output,
            workspace,
        )?;
        savings += block_length as i64 - written as i64;
        position += written;
        block_start = block_end;
    }

    Ok(position)
}

pub(crate) fn find_next_block_length(
    remaining: &[u8],
    savings: i64,
    level_parameters: LevelParameters,
) -> usize {
    match level_parameters.strategy {
        level_table::Strategy::Ultra2 => {
            crate::block::block_presplitter::find_block_length(remaining, savings)
        }
        level_table::Strategy::Fast | level_table::Strategy::Lazy2 => {
            remaining.len().min(MAX_BLOCK_SIZE)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write,
        path::PathBuf,
        process::{Command, Stdio},
    };

    use super::*;
    use crate::decoder::{DecodeWorkspace, decompress, get_decompressed_size};

    fn find_zstd_cli() -> Option<PathBuf> {
        let output = Command::new("zstd")
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()?;
        if output.success() {
            Some(PathBuf::from("zstd"))
        } else {
            None
        }
    }

    fn decompress_with_cli(frame: &[u8]) -> Vec<u8> {
        let Some(zstd_path) = find_zstd_cli() else {
            return Vec::new();
        };
        let mut command = Command::new(zstd_path);
        command.arg("-d").arg("-c").arg("-q");
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("failed to start zstd process");
        {
            let mut stdin = child.stdin.take().expect("failed to open zstd stdin");
            stdin
                .write_all(frame)
                .expect("failed to write to zstd stdin");
        }
        let output = child
            .wait_with_output()
            .expect("failed to read zstd output");
        if !output.status.success() {
            panic!(
                "zstd command failed with status {:?}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        output.stdout
    }

    fn build_deterministic_text(length: usize) -> Vec<u8> {
        let paragraph = "the quick brown fox jumps over the lazy dog while a sphinx of black \
        quartz judges its vow and pack my box with five dozen liquor jugs before the vexingly \
        quick daft zebras jump away. ";
        let mut text = Vec::with_capacity(length);
        let mut counter: u64 = 0;
        while text.len() < length {
            text.extend_from_slice(paragraph.as_bytes());
            text.extend_from_slice(counter.to_string().as_bytes());
            text.push(b' ');
            counter += 1;
        }
        text.truncate(length);
        text
    }

    #[test]
    fn raw_zstd_frame_decodes_with_cli() {
        if find_zstd_cli().is_none() {
            println!("zstd CLI not found on PATH, skipping test");
            return;
        }

        let text = build_deterministic_text(300 * 1024);
        let options = CompressOptions::zstd();
        let mut workspace = EncodeWorkspace::new_boxed();
        let mut output = vec![0u8; get_max_compressed_size(text.len(), &options)];
        let written = compress(&text, &mut output, &options, &mut workspace).unwrap();
        output.truncate(written);

        let decoded = decompress_with_cli(&output);
        assert_eq!(decoded, text);
    }

    #[test]
    fn empty_input_frame_round_trips() {
        let zstd_options = CompressOptions::zstd();
        let mut workspace = EncodeWorkspace::new_boxed();
        let mut zstd_output = vec![0u8; get_max_compressed_size(0, &zstd_options)];
        let zstd_written = compress(&[], &mut zstd_output, &zstd_options, &mut workspace).unwrap();
        zstd_output.truncate(zstd_written);

        let mut decode_workspace = DecodeWorkspace::new_boxed();
        let mut decoded = vec![0u8; 16];
        let zstd_decoded_length =
            decompress(&zstd_output, &mut decoded, &mut decode_workspace).unwrap();
        assert_eq!(zstd_decoded_length, 0);

        if find_zstd_cli().is_some() {
            let cli_decoded = decompress_with_cli(&zstd_output);
            assert!(cli_decoded.is_empty());
        } else {
            println!("zstd CLI not found on PATH, skipping CLI check");
        }
    }

    #[test]
    fn rejects_level_other_than_one() {
        let mut options = CompressOptions::zstd();
        options.level = 2;
        let mut workspace = EncodeWorkspace::new_boxed();
        let mut output = vec![0u8; 64];
        assert_eq!(
            compress(&[1, 2, 3], &mut output, &options, &mut workspace),
            Err(EncodeError::BadOptions)
        );
    }

    #[test]
    fn rejects_level_zero_and_thirteen() {
        let mut workspace = EncodeWorkspace::new_boxed();
        let mut output = vec![0u8; 64];

        let mut zero_options = CompressOptions::zstd();
        zero_options.level = 0;
        assert_eq!(
            compress(&[1, 2, 3], &mut output, &zero_options, &mut workspace),
            Err(EncodeError::BadOptions)
        );

        let mut thirteen_options = CompressOptions::zstd();
        thirteen_options.level = 13;
        assert_eq!(
            compress(&[1, 2, 3], &mut output, &thirteen_options, &mut workspace),
            Err(EncodeError::BadOptions)
        );
    }

    #[test]
    #[cfg(feature = "checksum")]
    fn level_one_output_is_unchanged() {
        const GOLDEN_LEVEL_ONE_HASH: u64 = 0x2f06_611c_7565_c97c;
        const GOLDEN_LEVEL_ONE_LENGTH: usize = 2_793_047;

        let input = std::fs::read(
            "/Users/josorio/github/phenological/ionic/crates/parser/data/mzml/small.pwiz.1.1.mzML",
        );
        let Ok(input) = input else {
            std::println!("pwiz benchmark file not found, skipping test");
            return;
        };

        let options = CompressOptions::zstd();
        let mut workspace = EncodeWorkspace::new_boxed();
        let mut output = vec![0u8; get_max_compressed_size(input.len(), &options)];
        let written = compress(&input, &mut output, &options, &mut workspace).unwrap();
        output.truncate(written);

        assert_eq!(output.len(), GOLDEN_LEVEL_ONE_LENGTH);
        assert_eq!(xxhash64::hash_bytes(&output, 0), GOLDEN_LEVEL_ONE_HASH);
    }

    #[cfg(feature = "compression")]
    #[test]
    fn default_options_write_a_level_twelve_zstd_frame() {
        let options = CompressOptions {
            with_checksum: false,
            ..Default::default()
        };
        assert_eq!(options.level, 12);

        let text = build_deterministic_text(200 * 1024);
        let mut workspace = EncodeWorkspace::new_boxed_for_level(options.level).unwrap();
        let mut output = vec![0u8; get_max_compressed_size(text.len(), &options)];
        let written = compress(&text, &mut output, &options, &mut workspace).unwrap();
        let header = crate::frame::frame_header::read_frame_header(&output[..written]).unwrap();
        assert!(!header.has_checksum);

        let mut decode_workspace = DecodeWorkspace::new_boxed();
        let mut decoded = vec![0u8; text.len()];
        decompress(&output[..written], &mut decoded, &mut decode_workspace).unwrap();
        assert_eq!(decoded, text);
    }

    #[cfg(feature = "compression")]
    #[test]
    fn level_twenty_two_tables_grow_with_the_input_instead_of_the_level_maximum() {
        let mut workspace = EncodeWorkspace::new_boxed_for_level(22).unwrap();
        assert!(workspace.table_memory.len() < 1 << 20);

        let text = build_deterministic_text(300 * 1024);
        let mut options = CompressOptions::zstd();
        options.level = 22;
        let mut output = vec![0u8; get_max_compressed_size(text.len(), &options)];
        let written = compress(&text, &mut output, &options, &mut workspace).unwrap();
        let grown_length = workspace.table_memory.len();
        let table_parameters =
            level_table::get_level_parameters_for_input_length(22, text.len()).unwrap();
        assert_eq!(
            grown_length,
            crate::levels::finder_tables::table_memory_length(table_parameters)
        );
        assert!(grown_length < 1 << 24);
        let header = crate::frame::frame_header::read_frame_header(&output[..written]).unwrap();
        assert_eq!(header.window_size, 1u64 << 19);

        let mut decode_workspace = DecodeWorkspace::new_boxed();
        let mut decoded = vec![0u8; text.len()];
        decompress(&output[..written], &mut decoded, &mut decode_workspace).unwrap();
        assert_eq!(decoded, text);

        let small_text = build_deterministic_text(20 * 1024);
        let small_written = compress(&small_text, &mut output, &options, &mut workspace).unwrap();
        assert_eq!(workspace.table_memory.len(), grown_length);
        let mut small_decoded = vec![0u8; small_text.len()];
        decompress(
            &output[..small_written],
            &mut small_decoded,
            &mut decode_workspace,
        )
        .unwrap();
        assert_eq!(small_decoded, small_text);
    }

    #[test]
    fn reused_workspace_matches_fresh_workspace_output() {
        let mut first_text = vec![b'#'; 7];
        first_text.extend_from_slice(&build_deterministic_text(300 * 1024));
        let second_text = build_deterministic_text(40 * 1024);
        for level in [1u8, 9] {
            let options = CompressOptions {
                with_checksum: false,
                level,
            };
            let mut output = vec![0u8; get_max_compressed_size(first_text.len(), &options)];
            let mut reused = EncodeWorkspace::new_boxed_for_level(level).unwrap();
            compress(&first_text, &mut output, &options, &mut reused).unwrap();
            let reused_length = compress(&second_text, &mut output, &options, &mut reused).unwrap();
            let reused_output = output[..reused_length].to_vec();
            let mut fresh = EncodeWorkspace::new_boxed_for_level(level).unwrap();
            let fresh_length = compress(&second_text, &mut output, &options, &mut fresh).unwrap();
            assert_eq!(reused_output, output[..fresh_length]);
        }
    }

    #[test]
    #[cfg(feature = "checksum")]
    fn level_nine_zstd_frame_header_window_log_is_twenty_two() {
        let options = CompressOptions {
            with_checksum: true,
            level: 9,
        };
        let mut workspace = EncodeWorkspace::new_boxed_for_level(9).unwrap();
        let text = build_deterministic_text(64 * 1024);
        let mut output = vec![0u8; get_max_compressed_size(text.len(), &options)];
        let written = compress(&text, &mut output, &options, &mut workspace).unwrap();
        output.truncate(written);

        let header = crate::frame::frame_header::read_frame_header(&output).unwrap();
        assert_eq!(header.window_size, 1u64 << 22);

        if find_zstd_cli().is_some() {
            let decoded = decompress_with_cli(&output);
            assert_eq!(decoded, text);
        } else {
            println!("zstd CLI not found on PATH, skipping CLI check");
        }
    }

    #[test]
    #[cfg(feature = "checksum")]
    fn level_nine_zstd_frame_round_trips_through_our_decoder_and_the_cli() {
        let options = CompressOptions {
            with_checksum: true,
            level: 9,
        };
        let mut workspace = EncodeWorkspace::new_boxed_for_level(9).unwrap();
        let text = build_deterministic_text(600 * 1024);
        let mut output = vec![0u8; get_max_compressed_size(text.len(), &options)];
        let written = compress(&text, &mut output, &options, &mut workspace).unwrap();
        output.truncate(written);

        let mut decode_workspace = DecodeWorkspace::new_boxed();
        let mut decoded = vec![0u8; text.len()];
        let decoded_length = decompress(&output, &mut decoded, &mut decode_workspace).unwrap();
        assert_eq!(decoded_length, text.len());
        assert_eq!(decoded, text);

        if find_zstd_cli().is_some() {
            let cli_decoded = decompress_with_cli(&output);
            assert_eq!(cli_decoded, text);
        } else {
            println!("zstd CLI not found on PATH, skipping CLI check");
        }
    }

    fn generate_incompressible_bytes(length: usize, seed: u64) -> Vec<u8> {
        let mut state = if seed == 0 { 1 } else { seed };
        let mut bytes = Vec::with_capacity(length);
        while bytes.len() < length {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            bytes.push((state & 0xFF) as u8);
        }
        bytes
    }

    #[test]
    fn max_compressed_size_bound_holds_for_incompressible_input() {
        let lengths = [0usize, 1, 65535, 65536, 65537, 214289, 400000];
        let options = CompressOptions::zstd();
        let mut workspace = EncodeWorkspace::new_boxed();

        for &length in &lengths {
            let input = generate_incompressible_bytes(length, length as u64 + 1);
            let max_length = get_max_compressed_size(input.len(), &options);
            let mut output = vec![0u8; max_length];
            let result = compress(&input, &mut output, &options, &mut workspace);
            assert!(result.is_ok(), "length {length} failed: {result:?}");
        }
    }

    fn make_barely_compressible_bytes(length: usize, seed: u64) -> Vec<u8> {
        let mut state = seed;
        let mut bytes = Vec::with_capacity(length);
        while bytes.len() < length {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let byte = (state & 0xFF) as u8;
            if byte < 40 {
                bytes.push(0);
            } else {
                bytes.push(byte);
            }
        }
        bytes.truncate(length);
        bytes
    }

    #[test]
    fn raw_fallback_triggers_at_every_side_of_the_compressed_size_boundary() {
        let cli_available = find_zstd_cli().is_some();
        let mut saw_raw = false;
        let mut saw_compressed = false;
        let mut workspace = EncodeWorkspace::new_boxed();

        let options = CompressOptions::zstd();

        for length in (200..4000).step_by(17) {
            for seed in [0x1234_5678_9ABC_DEF0u64, 0x0FED_CBA9_8765_4321u64] {
                let input = make_barely_compressible_bytes(length, seed ^ length as u64);

                let mut frame = vec![0u8; get_max_compressed_size(input.len(), &options)];
                let written = compress(&input, &mut frame, &options, &mut workspace).unwrap();
                frame.truncate(written);

                let header = crate::frame::frame_header::read_frame_header(&frame).unwrap();
                let block_header =
                    crate::frame::block_header::read_block_header(&frame[header.header_length..])
                        .unwrap();
                match block_header.block_type {
                    crate::frame::block_header::BlockType::Raw => saw_raw = true,
                    crate::frame::block_header::BlockType::Compressed => saw_compressed = true,
                    crate::frame::block_header::BlockType::Rle => {}
                }

                let mut decode_workspace = DecodeWorkspace::new_boxed();
                let mut decoded = vec![0u8; input.len() + 4096];
                let decoded_length =
                    decompress(&frame, &mut decoded, &mut decode_workspace).unwrap();
                decoded.truncate(decoded_length);
                assert_eq!(
                    decoded, input,
                    "our decoder disagreed for length {length} seed {seed:#x}"
                );

                if cli_available {
                    let cli_decoded = decompress_with_cli(&frame);
                    assert_eq!(
                        cli_decoded, input,
                        "zstd cli disagreed for length {length} seed {seed:#x}"
                    );
                }
            }
        }

        assert!(
            saw_raw,
            "no case in the sweep landed on a raw block, boundary not exercised"
        );
        assert!(
            saw_compressed,
            "no case in the sweep landed on a compressed block, boundary not exercised"
        );
    }

    fn compress_with_cosmoz(input: &[u8], options: &CompressOptions) -> Vec<u8> {
        let mut workspace = EncodeWorkspace::new_boxed();
        let mut output = vec![0u8; get_max_compressed_size(input.len(), options)];
        let written = compress(input, &mut output, options, &mut workspace).unwrap();
        output.truncate(written);
        output
    }

    fn decompress_with_cosmoz(frame: &[u8]) -> Vec<u8> {
        let mut workspace = DecodeWorkspace::new_boxed();
        let decompressed_length = get_decompressed_size(frame).unwrap().unwrap() as usize;
        let mut output = vec![0u8; decompressed_length];
        let written = decompress(frame, &mut output, &mut workspace).unwrap();
        output.truncate(written);
        output
    }

    fn similar_text_of_length(length: usize, variant: usize) -> Vec<u8> {
        let vocabulary: [&str; 20] = [
            "the", "quick", "brown", "fox", "jumps", "over", "lazy", "dog", "while", "sun", "sets",
            "slowly", "behind", "distant", "hills", "wind", "carries", "scent", "rain", "valley",
        ];
        let mut text = Vec::with_capacity(length + 32);
        let mut state = 0x9E3779B97F4A7C15u64.wrapping_add(variant as u64);
        while text.len() < length {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let word = vocabulary[(state as usize) % vocabulary.len()];
            text.extend_from_slice(word.as_bytes());
            text.push(b'a' + ((state >> 40) % 16) as u8);
            text.push(b' ');
        }
        text.truncate(length);
        text
    }

    fn six_similar_blocks() -> Vec<u8> {
        let block_size = MAX_BLOCK_SIZE;
        let mut input = Vec::with_capacity(block_size * 6);
        for block_index in 0..6 {
            input.extend_from_slice(&similar_text_of_length(block_size, block_index));
        }
        input
    }

    fn read_literals_type(body: &[u8]) -> crate::block::literals::LiteralsType {
        crate::block::literals::read_literals_header(body)
            .unwrap()
            .literals_type
    }

    fn walk_blocks(mut stream: &[u8]) -> Vec<Option<crate::block::literals::LiteralsType>> {
        let mut blocks = Vec::new();
        loop {
            let header = crate::frame::block_header::read_block_header(stream).unwrap();
            let body = &stream[3..3 + header.block_size];
            let info = match header.block_type {
                crate::frame::block_header::BlockType::Compressed => Some(read_literals_type(body)),
                _ => None,
            };
            blocks.push(info);
            stream = &stream[3 + header.block_size..];
            if header.is_last {
                break;
            }
        }
        blocks
    }

    fn zstd_frame_blocks(frame: &[u8]) -> Vec<Option<crate::block::literals::LiteralsType>> {
        let header = crate::frame::frame_header::read_frame_header(frame).unwrap();
        walk_blocks(&frame[header.header_length..])
    }

    #[test]
    fn six_similar_blocks_use_repeat_and_treeless_and_round_trip() {
        let input = six_similar_blocks();

        let zstd_options = CompressOptions::zstd();
        let zstd_frame = compress_with_cosmoz(&input, &zstd_options);
        let decoded_by_us = decompress_with_cosmoz(&zstd_frame);
        assert_eq!(decoded_by_us, input);

        let blocks = zstd_frame_blocks(&zstd_frame);
        assert!(blocks.len() >= 6, "expected at least six blocks");

        let mut saw_treeless = false;
        for literals_type in blocks.iter().flatten() {
            if *literals_type == crate::block::literals::LiteralsType::Treeless {
                saw_treeless = true;
            }
        }
        assert!(saw_treeless, "expected at least one block to use Treeless");

        if find_zstd_cli().is_some() {
            let decoded_by_cli = decompress_with_cli(&zstd_frame);
            assert_eq!(decoded_by_cli, input);
        }
    }
}
