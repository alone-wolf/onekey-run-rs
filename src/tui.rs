use std::collections::VecDeque;
use std::io::{self, Stdout};
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Tabs, Wrap};
use ratatui::{Frame, Terminal};

use crate::error::{AppError, AppResult};
use crate::orchestrator::ShutdownController;
use crate::process::{self, LogEvent, LogStream, SpawnedProcess};
use crate::runtime_state::{self, RuntimeEvent};

const MAX_LOG_LINES: usize = 500;
const MAX_EVENTS: usize = 500;

pub fn run_dashboard(
    project_root: &Path,
    running: &mut [SpawnedProcess],
    log_rx: Receiver<LogEvent>,
    shutdown: ShutdownController,
) -> AppResult<()> {
    let mut terminal = TerminalSession::enter()?;
    let mut state = DashboardState::new(running);

    let result = loop {
        state.drain_logs(&log_rx);
        state.refresh_events(project_root);

        if shutdown.shutdown_requested() {
            state.set_notice("interrupt received, stopping services. press Ctrl-C again to force.");
            break Ok(());
        }

        let mut exit_result = None;
        for process in running.iter_mut() {
            if let Some(status) = process::service_exited(&mut process.child)? {
                state.mark_exited(&process.state.service_name, format!("Exited ({status})"));
                if !runtime_state::state_path(project_root).exists() {
                    exit_result = Some(Ok(()));
                } else {
                    exit_result = Some(Err(AppError::runtime_failed(format!(
                        "service `{}` exited with status {status}",
                        process.state.service_name
                    ))));
                }
                break;
            }
        }
        if let Some(result) = exit_result {
            break result;
        }

        terminal.draw(&state)?;

        if event::poll(Duration::from_millis(150)).map_err(|error| {
            AppError::runtime_failed(format!("failed to poll TUI events: {error}"))
        })? {
            let event = event::read().map_err(|error| {
                AppError::runtime_failed(format!("failed to read TUI event: {error}"))
            })?;
            if state.handle_event(event, &shutdown) {
                break Ok(());
            }
        }
    };

    terminal.exit()?;
    result
}

struct DashboardState {
    services: Vec<ServicePanel>,
    events: VecDeque<RuntimeEvent>,
    main_tab: DashboardTab,
    selected: usize,
    notice: String,
}

struct ServicePanel {
    name: String,
    pid: u32,
    status: String,
    logs: VecDeque<String>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum DashboardTab {
    Overview,
    Logs,
    Events,
}

impl DashboardState {
    fn new(running: &[SpawnedProcess]) -> Self {
        let services = running
            .iter()
            .map(|process| ServicePanel {
                name: process.state.service_name.clone(),
                pid: process.state.pid,
                status: "Running".to_owned(),
                logs: VecDeque::new(),
            })
            .collect();

        Self {
            services,
            events: VecDeque::new(),
            main_tab: DashboardTab::Overview,
            selected: 0,
            notice:
                "Tab switch panel, Left/Right switch service, 1-9 jump service, q/Esc graceful stop"
                    .to_owned(),
        }
    }

    fn drain_logs(&mut self, log_rx: &Receiver<LogEvent>) {
        while let Ok(event) = log_rx.try_recv() {
            self.push_log(event);
        }
    }

    fn push_log(&mut self, event: LogEvent) {
        if let Some(panel) = self
            .services
            .iter_mut()
            .find(|panel| panel.name == event.service_name)
        {
            let prefix = match event.stream {
                LogStream::Stdout => "out",
                LogStream::Stderr => "err",
            };
            panel.logs.push_back(format!("[{prefix}] {}", event.line));
            while panel.logs.len() > MAX_LOG_LINES {
                panel.logs.pop_front();
            }
        }
    }

    fn mark_exited(&mut self, service_name: &str, status: String) {
        if let Some(panel) = self
            .services
            .iter_mut()
            .find(|panel| panel.name == service_name)
        {
            panel.status = status;
        }
    }

