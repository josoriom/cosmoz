# osmos

Pure Rust compressor and decompressor.

## Features

- `alloc`: boxed workspaces, `new_boxed_for_level`.
- `std`: implies `alloc`; needed by any std consumer on wasm32.
- `parallel`: implies `std`; threads over chunks.
- `wasm-exports`: standalone browser module with C exports and its own panic handler; never enable it when linking osmos into another crate.
- `levels` (on by default): compression levels 2-12 and block splitting; without it only level 1 compiles.
- `checksum` (on by default): xxhash64/xxhash3 and checksum verification; without it `with_checksum: true` is rejected and the decoder skips verification.
- `encoder` (on by default): the whole encoder; without it the crate is decoder-only.

## Benchmarks

Apple M4. Input split into 4 MB pieces, one frame per piece. Native: single thread, levels 1 and 9. Browser: level 1, pieces spread over the threads. All outputs verified byte for byte against the input.


Files: `iron` is a 154 MB mass spectrometry mzML. `pwiz` is a 5.1 MB mzML.

### Native

| File | Codec | Size L1 | Size L9 | Compress L1 MB/s | Compress L9 MB/s | Decompress L1 MB/s | Decompress L9 MB/s |
|---|---|---|---|---|---|---|---|
| iron | libzstd | 89,883,484 | 89,184,642 | 1492 ± 83 (10) | 545 ± 11 (10) | 1782 ± 53 (10) | 1846 ± 26 (10) |
| iron | ruzstd | 99,249,402 | — | 107 ± 1 (10) | — | 337 ± 1 (10) | — |
| iron | osmos/zstd | 90,066,036 | 89,047,393 | 984 ± 12 (10) | 571 ± 6 (10) | 1959 ± 11 (10) | 2013 ± 21 (10) |
| iron | osmos/osmos | 90,085,370 | 89,075,206 | 993 ± 11 (10) | 554 ± 18 (10) | 1964 ± 12 (10) | 2061 ± 15 (10) |
| pwiz | libzstd | 2,788,540 | 1,883,317 | 915 ± 15 (10) | 166 ± 9 (10) | 1343 ± 28 (10) | 1815 ± 40 (10) |
| pwiz | ruzstd | 3,151,646 | — | 93 ± 12 (10) | — | 321 ± 3 (10) | — |
| pwiz | osmos/zstd | 2,184,729 | 1,885,505 | 377 ± 4 (10) | 148 ± 25 (10) | 1099 ± 15 (10) | 1527 ± 45 (10) |
| pwiz | osmos/osmos | 2,185,364 | 1,887,214 | 372 ± 8 (10) | 160 ± 2 (10) | 1131 ± 24 (10) | 1636 ± 41 (10) |

### Browser engine (wasm, Node workers)

Threads are Web Workers. Each piece is copied to a worker and back, which is included in the time. osmos and ruzstd are built for wasm32 with `+simd128` and `+bulk-memory` (`www/build.sh`); libzstd is the published Emscripten build.

| File | Codec | Threads | Size | Ratio | Compress MB/s | Decompress MB/s |
|---|---|---|---|---|---|---|
| iron | libzstd | 1 | 89,883,484 | 1.713 | 339 ± 11 (10) | 995 ± 37 (10) |
| iron | ruzstd | 1 | 99,249,402 | 1.552 | 87 ± 1 (10) | 290 ± 2 (10) |
| iron | osmos/zstd | 1 | 90,143,276 | 1.709 | 316 ± 3 (10) | 1083 ± 12 (10) |
| iron | osmos/osmos | 1 | 90,162,459 | 1.708 | 315 ± 3 (10) | 1143 ± 17 (10) |
| iron | libzstd | 10 | 89,883,484 | 1.713 | 517 ± 128 (10) | 4227 ± 667 (10) |
| iron | ruzstd | 10 | 99,249,402 | 1.552 | 338 ± 73 (10) | 1714 ± 203 (10) |
| iron | osmos/zstd | 10 | 90,143,276 | 1.709 | 548 ± 26 (10) | 4844 ± 293 (10) |
| iron | osmos/osmos | 10 | 90,162,459 | 1.708 | 551 ± 25 (10) | 4688 ± 255 (10) |
| pwiz | libzstd | 1 | 2,788,540 | 1.830 | 468 ± 56 (10) | 773 ± 107 (10) |
| pwiz | ruzstd | 1 | 3,151,646 | 1.619 | 89 ± 9 (10) | 262 ± 23 (10) |
| pwiz | osmos/zstd | 1 | 2,184,765 | 2.336 | 269 ± 25 (10) | 694 ± 76 (10) |
| pwiz | osmos/osmos | 1 | 2,185,394 | 2.335 | 276 ± 25 (10) | 743 ± 94 (10) |
| pwiz | libzstd | 10 | 2,788,540 | 1.830 | 568 ± 71 (10) | 952 ± 135 (10) |
| pwiz | ruzstd | 10 | 3,151,646 | 1.619 | 109 ± 14 (10) | 305 ± 30 (10) |
| pwiz | osmos/zstd | 10 | 2,184,765 | 2.336 | 352 ± 41 (10) | 920 ± 115 (10) |
| pwiz | osmos/osmos | 10 | 2,185,394 | 2.335 | 346 ± 40 (10) | 915 ± 118 (10) |
