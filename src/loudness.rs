use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

const TARGET_PEAK_DB: f32 = -1.0;
const MIN_PEAK_DB: f32 = -1.5;
const MAX_PEAK_DB: f32 = -0.5;
const SILENCE_THRESHOLD_DB: i32 = -48;
const MIN_REPORTED_SILENCE_SECS: f64 = 0.1;
const RETAINED_SILENCE_SECS: f64 = 0.9;

#[derive(Debug, Clone)]
pub struct TrackReport {
    pub peak_db: f32,
    pub leading_silence_secs: f64,
    pub trailing_silence_secs: f64,
    pub duration_secs: f64,
}

impl TrackReport {
    pub fn needs_front_trim(&self) -> bool {
        self.leading_silence_secs >= 1.0
    }

    pub fn needs_rear_trim(&self) -> bool {
        self.trailing_silence_secs >= 1.0
    }

    pub fn needs_normalizing(&self) -> bool {
        peak_needs_normalizing(self.peak_db)
    }

    pub fn needs_processing(&self) -> bool {
        self.needs_front_trim() || self.needs_rear_trim() || self.needs_normalizing()
    }
}

pub fn scan_track(path: &Path, cancelled: &AtomicBool) -> Result<TrackReport, String> {
    let output = run_ffmpeg(
        Command::new("ffmpeg")
            .args(["-hide_banner", "-nostats", "-nostdin", "-i"])
            .arg(path)
            .args([
                "-af",
                &format!(
                    "astats=metadata=0:reset=0,silencedetect=noise={SILENCE_THRESHOLD_DB}dB:d={MIN_REPORTED_SILENCE_SECS}"
                ),
                "-vn",
                "-f",
                "null",
                "-",
            ]),
        cancelled,
    )?;

    if !output.status.success() {
        return Err(ffmpeg_error(&output));
    }

    parse_track_report(&String::from_utf8_lossy(&output.stderr))
        .ok_or_else(|| "FFmpeg did not report a complete peak and duration analysis".to_string())
}

pub fn peak_needs_normalizing(peak_db: f32) -> bool {
    peak_db.is_finite() && !(MIN_PEAK_DB..=MAX_PEAK_DB).contains(&peak_db)
}

pub fn process_track(
    path: &Path,
    report: &TrackReport,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let trim_front = report.needs_front_trim();
    let trim_rear = report.needs_rear_trim();
    let normalize = report.needs_normalizing();
    if !trim_front && !trim_rear && !normalize {
        return Ok(());
    }

    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or_else(|| "Track has no supported file extension".to_string())?;
    let (codec, quality) = if extension.eq_ignore_ascii_case("ogg") {
        ("libvorbis", "5")
    } else if extension.eq_ignore_ascii_case("mp3") {
        ("libmp3lame", "2")
    } else {
        return Err(format!("Unsupported audio format: {extension}"));
    };

    let temporary_path = temporary_path(path, extension)?;
    let mut filters = Vec::new();
    if trim_front || trim_rear {
        let start = if trim_front {
            (report.leading_silence_secs - RETAINED_SILENCE_SECS).max(0.0)
        } else {
            0.0
        };
        let end = if trim_rear {
            report.duration_secs - report.trailing_silence_secs + RETAINED_SILENCE_SECS
        } else {
            report.duration_secs
        };
        if !start.is_finite() || !end.is_finite() || end - start < 0.05 {
            return Err("Detected silence leaves no safe audio region to retain".to_string());
        }

        let mut trim_options = Vec::new();
        if trim_front {
            trim_options.push(format!("start={start:.6}"));
        }
        if trim_rear {
            trim_options.push(format!("end={end:.6}"));
        }
        filters.push(format!("atrim={}", trim_options.join(":")));
        filters.push("asetpts=PTS-STARTPTS".to_string());
    }
    if normalize {
        let gain_db = TARGET_PEAK_DB - report.peak_db;
        filters.push(format!("volume={gain_db:.3}dB"));
    }
    let filter = filters.join(",");
    let output = run_ffmpeg(
        Command::new("ffmpeg")
            .args(["-hide_banner", "-nostats", "-nostdin", "-y", "-i"])
            .arg(path)
            .args(["-map", "0:a:0", "-map_metadata", "0", "-vn", "-af"])
            .arg(filter)
            .args(["-c:a", codec, "-q:a", quality])
            .arg(&temporary_path),
        cancelled,
    );

    let output = match output {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            let _ = fs::remove_file(&temporary_path);
            return Err(ffmpeg_error(&output));
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary_path);
            return Err(error);
        }
    };

    if cancelled.load(Ordering::Relaxed) {
        let _ = fs::remove_file(&temporary_path);
        return Err("Cancelled".to_string());
    }

    fs::rename(&temporary_path, path).map_err(|error| {
        let _ = fs::remove_file(&temporary_path);
        format!("Could not replace original track: {error}")
    })?;
    drop(output);
    Ok(())
}

