use core::sync::atomic::{AtomicUsize, Ordering};
use std::{sync::Mutex, thread, vec::Vec};

use crate::{
    block::repeat_offsets::RepeatOffsets,
    encode_error::EncodeError,
    encoder::{EncodeWorkspace, compress_chunk},
    frame::{
        block_header::{BLOCK_HEADER_LENGTH, MAX_BLOCK_SIZE},
        chunk_index::ChunkEntry,
        frame_header::FrameFormat,
    },
    levels::MatchFinder,
};

fn max_chunk_compressed_size(chunk_length: usize) -> usize {
    let block_count = if chunk_length == 0 {
        1
    } else {
        chunk_length.div_ceil(MAX_BLOCK_SIZE)
    };
    chunk_length + block_count * BLOCK_HEADER_LENGTH
}

fn chunk_bounds(input_length: usize, chunk_size: usize, chunk_number: usize) -> (usize, usize) {
    let chunk_start = chunk_number * chunk_size;
    let chunk_end = (chunk_start + chunk_size).min(input_length);
    (chunk_start, chunk_end)
}

pub fn compress_chunks_in_parallel(
    input: &[u8],
    chunk_size: usize,
    level: u8,
    output_after_index: &mut [u8],
    entries: &mut [ChunkEntry],
) -> Result<usize, EncodeError> {
    let chunk_count = entries.len();
    if chunk_count == 0 {
        return Err(EncodeError::BadOptions);
    }

    let region_size = max_chunk_compressed_size(chunk_size);
    let mut regions: Vec<Option<&mut [u8]>> = Vec::with_capacity(chunk_count);
    let mut remaining_output = &mut *output_after_index;
    for chunk_number in 0..chunk_count {
        let split_length = if chunk_number + 1 < chunk_count {
            region_size
        } else {
            remaining_output.len()
        };
        if remaining_output.len() < split_length {
            return Err(EncodeError::OutputTooSmall);
        }
        let (region, rest) = remaining_output.split_at_mut(split_length);
        regions.push(Some(region));
        remaining_output = rest;
    }

    let regions = Mutex::new(regions);
    let compressed_lengths: Vec<AtomicUsize> =
        (0..chunk_count).map(|_| AtomicUsize::new(0)).collect();
    let next_chunk_number = AtomicUsize::new(0);
    let first_error: Mutex<Option<EncodeError>> = Mutex::new(None);

    let thread_count = thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
        .min(chunk_count);

    thread::scope(|scope| {
        for _ in 0..thread_count {
            scope.spawn(|| {
                if let Err(error) = run_worker(
                    input,
                    chunk_size,
                    level,
                    &regions,
                    &compressed_lengths,
                    &next_chunk_number,
                    &first_error,
                ) {
                    let mut first_error = first_error.lock().unwrap();
                    if first_error.is_none() {
                        *first_error = Some(error);
                    }
                }
            });
        }
    });

    if let Some(error) = first_error.into_inner().unwrap() {
        return Err(error);
    }

    let mut position = 0usize;
    for chunk_number in 0..chunk_count {
        let compressed_length = compressed_lengths[chunk_number].load(Ordering::Relaxed);
        let region_start = chunk_number * region_size;
        output_after_index.copy_within(region_start..region_start + compressed_length, position);
        let (chunk_start, chunk_end) = chunk_bounds(input.len(), chunk_size, chunk_number);
        entries[chunk_number] = ChunkEntry {
            compressed_length,
            decompressed_length: chunk_end - chunk_start,
        };
        position += compressed_length;
    }

    Ok(position)
}

fn run_worker(
    input: &[u8],
    chunk_size: usize,
    level: u8,
    regions: &Mutex<Vec<Option<&mut [u8]>>>,
    compressed_lengths: &[AtomicUsize],
    next_chunk_number: &AtomicUsize,
    first_error: &Mutex<Option<EncodeError>>,
) -> Result<(), EncodeError> {
    let mut workspace = EncodeWorkspace::new_boxed_for_level(level)?;
    workspace.prepare_tables_for_input(chunk_size.min(input.len()))?;
    loop {
        let chunk_number = next_chunk_number.fetch_add(1, Ordering::Relaxed);
        if chunk_number >= compressed_lengths.len() || first_error.lock().unwrap().is_some() {
            return Ok(());
        }
        let (chunk_start, chunk_end) = chunk_bounds(input.len(), chunk_size, chunk_number);
        let chunk_content = &input[chunk_start..chunk_end];
        let region = regions.lock().unwrap()[chunk_number]
            .take()
            .ok_or(EncodeError::OutputTooSmall)?;

        workspace.match_finder.reset(chunk_content.len());
        workspace.repeat_offsets = RepeatOffsets::new();

        let compressed_length =
            compress_chunk(chunk_content, FrameFormat::Cosmoz, region, &mut workspace)?;
        compressed_lengths[chunk_number].store(compressed_length, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use crate::decoder::{DecodeWorkspace, decompress};
    use crate::encoder::{
        CompressFormat, CompressOptions, EncodeWorkspace, compress, get_max_compressed_size,
    };
    use std::vec::Vec;

    fn build_input(length: usize) -> Vec<u8> {
        let mut state = 7u64;
        let words: [&[u8]; 6] = [
            b"alpha ", b"beta ", b"gamma ", b"delta ", b"omega ", b"sigma ",
        ];
        let mut input = Vec::with_capacity(length);
        while input.len() < length {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            input.extend_from_slice(words[(state >> 60) as usize % words.len()]);
            input.push((state >> 40) as u8);
        }
        input.truncate(length);
        input
    }

    fn compress_at_level(input: &[u8], level: u8) -> Vec<u8> {
        let options = CompressOptions {
            format: CompressFormat::Cosmoz {
                chunk_size: 64 * 1024,
            },
            with_checksum: false,
            level,
        };
        let mut workspace = EncodeWorkspace::new_boxed_for_level(level).unwrap();
        let mut output = vec![0u8; get_max_compressed_size(input.len(), &options)];
        let written = compress(input, &mut output, &options, &mut workspace).unwrap();
        output.truncate(written);
        output
    }

    #[test]
    fn parallel_chunks_use_the_requested_level() {
        let input = build_input(512 * 1024);
        let level_one = compress_at_level(&input, 1);
        let level_nine = compress_at_level(&input, 9);
        assert!(level_nine.len() < level_one.len());
        let mut decoded = vec![0u8; input.len()];
        let mut decode_workspace = DecodeWorkspace::new_boxed();
        decompress(&level_nine, &mut decoded, &mut decode_workspace).unwrap();
        assert_eq!(decoded, input);
    }
}
