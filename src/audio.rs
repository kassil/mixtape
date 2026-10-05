use crate::track::FileEntry;
use rodio::{Decoder, OutputStream, Sink, Source};
use std::fs::File;
use std::io::BufReader;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const DEFAULT_SILENCE_SECS: u64 = 2;

pub fn track_duration(path: &std::path::Path) -> Result<Duration, String> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(path)
        .output()
        .map_err(|error| format!("Could not start FFmpeg: {error}"))?;
    parse_duration(&String::from_utf8_lossy(&output.stderr))
        .ok_or_else(|| "FFmpeg did not report a track duration".to_string())
}

fn parse_duration(output: &str) -> Option<Duration> {
    let duration = output.lines().find_map(|line| {
        let (_, value) = line.split_once("Duration:")?;
        Some(value.split(',').next()?.trim())
    })?;
    let mut parts = duration.split(':');
    let hours: u64 = parts.next()?.parse().ok()?;
    let minutes: u64 = parts.next()?.parse().ok()?;
    let seconds: f64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Duration::try_from_secs_f64(hours as f64 * 3600.0 + minutes as f64 * 60.0 + seconds).ok()
}

pub fn play_sequence_from(
    entries: &[FileEntry],
    start_index: usize,
    silence_secs: u64,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let Some(starting_entry) = entries.get(start_index) else {
        return Err("No track selected".to_string());
    };
    let target_side = match &starting_entry.side {
        crate::track::TrackSide::SideA(_) => crate::track::TrackSide::SideA(0),
        crate::track::TrackSide::SideB(_) => crate::track::TrackSide::SideB(0),
        _ => return Err("Selected entry is not a side track".to_string()),
    };

    let (_stream, handle) = OutputStream::try_default()
        .map_err(|error| format!("Could not open audio output: {error}"))?;
    let sink =
        Sink::try_new(&handle).map_err(|error| format!("Could not create player: {error}"))?;

    let remaining: Vec<_> = entries
        .iter()
        .enumerate()
        .skip(start_index)
        .filter(|(_, entry)| {
            !entry.is_dir
                && std::mem::discriminant(&entry.side) == std::mem::discriminant(&target_side)
        })
        .collect();

    let mut playable_tracks = Vec::new();
    for (_, track) in remaining {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(());
        }
        if let Ok(file) = File::open(&track.path) {
            if let Ok(decoder) = Decoder::new(BufReader::new(file)) {
                playable_tracks.push(decoder);
            }
        }
    }

    let playable_count = playable_tracks.len();
    for (index, decoder) in playable_tracks.into_iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            sink.stop();
            return Ok(());
        }
        sink.append(decoder.buffered());

        if index + 1 < playable_count {
            let silence = rodio::source::Zero::<i16>::new(1, 44100)
                .take_duration(Duration::from_secs(silence_secs));
            sink.append(silence);
        }
    }

    while !sink.empty() {
        if cancelled.load(Ordering::Relaxed) {
            sink.stop();
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_duration;
    use std::time::Duration;

    #[test]
    fn parses_ffmpeg_duration_with_fractional_seconds() {
        assert_eq!(
            parse_duration(
                "Input #0, ogg, from 'track.ogg':\n  Duration: 01:02:03.50, start: 0.000000"
            ),
            Some(Duration::from_millis(3_723_500))
        );
    }

    #[test]
    fn ignores_unknown_or_malformed_durations() {
        assert_eq!(parse_duration("Duration: N/A, start: 0.000000"), None);
        assert_eq!(parse_duration("Duration: 1:2, start: 0.000000"), None);
    }
}
