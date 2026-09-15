# osmo

Pure Rust compressor and decompressor.

## Benchmarks

Apple M4, level 1. Input split into 4 MB pieces, one frame per piece, pieces spread over the threads. All outputs verified byte for byte against the input.


Files: `iron` is a 154 MB mass spectrometry mzML. `pwiz` is a 5.1 MB mzML.

### Native

| File | Codec | Threads | Size | Ratio | Compress MB/s | Decompress MB/s |
|---|---|---|---|---|---|---|
| iron | libzstd | 1 | 89,883,484 | 1.713 | 1695 ± 72 (10) | 1880 ± 28 (10) |
| iron | ruzstd | 1 | 99,249,402 | 1.552 | 116 ± 1 (10) | 359 ± 3 (10) |
| iron | osmo/zstd | 1 | 90,143,276 | 1.709 | 1109 ± 10 (10) | 1444 ± 10 (10) |
| iron | osmo/osmo | 1 | 90,162,459 | 1.708 | 1102 ± 8 (10) | 1689 ± 14 (10) |
| iron | libzstd | 10 | 89,883,484 | 1.713 | 8546 ± 200 (10) | 10516 ± 250 (10) |
| iron | ruzstd | 10 | 99,249,402 | 1.552 | 571 ± 12 (10) | 2274 ± 35 (10) |
| iron | osmo/zstd | 10 | 90,143,276 | 1.709 | 5597 ± 127 (10) | 8380 ± 186 (10) |
| iron | osmo/osmo | 10 | 90,162,459 | 1.708 | 5548 ± 153 (10) | 8755 ± 261 (10) |
| pwiz | libzstd | 1 | 2,788,540 | 1.830 | 957 ± 21 (10) | 1441 ± 43 (10) |
| pwiz | ruzstd | 1 | 3,151,646 | 1.619 | 108 ± 1 (10) | 341 ± 5 (10) |
| pwiz | osmo/zstd | 1 | 2,184,765 | 2.336 | 393 ± 3 (10) | 984 ± 19 (10) |
| pwiz | osmo/osmo | 1 | 2,185,394 | 2.335 | 392 ± 4 (10) | 1051 ± 20 (10) |
| pwiz | libzstd | 10 | 2,788,540 | 1.830 | 1181 ± 18 (10) | 1791 ± 62 (10) |
| pwiz | ruzstd | 10 | 3,151,646 | 1.619 | 130 ± 1 (10) | 396 ± 4 (10) |
| pwiz | osmo/zstd | 10 | 2,184,765 | 2.336 | 511 ± 6 (10) | 1246 ± 34 (10) |
| pwiz | osmo/osmo | 10 | 2,185,394 | 2.335 | 505 ± 7 (10) | 1328 ± 31 (10) |

### Browser engine (wasm, Node workers)

Threads are Web Workers. Each piece is copied to a worker and back, which is included in the time. osmo and ruzstd are built for wasm32 with `+simd128` and `+bulk-memory` (`www/build.sh`); libzstd is the published Emscripten build.

| File | Codec | Threads | Size | Ratio | Compress MB/s | Decompress MB/s |
|---|---|---|---|---|---|---|
| iron | libzstd | 1 | 89,883,484 | 1.713 | 339 ± 11 (10) | 995 ± 37 (10) |
| iron | ruzstd | 1 | 99,249,402 | 1.552 | 87 ± 1 (10) | 290 ± 2 (10) |
| iron | osmo/zstd | 1 | 90,143,276 | 1.709 | 316 ± 3 (10) | 1083 ± 12 (10) |
| iron | osmo/osmo | 1 | 90,162,459 | 1.708 | 315 ± 3 (10) | 1143 ± 17 (10) |
| iron | libzstd | 10 | 89,883,484 | 1.713 | 517 ± 128 (10) | 4227 ± 667 (10) |
| iron | ruzstd | 10 | 99,249,402 | 1.552 | 338 ± 73 (10) | 1714 ± 203 (10) |
| iron | osmo/zstd | 10 | 90,143,276 | 1.709 | 548 ± 26 (10) | 4844 ± 293 (10) |
| iron | osmo/osmo | 10 | 90,162,459 | 1.708 | 551 ± 25 (10) | 4688 ± 255 (10) |
| pwiz | libzstd | 1 | 2,788,540 | 1.830 | 468 ± 56 (10) | 773 ± 107 (10) |
| pwiz | ruzstd | 1 | 3,151,646 | 1.619 | 89 ± 9 (10) | 262 ± 23 (10) |
| pwiz | osmo/zstd | 1 | 2,184,765 | 2.336 | 269 ± 25 (10) | 694 ± 76 (10) |
| pwiz | osmo/osmo | 1 | 2,185,394 | 2.335 | 276 ± 25 (10) | 743 ± 94 (10) |
| pwiz | libzstd | 10 | 2,788,540 | 1.830 | 568 ± 71 (10) | 952 ± 135 (10) |
| pwiz | ruzstd | 10 | 3,151,646 | 1.619 | 109 ± 14 (10) | 305 ± 30 (10) |
| pwiz | osmo/zstd | 10 | 2,184,765 | 2.336 | 352 ± 41 (10) | 920 ± 115 (10) |
| pwiz | osmo/osmo | 10 | 2,185,394 | 2.335 | 346 ± 40 (10) | 915 ± 118 (10) |
