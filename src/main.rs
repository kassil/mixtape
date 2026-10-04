mod audio;
mod loudness;
mod track;

use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use std::{
    io,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    thread,
    time::Duration,
};
use track::{scan_dir, FileEntry, TrackSide};

struct Candidate {
    entry: FileEntry,
    peak_db: f32,
    state: CandidateState,
}

enum CandidateState {
    Pending,
    Normalized,
    Failed(String),
}

#[derive(PartialEq, Eq)]
enum ModalPhase {
    Scanning,
    Reviewing,
    Normalizing,
}

enum NormalizeMessage {
    ScanProgress {
        index: usize,
        total: usize,
        name: String,
    },
    ScanComplete {
        candidates: Vec<Candidate>,
        errors: Vec<String>,
    },
    NormalizeComplete {
        index: usize,
        result: Result<(), String>,
    },
}

struct NormalizeModal {
    phase: ModalPhase,
    current: String,
    completed: usize,
    total: usize,
    candidates: Vec<Candidate>,
    errors: Vec<String>,
    selected: usize,
    cancelled: Arc<AtomicBool>,
    receiver: Receiver<NormalizeMessage>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut entries = scan_dir(&current_dir);
    let mut list_state = ListState::default();
    if !entries.is_empty() {
        list_state.select(Some(0));
    }
    let mut normalization: Option<NormalizeModal> = None;

