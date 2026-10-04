use crate::track::FileEntry;
use rodio::{Decoder, OutputStream, Sink, Source};
use std::fs::File;
use std::io::BufReader;
use std::time::Duration;

pub fn play_sequence_from(entries: &[FileEntry], start_index: usize, silence_secs: u64) {
    let target_side = match &entries[start_index].side {
        crate::track::TrackSide::SideA(_) => crate::track::TrackSide::SideA(0),
        crate::track::TrackSide::SideB(_) => crate::track::TrackSide::SideB(0),
        _ => return, // Don't play sequence for unmatched files
    };

    let (_stream, handle) = OutputStream::try_default().unwrap();
    let sink = Sink::try_new(&handle).unwrap();

    let remaining: Vec<_> = entries
        .iter()
        .enumerate()
        .skip(start_index)
        .filter(|(_, entry)| {
            !entry.is_dir
                && std::mem::discriminant(&entry.side) == std::mem::discriminant(&target_side)
        })
        .collect();

    for (index, (_, track)) in remaining.iter().enumerate() {
        if let Ok(file) = File::open(&track.path) {
            if let Ok(decoder) = Decoder::new(BufReader::new(file)) {
                // Buffer audio into memory to prevent disk skip
                sink.append(decoder.buffered());

                if index + 1 < remaining.len() {
                    let silence = rodio::source::Zero::<i16>::new(1, 44100)
                        .take_duration(Duration::from_secs(silence_secs));
                    sink.append(silence);
                }
            }
        }
    }

    sink.sleep_until_end();
}
