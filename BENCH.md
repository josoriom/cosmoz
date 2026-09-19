# Bench

## cosmoz vs libzstd

Apple M4, 2026-09-19. File: `iron`, a 154,013,962-byte mass spectrometry mzML. The file is split into 4 MB pieces and each piece is compressed as one frame. Checksums off. Each level has one warm-up run, then 10 timed runs: mean ± standard deviation. Every output is decoded with libzstd and compared byte for byte with the input. 1 MB = 1,048,576 bytes.

Codecs: `libzstd` 1.5.7 (the `zstd` crate 0.13.3 natively, `@bokuweb/zstd-wasm` 0.0.27 in wasm). `cosmoz` from this repository.

### Native

iron mzML (154,013,962 bytes), 4 MB frames, 10 runs, Apple M4, single thread. `rustc` 1.98.1, release profile with LTO.

| Level | Codec | Final size (bytes) | Size vs libzstd | Compress time (s) | MB/s | Time vs libzstd |
|---|---|---|---|---|---|---|
| 1 | libzstd | 89,883,484 | | 0.090 ± 0.003 | 1626.7 ± 49.9 | |
| 1 | cosmoz | 90,352,433 | +468,949 (+0.52%) | 0.115 ± 0.000 | 1275.6 ± 3.9 | 1.28× slower |
| 9 | libzstd | 89,184,642 | | 0.261 ± 0.013 | 563.6 ± 25.4 | |
| 9 | cosmoz | 89,047,245 | −137,397 (−0.15%) | 0.239 ± 0.005 | 615.7 ± 11.9 | 1.09× faster |
| 12 | libzstd | 89,135,068 | | 0.456 ± 0.023 | 322.7 ± 15.8 | |
| 12 | cosmoz | 89,017,438 | −117,630 (−0.13%) | 0.362 ± 0.008 | 405.9 ± 9.2 | 1.26× faster |
| 22 | libzstd | 89,266,091 | | 23.032 ± 0.123 | 6.4 ± 0.0 | |
| 22 | cosmoz | 89,316,476 | +50,385 (+0.06%) | 28.190 ± 0.035 | 5.2 ± 0.0 | 1.22× slower |

### Browser engine (wasm, Node worker)

iron mzML (154,013,962 bytes), 4 MB frames, 10 runs, Apple M4, Node 26.5.0. One worker per codec. Each piece is copied to the worker and back, and the copy is included in the time. cosmoz is built for `wasm32-unknown-unknown` with `+simd128` and `+bulk-memory`. libzstd is the published Emscripten build (`@bokuweb/zstd-wasm`). Sizes are the same as native.

| Level | Codec | Final size (bytes) | Size vs libzstd | Compress time (s) | MB/s | Time vs libzstd |
|---|---|---|---|---|---|---|
| 1 | libzstd | 89,883,484 | | 0.458 ± 0.012 | 321.0 ± 8.3 | |
| 1 | cosmoz | 90,352,433 | +468,949 (+0.52%) | 0.456 ± 0.025 | 322.7 ± 16.2 | 1.00× (same) |
| 9 | libzstd | 89,184,642 | | 0.851 ± 0.014 | 172.7 ± 2.8 | |
| 9 | cosmoz | 89,047,245 | −137,397 (−0.15%) | 0.665 ± 0.013 | 221.1 ± 4.3 | 1.28× faster |
| 12 | libzstd | 89,135,068 | | 1.509 ± 0.041 | 97.4 ± 2.7 | |
| 12 | cosmoz | 89,017,438 | −117,630 (−0.13%) | 0.972 ± 0.022 | 151.2 ± 3.5 | 1.55× faster |
| 22 | libzstd | 89,266,091 | | 34.238 ± 0.101 | 4.3 ± 0.0 | |
| 22 | cosmoz | 89,316,476 | +50,385 (+0.06%) | 36.407 ± 0.458 | 4.0 ± 0.0 | 1.06× slower |
