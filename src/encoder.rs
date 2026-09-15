use crate::block::block_writer::write_block;
use crate::block::repeat_offsets::RepeatOffsets;
use crate::block::sequence_record::{MAX_SEQUENCES_PER_BLOCK, SequenceRecord};
use crate::block::sequence_writer::SequenceEncodeTables;
#[cfg(feature = "alloc")]
use crate::block::sequences::TableMode;
use crate::encode_error::EncodeError;
use crate::entropy::fse_encode_table::FseEncodeTable;
use crate::entropy::huffman_encode_table::HuffmanEncodeTable;
use crate::frame::block_header::{BLOCK_HEADER_LENGTH, MAX_BLOCK_SIZE};
#[cfg(feature = "parallel")]
use crate::frame::chunk_index::ChunkEntry;
use crate::frame::chunk_index::{CHUNK_COUNT_LENGTH, CHUNK_ENTRY_LENGTH};
use crate::frame::frame_header::{FrameFormat, OSMO_CHECKSUM_LENGTH, ZSTD_CHECKSUM_LENGTH};
use crate::frame::frame_writer::{MAX_FRAME_HEADER_LENGTH, write_checksum, write_frame_header};
use crate::hash::{xxhash3, xxhash64};
use crate::match_finder::MatchFinder;
#[cfg(feature = "alloc")]
use crate::match_finder::hash_table_finder::HASH_TABLE_SIZE;
use crate::match_finder::hash_table_finder::HashTableFinder;

pub const MAX_CHUNK_SIZE: usize = 4 * 1024 * 1024;
pub const DEFAULT_CHUNK_SIZE: usize = MAX_CHUNK_SIZE;
const WINDOW_LOG: u8 = 20;

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

    pub const fn osmo() -> Self {
        Self {
            format: FrameFormat::Osmo,
            with_checksum: true,
            chunk_size: DEFAULT_CHUNK_SIZE,
            level: 1,
        }
    }
}

pub struct EncodeWorkspace {
    pub match_finder: HashTableFinder,
    pub repeat_offsets: RepeatOffsets,
    pub sequences: [SequenceRecord; MAX_SEQUENCES_PER_BLOCK],
    pub literals: [u8; MAX_BLOCK_SIZE],
    pub block_scratch: [u8; MAX_BLOCK_SIZE + 1024],
    pub entropy_scratch: [u8; MAX_BLOCK_SIZE + 1024],
    pub huffman_table: HuffmanEncodeTable,
    pub weight_fse_table: FseEncodeTable,
    pub sequence_tables: SequenceEncodeTables,
}

