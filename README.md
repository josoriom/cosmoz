# cosmoz

Pure Rust zstd compressor and decompressor.

## Install

```bash
cargo add cosmoz --features compression
```

| Feature | Default | Enables |
|---|---|---|
| `checksum` | no | Frame checksums. |
| `compression` | no | Compression, levels 1, 9, 12, 22. Without it the crate only decompresses. |
| `std` | yes | `Compressor::write_to` and `Decompressor::read_from` over `std::io`. |
| `wasm-exports` | no | The C-ABI functions used by the wasm32 build. Turns on `std`. |

The core is `no_std`, but cosmoz always needs an allocator. A tag of "no allocation per call" means the call itself allocates nothing once the `Encoder` or `Decoder` exists.

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
- `checksum: false`, or `true` with the `checksum` feature

`DecompressOptions::default()`:
- `verify_checksum: false`, or `true` with the `checksum` feature
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

`Compressor::new` gives a `Compressor<RawBytes>`; `Compressor::write_to(file, ..)` gives a `Compressor<File>`. Same for `Decompressor`.

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

`Compressor::write_to` and `Decompressor::read_from` do the same over any `std::io::Write`/`std::io::Read`, such as a file.

Always call `finish()`. It writes the end of the frame and returns any write error. A `Compressor` dropped without `finish()` leaves a truncated frame that no decoder accepts.

Here a compressed file is read, decompressed and written back compressed at another level:

```rust
use cosmoz::{CompressOptions, Compressor, DecompressOptions, Decompressor};
use std::io::{Read, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::open("data.zst")?;
    let mut decompressor = Decompressor::read_from(file, &DecompressOptions::default())?;
    let mut data = Vec::new();
    decompressor.read_to_end(&mut data)?;

    let file = std::fs::File::create("data_copy.zst")?;
    let options = CompressOptions { level: 22, ..Default::default() };
    let mut compressor = Compressor::write_to(file, &options)?;
    compressor.write_all(&data)?;
    compressor.finish()?;

    Ok(())
}
```

## Buffer reuse 
### **no-std, no allocation per call**

`Encoder`/`Decoder` are allocated once and reused across calls for zero allocation per call.

`encoder.compress_into` and `decoder.decompress_into` allocate nothing per call; create `Encoder`/`Decoder` once with your options and reuse them.

```rust
use cosmoz::{CompressOptions, DecompressOptions, Decoder, Encoder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data = b"some bytes to compress".repeat(100);

    let mut encoder = Encoder::new(&CompressOptions::default())?;
    let mut packed = vec![0u8; encoder.max_compressed_size(data.len())];
    let written = encoder.compress_into(&data, &mut packed)?;
    packed.truncate(written);

    let mut decoder = Decoder::new(&DecompressOptions::default());
    let content_size = cosmoz::decompressed_size(&packed)?.expect("frame has no content size");
    let mut unpacked = vec![0u8; content_size as usize];
    let written = decoder.decompress_into(&packed, &mut unpacked)?;

    assert_eq!(written, unpacked.len());
    Ok(())
}
```

Benchmarks: [BENCH.md](BENCH.md).
