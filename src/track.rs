use regex::Regex;
use ratatui::style::{Color, Style, Modifier};
use std::fs;
use std::path::PathBuf;

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
}

impl FileEntry {
    /// Color mapping: Side A (Green), Side B (Yellow), Unmatched (Dark Gray)
    pub fn style(&self) -> Style {
        match self.side {
            TrackSide::SideA(_) => Style::default().fg(Color::Green),
            TrackSide::SideB(_) => Style::default().fg(Color::Yellow),
            TrackSide::Unmatched => Style::default().fg(Color::DarkGray),
        }
    }
}

pub fn scan_current_dir() -> Vec<FileEntry> {
    let re = Regex::new(r"^(?i)([AB])(\d+)\.(ogg|mp3)$").unwrap();
    let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    
    let mut entries = Vec::new();

    if let Ok(read_dir) = fs::read_dir(current_dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.is_file() {
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                let side = if let Some(caps) = re.captures(&name) {
                    let side_char = caps[1].to_uppercase();
                    let num: u32 = caps[2].parse().unwrap_or(0);
                    if side_char == "A" {
                        TrackSide::SideA(num)
                    } else {
                        TrackSide::SideB(num)
                    }
                } else {
                    TrackSide::Unmatched
                };

                entries.push(FileEntry { name, path, side });
            }
        }
    }

    // Sort alphabetically naturally groups A01..A99, then B01..B99, then other files
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}