impl EncodeWorkspace {
    pub const fn new() -> Self {
        Self {
            match_finder: HashTableFinder::new(WINDOW_LOG),
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
        let layout = core::alloc::Layout::new::<Self>();
        unsafe {
            let raw = alloc::alloc::alloc_zeroed(layout) as *mut Self;
            if raw.is_null() {
                alloc::alloc::handle_alloc_error(layout);
            }
            write_initial_values(raw);
            alloc::boxed::Box::from_raw(raw)
        }
    }
}

#[cfg(feature = "alloc")]
unsafe fn write_initial_values(target: *mut EncodeWorkspace) {
    unsafe {
        let match_finder = core::ptr::addr_of_mut!((*target).match_finder);
        core::ptr::write(
            core::ptr::addr_of_mut!((*match_finder).positions),
            [u32::MAX; HASH_TABLE_SIZE],
        );
        core::ptr::write(
            core::ptr::addr_of_mut!((*match_finder).window_log),
            WINDOW_LOG,
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

    #[test]
    fn new_boxed_matches_new_field_by_field() {
        let boxed_workspace = EncodeWorkspace::new_boxed();

        let large_stack_thread = std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(move || {
                let stack_workspace = EncodeWorkspace::new();

                assert_eq!(
                    boxed_workspace.match_finder.positions[0],
                    stack_workspace.match_finder.positions[0]
                );
                assert_eq!(
                    boxed_workspace.match_finder.positions[HASH_TABLE_SIZE - 1],
                    stack_workspace.match_finder.positions[HASH_TABLE_SIZE - 1]
                );
                assert_eq!(
                    boxed_workspace.match_finder.window_log,
                    stack_workspace.match_finder.window_log
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

pub fn get_max_compressed_size(input_length: usize, options: &CompressOptions) -> usize {
    let checksum_size = if options.with_checksum {
        match options.format {
            FrameFormat::Zstd => ZSTD_CHECKSUM_LENGTH,
            FrameFormat::Osmo => OSMO_CHECKSUM_LENGTH,
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
        FrameFormat::Osmo => {
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
    if options.level != 1 {
        return Err(EncodeError::BadOptions);
    }

    match options.format {
        FrameFormat::Zstd => compress_zstd_frame(input, output, options, workspace),
        FrameFormat::Osmo => compress_osmo_frame(input, output, options, workspace),
    }
}

fn compress_zstd_frame(
    input: &[u8],
    output: &mut [u8],
    options: &CompressOptions,
    workspace: &mut EncodeWorkspace,
) -> Result<usize, EncodeError> {
    let mut position = write_frame_header(
        output,
        FrameFormat::Zstd,
        input.len() as u64,
        WINDOW_LOG,
        options.with_checksum,
    )?;

    workspace.match_finder.reset();
    workspace.repeat_offsets = RepeatOffsets::new();

    let blocks_output = output
        .get_mut(position..)
        .ok_or(EncodeError::OutputTooSmall)?;
    position += compress_chunk(input, FrameFormat::Zstd, blocks_output, workspace)?;

    if options.with_checksum {
        let hash = xxhash64::hash_bytes(input, 0);
        let checksum_output = output
            .get_mut(position..)
            .ok_or(EncodeError::OutputTooSmall)?;
        position += write_checksum(checksum_output, FrameFormat::Zstd, hash)?;
    }

    Ok(position)
}

fn compress_osmo_frame(
    input: &[u8],
    output: &mut [u8],
    options: &CompressOptions,
    workspace: &mut EncodeWorkspace,
) -> Result<usize, EncodeError> {
    let chunk_size = clamp_chunk_size(options.chunk_size);
    let chunk_count = if input.is_empty() {
        1
    } else {
        input.len().div_ceil(chunk_size)
    };

    let header_length = write_frame_header(
        output,
        FrameFormat::Osmo,
        input.len() as u64,
        WINDOW_LOG,
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
                compress_chunk(chunk_content, FrameFormat::Osmo, chunk_output, workspace)?;

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

    if options.with_checksum {
        let hash = xxhash3::hash_bytes(input);
        let checksum_output = output
            .get_mut(position..)
            .ok_or(EncodeError::OutputTooSmall)?;
        position += write_checksum(checksum_output, FrameFormat::Osmo, hash)?;
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
    use super::*;
    use crate::decoder::{DecodeWorkspace, decompress, get_decompressed_size};
    use std::io::Write;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    fn find_zstd_cli() -> Option<PathBuf> {
        let output = Command::new("which").arg("zstd").output().ok()?;
        if !output.status.success() {
            return None;
        }
        let path_string = String::from_utf8(output.stdout).ok()?;
        let trimmed_path = path_string.trim();
        if trimmed_path.is_empty() {
            None
        } else {
            Some(PathBuf::from(trimmed_path))
        }
    }

    fn decompress_with_cli(frame: &[u8]) -> Vec<u8> {
        let zstd_path = find_zstd_cli().expect("zstd CLI not found on PATH");
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
    fn raw_osmo_frame_round_trips() {
        let text = build_deterministic_text(2 * 1024 * 1024 + 512 * 1024);
        let mut options = CompressOptions::osmo();
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

        let osmo_options = CompressOptions::osmo();
        let mut osmo_output = vec![0u8; get_max_compressed_size(0, &osmo_options)];
        let osmo_written = compress(&[], &mut osmo_output, &osmo_options, &mut workspace).unwrap();
        osmo_output.truncate(osmo_written);

        let osmo_decoded_length =
            decompress(&osmo_output, &mut decoded, &mut decode_workspace).unwrap();
        assert_eq!(osmo_decoded_length, 0);
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
                    format: FrameFormat::Osmo,
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
}
