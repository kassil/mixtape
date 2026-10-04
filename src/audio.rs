use rodio::{Decoder, OutputStream, Sink, Source};
use std::fs::File;
use std::io::BufReader;
use std::time::Duration;
use crate::track::FileEntry;

pub fn play_sequence_from(entries: &[FileEntry], start_index: usize, silence_secs: u64) {
    let target_side = match &entries[start_index].side {
        crate::track::TrackSide::SideA(_) => crate::track::TrackSide::SideA(0),
        crate::track::TrackSide::SideB(_) => crate::track::TrackSide::SideB(0),
        _ => return, // Don't play sequence for unmatched files
    };

    let (_stream, handle) = OutputStream::try_default().unwrap();
    let sink = Sink::try_new(&handle).unwrap();

    // Filter to remaining tracks on the selected side from start_index onward
    let remaining = entries.iter().enumerate().skip(start_index).filter(|(_, e)| {
        std::mem::discriminant(&e.side) == std::mem::discriminant(&target_side)
    });

    for (_, track) in remaining {
        if let Ok(file) = File::open(&track.path) {
            if let Ok(decoder) = Decoder::new(BufReader::new(file)) {
                // Buffer audio into memory to prevent disk skip
                sink.append(decoder.buffered());

                // Inter-track silence
                let silence = rodio::source::Zero::<i16>::new(1, 44100)
                    .take_duration(Duration::from_secs(silence_secs));
                sink.append(silence);
            }
        }
    }

    sink.sleep_until_end();
}
