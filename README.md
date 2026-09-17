# cosmoz

Pure Rust zstd compressor and decompressor. Zero dependencies, `no_std` core, same code on native and wasm32.

## Install

```bash
cargo add cosmoz --features std
```

Features:

| Feature | Default | Enables |
|---|---|---|
| `encoder` | yes | Compression. Without it the crate only decodes. |
| `levels` | yes | Levels 9, 12 and 22 (9–22 also need `alloc`). Without it only level 1. |
| `checksum` | yes | Frame checksums and their verification. |
| `alloc` | no | Heap workspaces (`new_boxed`, `new_boxed_for_level`) and `StreamEncoder`. |
| `std` | no | `alloc` plus `StreamWriter` (`std::io::Write`). |

## Usage

Compression levels: `1`, `9`, `12`, `22`.

`CompressOptions::default()`: zstd frame, level 12, checksum on.

Frame formats (`CompressOptions::format`):

- `CompressFormat::Zstd`: standard zstd frame, readable by any zstd decoder. `CompressOptions::zstd()`.

### Compress

```rust
use cosmoz::{CompressOptions, EncodeWorkspace, compress, get_max_compressed_size};

let input = std::fs::read("data.mzML").unwrap();

let options = CompressOptions {
    level: 12,
    ..Default::default()
};

let mut workspace = EncodeWorkspace::new_boxed_for_level(options.level).unwrap();
let mut frame = vec![0u8; get_max_compressed_size(input.len(), &options)];
let written = compress(&input, &mut frame, &options, &mut workspace).unwrap();
frame.truncate(written);
```

Reuse the workspace for the next input at the same level. Creating it allocates the match tables.

### Decompress

```rust
use cosmoz::{DecodeWorkspace, decompress, get_decompressed_size};

let content_size = get_decompressed_size(&frame).unwrap().expect("frame has no content size");

let mut workspace = DecodeWorkspace::new_boxed();
let mut output = vec![0u8; content_size as usize];
let written = decompress(&frame, &mut output, &mut workspace).unwrap();
assert_eq!(written, output.len());
```

`decompress` reads zstd and cosmoz frames, and several concatenated frames. The output buffer must be large enough for the whole content.

### Stream

```rust
use std::io::Write;
use cosmoz::StreamWriter;

let file = std::fs::File::create("data.mzML.zst").unwrap();
let mut writer = StreamWriter::new(file, 12).unwrap();
writer.write_all(b"first part").unwrap();
writer.write_all(b"second part").unwrap();
let file = writer.finish().unwrap();
```

`StreamWriter` writes one zstd frame without a content size. Call `finish` to write the last block; dropping the writer without it leaves an incomplete frame. Without `std`, use `StreamEncoder::write` and `StreamEncoder::finish` with a `Vec<u8>` output.

### `no_std` without `alloc`

Only level 1. `EncodeWorkspace::new()` and `DecodeWorkspace::new()` are `const`, so the workspaces can live in a `static`.

Benchmarks: [BENCH.md](BENCH.md).