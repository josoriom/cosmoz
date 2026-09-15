use osmo::decoder::{DecodeWorkspace, decompress};
use osmo::encoder::{CompressOptions, EncodeWorkspace, compress, get_max_compressed_size};
use osmo::frame::frame_header::FrameFormat;
use std::env;
use std::fs;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const REPEAT_COUNT: usize = 5;

struct RowResult {
    name: &'static str,
    compressed_bytes: usize,
    ratio: f64,
    compress_megabytes_per_second: f64,
    decompress_megabytes_per_second: f64,
}

fn megabytes_per_second(byte_count: usize, elapsed: Duration) -> f64 {
    let seconds = elapsed.as_secs_f64();
    if seconds <= 0.0 {
        return f64::INFINITY;
    }
    (byte_count as f64 / (1024.0 * 1024.0)) / seconds
}

fn best_duration(durations: &[Duration]) -> Duration {
    *durations.iter().min().unwrap()
}

fn bench_format(name: &'static str, format: FrameFormat, input: &[u8]) -> RowResult {
    let options = CompressOptions {
        format,
        with_checksum: true,
        chunk_size: osmo::encoder::DEFAULT_CHUNK_SIZE,
        level: 1,
    };

    let mut encode_workspace = EncodeWorkspace::new_boxed();
    let mut output = vec![0u8; get_max_compressed_size(input.len(), &options)];

    let mut compress_durations = Vec::with_capacity(REPEAT_COUNT);
    let mut compressed_length = 0usize;
    for _ in 0..REPEAT_COUNT {
        let started_at = Instant::now();
        compressed_length = compress(input, &mut output, &options, &mut encode_workspace).unwrap();
        compress_durations.push(started_at.elapsed());
    }
    output.truncate(compressed_length);

    let mut decode_workspace = DecodeWorkspace::new_boxed();
    let mut decoded = vec![0u8; input.len()];

    let mut decompress_durations = Vec::with_capacity(REPEAT_COUNT);
    for _ in 0..REPEAT_COUNT {
        let started_at = Instant::now();
        let written = decompress(&output, &mut decoded, &mut decode_workspace).unwrap();
        decompress_durations.push(started_at.elapsed());
        assert_eq!(written, input.len());
    }
    assert_eq!(decoded, input);

    let compress_best = best_duration(&compress_durations);
    let decompress_best = best_duration(&decompress_durations);

    RowResult {
        name,
        compressed_bytes: compressed_length,
        ratio: input.len() as f64 / compressed_length as f64,
        compress_megabytes_per_second: megabytes_per_second(input.len(), compress_best),
        decompress_megabytes_per_second: megabytes_per_second(input.len(), decompress_best),
    }
}

fn find_zstd_cli() -> Option<String> {
    let output = Command::new("which").arg("zstd").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let trimmed = path.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn run_cli_on_file(zstd_path: &str, args: &[&str], input_path: &str) -> (Vec<u8>, Duration) {
    let mut command = Command::new(zstd_path);
    command.args(args).arg(input_path);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let started_at = Instant::now();
    let child = command.spawn().expect("failed to start zstd process");
    let output = child.wait_with_output().expect("failed to read output");
    let elapsed = started_at.elapsed();
    if !output.status.success() {
        panic!(
            "zstd command failed with status {:?}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    (output.stdout, elapsed)
}

fn bench_cli(zstd_path: &str, input_path: &str, input: &[u8]) -> RowResult {
    let mut compress_durations = Vec::with_capacity(REPEAT_COUNT);
    let mut compressed = Vec::new();
    for _ in 0..REPEAT_COUNT {
        let (bytes, elapsed) = run_cli_on_file(zstd_path, &["-1", "-c", "-q", "-T1"], input_path);
        compressed = bytes;
        compress_durations.push(elapsed);
    }

    let mut compressed_path = env::temp_dir();
    compressed_path.push(format!("osmo_bench_{}.zst", std::process::id()));
    fs::write(&compressed_path, &compressed).expect("failed to write compressed temp file");
    let compressed_path_string = compressed_path.to_str().unwrap().to_string();

    let mut decompress_durations = Vec::with_capacity(REPEAT_COUNT);
    for _ in 0..REPEAT_COUNT {
        let (decoded, elapsed) =
            run_cli_on_file(zstd_path, &["-d", "-c", "-q"], &compressed_path_string);
        decompress_durations.push(elapsed);
        assert_eq!(decoded, input);
    }
    let _ = fs::remove_file(&compressed_path);

    let compress_best = best_duration(&compress_durations);
    let decompress_best = best_duration(&decompress_durations);

    RowResult {
        name: "zstd cli -1",
        compressed_bytes: compressed.len(),
        ratio: input.len() as f64 / compressed.len() as f64,
        compress_megabytes_per_second: megabytes_per_second(input.len(), compress_best),
        decompress_megabytes_per_second: megabytes_per_second(input.len(), decompress_best),
    }
}

fn print_row(result: &RowResult) {
    println!(
        "{:<18} {:>12} {:>8.3} {:>16.2} {:>18.2}",
        result.name,
        result.compressed_bytes,
        result.ratio,
        result.compress_megabytes_per_second,
        result.decompress_megabytes_per_second
    );
}

fn main() {
    let path = env::args().nth(1).expect("usage: bench <file>");
    let input = fs::read(&path).expect("failed to read input file");

    println!(
        "file {path}, size {} bytes, best of {REPEAT_COUNT} runs",
        input.len()
    );
    println!(
        "{:<18} {:>12} {:>8} {:>16} {:>18}",
        "row", "compressed", "ratio", "compress MB/s", "decompress MB/s"
    );

    let zstd_format_result = bench_format("osmo zstd format", FrameFormat::Zstd, &input);
    print_row(&zstd_format_result);

    let osmo_format_result = bench_format("osmo osmo format", FrameFormat::Osmo, &input);
    print_row(&osmo_format_result);

    match find_zstd_cli() {
        Some(zstd_path) => {
            let cli_result = bench_cli(&zstd_path, &path, &input);
            print_row(&cli_result);
            println!("zstd cli timing includes process startup for every run");
        }
        None => {
            println!("zstd CLI not found on PATH, skipping reference numbers");
        }
    }
}
