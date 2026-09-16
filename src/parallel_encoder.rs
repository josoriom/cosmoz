use core::sync::atomic::{AtomicUsize, Ordering};
use std::{sync::Mutex, thread, vec, vec::Vec};

use crate::{
    block::repeat_offsets::RepeatOffsets,
    encode_error::EncodeError,
    encoder::{EncodeWorkspace, compress_chunk},
    frame::{
        block_header::{BLOCK_HEADER_LENGTH, MAX_BLOCK_SIZE},
        chunk_index::ChunkEntry,
        frame_header::FrameFormat,
    },
    match_finder::MatchFinder,
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
    output_after_index: &mut [u8],
    entries: &mut [ChunkEntry],
) -> Result<usize, EncodeError> {
    let chunk_count = entries.len();
    if chunk_count == 0 {
        return Err(EncodeError::BadOptions);
    }

    let chunk_slots: Mutex<Vec<Option<Vec<u8>>>> =
        Mutex::new((0..chunk_count).map(|_| None).collect());
    let next_chunk_number = AtomicUsize::new(0);
    let first_error: Mutex<Option<EncodeError>> = Mutex::new(None);

    let thread_count = thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
        .min(chunk_count);

    thread::scope(|scope| {
        for _ in 0..thread_count {
            scope.spawn(|| {
                let mut workspace = EncodeWorkspace::new_boxed();
                loop {
                    let chunk_number = next_chunk_number.fetch_add(1, Ordering::Relaxed);
                    if chunk_number >= chunk_count {
                        break;
                    }

                    if first_error.lock().unwrap().is_some() {
                        break;
                    }

                    let (chunk_start, chunk_end) =
                        chunk_bounds(input.len(), chunk_size, chunk_number);
                    let chunk_content = &input[chunk_start..chunk_end];

                    workspace.match_finder.reset();
                    workspace.repeat_offsets = RepeatOffsets::new();

                    let mut scratch = vec![0u8; max_chunk_compressed_size(chunk_content.len())];
                    let result = compress_chunk(
                        chunk_content,
                        FrameFormat::Osmos,
                        &mut scratch,
                        &mut workspace,
                    );

                    match result {
                        Ok(compressed_length) => {
                            scratch.truncate(compressed_length);
                            chunk_slots.lock().unwrap()[chunk_number] = Some(scratch);
                        }
                        Err(error) => {
                            let mut first_error = first_error.lock().unwrap();
                            if first_error.is_none() {
                                *first_error = Some(error);
                            }
                            break;
                        }
                    }
                }
            });
        }
    });

    if let Some(error) = first_error.into_inner().unwrap() {
        return Err(error);
    }

    let chunk_slots = chunk_slots.into_inner().unwrap();
    let mut position = 0usize;
    for chunk_number in 0..chunk_count {
        let chunk_bytes = chunk_slots[chunk_number]
            .as_ref()
            .expect("chunk output slot missing");
        let (chunk_start, chunk_end) = chunk_bounds(input.len(), chunk_size, chunk_number);
        let output_slice = output_after_index
            .get_mut(position..position + chunk_bytes.len())
            .ok_or(EncodeError::OutputTooSmall)?;
        output_slice.copy_from_slice(chunk_bytes);
        entries[chunk_number] = ChunkEntry {
            compressed_length: chunk_bytes.len(),
            decompressed_length: chunk_end - chunk_start,
        };
        position += chunk_bytes.len();
    }

    Ok(position)
}
