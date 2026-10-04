mod track;
mod audio;

use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Terminal,
};
use std::io;
use track::{scan_current_dir, TrackSide};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut entries = scan_current_dir();
    let mut list_state = ListState::default();
    if !entries.is_empty() {
        list_state.select(Some(0));
    }

    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(3)].as_ref())
                .split(f.area());

            let items: Vec<ListItem> = entries
                .iter()
                .map(|e| {
                    let label = match e.side {
                        TrackSide::SideA(num) => format!("[Side A {:02}] {}", num, e.name),
                        TrackSide::SideB(num) => format!("[Side B {:02}] {}", num, e.name),
                        TrackSide::Unmatched => format!("[  Other  ] {}", e.name),
                    };
                    ListItem::new(label).style(e.style())
                })
                .collect();

            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(" Mixtape Directory "))
                .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD))
                .highlight_symbol("> ");

            f.render_stateful_widget(list, chunks[0], &mut list_state);

            let status_text = " [↑/↓] Navigate  |  [ENTER] Play Sequence  |  [R] Refresh  |  [Q] Quit";
            let status = Paragraph::new(status_text)
                .block(Block::default().borders(Borders::ALL).title(" Controls "));
            f.render_widget(status, chunks[1]);
        })?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Char('r') => {
                    entries = scan_current_dir();
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
                        let entries_clone = entries.clone();
                        std::thread::spawn(move || {
                            audio::play_sequence_from(&entries_clone, selected, 2);
                        });
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
