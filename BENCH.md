# Bench

## cosmoz vs libzstd on mzML

Apple M4, 2026-09-19. Each file is split into 4 MB pieces and each piece is compressed as one frame. Checksums off. The encoder and output buffer are created once per level, outside the timing. Each level has one warm-up pass, then 10 timed runs: mean ± standard deviation. Small files are compressed several times per timed run so each run lasts about one second, and the time shown is for one pass. Every output is decoded with libzstd and compared byte for byte with the input. 1 MB = 1,048,576 bytes.

Native: single thread, `rustc` 1.98.1, release profile with LTO, libzstd 1.5.7 from the `zstd` crate 0.13.3. Browser (wasm): Node 26.5.0, one worker per codec, each piece copied to the worker and back with the copy included in the time; cosmoz built for `wasm32-unknown-unknown` with `+simd128` and `+bulk-memory`, libzstd from `@bokuweb/zstd-wasm` 0.0.27 (Emscripten build).

### `iron`: large mzML (154,013,962 bytes)

Native:

| Level | Codec | Final size (bytes) | Size vs libzstd | Compress time (ms) | MB/s | Time vs libzstd |
|---|---|---|---|---|---|---|
| 1 | libzstd | 89,883,484 | | 95.423 ± 0.141 | 1539.2 ± 2.3 | |
| 1 | cosmoz | 89,870,863 | −12,621 (−0.01%) | 111.935 ± 0.118 | 1312.2 ± 1.4 | 1.17× slower |
| 9 | libzstd | 89,184,642 | | 262.813 ± 0.716 | 558.9 ± 1.5 | |
| 9 | cosmoz | 89,047,245 | −137,397 (−0.15%) | 239.102 ± 1.041 | 614.3 ± 2.7 | 1.10× faster |
| 12 | libzstd | 89,135,068 | | 447.844 ± 5.282 | 328.0 ± 3.8 | |
| 12 | cosmoz | 89,017,438 | −117,630 (−0.13%) | 344.093 ± 1.089 | 426.9 ± 1.4 | 1.30× faster |
| 22 | libzstd | 89,266,091 | | 22488.433 ± 12.956 | 6.5 ± 0.0 | |
| 22 | cosmoz | 89,316,476 | +50,385 (+0.06%) | 27830.625 ± 11.054 | 5.3 ± 0.0 | 1.24× slower |

Browser (wasm):

| Level | Codec | Final size (bytes) | Size vs libzstd | Compress time (ms) | MB/s | Time vs libzstd |
|---|---|---|---|---|---|---|
| 1 | libzstd | 89,883,484 | | 430.022 ± 0.527 | 341.6 ± 0.4 | |
| 1 | cosmoz | 89,870,863 | −12,621 (−0.01%) | 420.758 ± 0.549 | 349.1 ± 0.5 | 1.02× faster |
| 9 | libzstd | 89,184,642 | | 829.891 ± 0.751 | 177.0 ± 0.2 | |
| 9 | cosmoz | 89,047,245 | −137,397 (−0.15%) | 642.442 ± 0.824 | 228.6 ± 0.3 | 1.29× faster |
| 12 | libzstd | 89,135,068 | | 1436.811 ± 11.718 | 102.2 ± 0.8 | |
| 12 | cosmoz | 89,017,438 | −117,630 (−0.13%) | 915.776 ± 2.456 | 160.4 ± 0.4 | 1.57× faster |
| 22 | libzstd | 89,266,091 | | 33148.910 ± 35.910 | 4.4 ± 0.0 | |
| 22 | cosmoz | 89,316,476 | +50,385 (+0.06%) | 70770.263 ± 104033.539 | 3.7 ± 1.1 | 2.13× slower |

### `pwiz`: small mzML (5,103,183 bytes)

Native:

| Level | Codec | Final size (bytes) | Size vs libzstd | Compress time (ms) | MB/s | Time vs libzstd |
|---|---|---|---|---|---|---|
| 1 | libzstd | 2,788,540 | | 5.105 ± 0.019 | 953.4 ± 3.5 | |
| 1 | cosmoz | 2,793,554 | +5,014 (+0.18%) | 6.501 ± 0.028 | 748.7 ± 3.2 | 1.27× slower |
| 9 | libzstd | 1,883,317 | | 27.220 ± 0.471 | 178.8 ± 3.1 | |
| 9 | cosmoz | 1,885,497 | +2,180 (+0.12%) | 30.106 ± 0.457 | 161.7 ± 2.5 | 1.11× slower |
| 12 | libzstd | 1,881,850 | | 55.108 ± 1.086 | 88.3 ± 1.8 | |
| 12 | cosmoz | 1,884,169 | +2,319 (+0.12%) | 61.079 ± 0.919 | 79.7 ± 1.2 | 1.11× slower |
| 22 | libzstd | 1,671,492 | | 445.601 ± 5.521 | 10.9 ± 0.1 | |
| 22 | cosmoz | 1,674,469 | +2,977 (+0.18%) | 499.942 ± 8.021 | 9.7 ± 0.2 | 1.12× slower |

Browser (wasm):

| Level | Codec | Final size (bytes) | Size vs libzstd | Compress time (ms) | MB/s | Time vs libzstd |
|---|---|---|---|---|---|---|
| 1 | libzstd | 2,788,540 | | 9.680 ± 0.025 | 502.8 ± 1.3 | |
| 1 | cosmoz | 2,793,554 | +5,014 (+0.18%) | 8.402 ± 0.025 | 579.2 ± 1.7 | 1.15× faster |
| 9 | libzstd | 1,883,317 | | 51.368 ± 0.173 | 94.7 ± 0.3 | |
| 9 | cosmoz | 1,885,497 | +2,180 (+0.12%) | 46.645 ± 0.220 | 104.3 ± 0.5 | 1.10× faster |
| 12 | libzstd | 1,881,850 | | 112.642 ± 0.314 | 43.2 ± 0.1 | |
| 12 | cosmoz | 1,884,169 | +2,319 (+0.12%) | 156.114 ± 0.485 | 31.2 ± 0.1 | 1.39× slower |
| 22 | libzstd | 1,671,492 | | 619.175 ± 1.439 | 7.9 ± 0.0 | |
| 22 | cosmoz | 1,674,469 | +2,977 (+0.18%) | 587.881 ± 1.187 | 8.3 ± 0.0 | 1.05× faster |
