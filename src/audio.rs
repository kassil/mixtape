use crate::track::FileEntry;
use rodio::{Decoder, OutputStream, Sink, Source};
use std::fs::File;
use std::io::BufReader;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc::Sender, Arc, Mutex};
use std::time::Duration;

pub const DEFAULT_SILENCE_SECS: u64 = 2;

#[derive(Default)]
pub struct PlaybackControl {
    cancelled: AtomicBool,
    sink: Mutex<Option<Arc<Sink>>>,
}

impl PlaybackControl {
    pub fn stop(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Some(sink) = self.sink.lock().unwrap().as_ref() {
            sink.stop();
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

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
    control: &PlaybackControl,
    status_sender: &Sender<String>,
) -> Result<(), String> {
    let Some(starting_entry) = entries.get(start_index) else {
        return Err("No track selected".to_string());
    };
    // We play to the end of the current side
    let target_side = match &starting_entry.side {
        crate::track::TrackSide::SideA(_) => crate::track::TrackSide::SideA(0),
        crate::track::TrackSide::SideB(_) => crate::track::TrackSide::SideB(0),
        _ => return Err("Selected entry is not a side track".to_string()),
    };
    // Set up the audio output and player
    let (_stream, handle) = OutputStream::try_default()
        .map_err(|error| format!("Could not open audio output: {error}"))?;
    let sink = Arc::new(
        Sink::try_new(&handle).map_err(|error| format!("Could not create player: {error}"))?,
    );
    *control.sink.lock().unwrap() = Some(Arc::clone(&sink));
    // Collect the remaining tracks on the same side starting from the selected track
    let mut remaining = entries.iter().skip(start_index).filter(|entry| {
        !entry.is_dir && std::mem::discriminant(&entry.side) == std::mem::discriminant(&target_side)
    });

    let result = play_tracks(&sink, &mut remaining, silence_secs, control, status_sender);
    control.sink.lock().unwrap().take();
    result
}

fn play_tracks<'a>(
    sink: &Sink,
    remaining: &mut impl Iterator<Item = &'a FileEntry>,
    silence_secs: u64,
    control: &PlaybackControl,
    status_sender: &Sender<String>,
) -> Result<(), String> {
    // Get the first playable track from the iterator
    let Some((mut current_name, mut decoder)) = next_playable_track(remaining, control) else {
        return Ok(());
    };

    loop {
        if control.is_cancelled() {
            sink.stop();
            return Ok(());
        }

        let _ = status_sender.send(format!("Playing {current_name}"));
        sink.append(decoder);
        let next = next_playable_track(remaining, control);
        // This thread waits while UI thread continues to handles keystrokes.
        sink.sleep_until_end();

        if control.is_cancelled() {
            // Woken after UI thread stopped the sink
            sink.stop();
            return Ok(());
        }

        // Check if there is a next track to play
        let Some((next_name, next_decoder)) = next else {
            break;
        };

        // Announce the upcoming track
        let _ = status_sender.send(format!("Upcoming: {next_name}"));
        // Silence between tracks
        //TODO Is it wise to assume a sample rate of 44100 Hz for the silence?
        //TODO Should we reuse the same silence source for all gaps?
        let silence = rodio::source::Zero::<i16>::new(1, 44100)
            .take_duration(Duration::from_secs(silence_secs));
        sink.append(silence);
        sink.sleep_until_end();

        if control.is_cancelled() {
            sink.stop();
            return Ok(());
        }

        current_name = next_name;
        decoder = next_decoder;
    }

    Ok(())
}

fn next_playable_track<'a>(
    remaining: &mut impl Iterator<Item = &'a FileEntry>,
    control: &PlaybackControl,
) -> Option<(String, Decoder<BufReader<File>>)> {
    for track in remaining {
        if control.is_cancelled() {
            return None;
        }
        let Ok(file) = File::open(&track.path) else {
            // Failed to open the file, skip to the next track
            continue;
        };
        if let Ok(decoder) = Decoder::new(BufReader::new(file)) {
            // Created a decoder for the track
            return Some((track.name.clone(), decoder));
        }
    }
    None
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
