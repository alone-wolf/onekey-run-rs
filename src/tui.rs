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
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Tabs, Wrap};
use ratatui::{Frame, Terminal};

use crate::error::{AppError, AppResult};
use crate::orchestrator::{RunPlan, RuntimeOutputContext, ShutdownController, WatchRuntime};
use crate::process::{self, LogEvent, LogStream, SpawnedProcess};
use crate::runtime_state::{self, RuntimeEvent, RuntimeState};

const MAX_LOG_LINES: usize = 500;
const MAX_EVENTS: usize = 500;

pub fn run_dashboard(
    plan: &RunPlan,
    running: &mut Vec<SpawnedProcess>,
    runtime_state: &mut RuntimeState,
    watch_runtime: Option<&mut WatchRuntime>,
    log_rx: Receiver<LogEvent>,
    shutdown: ShutdownController,
    output_context: &RuntimeOutputContext,
) -> AppResult<()> {
    let mut terminal = TerminalSession::enter()?;
    let mut state = DashboardState::new(&plan.services, running);
    let mut watch_runtime = watch_runtime;

    let result = loop {
        state.sync_running(&plan.services, running);
        state.drain_logs(&log_rx);
        state.refresh_events(&plan.project_root);

        if shutdown.shutdown_requested() {
            state.set_notice("interrupt received, stopping services. press Ctrl-C again to force.");
            break Ok(());
        }

        if let Some(watch_runtime) = watch_runtime.as_mut() {
            watch_runtime.tick(plan, running, runtime_state, &shutdown, output_context)?;
            state.sync_running(&plan.services, running);
        }

        let mut exit_result = None;
        for process in running.iter_mut() {
            if let Some(status) = process::service_exited(&mut process.child)? {
                let graceful = !runtime_state::state_path(&plan.project_root).exists();
                state.mark_exited(
                    &process.state.service_name,
                    format!("exit status {status}"),
                    graceful,
                );
                if graceful {
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
    log_scroll: usize,
    event_scroll: usize,
    notice: String,
}

struct ServicePanel {
    name: String,
    pid: u32,
    watching: bool,
    status: ServiceStatus,
    last_watch_event: Option<String>,
    logs: VecDeque<String>,
}

enum ServiceStatus {
    Running,
    Stopped(String),
    Failed(String),
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum DashboardTab {
    Overview,
    Logs,
    Events,
}

impl DashboardState {
    fn new(services: &[crate::config::ResolvedServiceConfig], running: &[SpawnedProcess]) -> Self {
        let running_by_name = running
            .iter()
            .map(|process| (process.state.service_name.as_str(), process))
            .collect::<std::collections::BTreeMap<_, _>>();
        let services = services
            .iter()
            .map(|service| {
                let running = running_by_name.get(service.name.as_str());
                ServicePanel {
                    name: service.name.clone(),
                    pid: running.map(|process| process.state.pid).unwrap_or(0),
                    watching: service.watch.is_some(),
                    status: if running.is_some() {
                        ServiceStatus::Running
                    } else {
                        ServiceStatus::Stopped("not running".to_owned())
                    },
                    last_watch_event: None,
                    logs: VecDeque::new(),
                }
            })
            .collect();

        Self {
            services,
            events: VecDeque::new(),
            main_tab: DashboardTab::Overview,
            selected: 0,
            log_scroll: 0,
            event_scroll: 0,
            notice: "Up/Down select service | Left/Right/Tab switch panel | j/k or PgUp/PgDn scroll | Home/End jump | q/Esc stop".to_owned(),
        }
    }

    fn sync_running(
        &mut self,
        services: &[crate::config::ResolvedServiceConfig],
        running: &[SpawnedProcess],
    ) {
        let running_by_name = running
            .iter()
            .map(|process| (process.state.service_name.as_str(), process))
            .collect::<std::collections::BTreeMap<_, _>>();

        for service in services {
            let Some(panel) = self
                .services
                .iter_mut()
                .find(|panel| panel.name == service.name)
            else {
                continue;
            };

            if let Some(process) = running_by_name.get(service.name.as_str()) {
                panel.pid = process.state.pid;
                panel.status = ServiceStatus::Running;
            } else if matches!(panel.status, ServiceStatus::Running) {
                panel.pid = 0;
                panel.status = ServiceStatus::Stopped("not running".to_owned());
            }
        }
    }

    fn drain_logs(&mut self, log_rx: &Receiver<LogEvent>) {
        while let Ok(event) = log_rx.try_recv() {
            self.push_log(event);
        }
    }

    fn push_log(&mut self, event: LogEvent) {
        let selected_name = self.selected_service_name().to_owned();
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
            if panel.name == selected_name && self.log_scroll > 0 {
                self.log_scroll = self.log_scroll.saturating_add(1);
            }
        }
    }

    fn mark_exited(&mut self, service_name: &str, detail: String, graceful: bool) {
        if let Some(panel) = self
            .services
            .iter_mut()
            .find(|panel| panel.name == service_name)
        {
            panel.status = if graceful {
                ServiceStatus::Stopped(detail)
            } else {
                ServiceStatus::Failed(detail)
            };
        }
    }

    fn refresh_events(&mut self, project_root: &Path) {
        if let Ok(events) = runtime_state::load_events(project_root) {
            self.events = events.into_iter().rev().take(MAX_EVENTS).collect();
            self.events.make_contiguous().reverse();
            self.refresh_watch_summaries();
        }
    }

    fn refresh_watch_summaries(&mut self) {
        for panel in &mut self.services {
            panel.last_watch_event = self
                .events
                .iter()
                .rev()
                .find(|event| {
                    event.service_name.as_deref() == Some(panel.name.as_str())
                        && event.event_type.starts_with("watch_")
                })
                .map(|event| format!("{} | {}", event.event_type, event.detail));
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
                    self.select_previous_tab();
                    false
                }
                KeyCode::Tab | KeyCode::Right => {
                    self.select_next_tab();
                    false
                }
                KeyCode::Left => {
                    self.select_previous_tab();
                    false
                }
                KeyCode::Up => {
                    self.select_previous_service();
                    false
                }
                KeyCode::Down => {
                    self.select_next_service();
                    false
                }
                KeyCode::PageUp => {
                    self.scroll_current_panel_up(10);
                    false
                }
                KeyCode::PageDown => {
                    self.scroll_current_panel_down(10);
                    false
                }
                KeyCode::Home => {
                    self.scroll_current_panel_to_oldest();
                    false
                }
                KeyCode::End => {
                    self.scroll_current_panel_to_latest();
                    false
                }
                KeyCode::Char('k') => {
                    self.scroll_current_panel_up(1);
                    false
                }
                KeyCode::Char('j') => {
                    self.scroll_current_panel_down(1);
                    false
                }
                KeyCode::Char(ch) if ch.is_ascii_digit() => {
                    let index = ch.to_digit(10).unwrap_or(0) as usize;
                    if index > 0 && index <= self.services.len() {
                        self.selected = index - 1;
                        self.log_scroll = 0;
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
                Constraint::Min(12),
                Constraint::Length(3),
            ])
            .split(frame.area());

        self.render_header(frame, chunks[0]);
        self.render_body(frame, chunks[1]);

        let footer = Paragraph::new(self.notice.as_str()).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Keys / Notice"),
        );
        frame.render_widget(footer, chunks[2]);
    }

    fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = notice.into();
    }

    fn selected_service(&self) -> Option<&ServicePanel> {
        self.services.get(self.selected)
    }

    fn selected_service_name(&self) -> &str {
        self.selected_service()
            .map(|service| service.name.as_str())
            .unwrap_or("")
    }

    fn running_count(&self) -> usize {
        self.services
            .iter()
            .filter(|service| matches!(service.status, ServiceStatus::Running))
            .count()
    }

    fn watching_count(&self) -> usize {
        self.services
            .iter()
            .filter(|service| service.watching)
            .count()
    }

    fn select_previous_service(&mut self) {
        if self.services.is_empty() {
            return;
        }
        if self.selected == 0 {
            self.selected = self.services.len() - 1;
        } else {
            self.selected -= 1;
        }
        self.log_scroll = 0;
    }

    fn select_next_service(&mut self) {
        if self.services.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.services.len();
        self.log_scroll = 0;
    }

    fn select_previous_tab(&mut self) {
        self.main_tab = match self.main_tab {
            DashboardTab::Overview => DashboardTab::Events,
            DashboardTab::Logs => DashboardTab::Overview,
            DashboardTab::Events => DashboardTab::Logs,
        };
    }

    fn select_next_tab(&mut self) {
        self.main_tab = match self.main_tab {
            DashboardTab::Overview => DashboardTab::Logs,
            DashboardTab::Logs => DashboardTab::Events,
            DashboardTab::Events => DashboardTab::Overview,
        };
    }

    fn scroll_current_panel_up(&mut self, lines: usize) {
        match self.main_tab {
            DashboardTab::Logs => {
                self.log_scroll = self.log_scroll.saturating_add(lines);
            }
            DashboardTab::Events => {
                self.event_scroll = self.event_scroll.saturating_add(lines);
            }
            DashboardTab::Overview => {}
        }
    }

    fn scroll_current_panel_down(&mut self, lines: usize) {
        match self.main_tab {
            DashboardTab::Logs => {
                self.log_scroll = self.log_scroll.saturating_sub(lines);
            }
            DashboardTab::Events => {
                self.event_scroll = self.event_scroll.saturating_sub(lines);
            }
            DashboardTab::Overview => {}
        }
    }

    fn scroll_current_panel_to_oldest(&mut self) {
        match self.main_tab {
            DashboardTab::Logs => {
                self.log_scroll = usize::MAX;
            }
            DashboardTab::Events => {
                self.event_scroll = usize::MAX;
            }
            DashboardTab::Overview => {}
        }
    }

    fn scroll_current_panel_to_latest(&mut self) {
        match self.main_tab {
            DashboardTab::Logs => {
                self.log_scroll = 0;
            }
            DashboardTab::Events => {
                self.event_scroll = 0;
            }
            DashboardTab::Overview => {}
        }
    }

    fn render_header(&self, frame: &mut Frame<'_>, area: Rect) {
        let selected = self
            .selected_service()
            .map(|service| service.name.as_str())
            .unwrap_or("-");
        let panel = match self.main_tab {
            DashboardTab::Overview => "Overview",
            DashboardTab::Logs => "Logs",
            DashboardTab::Events => "Events",
        };
        let header = Paragraph::new(Line::from(vec![
            Span::styled(
                "onekey-run monitor",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "  services {}/{} running",
                self.running_count(),
                self.services.len()
            )),
            Span::raw(format!("  watching {}", self.watching_count())),
            Span::raw(format!("  selected {}", selected)),
            Span::raw(format!("  panel {}", panel)),
        ]))
        .block(Block::default().borders(Borders::ALL).title("Dashboard"));
        frame.render_widget(header, area);
    }

    fn render_body(&self, frame: &mut Frame<'_>, area: Rect) {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(36), Constraint::Min(20)])
            .split(area);
        self.render_sidebar(frame, columns[0]);
        self.render_main_panel(frame, columns[1]);
    }

    fn render_sidebar(&self, frame: &mut Frame<'_>, area: Rect) {
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(8), Constraint::Length(8)])
            .split(area);

        let visible_service_count = ((sections[0].height.saturating_sub(2)) as usize / 2).max(1);
        let (start, end) = self.service_window_bounds(visible_service_count);
        let service_items: Vec<ListItem<'_>> = self
            .services
            .iter()
            .enumerate()
            .skip(start)
            .take(end.saturating_sub(start))
            .map(|(index, service)| {
                let selected = index == self.selected;
                let line = Line::from(vec![
                    Span::styled(
                        if selected { "> " } else { "  " },
                        if selected {
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        },
                    ),
                    Span::styled(
                        service.name.clone(),
                        if selected {
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().add_modifier(Modifier::BOLD)
                        },
                    ),
                    Span::raw(if service.watching { "  watch on" } else { "" }),
                    Span::raw(format!("  pid {}", service.pid)),
                ]);
                let status = Line::from(vec![
                    Span::raw("   "),
                    Span::styled(
                        service.status.label(),
                        Style::default()
                            .fg(service.status.color())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!("  logs {}", service.logs.len())),
                ]);
                ListItem::new(vec![line, status])
            })
            .collect();

        let services = if service_items.is_empty() {
            List::new(vec![ListItem::new(Line::from("No services started."))])
        } else {
            List::new(service_items)
        }
        .block(Block::default().borders(Borders::ALL).title(format!(
            "Services {}-{} / {}",
            if self.services.is_empty() {
                0
            } else {
                start + 1
            },
            end,
            self.services.len()
        )));
        frame.render_widget(services, sections[0]);

        let detail_lines = self
            .selected_service()
            .map(|service| {
                let latest = service
                    .logs
                    .back()
                    .cloned()
                    .unwrap_or_else(|| "No logs captured yet.".to_owned());
                vec![
                    Line::from(format!("name: {}", service.name)),
                    Line::from(format!("pid: {}", service.pid)),
                    Line::from(format!("status: {}", service.status.label())),
                    Line::from(format!("detail: {}", service.status.detail())),
                    Line::from(format!(
                        "watch: {}",
                        if service.watching {
                            "enabled"
                        } else {
                            "disabled"
                        }
                    )),
                    Line::from(format!(
                        "last watch: {}",
                        service.last_watch_event.as_deref().unwrap_or("none")
                    )),
                    Line::from(format!("logs: {}", service.logs.len())),
                    Line::from("latest:"),
                    Line::from(latest),
                ]
            })
            .unwrap_or_else(|| vec![Line::from("No service selected.")]);
        let detail = Paragraph::new(detail_lines)
            .block(Block::default().borders(Borders::ALL).title("Selected"))
            .wrap(Wrap { trim: false });
        frame.render_widget(detail, sections[1]);
    }

    fn render_main_panel(&self, frame: &mut Frame<'_>, area: Rect) {
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(8)])
            .split(area);

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
            .block(Block::default().borders(Borders::ALL).title("Panel"))
            .highlight_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            );
        frame.render_widget(main_tabs, sections[0]);

        match self.main_tab {
            DashboardTab::Overview => self.render_overview_panel(frame, sections[1]),
            DashboardTab::Logs => self.render_logs_panel(frame, sections[1]),
            DashboardTab::Events => self.render_events_panel(frame, sections[1]),
        }
    }

    fn render_overview_panel(&self, frame: &mut Frame<'_>, area: Rect) {
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(8), Constraint::Min(8)])
            .split(area);
        let lower = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(sections[1]);

        let summary_lines = self
            .selected_service()
            .map(|service| {
                vec![
                    Line::from(format!("service: {}", service.name)),
                    Line::from(format!("pid: {}", service.pid)),
                    Line::from(format!("status: {}", service.status.label())),
                    Line::from(format!("detail: {}", service.status.detail())),
                    Line::from(format!(
                        "watch: {}",
                        if service.watching {
                            "enabled"
                        } else {
                            "disabled"
                        }
                    )),
                    Line::from(format!(
                        "last watch: {}",
                        service.last_watch_event.as_deref().unwrap_or("none")
                    )),
                    Line::from(format!("captured logs: {}", service.logs.len())),
                    Line::from(format!("captured events: {}", self.events.len())),
                ]
            })
            .unwrap_or_else(|| vec![Line::from("No services started.")]);
        let summary = Paragraph::new(summary_lines)
            .block(Block::default().borders(Borders::ALL).title("Summary"))
            .wrap(Wrap { trim: false });
        frame.render_widget(summary, sections[0]);

        let log_preview = Paragraph::new(self.render_selected_log_lines(12, 0))
            .block(Block::default().borders(Borders::ALL).title("Recent Logs"))
            .wrap(Wrap { trim: false });
        frame.render_widget(log_preview, lower[0]);

        let event_preview = Paragraph::new(self.render_event_lines(12, 0))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Recent Events"),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(event_preview, lower[1]);
    }

    fn render_logs_panel(&self, frame: &mut Frame<'_>, area: Rect) {
        let title = self
            .selected_service()
            .map(|service| {
                if self.log_scroll == 0 {
                    format!("Logs: {} (live)", service.name)
                } else {
                    format!("Logs: {} (scroll {} lines)", service.name, self.log_scroll)
                }
            })
            .unwrap_or_else(|| "Logs".to_owned());
        let visible_lines = area.height.saturating_sub(2) as usize;
        let logs = Paragraph::new(self.render_selected_log_lines(visible_lines, self.log_scroll))
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: false });
        frame.render_widget(logs, area);
    }

    fn render_events_panel(&self, frame: &mut Frame<'_>, area: Rect) {
        let title = if self.event_scroll == 0 {
            "Events (latest)".to_owned()
        } else {
            format!("Events (scroll {} lines)", self.event_scroll)
        };
        let visible_lines = area.height.saturating_sub(2) as usize;
        let events = Paragraph::new(self.render_event_lines(visible_lines, self.event_scroll))
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: false });
        frame.render_widget(events, area);
    }

    fn render_selected_log_lines(
        &self,
        visible_limit: usize,
        scroll_from_bottom: usize,
    ) -> Vec<Line<'_>> {
        let Some(service) = self.selected_service() else {
            return vec![Line::from("No services started.")];
        };

        if service.logs.is_empty() {
            return vec![Line::from("No logs captured yet.")];
        }

        let lines = service
            .logs
            .iter()
            .cloned()
            .map(Line::from)
            .collect::<Vec<_>>();
        self.window_lines(lines, visible_limit, scroll_from_bottom)
    }

    fn render_event_lines(&self, visible_limit: usize, scroll_from_bottom: usize) -> Vec<Line<'_>> {
        if self.events.is_empty() {
            return vec![Line::from("No orchestrator events captured yet.")];
        }

        let lines = self
            .events
            .iter()
            .map(|event| {
                let color = if event.event_type.starts_with("watch_") {
                    Color::Magenta
                } else if event.event_type.contains("failed") {
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
                        format!("[{}] {}", event.timestamp_unix_secs, event.event_type),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!("  svc={}  ", service)),
                    Span::raw(format!("hook={}  action={}  ", hook, action)),
                    Span::raw(event.detail.clone()),
                ])
            })
            .collect::<Vec<_>>();
        self.window_lines(lines, visible_limit, scroll_from_bottom)
    }

    fn window_lines<'a>(
        &self,
        lines: Vec<Line<'a>>,
        visible_limit: usize,
        scroll_from_bottom: usize,
    ) -> Vec<Line<'a>> {
        if visible_limit == 0 {
            return Vec::new();
        }

        if lines.len() <= visible_limit {
            return lines;
        }

        let max_offset = lines.len().saturating_sub(visible_limit);
        let offset = scroll_from_bottom.min(max_offset);
        let end = lines.len().saturating_sub(offset);
        let start = end.saturating_sub(visible_limit);
        lines[start..end].to_vec()
    }

    fn service_window_bounds(&self, visible_count: usize) -> (usize, usize) {
        if self.services.is_empty() {
            return (0, 0);
        }

        if self.services.len() <= visible_count {
            return (0, self.services.len());
        }

        let max_start = self.services.len().saturating_sub(visible_count);
        let centered_start = self.selected.saturating_sub(visible_count / 2);
        let start = centered_start.min(max_start);
        let end = (start + visible_count).min(self.services.len());
        (start, end)
    }
}

impl ServiceStatus {
    fn label(&self) -> &str {
        match self {
            Self::Running => "RUNNING",
            Self::Stopped(_) => "STOPPED",
            Self::Failed(_) => "FAILED",
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::Running => "process is alive",
            Self::Stopped(detail) | Self::Failed(detail) => detail.as_str(),
        }
    }

    fn color(&self) -> Color {
        match self {
            Self::Running => Color::Green,
            Self::Stopped(_) => Color::Yellow,
            Self::Failed(_) => Color::Red,
        }
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
