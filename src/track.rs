use ratatui::style::{Color, Style};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackSide {
    SideA(u32),
    SideB(u32),
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
                let supported_audio = path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        extension.eq_ignore_ascii_case("ogg")
                            || extension.eq_ignore_ascii_case("mp3")
                    });
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
        (true, true) => match (a.name == "..", b.name == "..") {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        },
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
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

#[cfg(test)]
mod tests {
    use super::{scan_dir, TrackSide};
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
}
