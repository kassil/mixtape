use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

const TARGET_PEAK_DB: f32 = -1.0;

pub fn scan_peak_db(path: &Path, cancelled: &AtomicBool) -> Result<f32, String> {
    let output = run_ffmpeg(
        Command::new("ffmpeg")
            .args(["-hide_banner", "-nostats", "-nostdin", "-i"])
            .arg(path)
            .args(["-af", "volumedetect", "-vn", "-f", "null", "-"]),
        cancelled,
    )?;

    if !output.status.success() {
        return Err(ffmpeg_error(&output));
    }

    parse_max_volume(&String::from_utf8_lossy(&output.stderr))
        .ok_or_else(|| "FFmpeg did not report a measurable peak".to_string())
}

pub fn peak_needs_normalizing(peak_db: f32) -> bool {
    peak_db.is_finite() && !(TARGET_PEAK_DB..=0.0).contains(&peak_db)
}

pub fn normalize_track(path: &Path, peak_db: f32, cancelled: &AtomicBool) -> Result<(), String> {
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
    let gain_db = TARGET_PEAK_DB - peak_db;
    let filter = format!("volume={gain_db:.3}dB");
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
        .map_err(|error| format!("Could not start FFmpeg: {error}"))?;

    loop {
        if cancelled.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Cancelled".to_string());
        }

        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().map_err(|error| error.to_string()),
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Could not wait for FFmpeg: {error}"));
            }
        }
    }
}

fn parse_max_volume(output: &str) -> Option<f32> {
    output.lines().find_map(|line| {
        let (_, value) = line.split_once("max_volume:")?;
        value.split_whitespace().next()?.parse().ok()
    })
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
        normalize_track, parse_max_volume, peak_needs_normalizing, scan_peak_db, TARGET_PEAK_DB,
    };
    use std::process::Command;
    use std::sync::atomic::AtomicBool;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_peak_volume_from_ffmpeg_output() {
        assert_eq!(
            parse_max_volume("[Parsed_volumedetect] max_volume: -4.2 dB"),
            Some(-4.2)
        );
        assert_eq!(
            parse_max_volume("[Parsed_volumedetect] max_volume: -inf dB"),
            Some(f32::NEG_INFINITY)
        );
        assert_eq!(parse_max_volume("no peak reported"), None);
    }

    #[test]
    fn only_flags_peaks_outside_target_range() {
        assert!(peak_needs_normalizing(-6.0));
        assert!(!peak_needs_normalizing(-1.0));
        assert!(!peak_needs_normalizing(-0.2));
        assert!(peak_needs_normalizing(0.2));
        assert!(!peak_needs_normalizing(f32::NEG_INFINITY));
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
            let before = scan_peak_db(&path, &cancelled)?;
            assert!(peak_needs_normalizing(before));
            normalize_track(&path, before, &cancelled)?;
            let after = scan_peak_db(&path, &cancelled)?;
            assert!((after - TARGET_PEAK_DB).abs() < 0.5);
            Ok::<(), String>(())
        })();
        let _ = std::fs::remove_file(&path);
        result.unwrap();
    }
}
