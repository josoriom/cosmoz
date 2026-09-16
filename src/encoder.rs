#[cfg(feature = "alloc")]
use crate::block::sequences::TableMode;
#[cfg(feature = "parallel")]
use crate::frame::chunk_index::ChunkEntry;
#[cfg(not(feature = "alloc"))]
use crate::match_finder::hash_table_finder::{HASH_TABLE_SIZE, HashTableFinder};
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
        chunk_index::{CHUNK_COUNT_LENGTH, CHUNK_ENTRY_LENGTH},
        frame_header::{FrameFormat, OSMOS_CHECKSUM_LENGTH, ZSTD_CHECKSUM_LENGTH},
        frame_writer::{MAX_FRAME_HEADER_LENGTH, write_frame_header},
    },
    match_finder::{
        AnyFinder, MAX_OFFSET_LOG, MatchFinder,
        level_table::{self, LevelParameters},
    },
};
#[cfg(feature = "checksum")]
use crate::{
    frame::frame_writer::write_checksum,
    hash::{xxhash3, xxhash64},
};

pub const MAX_CHUNK_SIZE: usize = 4 * 1024 * 1024;
pub const DEFAULT_CHUNK_SIZE: usize = MAX_CHUNK_SIZE;

fn window_log_for_frame(format: FrameFormat, level_parameters: LevelParameters) -> u8 {
    match format {
        FrameFormat::Zstd => level_parameters.window_log,
        FrameFormat::Osmos => level_parameters.window_log.min(MAX_OFFSET_LOG),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompressOptions {
    pub format: FrameFormat,
    pub with_checksum: bool,
    pub chunk_size: usize,
    pub level: u8,
}

impl CompressOptions {
    pub const fn zstd() -> Self {
        Self {
            format: FrameFormat::Zstd,
            with_checksum: true,
            chunk_size: 0,
            level: 1,
        }
    }

    pub const fn osmos() -> Self {
        Self {
            format: FrameFormat::Osmos,
            with_checksum: true,
            chunk_size: DEFAULT_CHUNK_SIZE,
            level: 1,
        }
    }
}

pub struct EncodeWorkspace {
    pub level: u8,
    pub level_parameters: LevelParameters,
    pub match_finder: AnyFinder<'static>,
    #[cfg(feature = "alloc")]
    pub table_memory: alloc::boxed::Box<[u32]>,
    pub repeat_offsets: RepeatOffsets,
    pub sequences: [SequenceRecord; MAX_SEQUENCES_PER_BLOCK],
    pub literals: [u8; MAX_BLOCK_SIZE],
    pub block_scratch: [u8; MAX_BLOCK_SIZE + 1024],
    pub entropy_scratch: [u8; MAX_BLOCK_SIZE + 1024],
    pub huffman_table: HuffmanEncodeTable,
    pub weight_fse_table: FseEncodeTable,
    pub sequence_tables: SequenceEncodeTables,
}

#[cfg(not(feature = "alloc"))]
impl EncodeWorkspace {
    pub const fn new() -> Self {
        Self {
            level: level_table::MIN_LEVEL,
            level_parameters: level_table::level_one_parameters(),
            match_finder: AnyFinder::for_level(level_table::level_one_parameters()),
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
            entropy_scratch: [0u8; MAX_BLOCK_SIZE + 1024],
            huffman_table: HuffmanEncodeTable::new(),
            weight_fse_table: FseEncodeTable::new(),
            sequence_tables: SequenceEncodeTables::new(),
        }
    }

    #[allow(dead_code)]
    pub(crate) const fn zeroed() -> Self {
        Self {
            level: 0,
            level_parameters: LevelParameters {
                window_log: 0,
                chain_log: 0,
                hash_log: 0,
                search_log: 0,
                min_match: 0,
                target_length: 0,
                strategy: level_table::Strategy::Fast,
            },
            match_finder: AnyFinder::Fast(HashTableFinder {
                positions: [0; HASH_TABLE_SIZE],
                window_log: 0,
            }),
            repeat_offsets: RepeatOffsets {
                first: 0,
                second: 0,
                third: 0,
            },
            sequences: [SequenceRecord {
                literal_length: 0,
                match_length: 0,
                offset_value: 0,
            }; MAX_SEQUENCES_PER_BLOCK],
            literals: [0u8; MAX_BLOCK_SIZE],
            block_scratch: [0u8; MAX_BLOCK_SIZE + 1024],
            entropy_scratch: [0u8; MAX_BLOCK_SIZE + 1024],
            huffman_table: HuffmanEncodeTable::new(),
            weight_fse_table: FseEncodeTable::new(),
            sequence_tables: SequenceEncodeTables::new(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn write_initial_values_unchecked(&mut self) {
        self.level = level_table::MIN_LEVEL;
        self.level_parameters = level_table::level_one_parameters();
        self.match_finder = AnyFinder::for_level(self.level_parameters);
        self.repeat_offsets = RepeatOffsets::new();
    }
}

#[cfg(feature = "alloc")]
impl EncodeWorkspace {
    pub fn new() -> Self {
        let level_parameters = level_table::level_one_parameters();
        Self {
            level: level_table::MIN_LEVEL,
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
            entropy_scratch: [0u8; MAX_BLOCK_SIZE + 1024],
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

#[cfg(feature = "alloc")]
impl EncodeWorkspace {
    pub fn new_boxed() -> alloc::boxed::Box<Self> {
        build_boxed_workspace(level_table::MIN_LEVEL, level_table::level_one_parameters())
    }

    pub fn new_boxed_for_level(level: u8) -> Result<alloc::boxed::Box<Self>, EncodeError> {
        let level_parameters =
            level_table::get_level_parameters(level).ok_or(EncodeError::BadOptions)?;
        Ok(build_boxed_workspace(level, level_parameters))
    }
}

#[cfg(feature = "alloc")]
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

#[cfg(feature = "alloc")]
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

        let table_memory_length =
            crate::match_finder::finder_tables::table_memory_length(level_parameters);
        let mut table_memory: alloc::boxed::Box<[u32]> =
            alloc::vec![0u32; table_memory_length].into_boxed_slice();

        let table_memory_ptr = table_memory.as_mut_ptr();
        let table_memory_len = table_memory.len();
        let borrowed_memory: &'static mut [u32] =
            core::slice::from_raw_parts_mut(table_memory_ptr, table_memory_len);
        let storage = crate::match_finder::finder_tables::split_table_storage(
            borrowed_memory,
            level_parameters,
        );
        let match_finder = crate::match_finder::AnyFinder::for_level_with_storage_at(
            level,
            level_parameters,
            storage,
        );

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

#[cfg(all(test, feature = "alloc"))]
mod new_boxed_tests {
    use super::*;
    use crate::match_finder::hash_table_finder::HASH_TABLE_SIZE;

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
                    crate::match_finder::finder_tables::table_memory_length(
                        boxed_workspace.level_parameters
                    )
                );

                let boxed_finder = match &boxed_workspace.match_finder {
                    AnyFinder::Fast(finder) => finder,
                    AnyFinder::DoubleFast(_) => panic!("level 1 workspace must select Fast"),
                    AnyFinder::Chain(_) => panic!("level 1 workspace must select Fast"),
                    AnyFinder::Row(_) => panic!("level 1 workspace must select Fast"),
                };
                let stack_finder = match &stack_workspace.match_finder {
                    AnyFinder::Fast(finder) => finder,
                    AnyFinder::DoubleFast(_) => panic!("level 1 workspace must select Fast"),
                    AnyFinder::Chain(_) => panic!("level 1 workspace must select Fast"),
                    AnyFinder::Row(_) => panic!("level 1 workspace must select Fast"),
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

fn clamp_chunk_size(chunk_size: usize) -> usize {
    chunk_size.clamp(1, MAX_CHUNK_SIZE)
}

fn window_size_for_log(window_log: u8) -> usize {
    if window_log as u32 >= usize::BITS {
        usize::MAX
    } else {
        1usize << window_log
    }
}

fn plan_osmos_chunking(input_length: usize, chunk_size: usize, window_log: u8) -> (usize, usize) {
    let configured_chunk_size = clamp_chunk_size(chunk_size);
    if input_length == 0 {
        return (configured_chunk_size, 1);
    }
    let split_chunk_count = input_length.div_ceil(configured_chunk_size);
    let fits_in_one_window = input_length <= window_size_for_log(window_log);
    if split_chunk_count <= 2 && fits_in_one_window {
        (input_length, 1)
    } else {
        (configured_chunk_size, split_chunk_count)
    }
}

pub fn get_max_compressed_size(input_length: usize, options: &CompressOptions) -> usize {
    let checksum_size = if options.with_checksum {
        match options.format {
            FrameFormat::Zstd => ZSTD_CHECKSUM_LENGTH,
            FrameFormat::Osmos => OSMOS_CHECKSUM_LENGTH,
        }
    } else {
        0
    };

    match options.format {
        FrameFormat::Zstd => {
            let block_count = if input_length == 0 {
                1
            } else {
                input_length.div_ceil(MAX_BLOCK_SIZE)
            };
            let blocks_size = input_length + block_count * BLOCK_HEADER_LENGTH;
            MAX_FRAME_HEADER_LENGTH + blocks_size + checksum_size
        }
        FrameFormat::Osmos => {
            let chunk_size = clamp_chunk_size(options.chunk_size);
            let chunk_count = if input_length == 0 {
                1
            } else {
                input_length.div_ceil(chunk_size)
            };
            let block_count = chunk_count + input_length.div_ceil(MAX_BLOCK_SIZE);
            let blocks_size = input_length + block_count * BLOCK_HEADER_LENGTH;
            let index_size = CHUNK_COUNT_LENGTH + chunk_count * CHUNK_ENTRY_LENGTH;
            MAX_FRAME_HEADER_LENGTH + index_size + blocks_size + checksum_size
        }
    }
}

pub fn compress(
    input: &[u8],
    output: &mut [u8],
    options: &CompressOptions,
    workspace: &mut EncodeWorkspace,
) -> Result<usize, EncodeError> {
    if !(level_table::MIN_LEVEL..=level_table::MAX_LEVEL).contains(&options.level) {
        return Err(EncodeError::BadOptions);
    }
    if options.level != workspace.level {
        return Err(EncodeError::BadOptions);
    }
    #[cfg(not(feature = "checksum"))]
    if options.with_checksum {
        return Err(EncodeError::BadOptions);
    }

    match options.format {
        FrameFormat::Zstd => compress_zstd_frame(input, output, options, workspace),
        FrameFormat::Osmos => compress_osmos_frame(input, output, options, workspace),
    }
}

fn compress_zstd_frame(
    input: &[u8],
    output: &mut [u8],
    options: &CompressOptions,
    workspace: &mut EncodeWorkspace,
) -> Result<usize, EncodeError> {
    let window_log = window_log_for_frame(FrameFormat::Zstd, workspace.level_parameters);
    let mut position = write_frame_header(
        output,
        FrameFormat::Zstd,
        input.len() as u64,
        window_log,
        options.with_checksum,
    )?;

    workspace.match_finder.reset();
    workspace.repeat_offsets = RepeatOffsets::new();

    let blocks_output = output
        .get_mut(position..)
        .ok_or(EncodeError::OutputTooSmall)?;
    position += compress_chunk(input, FrameFormat::Zstd, blocks_output, workspace)?;

    #[cfg(feature = "checksum")]
    if options.with_checksum {
        let hash = xxhash64::hash_bytes(input, 0);
        let checksum_output = output
            .get_mut(position..)
            .ok_or(EncodeError::OutputTooSmall)?;
        position += write_checksum(checksum_output, FrameFormat::Zstd, hash)?;
    }

    Ok(position)
}

fn compress_osmos_frame(
    input: &[u8],
    output: &mut [u8],
    options: &CompressOptions,
    workspace: &mut EncodeWorkspace,
) -> Result<usize, EncodeError> {
    let window_log = window_log_for_frame(FrameFormat::Osmos, workspace.level_parameters);
    let (chunk_size, chunk_count) =
        plan_osmos_chunking(input.len(), options.chunk_size, window_log);

    let header_length = write_frame_header(
        output,
        FrameFormat::Osmos,
        input.len() as u64,
        window_log,
        options.with_checksum,
    )?;

    let entries_length = chunk_count
        .checked_mul(CHUNK_ENTRY_LENGTH)
        .ok_or(EncodeError::InputTooLarge)?;
    let index_length = CHUNK_COUNT_LENGTH
        .checked_add(entries_length)
        .ok_or(EncodeError::InputTooLarge)?;

    if output.len() < header_length + index_length {
        return Err(EncodeError::OutputTooSmall);
    }
    output[header_length..header_length + CHUNK_COUNT_LENGTH]
        .copy_from_slice(&(chunk_count as u32).to_le_bytes());

    let mut position = header_length + index_length;

    #[cfg(feature = "parallel")]
    let use_parallel_path = chunk_count >= 2;
    #[cfg(not(feature = "parallel"))]
    let use_parallel_path = false;

    if use_parallel_path {
        #[cfg(feature = "parallel")]
        {
            let mut entries = vec![
                ChunkEntry {
                    compressed_length: 0,
                    decompressed_length: 0,
                };
                chunk_count
            ];
            let chunks_output = output
                .get_mut(position..)
                .ok_or(EncodeError::OutputTooSmall)?;
            let written = crate::parallel_encoder::compress_chunks_in_parallel(
                input,
                chunk_size,
                chunks_output,
                &mut entries,
            )?;

            for (chunk_number, entry) in entries.iter().enumerate() {
                let compressed_length_u32 = u32::try_from(entry.compressed_length)
                    .map_err(|_| EncodeError::InputTooLarge)?;
                let decompressed_length_u32 = u32::try_from(entry.decompressed_length)
                    .map_err(|_| EncodeError::InputTooLarge)?;
                let entry_offset =
                    header_length + CHUNK_COUNT_LENGTH + chunk_number * CHUNK_ENTRY_LENGTH;
                output[entry_offset..entry_offset + 4]
                    .copy_from_slice(&compressed_length_u32.to_le_bytes());
                output[entry_offset + 4..entry_offset + 8]
                    .copy_from_slice(&decompressed_length_u32.to_le_bytes());
            }

            position += written;
        }
    } else {
        let mut chunk_start = 0usize;
        for chunk_number in 0..chunk_count {
            let chunk_end = if input.is_empty() {
                0
            } else {
                (chunk_start + chunk_size).min(input.len())
            };
            let chunk_content = &input[chunk_start..chunk_end];

            workspace.match_finder.reset();
            workspace.repeat_offsets = RepeatOffsets::new();

            let chunk_output = output
                .get_mut(position..)
                .ok_or(EncodeError::OutputTooSmall)?;
            let compressed_length =
                compress_chunk(chunk_content, FrameFormat::Osmos, chunk_output, workspace)?;

            let compressed_length_u32 =
                u32::try_from(compressed_length).map_err(|_| EncodeError::InputTooLarge)?;
            let decompressed_length_u32 =
                u32::try_from(chunk_content.len()).map_err(|_| EncodeError::InputTooLarge)?;

            let entry_offset =
                header_length + CHUNK_COUNT_LENGTH + chunk_number * CHUNK_ENTRY_LENGTH;
            output[entry_offset..entry_offset + 4]
                .copy_from_slice(&compressed_length_u32.to_le_bytes());
            output[entry_offset + 4..entry_offset + 8]
                .copy_from_slice(&decompressed_length_u32.to_le_bytes());

            position += compressed_length;
            chunk_start = chunk_end;
        }
    }

    #[cfg(feature = "checksum")]
    if options.with_checksum {
        let hash = xxhash3::hash_bytes(input);
        let checksum_output = output
            .get_mut(position..)
            .ok_or(EncodeError::OutputTooSmall)?;
        position += write_checksum(checksum_output, FrameFormat::Osmos, hash)?;
    }

    Ok(position)
}

pub(crate) fn compress_chunk(
    input: &[u8],
    format: FrameFormat,
    output: &mut [u8],
    workspace: &mut EncodeWorkspace,
) -> Result<usize, EncodeError> {
    if input.is_empty() {
        return write_block(input, 0, format, true, output, workspace);
    }

    let mut position = 0usize;
    let mut block_start = 0usize;
    while block_start < input.len() {
        let block_end = (block_start + MAX_BLOCK_SIZE).min(input.len());
        let is_last = block_end == input.len();
        let block_output = output
            .get_mut(position..)
            .ok_or(EncodeError::OutputTooSmall)?;
        position += write_block(
            &input[..block_end],
            block_start,
            format,
            is_last,
            block_output,
            workspace,
        )?;
        block_start = block_end;
    }

    Ok(position)
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
    fn raw_osmos_frame_round_trips() {
        let text = build_deterministic_text(2 * 1024 * 1024 + 512 * 1024);
        let mut options = CompressOptions::osmos();
        options.chunk_size = 1024 * 1024;
        let chunk_count = text.len().div_ceil(options.chunk_size);
        assert_eq!(chunk_count, 3);

        let mut workspace = EncodeWorkspace::new_boxed();
        let mut output = vec![0u8; get_max_compressed_size(text.len(), &options)];
        let written = compress(&text, &mut output, &options, &mut workspace).unwrap();
        output.truncate(written);

        assert_eq!(
            get_decompressed_size(&output).unwrap(),
            Some(text.len() as u64)
        );

        let mut decode_workspace = DecodeWorkspace::new_boxed();
        let mut decoded = vec![0u8; text.len()];
        let decoded_length = decompress(&output, &mut decoded, &mut decode_workspace).unwrap();
        assert_eq!(decoded_length, text.len());
        assert_eq!(decoded, text);
    }

    #[test]
    fn empty_input_frames_round_trip_in_both_formats() {
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

        let osmos_options = CompressOptions::osmos();
        let mut osmos_output = vec![0u8; get_max_compressed_size(0, &osmos_options)];
        let osmos_written =
            compress(&[], &mut osmos_output, &osmos_options, &mut workspace).unwrap();
        osmos_output.truncate(osmos_written);

        let osmos_decoded_length =
            decompress(&osmos_output, &mut decoded, &mut decode_workspace).unwrap();
        assert_eq!(osmos_decoded_length, 0);
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
    fn level_one_output_is_unchanged() {
        const GOLDEN_LEVEL_ONE_HASH: u64 = 0xc641_9933_b376_6a9c;
        const GOLDEN_LEVEL_ONE_LENGTH: usize = 2_173_689;

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

    #[test]
    fn level_nine_zstd_frame_header_window_log_is_twenty_two() {
        let options = CompressOptions {
            format: FrameFormat::Zstd,
            with_checksum: true,
            chunk_size: 0,
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
    fn level_nine_zstd_frame_round_trips_through_our_decoder_and_the_cli() {
        let options = CompressOptions {
            format: FrameFormat::Zstd,
            with_checksum: true,
            chunk_size: 0,
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
    fn max_compressed_size_bound_is_exact_for_worst_case_chunking() {
        let lengths = [0usize, 1, 65535, 65536, 65537, 214289, 400000];
        let chunk_sizes = [1024usize, 65536, MAX_CHUNK_SIZE];
        let mut workspace = EncodeWorkspace::new_boxed();

        for &length in &lengths {
            let input = generate_incompressible_bytes(length, length as u64 + 1);
            for &chunk_size in &chunk_sizes {
                let options = CompressOptions {
                    format: FrameFormat::Osmos,
                    with_checksum: true,
                    chunk_size,
                    level: 1,
                };
                let max_length = get_max_compressed_size(input.len(), &options);
                let mut output = vec![0u8; max_length];
                let result = compress(&input, &mut output, &options, &mut workspace);
                assert!(
                    result.is_ok(),
                    "length {length} chunk_size {chunk_size} failed: {result:?}"
                );
            }
        }
    }

    fn read_chunk_count(frame: &[u8]) -> usize {
        let header = crate::frame::frame_header::read_frame_header(frame).unwrap();
        let index =
            crate::frame::chunk_index::ChunkIndex::read(&frame[header.header_length..]).unwrap();
        index.chunk_count
    }

    #[test]
    fn merges_two_chunks_into_one_when_the_merged_chunk_fits_the_window() {
        let text = build_deterministic_text(6000);
        let mut options = CompressOptions::osmos();
        options.chunk_size = 4096;
        let split_chunk_count = text.len().div_ceil(options.chunk_size);
        assert_eq!(split_chunk_count, 2);

        let mut workspace = EncodeWorkspace::new_boxed();
        let mut output = vec![0u8; get_max_compressed_size(text.len(), &options)];
        let written = compress(&text, &mut output, &options, &mut workspace).unwrap();
        output.truncate(written);

        assert_eq!(read_chunk_count(&output), 1);

        let mut decode_workspace = DecodeWorkspace::new_boxed();
        let mut decoded = vec![0u8; text.len()];
        let decoded_length = decompress(&output, &mut decoded, &mut decode_workspace).unwrap();
        assert_eq!(decoded_length, text.len());
        assert_eq!(decoded, text);
    }

    #[test]
    fn keeps_the_split_when_the_merged_chunk_would_need_a_larger_window() {
        let text = build_deterministic_text(1_400_000);
        let mut options = CompressOptions::osmos();
        options.chunk_size = 700_000;
        options.level = 1;
        let split_chunk_count = text.len().div_ceil(options.chunk_size);
        assert_eq!(split_chunk_count, 2);
        assert!(text.len() > (1usize << level_table::level_one_parameters().window_log));

        let mut workspace = EncodeWorkspace::new_boxed_for_level(1).unwrap();
        let mut output = vec![0u8; get_max_compressed_size(text.len(), &options)];
        let written = compress(&text, &mut output, &options, &mut workspace).unwrap();
        output.truncate(written);

        assert_eq!(read_chunk_count(&output), 2);

        let mut decode_workspace = DecodeWorkspace::new_boxed();
        let mut decoded = vec![0u8; text.len()];
        let decoded_length = decompress(&output, &mut decoded, &mut decode_workspace).unwrap();
        assert_eq!(decoded_length, text.len());
        assert_eq!(decoded, text);
    }

    #[test]
    fn single_chunk_osmos_frame_round_trips_through_the_parallel_decoder() {
        let text = build_deterministic_text(6000);
        let mut options = CompressOptions::osmos();
        options.chunk_size = 4096;

        let mut workspace = EncodeWorkspace::new_boxed();
        let mut output = vec![0u8; get_max_compressed_size(text.len(), &options)];
        let written = compress(&text, &mut output, &options, &mut workspace).unwrap();
        output.truncate(written);

        assert_eq!(read_chunk_count(&output), 1);

        let mut decode_workspace = DecodeWorkspace::new_boxed();
        let mut decoded = vec![0u8; text.len()];
        let decoded_length = decompress(&output, &mut decoded, &mut decode_workspace).unwrap();
        assert_eq!(decoded_length, text.len());
        assert_eq!(decoded, text);
    }

    #[test]
    #[ignore]
    fn report_osmos_and_zstd_sizes_on_bench_files() {
        let pwiz = std::fs::read(
            "/Users/josorio/github/phenological/ionic/crates/parser/data/mzml/small.pwiz.1.1.mzML",
        )
        .unwrap();
        let iron = std::fs::read(
            "/Users/josorio/github/josoriom/szstd/data/iron_ultrairon_SER_MS-AI-HILPOS@fNMR_IROr20_IROp011_LTR_16.mzML",
        )
        .unwrap();

        for level in [1u8, 6, 9, 12] {
            for (name, input) in [("pwiz", &pwiz), ("iron", &iron)] {
                let mut osmos_options = CompressOptions::osmos();
                osmos_options.level = level;
                let mut zstd_options = CompressOptions::zstd();
                zstd_options.level = level;

                let mut workspace = EncodeWorkspace::new_boxed_for_level(level).unwrap();

                let mut osmos_output =
                    vec![0u8; get_max_compressed_size(input.len(), &osmos_options)];
                let osmos_written =
                    compress(input, &mut osmos_output, &osmos_options, &mut workspace).unwrap();
                let osmos_chunk_count = read_chunk_count(&osmos_output[..osmos_written]);

                let mut zstd_output =
                    vec![0u8; get_max_compressed_size(input.len(), &zstd_options)];
                let zstd_written =
                    compress(input, &mut zstd_output, &zstd_options, &mut workspace).unwrap();

                println!(
                    "{name} level {level}: osmos {osmos_written} bytes (chunk_count {osmos_chunk_count}), zstd {zstd_written} bytes"
                );
            }
        }
    }

    #[test]
    fn empty_input_osmos_chunk_count_is_still_one() {
        let options = CompressOptions::osmos();
        let mut workspace = EncodeWorkspace::new_boxed();
        let mut output = vec![0u8; get_max_compressed_size(0, &options)];
        let written = compress(&[], &mut output, &options, &mut workspace).unwrap();
        output.truncate(written);

        assert_eq!(read_chunk_count(&output), 1);
    }
}
