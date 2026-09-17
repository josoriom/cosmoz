# Bench

## cosmoz vs libzstd and ruzstd

Apple M4, 2026-09-17. Each file is split into 4 MB pieces and each piece is compressed as one frame. Checksums off. Levels 1 and 9: mean ± standard deviation of 10 runs. Level 22: 3 runs. Every output is decoded and compared byte for byte with the input.

Files: `iron` is a 154 MB mass spectrometry mzML. `pwiz` is a 5.1 MB mzML.

Codecs: `libzstd` 1.5.7 (the `zstd` crate). `ruzstd` 0.9 has only one level (`Fastest`). `cosmoz/zstd` writes zstd frames, `cosmoz/cosmoz` writes cosmoz frames.

### Native

Single thread.

| File | Codec | Size L1 | Size L9 | Size L22 | Compress L1 MB/s | Compress L9 MB/s | Compress L22 MB/s | Decompress L1 MB/s | Decompress L9 MB/s | Decompress L22 MB/s |
|---|---|---|---|---|---|---|---|---|---|---|
| iron | libzstd | 89,883,484 | 89,184,642 | 89,266,091 | 1570 ± 53 (10) | 552 ± 13 (10) | 6 ± 0 (3) | 1855 ± 35 (10) | 1825 ± 12 (10) | 1623 ± 12 (3) |
| iron | ruzstd | 99,249,254 | — | — | 106 ± 0 (10) | — | — | 332 ± 2 (10) | — | — |
| iron | cosmoz/zstd | 90,352,433 | 89,047,245 | 89,316,476 | 1176 ± 13 (10) | 560 ± 11 (10) | 5 ± 0 (3) | 2177 ± 15 (10) | 2118 ± 20 (10) | 1839 ± 18 (3) |
| iron | cosmoz/cosmoz | 90,371,391 | 89,074,910 | 89,405,564 | 1178 ± 13 (10) | 559 ± 15 (10) | 5 ± 0 (3) | 2163 ± 14 (10) | 2182 ± 19 (10) | 1748 ± 21 (3) |
| pwiz | libzstd | 2,788,540 | 1,883,317 | 1,671,492 | 924 ± 10 (10) | 170 ± 2 (10) | 11 ± 0 (3) | 1376 ± 21 (10) | 1800 ± 44 (10) | 1431 ± 28 (3) |
| pwiz | ruzstd | 3,151,638 | — | — | 102 ± 0 (10) | — | — | 329 ± 2 (10) | — | — |
| pwiz | cosmoz/zstd | 2,356,719 | 1,885,497 | 1,674,469 | 612 ± 4 (10) | 143 ± 18 (10) | 10 ± 0 (3) | 1740 ± 41 (10) | 1698 ± 76 (10) | 1403 ± 32 (3) |
| pwiz | cosmoz/cosmoz | 2,357,306 | 1,887,198 | 1,676,429 | 611 ± 4 (10) | 149 ± 2 (10) | 10 ± 0 (3) | 1800 ± 51 (10) | 1795 ± 46 (10) | 1366 ± 33 (3) |

### Browser engine (wasm, Node worker)

One worker per codec. Each piece is copied to the worker and back, and the copy is included in the time. cosmoz and ruzstd are built for wasm32 with `+simd128` and `+bulk-memory`. libzstd is the published Emscripten build (`@bokuweb/zstd-wasm`).

| File | Codec | Size L1 | Size L9 | Size L22 | Compress L1 MB/s | Compress L9 MB/s | Compress L22 MB/s | Decompress L1 MB/s | Decompress L9 MB/s | Decompress L22 MB/s |
|---|---|---|---|---|---|---|---|---|---|---|
| iron | libzstd | 89,883,484 | 89,184,642 | 89,266,091 | 819 ± 11 (10) | 299 ± 4 (10) | 4 ± 0 (3) | 909 ± 12 (10) | 908 ± 5 (10) | 966 ± 3 (3) |
| iron | ruzstd | 99,249,254 | — | — | 98 ± 0 (10) | — | — | 265 ± 1 (10) | — | — |
| iron | cosmoz/zstd | 90,352,433 | 89,047,245 | 89,316,476 | 813 ± 14 (10) | 371 ± 5 (10) | 4 ± 0 (3) | 982 ± 2 (10) | 1005 ± 15 (10) | 978 ± 5 (3) |
| iron | cosmoz/cosmoz | 90,371,391 | 89,074,910 | 89,405,564 | 813 ± 7 (10) | 370 ± 4 (10) | 4 ± 0 (3) | 974 ± 10 (10) | 986 ± 16 (10) | 839 ± 36 (3) |
| pwiz | libzstd | 2,788,540 | 1,883,317 | 1,671,492 | 466 ± 17 (10) | 98 ± 1 (10) | 8 ± 0 (3) | 745 ± 8 (10) | 967 ± 68 (10) | 933 ± 6 (3) |
| pwiz | ruzstd | 3,151,638 | — | — | 90 ± 2 (10) | — | — | 245 ± 3 (10) | — | — |
| pwiz | cosmoz/zstd | 2,356,719 | 1,885,497 | 1,674,469 | 463 ± 15 (10) | 93 ± 1 (10) | 8 ± 0 (3) | 964 ± 60 (10) | 1036 ± 68 (10) | 927 ± 17 (3) |
| pwiz | cosmoz/cosmoz | 2,357,306 | 1,887,198 | 1,676,429 | 466 ± 11 (10) | 92 ± 2 (10) | 8 ± 0 (3) | 922 ± 49 (10) | 992 ± 48 (10) | 866 ± 19 (3) |
