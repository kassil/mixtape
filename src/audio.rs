use crate::track::FileEntry;
use rodio::{Decoder, OutputStream, Sink, Source};
use std::fs::File;
use std::io::BufReader;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const DEFAULT_SILENCE_SECS: u64 = 2;

pub fn track_duration(path: &std::path::Path) -> Result<Duration, String> {
    let file = File::open(path).map_err(|error| format!("Could not open track: {error}"))?;
    let decoder = Decoder::new(BufReader::new(file))
        .map_err(|error| format!("Could not read track metadata: {error}"))?;
    decoder
        .total_duration()
        .ok_or_else(|| "Track duration is unavailable".to_string())
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