fn run_ffmpeg(command: &mut Command, cancelled: &AtomicBool) -> Result<Output, String> {
    let mut child = command
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "Could not start FFmpeg: the `ffmpeg` executable was not found in PATH. Install FFmpeg and make sure `ffmpeg` is available from the terminal.".to_string()
            } else {
                format!("Could not start FFmpeg: {error}")
            }
        })?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Could not capture FFmpeg output".to_string())?;
    let stderr_reader = thread::spawn(move || {
        let mut output = Vec::new();
        let result = stderr.read_to_end(&mut output);
        (result, output)
    });

    loop {
        if cancelled.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stderr_reader.join();
            return Err("Cancelled".to_string());
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                let (read_result, stderr) = stderr_reader
                    .join()
                    .map_err(|_| "Could not collect FFmpeg output".to_string())?;
                read_result.map_err(|error| format!("Could not read FFmpeg output: {error}"))?;
                return Ok(Output {
                    status,
                    stdout: Vec::new(),
                    stderr,
                });
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stderr_reader.join();
                return Err(format!("Could not wait for FFmpeg: {error}"));
            }
        }
    }
}

fn parse_peak_level(output: &str) -> Option<f32> {
    output
        .lines()
        .filter_map(|line| {
            let (_, value) = line.split_once("Peak level dB:")?;
            value.split_whitespace().next()?.parse::<f32>().ok()
        })
        .filter(|peak| !peak.is_nan())
        .reduce(f32::max)
}

fn parse_track_report(output: &str) -> Option<TrackReport> {
    let peak_db = parse_peak_level(output)?;
    let duration_secs = parse_media_duration(output)?;
    if duration_secs <= 0.0 {
        return None;
    }

    let (intervals, pending_start) = parse_silence_intervals(output);
    let leading_silence_secs = intervals
        .first()
        .filter(|interval| interval.0 <= 0.01)
        .map(|interval| (interval.1 - interval.0).max(0.0))
        .unwrap_or(0.0);
    let trailing_silence_secs = pending_start
        .map(|start| (duration_secs - start).max(0.0))
        .or_else(|| {
            intervals
                .last()
                .filter(|interval| duration_secs - interval.1 <= 0.05)
                .map(|interval| (interval.1 - interval.0).max(0.0))
        })
        .unwrap_or(0.0);

    Some(TrackReport {
        peak_db,
        leading_silence_secs,
        trailing_silence_secs,
        duration_secs,
    })
}

fn parse_silence_intervals(output: &str) -> (Vec<(f64, f64)>, Option<f64>) {
    let mut intervals = Vec::new();
    let mut pending_start = None;

    for line in output.lines() {
        if line.contains("silence_start:") {
            pending_start = parse_value_after(line, "silence_start:");
        } else if line.contains("silence_end:") {
            if let (Some(start), Some(end)) = (
                pending_start.take(),
                parse_value_after(line, "silence_end:"),
            ) {
                intervals.push((start, end));
            }
        }
    }

    (intervals, pending_start)
}

