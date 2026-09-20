# Bench

## cosmoz vs libzstd on mzML

Apple M4, 2026-09-20. Each file is split into 4 MB pieces and each piece is compressed as one frame. Checksums off. The encoder and output buffer are created once per level, outside the timing. The two codecs are interleaved inside every timed run, so a warm machine does not favour whichever one runs first. Each level has one warm-up pass, then 5 timed runs: mean ± standard deviation. Small files are compressed several times per timed run so each run lasts about one second, and the time shown is for one pass. Every output is decoded with libzstd and compared byte for byte with the input. 1 MB = 1,048,576 bytes.

Native: single thread, `rustc` 1.98.1, release profile with LTO, libzstd 1.5.7 from the `zstd` crate 0.13.3. Browser (wasm): Node 26.5.0, one worker per codec, each piece copied to the worker and back with the copy included in the time; cosmoz built for `wasm32-unknown-unknown` with `+simd128` and `+bulk-memory`, libzstd from `@bokuweb/zstd-wasm` 0.0.27 (Emscripten build). The browser level 22 rows for `iron` are measured one codec at a time, because two workers holding level 22 tables at once push this machine into swapping.

### `iron`: large mzML (154,013,962 bytes)

Native:

| Level | Codec | Final size (bytes) | Size vs libzstd | Compress time (ms) | MB/s | Time vs libzstd |
|---|---|---|---|---|---|---|
| 1 | libzstd | 89,883,484 | | 96.833 ± 0.319 | 1516.8 ± 5.0 | |
| 1 | cosmoz | 89,869,342 | −14,142 (−0.02%) | 112.658 ± 0.330 | 1303.8 ± 3.8 | 1.16× slower |
| 9 | libzstd | 89,184,642 | | 285.029 ± 2.932 | 515.4 ± 5.3 | |
| 9 | cosmoz | 89,045,313 | −139,329 (−0.16%) | 258.229 ± 1.240 | 568.8 ± 2.7 | 1.10× faster |
| 12 | libzstd | 89,135,068 | | 500.996 ± 8.453 | 293.3 ± 5.1 | |
| 12 | cosmoz | 89,001,212 | −133,856 (−0.15%) | 444.910 ± 10.501 | 330.3 ± 7.8 | 1.13× faster |
| 22 | libzstd | 89,266,091 | | 23258.755 ± 29.814 | 6.3 ± 0.0 | |
| 22 | cosmoz | 89,295,233 | +29,142 (+0.03%) | 22231.404 ± 11.149 | 6.6 ± 0.0 | 1.05× faster |

Browser (wasm):

| Level | Codec | Final size (bytes) | Size vs libzstd | Compress time (ms) | MB/s | Time vs libzstd |
|---|---|---|---|---|---|---|
| 1 | libzstd | 89,883,484 | | 455.757 ± 1.757 | 322.3 ± 1.2 | |
| 1 | cosmoz | 89,869,342 | −14,142 (−0.02%) | 443.473 ± 0.787 | 331.2 ± 0.6 | 1.03× faster |
| 9 | libzstd | 89,184,642 | | 876.066 ± 1.297 | 167.7 ± 0.2 | |
| 9 | cosmoz | 89,045,313 | −139,329 (−0.16%) | 672.997 ± 7.747 | 218.3 ± 2.5 | 1.30× faster |
| 12 | libzstd | 89,135,068 | | 1608.883 ± 51.379 | 91.4 ± 2.8 | |
| 12 | cosmoz | 89,001,212 | −133,856 (−0.15%) | 1122.926 ± 42.003 | 131.0 ± 5.0 | 1.43× faster |
| 22 | libzstd | 89,266,091 | | 34611.210 ± 139.215 | 4.2 ± 0.0 | |
| 22 | cosmoz | 89,295,233 | +29,142 (+0.03%) | 30346.929 ± 17.147 | 4.8 ± 0.0 | 1.14× faster |

### `pwiz`: small mzML (5,103,183 bytes)

Native:

| Level | Codec | Final size (bytes) | Size vs libzstd | Compress time (ms) | MB/s | Time vs libzstd |
|---|---|---|---|---|---|---|
| 1 | libzstd | 2,788,540 | | 5.230 ± 0.006 | 930.6 ± 1.0 | |
| 1 | cosmoz | 2,793,542 | +5,002 (+0.18%) | 6.085 ± 0.009 | 799.8 ± 1.2 | 1.16× slower |
| 9 | libzstd | 1,883,317 | | 28.565 ± 0.782 | 170.5 ± 4.5 | |
| 9 | cosmoz | 1,885,492 | +2,175 (+0.12%) | 29.729 ± 0.239 | 163.7 ± 1.3 | 1.04× slower |
| 12 | libzstd | 1,881,850 | | 56.752 ± 0.309 | 85.8 ± 0.5 | |
| 12 | cosmoz | 1,884,102 | +2,252 (+0.12%) | 58.446 ± 0.575 | 83.3 ± 0.8 | 1.03× slower |
| 22 | libzstd | 1,671,492 | | 453.832 ± 3.841 | 10.7 ± 0.1 | |
| 22 | cosmoz | 1,674,417 | +2,925 (+0.17%) | 438.986 ± 3.423 | 11.1 ± 0.1 | 1.03× faster |

Browser (wasm):

| Level | Codec | Final size (bytes) | Size vs libzstd | Compress time (ms) | MB/s | Time vs libzstd |
|---|---|---|---|---|---|---|
| 1 | libzstd | 2,788,540 | | 9.636 ± 0.125 | 505.2 ± 6.5 | |
| 1 | cosmoz | 2,793,542 | +5,002 (+0.18%) | 8.348 ± 0.129 | 583.2 ± 8.9 | 1.15× faster |
| 9 | libzstd | 1,883,317 | | 52.424 ± 0.276 | 92.8 ± 0.5 | |
| 9 | cosmoz | 1,885,492 | +2,175 (+0.12%) | 45.430 ± 0.178 | 107.1 ± 0.4 | 1.15× faster |
| 12 | libzstd | 1,881,850 | | 131.210 ± 1.311 | 37.1 ± 0.4 | |
| 12 | cosmoz | 1,884,102 | +2,252 (+0.12%) | 147.771 ± 1.443 | 32.9 ± 0.3 | 1.13× slower |
| 22 | libzstd | 1,671,492 | | 652.750 ± 2.141 | 7.5 ± 0.0 | |
| 22 | cosmoz | 1,674,417 | +2,925 (+0.17%) | 554.044 ± 1.450 | 8.8 ± 0.0 | 1.18× faster |