    loop {
        if let Some(modal) = normalization.as_mut() {
            while let Ok(message) = modal.receiver.try_recv() {
                match message {
                    NormalizeMessage::ScanProgress { index, total, name } => {
                        modal.completed = index;
                        modal.total = total;
                        modal.current = name;
                    }
                    NormalizeMessage::ScanComplete { candidates, errors } => {
                        modal.candidates = candidates;
                        modal.errors = errors;
                        modal.phase = ModalPhase::Reviewing;
                        modal.current.clear();
                        modal.selected = 0;
                    }
                    NormalizeMessage::NormalizeComplete { index, result } => {
                        if let Some(candidate) = modal.candidates.get_mut(index) {
                            candidate.state = match result {
                                Ok(()) => CandidateState::Normalized,
                                Err(error) => CandidateState::Failed(error),
                            };
                        }
                        modal.phase = ModalPhase::Reviewing;
                        modal.current.clear();
                    }
                }
            }
        }

        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(3)].as_ref())
                .split(f.area());

            let items: Vec<ListItem> = entries
                .iter()
                .map(|e| {
                    let label = if e.is_dir {
                        format!("[{}]", e.name)
                    } else {
                        match e.side {
                            TrackSide::SideA(num) => format!("[Side A {:02}] {}", num, e.name),
                            TrackSide::SideB(num) => format!("[Side B {:02}] {}", num, e.name),
                            TrackSide::Unmatched => format!("[  Other  ] {}", e.name),
                        }
                    };
                    ListItem::new(label).style(e.style())
                })
                .collect();

            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" {} ", current_dir.display())),
                )
                .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD))
                .highlight_symbol("> ");

            f.render_stateful_widget(list, chunks[0], &mut list_state);

            let status_text = " [↑/↓] Select  |  [ENTER] Open / Play  |  [N] Normalize  |  [R] Refresh  |  [Q] Quit";
            let status = Paragraph::new(status_text)
                .block(Block::default().borders(Borders::ALL).title(" Controls "));
            f.render_widget(status, chunks[1]);

            if let Some(modal) = normalization.as_ref() {
                draw_normalization_modal(f, modal);
            }
        })?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            if let Some(modal) = normalization.as_mut() {
                match key.code {
                    KeyCode::Esc => {
                        modal.cancelled.store(true, Ordering::Relaxed);
                        normalization = None;
                    }
                    KeyCode::Up if modal.phase == ModalPhase::Reviewing => {
                        if modal.selected > 0 {
                            modal.selected -= 1;
                        }
                    }
                    KeyCode::Down if modal.phase == ModalPhase::Reviewing => {
                        if modal.selected + 1 < modal.candidates.len() {
                            modal.selected += 1;
                        }
                    }
                    KeyCode::Enter if modal.phase == ModalPhase::Reviewing => {
                        if modal
                            .candidates
                            .get(modal.selected)
                            .is_some_and(|candidate| {
                                !matches!(candidate.state, CandidateState::Normalized)
                            })
                        {
                            start_normalization(modal);
                        }
                    }
                    _ => {}
                }
                continue;
            }

            match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Char('r') => {
                    entries = scan_dir(&current_dir);
                    list_state.select(if entries.is_empty() { None } else { Some(0) });
                }
                KeyCode::Char('n') => {
                    normalization = Some(start_scan(&entries));
                }
                KeyCode::Down => {
                    if let Some(selected) = list_state.selected() {
                        if selected + 1 < entries.len() {
                            list_state.select(Some(selected + 1));
                        }
                    }
                }
                KeyCode::Up => {
                    if let Some(selected) = list_state.selected() {
                        if selected > 0 {
                            list_state.select(Some(selected - 1));
                        }
                    }
                }
                KeyCode::Enter => {
                    if let Some(selected) = list_state.selected() {
                        if let Some(entry) = entries.get(selected) {
                            if entry.is_dir {
                                current_dir = entry.path.clone();
                                entries = scan_dir(&current_dir);
                                list_state.select(if entries.is_empty() { None } else { Some(0) });
                            } else if !matches!(entry.side, TrackSide::Unmatched) {
                                let entries_clone = entries.clone();
                                thread::spawn(move || {
                                    audio::play_sequence_from(&entries_clone, selected, 2);
                                });
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

fn start_scan(entries: &[FileEntry]) -> NormalizeModal {
    let tracks: Vec<FileEntry> = entries
        .iter()
        .filter(|entry| !entry.is_dir && !matches!(entry.side, TrackSide::Unmatched))
        .cloned()
        .collect();
    let total = tracks.len();
    let (sender, receiver) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);

    thread::spawn(move || {
        let mut candidates = Vec::new();
        let mut errors = Vec::new();

        for (index, entry) in tracks.into_iter().enumerate() {
            if worker_cancelled.load(Ordering::Relaxed) {
                return;
            }
            if sender
                .send(NormalizeMessage::ScanProgress {
                    index,
                    total,
                    name: entry.name.clone(),
                })
                .is_err()
            {
                return;
            }

            match loudness::scan_peak_db(&entry.path, &worker_cancelled) {
                Ok(peak_db) if loudness::peak_needs_normalizing(peak_db) => {
                    candidates.push(Candidate {
                        entry,
                        peak_db,
                        state: CandidateState::Pending,
                    });
                }
                Ok(_) => {}
                Err(error) if error != "Cancelled" => {
                    errors.push(format!("{}: {error}", entry.name));
                }
                Err(_) => return,
            }
        }

        let _ = sender.send(NormalizeMessage::ScanComplete { candidates, errors });
    });

    NormalizeModal {
        phase: ModalPhase::Scanning,
        current: "Starting scan".to_string(),
        completed: 0,
        total,
        candidates: Vec::new(),
        errors: Vec::new(),
        selected: 0,
        cancelled,
        receiver,
    }
}

fn start_normalization(modal: &mut NormalizeModal) {
    let index = modal.selected;
    let candidate = &modal.candidates[index];
    let path = candidate.entry.path.clone();
    let peak_db = candidate.peak_db;
    modal.current = candidate.entry.name.clone();
    modal.phase = ModalPhase::Normalizing;

    let (sender, receiver) = mpsc::channel();
    modal.receiver = receiver;
    let cancelled = Arc::clone(&modal.cancelled);
    thread::spawn(move || {
        let result = loudness::normalize_track(&path, peak_db, &cancelled);
        let _ = sender.send(NormalizeMessage::NormalizeComplete { index, result });
    });
}

fn draw_normalization_modal(frame: &mut Frame, modal: &NormalizeModal) {
    let area = centered_rect(75, 65, frame.area());
    frame.render_widget(Clear, area);

    match modal.phase {
        ModalPhase::Scanning => {
            let ratio = if modal.total == 0 {
                1.0
            } else {
                (modal.completed + 1).min(modal.total) as f64 / modal.total as f64
            };
            let progress = format!(
                "Scanning {}/{}: {}",
                (modal.completed + 1).min(modal.total),
                modal.total,
                modal.current
            );
            let gauge = Gauge::default()
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Normalize Tracks (Esc cancels) "),
                )
                .ratio(ratio)
                .label(progress)
                .gauge_style(Style::default().fg(Color::Green));
            frame.render_widget(gauge, area);
        }
        ModalPhase::Normalizing => {
            let paragraph = Paragraph::new(format!(
                "Normalizing {} to a -1 dB peak.\nPress Esc to cancel.",
                modal.current
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Normalize Track "),
            );
            frame.render_widget(paragraph, area);
        }
        ModalPhase::Reviewing => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(3)])
                .split(area);
            let items: Vec<ListItem> = modal
                .candidates
                .iter()
                .map(|candidate| {
                    let state = match &candidate.state {
                        CandidateState::Pending => "pending".to_string(),
                        CandidateState::Normalized => "normalized".to_string(),
                        CandidateState::Failed(error) => format!("failed: {error}"),
                    };
                    ListItem::new(format!(
                        "{}  ({:.1} dB peak)  {state}",
                        candidate.entry.name, candidate.peak_db
                    ))
                })
                .collect();
            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(format!(
                    " Needs normalization ({}) ",
                    modal.candidates.len()
                )))
                .highlight_style(
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("> ");
            let mut list_state = ListState::default();
            if !modal.candidates.is_empty() {
                list_state.select(Some(modal.selected));
            }
            frame.render_stateful_widget(list, chunks[0], &mut list_state);

            let footer = if modal.candidates.is_empty() {
                format!(
                    "No tracks need normalization. {} scan error(s). Esc closes.",
                    modal.errors.len()
                )
            } else {
                format!(
                    "Enter normalizes selected track | {} scan error(s) | Esc closes",
                    modal.errors.len()
                )
            };
            frame.render_widget(Paragraph::new(footer), chunks[1]);
        }
    }
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height) / 2),
            Constraint::Percentage(height),
            Constraint::Percentage((100 - height) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width) / 2),
            Constraint::Percentage(width),
            Constraint::Percentage((100 - width) / 2),
        ])
        .split(vertical[1])[1]
}