fn parse_value_after(line: &str, marker: &str) -> Option<f64> {
    let (_, value) = line.split_once(marker)?;
    let value: f64 = value.split_whitespace().next()?.parse().ok()?;
    value.is_finite().then_some(value)
}

fn parse_media_duration(output: &str) -> Option<f64> {
    let mut duration = None;
    for line in output.lines() {
        if let Some((_, value)) = line.split_once("Duration:") {
            if let Some(parsed) = parse_clock(value.split(',').next()?.trim()) {
                duration = Some(parsed);
            }
        }
        for token in line.split_whitespace() {
            if let Some(value) = token.strip_prefix("time=") {
                if let Some(parsed) = parse_clock(value) {
                    duration = Some(parsed);
                }
            }
        }
    }
    duration
}

fn parse_clock(value: &str) -> Option<f64> {
    let mut parts = value.split(':');
    let hours: u64 = parts.next()?.parse().ok()?;
    let minutes: u64 = parts.next()?.parse().ok()?;
    let seconds: f64 = parts.next()?.parse().ok()?;
    if parts.next().is_some()
        || minutes >= 60
        || !seconds.is_finite()
        || !(0.0..60.0).contains(&seconds)
    {
        return None;
    }
    let total = hours as f64 * 3600.0 + minutes as f64 * 60.0 + seconds;
    total.is_finite().then_some(total)
}

fn ffmpeg_error(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .last()
        .unwrap_or("FFmpeg failed")
        .to_string()
}

fn temporary_path(path: &Path, extension: &str) -> Result<PathBuf, String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Track has no valid file name".to_string())?;
    Ok(parent.join(format!(
        ".{name}.mixtape-{}.tmp.{extension}",
        std::process::id()
    )))
}

#[cfg(test)]
mod tests {
    use super::{
        parse_peak_level, parse_track_report, peak_needs_normalizing, process_track, scan_track,
        TrackReport, TARGET_PEAK_DB,
    };
    use std::process::Command;
    use std::sync::atomic::AtomicBool;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_unclipped_peak_level_from_ffmpeg_output() {
        assert_eq!(
            parse_peak_level("[Parsed_astats] Peak level dB: -4.2"),
            Some(-4.2)
        );
        assert_eq!(
            parse_peak_level("[Parsed_astats] Peak level dB: 3.131933"),
            Some(3.131933)
        );
        assert_eq!(
            parse_peak_level("[Parsed_astats] Peak level dB: -inf"),
            Some(f32::NEG_INFINITY)
        );
        assert_eq!(parse_peak_level("no peak reported"), None);
    }

    #[test]
    fn parses_peak_duration_and_edge_silence_report() {
        let output = "Input #0, ogg, from 'track.ogg':\n  Duration: 00:00:04.00, start: 0.000000, bitrate: 128 kb/s\n[silencedetect] silence_start: 0\n[silencedetect] silence_end: 1.50002 | silence_duration: 1.50002\n[silencedetect] silence_start: 2.5\n[silencedetect] silence_end: 4 | silence_duration: 1.5\n[Parsed_astats] Peak level dB: -6.0";
        let report = parse_track_report(output).unwrap();
        assert_eq!(report.peak_db, -6.0);
        assert!((report.leading_silence_secs - 1.50002).abs() < 0.001);
        assert!((report.trailing_silence_secs - 1.5).abs() < 0.001);
        assert!((report.duration_secs - 4.0).abs() < 0.001);
    }

    #[test]
    fn only_flags_peaks_outside_target_range() {
        assert!(peak_needs_normalizing(-6.0));
        assert!(!peak_needs_normalizing(-1.0));
        assert!(!peak_needs_normalizing(-1.5));
        assert!(!peak_needs_normalizing(-0.5));
        assert!(peak_needs_normalizing(-1.5001));
        assert!(peak_needs_normalizing(-0.4999));
        assert!(peak_needs_normalizing(-0.2));
        assert!(peak_needs_normalizing(0.2));
        assert!(!peak_needs_normalizing(f32::NEG_INFINITY));
    }

