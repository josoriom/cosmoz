# osmo

Pure Rust compressor and decompressor.

## Features

- `alloc`: boxed workspaces, `new_boxed_for_level`.
- `std`: implies `alloc`; needed by any std consumer on wasm32.
- `parallel`: implies `std`; threads over chunks.
- `wasm-exports`: standalone browser module with C exports and its own panic handler; never enable it when linking osmo into another crate.

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

### Levels (native, single thread)

| File | Codec | Level | Size | Ratio | Compress MB/s | Decompress MB/s |
|---|---|---|---|---|---|---|
| iron | libzstd | 1 | 89,852,846 | 1.714 | 1541 ± 28 (10) | 1768 ± 5 (10) |
| iron | osmo/zstd | 1 | 90,029,253 | 1.711 | 981 ± 6 (10) | 1373 ± 2 (10) |
| iron | osmo/osmo | 1 | 90,132,528 | 1.709 | 995 ± 5 (10) | 1702 ± 9 (10) |
| iron | libzstd | 3 | 89,477,389 | 1.721 | 1351 ± 6 (10) | 1741 ± 10 (10) |
| iron | osmo/zstd | 3 | 89,459,677 | 1.722 | 803 ± 6 (10) | 1400 ± 1 (10) |
| iron | osmo/osmo | 3 | 89,485,750 | 1.721 | 820 ± 5 (10) | 1745 ± 7 (10) |
| iron | libzstd | 6 | 89,205,647 | 1.727 | 630 ± 2 (10) | 1740 ± 26 (10) |
| iron | osmo/zstd | 6 | 89,138,660 | 1.728 | 179 ± 1 (10) | 1414 ± 2 (10) |
| iron | osmo/osmo | 6 | 89,186,745 | 1.727 | 178 ± 2 (10) | 1763 ± 9 (10) |
| iron | libzstd | 9 | 89,140,444 | 1.728 | 501 ± 6 (10) | 1749 ± 4 (10) |
| iron | osmo/zstd | 9 | 89,636,301 | 1.718 | 56 ± 1 (10) | 1336 ± 3 (10) |
| iron | osmo/osmo | 9 | 90,090,654 | 1.710 | 72 ± 1 (10) | 1626 ± 68 (10) |
| iron | libzstd | 12 | 89,089,974 | 1.729 | 286 ± 6 (10) | 1756 ± 2 (10) |
| iron | osmo/zstd | 12 | 90,668,495 | 1.699 | 27 ± 0.4 (10) | 1121 ± 3 (10) |
| iron | osmo/osmo | 12 | 90,310,490 | 1.705 | 34 ± 1 (10) | 1447 ± 5 (10) |
| pwiz | libzstd | 1 | 2,788,336 | 1.830 | 864 ± 174 (10) | 1372 ± 21 (10) |
| pwiz | osmo/zstd | 1 | 2,173,721 | 2.348 | 376 ± 4 (10) | 949 ± 12 (10) |
| pwiz | osmo/osmo | 1 | 2,185,374 | 2.335 | 367 ± 1 (10) | 1016 ± 13 (10) |
| pwiz | libzstd | 3 | 2,028,158 | 2.516 | 666 ± 6 (10) | 1750 ± 19 (10) |
| pwiz | osmo/zstd | 3 | 1,995,841 | 2.557 | 325 ± 3 (10) | 1289 ± 16 (10) |
| pwiz | osmo/osmo | 3 | 2,119,080 | 2.408 | 296 ± 4 (10) | 1358 ± 30 (10) |
| pwiz | libzstd | 6 | 1,816,636 | 2.809 | 243 ± 2 (10) | 1894 ± 33 (10) |
| pwiz | osmo/zstd | 6 | 1,927,735 | 2.647 | 81 ± 3 (10) | 1276 ± 19 (10) |
| pwiz | osmo/osmo | 6 | 2,037,308 | 2.505 | 79 ± 3 (10) | 1322 ± 40 (10) |
| pwiz | libzstd | 9 | 1,721,994 | 2.964 | 205 ± 4 (10) | 1984 ± 37 (10) |
| pwiz | osmo/zstd | 9 | 1,715,517 | 2.975 | 62 ± 3 (10) | 1348 ± 20 (10) |
| pwiz | osmo/osmo | 9 | 1,871,680 | 2.727 | 55 ± 0.2 (10) | 1377 ± 30 (10) |
| pwiz | libzstd | 12 | 1,720,177 | 2.967 | 110 ± 5 (10) | 1984 ± 49 (10) |
| pwiz | osmo/zstd | 12 | 1,717,612 | 2.971 | 37 ± 2 (10) | 1301 ± 24 (10) |
| pwiz | osmo/osmo | 12 | 1,867,332 | 2.733 | 32 ± 0.4 (10) | 1309 ± 23 (10) |

Levels 9 and 12 match or beat libzstd on size for pwiz; on iron they are 0.6 to 1.8 percent larger. Levels 5 and 6 on pwiz are 3 to 6 percent larger. Compress speed at levels 6 to 12 is 3x below libzstd on pwiz and 4 to 10x below on iron; decode speed does not change with the level. The osmo format loses 8 percent at levels 6 to 12 on pwiz because the 4 MB chunk reset cuts long-range matches on a 5 MB file.

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
