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
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use std::{
    collections::HashMap,
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

enum DurationMessage {
    Loaded {
        path: PathBuf,
        duration: Option<Duration>,
    },
}

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
    Cancelling,
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
    ScanCancelled,
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

struct PlaybackState {
    description: String,
    cancelled: Arc<AtomicBool>,
    stopping: bool,
    receiver: Receiver<Result<(), String>>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut entries = scan_dir(&current_dir);
    let (duration_sender, duration_receiver) = mpsc::channel();
    let mut durations = HashMap::new();
    load_durations(&entries, &duration_sender);
    let mut list_state = ListState::default();
    if !entries.is_empty() {
        list_state.select(Some(0));
    }
    let mut normalization: Option<NormalizeModal> = None;
    let mut playback: Option<PlaybackState> = None;
    let mut playback_notice: Option<String> = None;
    let mut quit_after_playback = false;
    let mut silence_secs = audio::DEFAULT_SILENCE_SECS;

    loop {
        while let Ok(DurationMessage::Loaded { path, duration }) = duration_receiver.try_recv() {
            durations.insert(path, duration);
        }

        let playback_result = playback
            .as_ref()
            .and_then(|active| active.receiver.try_recv().ok());
        if let Some(result) = playback_result {
            let was_stopping = playback.as_ref().is_some_and(|active| active.stopping);
            playback = None;
            playback_notice = Some(match result {
                Ok(()) if was_stopping => "Playback stopped".to_string(),
                Ok(()) => "Playback complete".to_string(),
                Err(error) => format!("Playback error: {error}"),
            });
        }

        if quit_after_playback && playback.is_none() {
            break;
        }

        let mut close_normalization = false;
        if let Some(modal) = normalization.as_mut() {
            while let Ok(message) = modal.receiver.try_recv() {
                match message {
                    NormalizeMessage::ScanProgress { index, total, name } => {
                        modal.completed = index;
                        modal.total = total;
                        modal.current = name;
                    }
                    NormalizeMessage::ScanComplete { candidates, errors } => {
                        if modal.phase == ModalPhase::Cancelling {
                            close_normalization = true;
                        } else {
                            modal.candidates = candidates;
                            modal.errors = errors;
                            modal.phase = ModalPhase::Reviewing;
                            modal.current.clear();
                            modal.selected = 0;
                        }
                    }
                    NormalizeMessage::ScanCancelled => {
                        close_normalization = true;
                    }
                    NormalizeMessage::NormalizeComplete { index, result } => {
                        if modal.phase == ModalPhase::Cancelling
                            || matches!(&result, Err(error) if error == "Cancelled")
                        {
                            close_normalization = true;
                        } else {
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
        }
        if close_normalization {
            normalization = None;
        }

        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints(
                    [Constraint::Min(3), Constraint::Length(3), Constraint::Length(3)].as_ref(),
                )
                .split(f.area());

            let items: Vec<ListItem> = entries
                .iter()
                .map(|e| {
                    let label = if e.is_dir {
                        format!("[{}]", e.name)
                    } else {
                        match e.side {
                            TrackSide::SideA(num) => format!(
                                "[Side A {:02}] {:>5} {}",
                                num,
                                format_duration(durations.get(&e.path).and_then(|duration| *duration)),
                                e.name
                            ),
                            TrackSide::SideB(num) => format!(
                                "[Side B {:02}] {:>5} {}",
                                num,
                                format_duration(durations.get(&e.path).and_then(|duration| *duration)),
                                e.name
                            ),
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

            let totals = side_totals(&entries, &durations, silence_secs);
            let totals_text = format!(
                "Side A: {}    Side B: {}    Pause: {}s",
                format_duration(totals.0),
                format_duration(totals.1),
                silence_secs
            );
            f.render_widget(
                Paragraph::new(totals_text)
                    .block(Block::default().borders(Borders::ALL).title(" Side totals ")),
                chunks[1],
            );

            let status_text = if let Some(active) = playback.as_ref() {
                if active.stopping {
                    format!("Stopping {}...", active.description)
                } else {
                    format!(
                        "Playing {} | [S] Stop | Sequence locked | [Q] Stop and Quit",
                        active.description
                    )
                }
            } else if let Some(notice) = playback_notice.as_ref() {
                format!(
                    "[↑/↓] Select | [ENTER] Open / Play | [[]/]] Pause: {}s | [N] Normalize | [R] Refresh | [Q] Quit | {notice}",
                    silence_secs
                )
            } else {
                format!(
                    " [↑/↓] Select  |  [ENTER] Open / Play  |  [[]/]] Pause: {}s  |  [N] Normalize  |  [R] Refresh  |  [Q] Quit",
                    silence_secs
                )
            };
            let status = Paragraph::new(status_text)
                .block(Block::default().borders(Borders::ALL).title(" Controls "));
            f.render_widget(status, chunks[2]);

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
                        if modal.phase == ModalPhase::Reviewing {
                            normalization = None;
                        } else if modal.phase != ModalPhase::Cancelling {
                            modal.cancelled.store(true, Ordering::Relaxed);
                            modal.phase = ModalPhase::Cancelling;
                            modal.current = "Waiting for worker to stop".to_string();
                        }
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

            if let Some(active) = playback.as_mut() {
                match key.code {
                    KeyCode::Char('s' | 'S') => {
                        active.stopping = true;
                        active.cancelled.store(true, Ordering::Relaxed);
                    }
                    KeyCode::Char('q' | 'Q') => {
                        active.stopping = true;
                        active.cancelled.store(true, Ordering::Relaxed);
                        quit_after_playback = true;
                    }
                    _ => {}
                }
                continue;
            }

            match key.code {
                KeyCode::Char('q' | 'Q') => break,
                // Refresh the directory listing
                KeyCode::Char('r' | 'R') => {
                    entries = scan_dir(&current_dir);
                    load_durations(&entries, &duration_sender);
                    list_state.select(if entries.is_empty() { None } else { Some(0) });
                }
                // Adjust silence duration
                KeyCode::Char('[') => silence_secs = silence_secs.saturating_sub(1),
                KeyCode::Char(']') => silence_secs = silence_secs.saturating_add(1),
                // Start normalization scan
                // TODO Maybe scan should start from the cursor
                KeyCode::Char('n' | 'N') => {
                    normalization = Some(start_scan(&entries));
                }
                // Navigate through the list of entries
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
                // Enter key: navigate into directories or play selected track
                KeyCode::Enter => {
                    if let Some(selected) = list_state.selected() {
                        if let Some(entry) = entries.get(selected) {
                            if entry.is_dir {
                                // Navigate into the selected directory
                                current_dir = entry.path.clone();
                                entries = scan_dir(&current_dir);
                                load_durations(&entries, &duration_sender);
                                list_state.select(if entries.is_empty() { None } else { Some(0) });
                            } else if !matches!(entry.side, TrackSide::Unmatched) {
                                // Play the sequence starting from the selected track
                                let entries_clone = entries.clone();
                                let track_name = entry.name.clone();
                                let cancelled = Arc::new(AtomicBool::new(false));
                                let worker_cancelled = Arc::clone(&cancelled);
                                let (sender, receiver) = mpsc::channel();
                                thread::spawn(move || {
                                    let result = audio::play_sequence_from(
                                        &entries_clone,
                                        selected,
                                        silence_secs,
                                        &worker_cancelled,
                                    );
                                    let _ = sender.send(result);
                                });
                                playback_notice = None;
                                playback = Some(PlaybackState {
                                    description: track_name,
                                    cancelled,
                                    stopping: false,
                                    receiver,
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

fn load_durations(entries: &[FileEntry], sender: &mpsc::Sender<DurationMessage>) {
    for entry in entries
        .iter()
        .filter(|entry| !entry.is_dir && !matches!(entry.side, TrackSide::Unmatched))
    {
        let path = entry.path.clone();
        let sender = sender.clone();
        thread::spawn(move || {
            let duration = audio::track_duration(&path).ok();
            let _ = sender.send(DurationMessage::Loaded { path, duration });
        });
    }
}

fn format_duration(duration: Option<Duration>) -> String {
    let Some(duration) = duration else {
        return "--:--".to_string();
    };
    let total_secs = duration.as_secs();
    format!("{}:{:02}", total_secs / 60, total_secs % 60)
}

fn side_totals(
    entries: &[FileEntry],
    durations: &HashMap<PathBuf, Option<Duration>>,
    silence_secs: u64,
) -> (Option<Duration>, Option<Duration>) {
    fn total_for_side(
        entries: &[FileEntry],
        durations: &HashMap<PathBuf, Option<Duration>>,
        is_side_a: bool,
        silence_secs: u64,
    ) -> Option<Duration> {
        let tracks: Vec<_> = entries
            .iter()
            .filter(|entry| {
                matches!(
                    (&entry.side, is_side_a),
                    (TrackSide::SideA(_), true) | (TrackSide::SideB(_), false)
                )
            })
            .collect();
        let mut total = Duration::ZERO;
        for entry in &tracks {
            total = total.checked_add((*durations.get(&entry.path)?)?)?;
        }
        total.checked_add(Duration::from_secs(
            silence_secs.saturating_mul(tracks.len().saturating_sub(1) as u64),
        ))
    }

    (
        total_for_side(entries, durations, true, silence_secs),
        total_for_side(entries, durations, false, silence_secs),
    )
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
                let _ = sender.send(NormalizeMessage::ScanCancelled);
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
                Err(_) => {
                    let _ = sender.send(NormalizeMessage::ScanCancelled);
                    return;
                }
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
        ModalPhase::Cancelling => {
            let paragraph = Paragraph::new(modal.current.as_str()).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Stopping Worker "),
            );
            frame.render_widget(paragraph, area);
        }
        ModalPhase::Reviewing => {
            let constraints = if modal.errors.is_empty() {
                vec![Constraint::Min(3), Constraint::Length(3)]
            } else {
                vec![
                    Constraint::Percentage(40),
                    Constraint::Min(3),
                    Constraint::Length(3),
                ]
            };
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints(constraints)
                .split(area);

            let list_area = if modal.errors.is_empty() {
                chunks[0]
            } else {
                let error_text = modal.errors.join("\n");
                let errors = Paragraph::new(error_text)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(format!(" Scan errors ({}) ", modal.errors.len())),
                    )
                    .style(Style::default().fg(Color::Red))
                    .wrap(Wrap { trim: false });
                frame.render_widget(errors, chunks[0]);
                chunks[1]
            };
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
            frame.render_stateful_widget(list, list_area, &mut list_state);

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