    #[test]
    fn trims_at_one_second_but_not_below_it() {
        let mut report = TrackReport {
            peak_db: -1.0,
            leading_silence_secs: 1.0,
            trailing_silence_secs: 1.0,
            duration_secs: 4.0,
        };
        assert!(report.needs_processing());
        assert!(report.needs_front_trim());
        assert!(report.needs_rear_trim());

        report.leading_silence_secs = 0.999;
        report.trailing_silence_secs = 0.999;
        assert!(!report.needs_processing());

        report.peak_db = -1.5001;
        assert!(report.needs_processing());
    }

    #[test]
    fn normalizes_generated_ogg_to_target_peak() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mixtape-loudness-{}-{suffix}.ogg",
            std::process::id()
        ));
        let generated = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.25",
                "-c:a",
                "libvorbis",
                "-q:a",
                "5",
            ])
            .arg(&path)
            .status();

        let Ok(status) = generated else {
            return;
        };
        if !status.success() {
            return;
        }

        let cancelled = AtomicBool::new(false);
        let result = (|| {
            let before = scan_track(&path, &cancelled)?;
            assert!(before.needs_normalizing());
            process_track(&path, &before, &cancelled)?;
            let after = scan_track(&path, &cancelled)?;
            assert!((after.peak_db - TARGET_PEAK_DB).abs() < 0.5);
            Ok::<(), String>(())
        })();
        let _ = std::fs::remove_file(&path);
        result.unwrap();
    }

    #[test]
    fn normalizes_overrange_mp3_to_target_peak_in_one_pass() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mixtape-overrange-{}-{suffix}.mp3",
            std::process::id()
        ));
        let generated = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "aevalsrc=0.9*sin(2*PI*440*t)+0.1*sin(2*PI*1320*t):s=44100:d=2",
                "-af",
                "volume=5dB",
                "-c:a",
                "libmp3lame",
                "-q:a",
                "2",
            ])
            .arg(&path)
            .status();

        let Ok(status) = generated else {
            return;
        };
        if !status.success() {
            return;
        }

        let cancelled = AtomicBool::new(false);
        let result = (|| {
            let before = scan_track(&path, &cancelled)?;
            assert!(before.peak_db > 0.0, "peak was {} dB", before.peak_db);
            process_track(&path, &before, &cancelled)?;

            let after = scan_track(&path, &cancelled)?;
            assert!(
                (after.peak_db - TARGET_PEAK_DB).abs() < 0.25,
                "processed peak was {} dB",
                after.peak_db
            );
            Ok::<(), String>(())
        })();
        let _ = std::fs::remove_file(&path);
        result.unwrap();
    }

    #[test]
    fn trims_generated_ogg_edges_and_normalizes_in_one_pass() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mixtape-side-prep-{}-{suffix}.ogg",
            std::process::id()
        ));
        let generated = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "aevalsrc=if(between(t\\,1.5\\,2.5)\\,0.5*sin(2*PI*440*t)\\,0):s=44100:d=4",
                "-c:a",
                "libvorbis",
                "-q:a",
                "5",
            ])
            .arg(&path)
            .status();

        let Ok(status) = generated else {
            return;
        };
        if !status.success() {
            return;
        }

        let cancelled = AtomicBool::new(false);
        let result = (|| {
            let before = scan_track(&path, &cancelled)?;
            assert!(before.needs_front_trim());
            assert!(before.needs_rear_trim());
            assert!(before.needs_normalizing());
            process_track(&path, &before, &cancelled)?;

            let after = scan_track(&path, &cancelled)?;
            assert!((after.peak_db - TARGET_PEAK_DB).abs() < 0.6);
            assert!((0.7..=1.1).contains(&after.leading_silence_secs));
            assert!((0.7..=1.1).contains(&after.trailing_silence_secs));
            assert!((2.5..=3.1).contains(&after.duration_secs));
            Ok::<(), String>(())
        })();
        let _ = std::fs::remove_file(&path);
        result.unwrap();
    }
}
