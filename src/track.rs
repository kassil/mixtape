use ratatui::style::{Color, Style};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackSide {
    SideA(u32),
    SideB(u32),
    // File that does not follow the track naming convention or directory
    Unmatched,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub side: TrackSide,
    pub is_dir: bool,
}

impl FileEntry {
    /// Color mapping: Side A (Green), Side B (Yellow), Unmatched (Dark Gray)
    pub fn style(&self) -> Style {
        if self.is_dir {
            return Style::default().fg(Color::Cyan);
        }

        match self.side {
            TrackSide::SideA(_) => Style::default().fg(Color::Green),
            TrackSide::SideB(_) => Style::default().fg(Color::Yellow),
            TrackSide::Unmatched => Style::default().fg(Color::DarkGray),
        }
    }
}

pub fn scan_dir(current_dir: &Path) -> Vec<FileEntry> {
    let re = Regex::new(r"(?i)^([AB])(\d+)").unwrap();
    let mut entries = Vec::new();

    if let Some(parent) = current_dir.parent().filter(|parent| *parent != current_dir) {
        // Add an entry for the parent directory ("..") to allow navigating up in the directory tree.
        entries.push(FileEntry {
            name: "..".to_string(),
            path: parent.to_path_buf(),
            side: TrackSide::Unmatched,
            is_dir: true,
        });
    }

    if let Ok(read_dir) = fs::read_dir(current_dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.is_file() || path.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                let is_dir = path.is_dir();
                // Audio file extension check
                let supported_audio = path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        extension.eq_ignore_ascii_case("ogg")
                            || extension.eq_ignore_ascii_case("mp3")
                    });
                // Determine the side and sequence number for the track based on the filename
                let side = if !is_dir && supported_audio {
                    if let Some(caps) = re.captures(&name) {
                        let side_char = caps[1].to_uppercase();
                        let num: u32 = caps[2].parse().unwrap_or(0);
                        if side_char == "A" {
                            TrackSide::SideA(num)
                        } else {
                            TrackSide::SideB(num)
                        }
                    } else {
                        TrackSide::Unmatched
                    }
                } else {
                    TrackSide::Unmatched
                };

                entries.push(FileEntry {
                    name,
                    path,
                    side,
                    is_dir,
                });
            }
        }
    }

    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        // Both are directories
        (true, true) => match (a.name == "..", b.name == "..") {
            // Parent dir appears first
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            // Sort dirs alphabetically ignoring case
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        },
        // Directories come before files
        (true, false) => std::cmp::Ordering::Less,
        // Files come after directories
        (false, true) => std::cmp::Ordering::Greater,
        // Both are files: sort by side and sequence number
        (false, false) => match (&a.side, &b.side) {
            (TrackSide::SideA(a_num), TrackSide::SideA(b_num))
            | (TrackSide::SideB(a_num), TrackSide::SideB(b_num)) => a_num.cmp(b_num),
            (TrackSide::SideA(_), TrackSide::SideB(_))
            | (TrackSide::SideA(_), TrackSide::Unmatched)
            | (TrackSide::SideB(_), TrackSide::Unmatched) => std::cmp::Ordering::Less,
            (TrackSide::SideB(_), TrackSide::SideA(_))
            | (TrackSide::Unmatched, TrackSide::SideA(_))
            | (TrackSide::Unmatched, TrackSide::SideB(_)) => std::cmp::Ordering::Greater,
            (TrackSide::Unmatched, TrackSide::Unmatched) => {
                a.name.to_lowercase().cmp(&b.name.to_lowercase())
            }
        },
    });
    entries
}

pub fn side_sequence_from(entries: &[FileEntry], start_index: usize) -> Vec<FileEntry> {
    let Some(starting_entry) = entries.get(start_index) else {
        return Vec::new();
    };
    if starting_entry.is_dir || matches!(starting_entry.side, TrackSide::Unmatched) {
        return Vec::new();
    }

    entries
        .iter()
        .skip(start_index)
        .filter(|entry| !entry.is_dir && same_side(&entry.side, &starting_entry.side))
        .cloned()
        .collect()
}

fn same_side(left: &TrackSide, right: &TrackSide) -> bool {
    matches!(
        (left, right),
        (TrackSide::SideA(_), TrackSide::SideA(_)) | (TrackSide::SideB(_), TrackSide::SideB(_))
    )
}

#[cfg(test)]
mod tests {
    use super::{scan_dir, side_sequence_from, TrackSide};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn lists_directories_before_numerically_sorted_side_tracks() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "mixtape-track-scan-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir_all(directory.join("Folder")).unwrap();
        for name in ["A10.ogg", "B1.mp3", "A2.ogg", "notes.txt"] {
            fs::write(directory.join(name), []).unwrap();
        }

        let entries = scan_dir(&directory);
        let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(
            &names[..5],
            &["..", "Folder", "A2.ogg", "A10.ogg", "B1.mp3"]
        );
        assert!(entries[0].is_dir);
        assert!(entries[1].is_dir);
        assert_eq!(entries[2].side, TrackSide::SideA(2));
        assert_eq!(entries[3].side, TrackSide::SideA(10));
        assert_eq!(entries[4].side, TrackSide::SideB(1));

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn side_sequence_starts_at_selected_track_and_stops_at_side_end() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "mixtape-side-sequence-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        for name in ["A1.ogg", "A2.ogg", "A3.ogg", "B1.ogg", "notes.txt"] {
            fs::write(directory.join(name), []).unwrap();
        }

        let entries = scan_dir(&directory);
        let start_index = entries
            .iter()
            .position(|entry| entry.name == "A2.ogg")
            .unwrap();
        let sequence = side_sequence_from(&entries, start_index);
        let names: Vec<_> = sequence.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["A2.ogg", "A3.ogg"]);
        assert!(side_sequence_from(&entries, usize::MAX).is_empty());

        fs::remove_dir_all(directory).unwrap();
    }
}
