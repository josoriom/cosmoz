# cosmoz

Pure Rust zstd compressor and decompressor. Zero dependencies, `no_std` core, same code on native and wasm32.

## Install

```bash
cargo add cosmoz --features std,compression
```

| Feature | Default | Enables |
|---|---|---|
| `checksum` | yes | Frame checksums. |
| `compression` | no | Compression, levels 1, 9, 12, 22. Without it the crate only decompresses. |
| `std` | no | `Compressor::to` and `Decompressor::from` over `std::io`. |
| `wasm-exports` | no | The C-ABI functions used by the wasm32 build. |

## Compress and decompress

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data = b"some bytes to compress".repeat(100);
    let packed = cosmoz::compress(&data)?;
    let unpacked = cosmoz::decompress(&packed)?;
    assert_eq!(unpacked, data);
    Ok(())
}
```

## Options

`CompressOptions::default()`:
- `level: 12`
- `checksum: true`

`DecompressOptions::default()`:
- `verify_checksum: true`
- `max_output_size: None`

```rust
use cosmoz::{CompressOptions, DecompressOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data = b"some bytes to compress".repeat(100);

    let compress_options = CompressOptions { level: 12, checksum: false, ..Default::default() };
    let packed = cosmoz::compress_with(&data, &compress_options)?;

    let decompress_options = DecompressOptions { max_output_size: Some(10 * 1024 * 1024), ..Default::default() };
    let unpacked = cosmoz::decompress_with(&packed, &decompress_options)?;

    assert_eq!(unpacked, data);
    Ok(())
}
```

## Streaming 
### **no-std**

`Compressor`/`Decompressor` accept input in any chunk size and buffer internally; `finish` errors if the compressed data ends mid-frame.

`Compressor::new` gives a `Compressor<Bytes>`; `Compressor::to(file, ..)` gives a `Compressor<File>`. Same for `Decompressor`.

```rust
use cosmoz::{CompressOptions, Compressor, DecompressOptions, Decompressor};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data = b"some bytes to compress".repeat(1000);

    let mut compressor = Compressor::new(&CompressOptions::default())?;
    let mut packed = Vec::new();
    for chunk in data.chunks(4096) {
        compressor.write(chunk, &mut packed)?;
    }
    compressor.finish(&mut packed)?;

    let mut decompressor = Decompressor::new(&DecompressOptions::default())?;
    let mut unpacked = Vec::new();
    for chunk in packed.chunks(4096) {
        decompressor.write(chunk, &mut unpacked)?;
    }
    decompressor.finish(&mut unpacked)?;

    assert_eq!(unpacked, data);
    Ok(())
}
```

### **std**

`Compressor::to` and `Decompressor::from` do the same over any `std::io::Write`/`std::io::Read`, such as a file. Here a compressed file is read, decompressed and written back compressed at another level:

```rust
use cosmoz::{CompressOptions, Compressor, DecompressOptions, Decompressor};
use std::io::{Read, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::open("data.zst")?;
    let mut decompressor = Decompressor::from(file, &DecompressOptions::default())?;
    let mut data = Vec::new();
    decompressor.read_to_end(&mut data)?;

    let file = std::fs::File::create("data_copy.zst")?;
    let options = CompressOptions { level: 22, ..Default::default() };
    let mut compressor = Compressor::to(file, &options)?;
    compressor.write_all(&data)?;
    compressor.finish()?;

    Ok(())
}
```

## Buffer reuse 
### **no-std, no-alloc**

`Encoder`/`Decoder` are allocated once and reused across calls for zero allocation per call.

`compress_into` and `decompress_into` allocate nothing per call; create `Encoder`/`Decoder` once and reuse them.

```rust
use cosmoz::{CompressOptions, DecompressOptions, Decoder, Encoder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data = b"some bytes to compress".repeat(100);

    let compress_options = CompressOptions::default();
    let mut encoder = Encoder::new(&compress_options)?;
    let mut packed = vec![0u8; cosmoz::max_compressed_size(data.len(), &compress_options)];
    let written = cosmoz::compress_into(&data, &mut packed, &compress_options, &mut encoder)?;
    packed.truncate(written);

    let decompress_options = DecompressOptions::default();
    let mut decoder = Decoder::new();
    let content_size = cosmoz::decompressed_size(&packed)?.expect("frame has no content size");
    let mut unpacked = vec![0u8; content_size as usize];
    let written = cosmoz::decompress_into(&packed, &mut unpacked, &decompress_options, &mut decoder)?;

    assert_eq!(written, unpacked.len());
    Ok(())
}
```

Benchmarks: [BENCH.md](BENCH.md).
