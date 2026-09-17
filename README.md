# cosmoz

Pure Rust compressor and decompressor.

## Features

- `alloc`: boxed workspaces, `new_boxed_for_level`.
- `std`: implies `alloc`; needed by any std consumer on wasm32.
- `parallel`: implies `std`; threads over chunks.
- `wasm-exports`: standalone browser module with C exports and its own panic handler; never enable it when linking cosmoz into another crate.
- `levels` (on by default): compression levels 9 and 12 and block splitting; without it only level 1 compiles.
- `checksum` (on by default): xxhash64/xxhash3 and checksum verification; without it `with_checksum: true` is rejected and the decoder skips verification.
- `encoder` (on by default): the whole encoder; without it the crate is decoder-only.

## Benchmarks

Apple M4. Input split into 4 MB pieces, one frame per piece. Native: single thread, levels 1 and 9. Browser: level 1, pieces spread over the threads. All outputs verified byte for byte against the input.


Files: `iron` is a 154 MB mass spectrometry mzML. `pwiz` is a 5.1 MB mzML.

### Native

| File | Codec | Size L1 | Size L9 | Compress L1 MB/s | Compress L9 MB/s | Decompress L1 MB/s | Decompress L9 MB/s |
|---|---|---|---|---|---|---|---|
| iron | libzstd | 89,883,484 | 89,184,642 | 1530 ± 37 (10) | 488 ± 23 (10) | 1756 ± 24 (10) | 1744 ± 20 (10) |
| iron | ruzstd | 99,249,254 | — | 104 ± 0 (10) | — | 332 ± 1 (10) | — |
| iron | cosmoz/zstd | 90,352,433 | 89,047,245 | 1183 ± 13 (10) | 563 ± 6 (10) | 2176 ± 13 (10) | 2102 ± 14 (10) |
| iron | cosmoz/cosmoz | 90,371,391 | 89,074,910 | 1181 ± 12 (10) | 526 ± 39 (10) | 2149 ± 15 (10) | 2103 ± 27 (10) |
| pwiz | libzstd | 2,788,540 | 1,883,317 | 903 ± 11 (10) | 141 ± 5 (10) | 1343 ± 20 (10) | 1699 ± 50 (10) |
| pwiz | ruzstd | 3,151,638 | — | 98 ± 1 (10) | — | 332 ± 3 (10) | — |
| pwiz | cosmoz/zstd | 2,356,719 | 1,885,497 | 615 ± 6 (10) | 146 ± 4 (10) | 1543 ± 53 (10) | 1587 ± 32 (10) |
| pwiz | cosmoz/cosmoz | 2,357,306 | 1,887,198 | 622 ± 12 (10) | 154 ± 2 (10) | 1629 ± 47 (10) | 1672 ± 48 (10) |

### Browser engine (wasm, Node workers)

Threads are Web Workers. Each piece is copied to a worker and back, which is included in the time. cosmoz and ruzstd are built for wasm32 with `+simd128` and `+bulk-memory` (`www/build.sh`); libzstd is the published Emscripten build.

| File | Codec | Size L1 | Size L9 | Compress L1 MB/s | Compress L9 MB/s | Decompress L1 MB/s | Decompress L9 MB/s |
|---|---|---|---|---|---|---|---|
| iron | libzstd | 89,883,484 | 89,184,642 | 830 ± 25 (10) | 292 ± 9 (10) | 917 ± 5 (10) | 901 ± 19 (10) |
| iron | ruzstd | 99,249,254 | — | 97 ± 2 (10) | — | 253 ± 12 (10) | — |
| iron | cosmoz/zstd | 90,352,433 | 89,047,245 | 797 ± 20 (10) | 357 ± 13 (10) | 967 ± 20 (10) | 999 ± 5 (10) |
| iron | cosmoz/cosmoz | 90,371,391 | 89,074,910 | 803 ± 8 (10) | 358 ± 9 (10) | 964 ± 11 (10) | 977 ± 12 (10) |
| pwiz | libzstd | 2,788,540 | 1,883,317 | 467 ± 20 (10) | 97 ± 1 (10) | 746 ± 14 (10) | 996 ± 43 (10) |
| pwiz | ruzstd | 3,151,638 | — | 90 ± 1 (10) | — | 243 ± 3 (10) | — |
| pwiz | cosmoz/zstd | 2,356,719 | 1,885,497 | 461 ± 12 (10) | 98 ± 1 (10) | 901 ± 41 (10) | 988 ± 54 (10) |
| pwiz | cosmoz/cosmoz | 2,357,306 | 1,887,198 | 471 ± 13 (10) | 97 ± 1 (10) | 869 ± 38 (10) | 940 ± 54 (10) |