    fn refresh_events(&mut self, project_root: &Path) {
        if let Ok(events) = runtime_state::load_events(project_root) {
            self.events = events.into_iter().rev().take(MAX_EVENTS).collect();
            self.events.make_contiguous().reverse();
        }
    }

    fn handle_event(&mut self, event: Event, shutdown: &ShutdownController) -> bool {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => {
                    self.notice = "graceful stop requested.".to_owned();
                    true
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if shutdown.force_requested() {
                        self.notice = "force stop requested.".to_owned();
                    } else {
                        self.notice =
                            "interrupt received, stopping services. press Ctrl-C again to force."
                                .to_owned();
                    }
                    true
                }
                KeyCode::BackTab => {
                    self.main_tab = match self.main_tab {
                        DashboardTab::Overview => DashboardTab::Events,
                        DashboardTab::Logs => DashboardTab::Overview,
                        DashboardTab::Events => DashboardTab::Logs,
                    };
                    false
                }
                KeyCode::Tab => {
                    self.main_tab = match self.main_tab {
                        DashboardTab::Overview => DashboardTab::Logs,
                        DashboardTab::Logs => DashboardTab::Events,
                        DashboardTab::Events => DashboardTab::Overview,
                    };
                    false
                }
                KeyCode::Left => {
                    if !self.services.is_empty() {
                        if self.selected == 0 {
                            self.selected = self.services.len() - 1;
                        } else {
                            self.selected -= 1;
                        }
                    }
                    false
                }
                KeyCode::Right => {
                    if !self.services.is_empty() {
                        self.selected = (self.selected + 1) % self.services.len();
                    }
                    false
                }
                KeyCode::Char(ch) if ch.is_ascii_digit() => {
                    let index = ch.to_digit(10).unwrap_or(0) as usize;
                    if index > 0 && index <= self.services.len() {
                        self.selected = index - 1;
                    }
                    false
                }
                _ => false,
            },
            _ => false,
        }
    }

    fn render(&self, frame: &mut Frame<'_>) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(7),
                Constraint::Min(8),
                Constraint::Length(2),
            ])
            .split(frame.area());

        let main_titles = vec![
            Line::from("Overview"),
            Line::from("Logs"),
            Line::from("Events"),
        ];
        let main_tabs = Tabs::new(main_titles)
            .select(match self.main_tab {
                DashboardTab::Overview => 0,
                DashboardTab::Logs => 1,
                DashboardTab::Events => 2,
            })
            .block(Block::default().borders(Borders::ALL).title("Panels"))
            .highlight_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            );
        frame.render_widget(main_tabs, chunks[0]);

        let titles: Vec<Line<'_>> = self
            .services
            .iter()
            .map(|service| Line::from(service.name.clone()))
            .collect();
        let tabs = Tabs::new(titles)
            .select(self.selected.min(self.services.len().saturating_sub(1)))
            .block(Block::default().borders(Borders::ALL).title("Services"))
            .highlight_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            );
        frame.render_widget(tabs, chunks[1]);

        let summary_items: Vec<ListItem<'_>> = self
            .services
            .iter()
            .map(|service| {
                let status_color = if service.status == "Running" {
                    Color::Green
                } else {
                    Color::Red
                };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{:<16}", service.name),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(" pid {:<7} ", service.pid)),
                    Span::styled(service.status.clone(), Style::default().fg(status_color)),
                ]))
            })
            .collect();
        let summary =
            List::new(summary_items).block(Block::default().borders(Borders::ALL).title("Status"));
        frame.render_widget(summary, chunks[2]);

        match self.main_tab {
            DashboardTab::Overview => {
                let overview_lines = self
                    .services
                    .get(self.selected)
                    .map(|service| {
                        vec![
                            Line::from(format!("service: {}", service.name)),
                            Line::from(format!("pid: {}", service.pid)),
                            Line::from(format!("status: {}", service.status)),
                            Line::from(format!("captured logs: {}", service.logs.len())),
                            Line::from(format!("captured events: {}", self.events.len())),
                        ]
                    })
                    .unwrap_or_else(|| vec![Line::from("No services started.")]);
                let overview = Paragraph::new(overview_lines)
                    .block(Block::default().borders(Borders::ALL).title("Overview"))
                    .wrap(Wrap { trim: false });
                frame.render_widget(overview, chunks[3]);
            }
            DashboardTab::Logs => {
                let log_lines = self
                    .services
                    .get(self.selected)
                    .map(|service| {
                        if service.logs.is_empty() {
                            vec![Line::from("No logs captured yet.")]
                        } else {
                            service
                                .logs
                                .iter()
                                .cloned()
                                .map(Line::from)
                                .collect::<Vec<_>>()
                        }
                    })
                    .unwrap_or_else(|| vec![Line::from("No services started.")]);
                let logs = Paragraph::new(log_lines)
                    .block(Block::default().borders(Borders::ALL).title("Logs"))
                    .wrap(Wrap { trim: false });
                frame.render_widget(logs, chunks[3]);
            }
            DashboardTab::Events => {
                let event_lines = self.render_event_lines();
                let events = Paragraph::new(event_lines)
                    .block(Block::default().borders(Borders::ALL).title("Events"))
                    .wrap(Wrap { trim: false });
                frame.render_widget(events, chunks[3]);
            }
        }

        let help = Paragraph::new(self.notice.as_str())
            .block(Block::default().borders(Borders::ALL).title("Keys"));
        frame.render_widget(help, chunks[4]);
    }

    fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = notice.into();
    }

    fn render_event_lines(&self) -> Vec<Line<'_>> {
        if self.events.is_empty() {
            return vec![Line::from("No orchestrator events captured yet.")];
        }

        self.events
            .iter()
            .rev()
            .take(MAX_EVENTS)
            .map(|event| {
                let color = if event.event_type.contains("failed") {
                    Color::Red
                } else if event.event_type.contains("timeout") {
                    Color::Yellow
                } else if event.event_type.contains("started") {
                    Color::Blue
                } else {
                    Color::Green
                };
                let service = event.service_name.as_deref().unwrap_or("-");
                let hook = event.hook_name.as_deref().unwrap_or("-");
                let action = event.action_name.as_deref().unwrap_or("-");
                Line::from(vec![
                    Span::styled(
                        format!("{:>10}", event.event_type),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(" | svc={} | hook={} | action={} | ", service, hook, action)),
                    Span::raw(event.detail.clone()),
                ])
            })
            .collect()
    }
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    active: bool,
}

