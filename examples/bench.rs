use osmos::decoder::{DecodeWorkspace, decompress};
use osmos::encoder::{CompressOptions, EncodeWorkspace, compress, get_max_compressed_size};
use osmos::frame::frame_header::FrameFormat;
use std::env;
use std::fs;
use std::time::Instant;

struct RowResult {
    name: &'static str,
    compressed_bytes: usize,
    ratio: f64,
    compress_mean: f64,
    compress_std: f64,
    decompress_mean: f64,
    decompress_std: f64,
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

fn std_dev(values: &[f64], mean_value: f64) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let variance = values
        .iter()
        .map(|value| (value - mean_value) * (value - mean_value))
        .sum::<f64>()
        / values.len() as f64;
    variance.sqrt()
}

fn megabytes_per_second(byte_count: usize, seconds: f64) -> f64 {
    if seconds <= 0.0 {
        return f64::INFINITY;
    }
    (byte_count as f64 / (1024.0 * 1024.0)) / seconds
}

fn bench_format(
    name: &'static str,
    format: FrameFormat,
    level: u8,
    runs: usize,
    input: &[u8],
) -> RowResult {
    let options = CompressOptions {
        format,
        with_checksum: true,
        chunk_size: osmos::encoder::DEFAULT_CHUNK_SIZE,
        level,
    };

    let mut encode_workspace = EncodeWorkspace::new_boxed_for_level(level).unwrap();
    let mut output = vec![0u8; get_max_compressed_size(input.len(), &options)];

    let mut compress_speeds = Vec::with_capacity(runs);
    let mut compressed_length = 0usize;
    for _ in 0..runs {
        let started_at = Instant::now();
        compressed_length = compress(input, &mut output, &options, &mut encode_workspace).unwrap();
        let elapsed = started_at.elapsed().as_secs_f64();
        compress_speeds.push(megabytes_per_second(input.len(), elapsed));
    }
    output.truncate(compressed_length);

    let mut decode_workspace = DecodeWorkspace::new_boxed();
    let mut decoded = vec![0u8; input.len()];

    let mut decompress_speeds = Vec::with_capacity(runs);
    for _ in 0..runs {
        let started_at = Instant::now();
        let written = decompress(&output, &mut decoded, &mut decode_workspace).unwrap();
        let elapsed = started_at.elapsed().as_secs_f64();
        decompress_speeds.push(megabytes_per_second(input.len(), elapsed));
        assert_eq!(written, input.len());
    }
    assert_eq!(decoded, input);

    let compress_mean = mean(&compress_speeds);
    let decompress_mean = mean(&decompress_speeds);

    RowResult {
        name,
        compressed_bytes: compressed_length,
        ratio: input.len() as f64 / compressed_length as f64,
        compress_mean,
        compress_std: std_dev(&compress_speeds, compress_mean),
        decompress_mean,
        decompress_std: std_dev(&decompress_speeds, decompress_mean),
    }
}

fn print_row(result: &RowResult, runs: usize) {
    println!(
        "{:<18} {:>12} {:>8.3} {:>10.2} ± {:<6.2} ({}) {:>10.2} ± {:<6.2} ({})",
        result.name,
        result.compressed_bytes,
        result.ratio,
        result.compress_mean,
        result.compress_std,
        runs,
        result.decompress_mean,
        result.decompress_std,
        runs
    );
}

fn parse_flag(args: &[String], flag: &str, default_value: u8) -> u8 {
    for index in 0..args.len() {
        if args[index] == flag && index + 1 < args.len() {
            return args[index + 1].parse().expect("invalid flag value");
        }
    }
    default_value
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let path = args
        .get(1)
        .expect("usage: bench <file> [--level N] [--runs N]");
    let level = parse_flag(&args, "--level", 1);
    let runs = parse_flag(&args, "--runs", 10) as usize;

    let input = fs::read(path).expect("failed to read input file");

    println!(
        "file {path}, size {} bytes, level {level}, {runs} runs",
        input.len()
    );
    println!(
        "{:<18} {:>12} {:>8} {:>19} {:>19}",
        "row", "compressed", "ratio", "compress MB/s", "decompress MB/s"
    );

    let zstd_format_result =
        bench_format("osmos zstd format", FrameFormat::Zstd, level, runs, &input);
    print_row(&zstd_format_result, runs);

    let osmos_format_result = bench_format(
        "osmos osmos format",
        FrameFormat::Osmos,
        level,
        runs,
        &input,
    );
    print_row(&osmos_format_result, runs);
}