impl TerminalSession {
    fn enter() -> AppResult<Self> {
        enable_raw_mode().map_err(|error| {
            AppError::runtime_failed(format!("failed to enable raw mode: {error}"))
        })?;

        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen).map_err(|error| {
            AppError::runtime_failed(format!("failed to enter alternate screen: {error}"))
        })?;

        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend).map_err(|error| {
            AppError::runtime_failed(format!("failed to create terminal: {error}"))
        })?;
        terminal.clear().map_err(|error| {
            AppError::runtime_failed(format!("failed to clear terminal: {error}"))
        })?;

        Ok(Self {
            terminal,
            active: true,
        })
    }

    fn draw(&mut self, state: &DashboardState) -> AppResult<()> {
        self.terminal
            .draw(|frame| state.render(frame))
            .map(|_| ())
            .map_err(|error| AppError::runtime_failed(format!("failed to draw TUI: {error}")))
    }

    fn exit(&mut self) -> AppResult<()> {
        if self.active {
            disable_raw_mode().map_err(|error| {
                AppError::runtime_failed(format!("failed to disable raw mode: {error}"))
            })?;
            execute!(self.terminal.backend_mut(), LeaveAlternateScreen).map_err(|error| {
                AppError::runtime_failed(format!("failed to leave alternate screen: {error}"))
            })?;
            self.terminal.show_cursor().map_err(|error| {
                AppError::runtime_failed(format!("failed to restore cursor: {error}"))
            })?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
            let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
            let _ = self.terminal.show_cursor();
            self.active = false;
        }
    }
}
