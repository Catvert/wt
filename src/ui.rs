//! Interactive interface (ratatui): worktree list, preview, keyboard actions.
//!
//! No business logic here — every key calls the same function as the matching
//! subcommand.

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Write};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
// Crossterm comes from ratatui's re-export: depending on the crate directly risks a
// version mismatch where event types would no longer be the same types.
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::config::{Ask, Prompt, PromptKind};
use crate::ops::{self, App};
use crate::util::Msg;

/// Lines kept: a test suite or a chatty `docker compose up` must not grow memory
/// without bound.
const MAX_LINES: usize = 5000;

/// Output of a running (or finished) action, shown in a panel.
struct Output {
    title: String,
    lines: Vec<Msg>,
    rx: Receiver<Msg>,
    running: bool,
    failed: bool,
    /// Manual offset from the bottom; 0 sticks to the last line.
    scroll: usize,
    /// Question asked in the panel when the action succeeds ("start now?").
    follow: Option<(String, ConfirmAction)>,
}

impl Output {
    /// Collects what the thread produced since the last frame.
    /// Returns true when the action has just finished.
    fn drain(&mut self) -> bool {
        let mut finished = false;
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Done(err) => {
                    self.running = false;
                    finished = true;
                    match err {
                        Some(e) => {
                            self.failed = true;
                            self.lines
                                .push(Msg::Warn(t!("ui.failed", error = e).to_string()));
                        }
                        None => self.lines.push(Msg::Ok(t!("ui.done").to_string())),
                    }
                }
                other => self.lines.push(other),
            }
            if self.lines.len() > MAX_LINES {
                self.lines.drain(..self.lines.len() - MAX_LINES);
            }
        }
        finished
    }
}

struct Row {
    slug: String,
    branch: String,
    /// `None` when the project declares no `[status] up`: there is no state to show,
    /// and a "stopped" dot would be a lie.
    up: Option<bool>,
    detail: String,
}

/// One item of a picker: the key is the returned value, the label is displayed.
struct Choice {
    key: String,
    label: String,
    detail: String,
    disabled: bool,
    checked: bool,
}

/// One row of a picker's filtered view.
struct Hit {
    /// Index of the choice in `Picker::items`.
    idx: usize,
    score: i32,
    /// Character positions the filter matched, in the label and in the detail — what
    /// the interface highlights.
    label: Vec<usize>,
    detail: Vec<usize>,
}

/// A picker: every choice, plus the fzf-style filter narrowing it down.
///
/// A list of tenants or of branches is long enough that scrolling to the right row is
/// the slow part; typing is what makes it quick.
struct Picker {
    title: String,
    items: Vec<Choice>,
    /// What the filter leaves, in display order. Rebuilt on every keystroke.
    view: Vec<Hit>,
    /// Row of `view` under the cursor.
    sel: usize,
    kind: PickKind,
    /// Multiple selection: TAB toggles, ENTER confirms the set.
    multi: bool,
    filter: String,
}

impl Picker {
    fn new(title: String, items: Vec<Choice>, kind: PickKind, multi: bool) -> Self {
        let mut p = Picker {
            title,
            items,
            view: Vec::new(),
            sel: 0,
            kind,
            multi,
            filter: String::new(),
        };
        p.refilter();
        p
    }

    /// Rebuilds the view from the filter.
    ///
    /// The query runs against what is displayed — label *and* detail joined — so
    /// `acme prod` finds the tenant whose environment only shows in the second column.
    fn refilter(&mut self) {
        if self.filter.trim().is_empty() {
            self.view = (0..self.items.len())
                .map(|idx| Hit {
                    idx,
                    score: 0,
                    label: Vec::new(),
                    detail: Vec::new(),
                })
                .collect();
        } else {
            self.view = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(idx, c)| {
                    let cut = c.label.chars().count();
                    let hay = format!("{} {}", c.label, c.detail);
                    let m = crate::fuzzy::matches(&self.filter, &hay)?;
                    // Split the positions back over the two columns; the separator we
                    // joined with sits at `cut` and belongs to neither.
                    Some(Hit {
                        idx,
                        score: m.score,
                        label: m.positions.iter().copied().filter(|p| *p < cut).collect(),
                        detail: m
                            .positions
                            .iter()
                            .filter(|p| **p > cut)
                            .map(|p| p - cut - 1)
                            .collect(),
                    })
                })
                .collect();
            // Best score first; on a tie the earliest match, then the declared order —
            // `acme-prod` before `prod-acme` for the query `acme`.
            self.view.sort_by_key(|h| {
                let first = h.label.first().or(h.detail.first()).copied().unwrap_or(0);
                (-h.score, first, h.idx)
            });
        }

        // Back to the top: after a keystroke the best match is the one the user is
        // after, and ENTER must never confirm a row that scrolled out of the query.
        self.sel = 0;
    }

    fn current_index(&self) -> Option<usize> {
        self.view.get(self.sel).map(|h| h.idx)
    }

    /// Puts the cursor on a given choice (a question's `default`).
    fn select(&mut self, idx: usize) {
        if let Some(row) = self.view.iter().position(|h| h.idx == idx) {
            self.sel = row;
        }
    }

    fn move_by(&mut self, delta: isize) {
        if self.view.is_empty() {
            return;
        }
        let last = self.view.len() - 1;
        self.sel = (self.sel as isize + delta).clamp(0, last as isize) as usize;
    }

    fn toggle(&mut self) {
        if !self.multi {
            return;
        }
        if let Some(idx) = self.current_index() {
            self.items[idx].checked = !self.items[idx].checked;
        }
    }

    fn edit_filter(&mut self, f: impl FnOnce(&mut String)) {
        f(&mut self.filter);
        self.refilter();
    }
}

/// Action deferred while the wt.toml questions are asked.
struct Pending {
    action: PendingAction,
    phase: Ask,
    slug: String,
    opts: BTreeMap<String, String>,
    queue: VecDeque<Prompt>,
}

enum PendingAction {
    New {
        branch: Option<String>,
        from: Option<String>,
    },
    Up,
}

enum Mode {
    List,
    Help,
    /// Generic picker (branch, task, editor, action, prompt answer).
    Pick(Picker),
    Input {
        title: String,
        buffer: String,
        kind: InputKind,
    },
    Confirm {
        question: String,
        action: ConfirmAction,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModeKind {
    List,
    Help,
    Pick,
    Input,
    Confirm,
}

impl Mode {
    fn kind(&self) -> ModeKind {
        match self {
            Mode::List => ModeKind::List,
            Mode::Help => ModeKind::Help,
            Mode::Pick(_) => ModeKind::Pick,
            Mode::Input { .. } => ModeKind::Input,
            Mode::Confirm { .. } => ModeKind::Confirm,
        }
    }
}

/// What a "yes" triggers.
enum ConfirmAction {
    Remove(String),
    /// Follow-up offered after a creation: start right away.
    StartAfterNew(String),
    /// Follow-up offered after opening a GUI editor.
    TerminalIn(String),
}

enum PickKind {
    Branch,
    /// Where a branch about to be created starts from.
    BranchBase,
    Task,
    Editor,
    Action,
    /// Address to open in the browser.
    OpenLink,
    /// Answer to a wt.toml question; the value feeds `opt.<name>`.
    Prompt {
        name: String,
        separator: String,
    },
}

enum InputKind {
    /// Slug of a new worktree; the branch — or, for a new one, what it starts from —
    /// has already been picked.
    Slug {
        branch: Option<String>,
        from: Option<String>,
    },
    /// `key=value` options for a start.
    Opts { slug: String },
    /// Free-form answer to a wt.toml question.
    Prompt { name: String },
}

struct Ui {
    app: Arc<App>,
    rows: Vec<Row>,
    sel: usize,
    mode: Mode,
    preview: Vec<String>,
    message: Option<String>,
    /// Action waiting for the answers to the project's questions.
    pending: Option<Pending>,
    /// Output of the running action, shown in a panel.
    output: Option<Output>,
    /// Areas of the last frame, to know what a click targets.
    zones: Zones,
    /// Scroll states kept between frames: without them we could not tell which row sits
    /// under the cursor in a scrolled list.
    list_state: ListState,
    pick_state: ListState,
    /// Mouse capture on? Turning it off gives text selection back to the terminal.
    mouse: bool,
    last_click: Option<(u16, u16, Instant)>,
    quit: bool,
}

#[derive(Default)]
struct Zones {
    list: Rect,
    popup: Rect,
    output: Rect,
}

pub fn run(app: Arc<App>) -> Result<()> {
    let mut terminal = ratatui::init();
    let mut ui = Ui {
        app,
        rows: Vec::new(),
        sel: 0,
        mode: Mode::List,
        preview: Vec::new(),
        message: None,
        pending: None,
        output: None,
        zones: Zones::default(),
        list_state: ListState::default(),
        pick_state: ListState::default(),
        mouse: true,
        last_click: None,
        quit: false,
    };
    ui.set_mouse(true);
    ui.refresh();
    let result = ui.event_loop(&mut terminal);
    ui.set_mouse(false);
    ratatui::restore();
    result
}

impl Ui {
    fn event_loop(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            terminal.draw(|f| self.draw(f))?;

            // While an action runs we poll the keyboard instead of blocking on it:
            // output must keep scrolling even when nobody types.
            let busy = self.output.as_ref().is_some_and(|o| o.running);
            if busy && !event::poll(Duration::from_millis(80))? {
                self.pump_output();
                continue;
            }

            let ev = event::read()?;
            self.pump_output();
            match ev {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    self.on_key(key, terminal)?;
                }
                Event::Mouse(m) => self.on_mouse(m, terminal)?,
                _ => {}
            }
        }
        Ok(())
    }

    /// Turns mouse capture on or off.
    ///
    /// Capturing the mouse takes away the terminal's native text selection, hence the
    /// toggle (`m` key) to copy a line out of the output panel.
    fn set_mouse(&mut self, on: bool) {
        let mut out = io::stdout();
        let _ = if on {
            execute!(out, EnableMouseCapture)
        } else {
            execute!(out, DisableMouseCapture)
        };
        self.mouse = on;
    }

    /// Index of the clicked row in a bordered list, accounting for scrolling.
    fn row_at(area: Rect, state: &ListState, y: u16) -> Option<usize> {
        let first = area.y + 1; // top border
        let last = area.y + area.height.saturating_sub(2);
        if y < first || y > last {
            return None;
        }
        Some(state.offset() + (y - first) as usize)
    }

    fn in_zone(zone: Rect, x: u16, y: u16) -> bool {
        zone.width > 0 && zone.contains(ratatui::layout::Position::new(x, y))
    }

    /// A click at the same spot within 400 ms of the previous one counts as confirm.
    fn is_double_click(&mut self, x: u16, y: u16) -> bool {
        let now = Instant::now();
        let double = matches!(
            self.last_click,
            Some((px, py, t)) if px == x && py == y && now.duration_since(t) < Duration::from_millis(400)
        );
        self.last_click = Some((x, y, now));
        double
    }

    fn on_mouse(&mut self, m: MouseEvent, term: &mut DefaultTerminal) -> Result<()> {
        let (x, y) = (m.column, m.row);

        // The output panel is modal: while it is up the wheel scrolls it and nothing
        // else reacts.
        if let Some(out) = &mut self.output {
            match m.kind {
                MouseEventKind::ScrollUp => out.scroll = (out.scroll + 3).min(out.lines.len()),
                MouseEventKind::ScrollDown => out.scroll = out.scroll.saturating_sub(3),
                // Double-click in the panel: closes it, like ENTER.
                MouseEventKind::Down(MouseButton::Left)
                    if !out.running && Self::in_zone(self.zones.output, x, y) =>
                {
                    self.close_output_if_double_click(x, y)
                }
                _ => {}
            }
            return Ok(());
        }

        if matches!(self.mode.kind(), ModeKind::Pick) {
            return self.on_mouse_pick(m, term);
        }
        if self.mode.kind() != ModeKind::List {
            return Ok(());
        }

        match m.kind {
            MouseEventKind::ScrollDown => {
                if self.sel + 1 < self.rows.len() {
                    self.sel += 1;
                    self.refresh_preview();
                }
            }
            MouseEventKind::ScrollUp => {
                if self.sel > 0 {
                    self.sel -= 1;
                    self.refresh_preview();
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if !Self::in_zone(self.zones.list, x, y) {
                    return Ok(());
                }
                let Some(idx) = Self::row_at(self.zones.list, &self.list_state, y) else {
                    return Ok(());
                };
                if idx >= self.rows.len() {
                    return Ok(());
                }
                let double = self.is_double_click(x, y);
                if idx != self.sel {
                    self.sel = idx;
                    self.refresh_preview();
                    return Ok(());
                }
                // Second click on the already selected row: open the actions, like
                // ENTER on the keyboard.
                if double {
                    self.open_action_menu();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn close_output_if_double_click(&mut self, x: u16, y: u16) {
        if self.is_double_click(x, y) {
            self.output = None;
            self.refresh();
        }
    }

    fn on_mouse_pick(&mut self, m: MouseEvent, term: &mut DefaultTerminal) -> Result<()> {
        let (x, y) = (m.column, m.row);
        let Mode::Pick(p) = &mut self.mode else {
            return Ok(());
        };
        match m.kind {
            MouseEventKind::ScrollDown => p.move_by(1),
            MouseEventKind::ScrollUp => p.move_by(-1),
            MouseEventKind::Down(MouseButton::Left) => {
                if !Self::in_zone(self.zones.popup, x, y) {
                    return Ok(());
                }
                let Some(row) = Self::row_at(self.zones.popup, &self.pick_state, y) else {
                    return Ok(());
                };
                if row >= p.view.len() {
                    return Ok(());
                }
                p.sel = row;
                // In multiple selection a click toggles: that is the expected gesture,
                // and nothing is confirmed until you are done.
                if p.multi {
                    p.toggle();
                    return Ok(());
                }
                if self.is_double_click(x, y) {
                    self.submit_pick(term)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Absorbs the lines produced by the running action.
    fn pump_output(&mut self) {
        let Some(out) = &mut self.output else {
            return;
        };
        if out.drain() {
            // The action changed the worktree's state: list and preview follow.
            self.refresh();
        }
    }

    /// Recomputes the list and the preview. The wt.toml probes run shell commands: that
    /// happens here, never while rendering a frame.
    fn refresh(&mut self) {
        let worktrees = self.app.list();
        self.rows = worktrees
            .iter()
            .map(|w| Row {
                slug: w.slug.clone(),
                branch: crate::git::current_branch(&w.path),
                up: self.app.is_up(w),
                detail: w
                    .state
                    .ports
                    .iter()
                    .map(|(k, v)| format!("{k}:{v}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            })
            .collect();
        if self.sel >= self.rows.len() {
            self.sel = self.rows.len().saturating_sub(1);
        }
        self.preview = match worktrees.get(self.sel) {
            Some(w) => self.app.preview(w),
            None => vec![t!("ui.empty_hint").to_string()],
        };
    }

    fn refresh_preview(&mut self) {
        let worktrees = self.app.list();
        self.preview = match worktrees.get(self.sel) {
            Some(w) => self.app.preview(w),
            None => Vec::new(),
        };
    }

    fn current(&self) -> Option<&Row> {
        self.rows.get(self.sel)
    }

    /// Shortcuts actually useful for THIS project: a repository without services offers
    /// neither start, nor stop, nor browser.
    fn shortcuts(&self) -> Vec<String> {
        let mut s = vec![
            t!("key.navigate").to_string(),
            t!("key.actions").to_string(),
            t!("key.new").to_string(),
        ];
        if self.app.has_up() {
            s.push(t!("key.start").to_string());
        }
        if self.app.has_down() {
            s.push(t!("key.stop").to_string());
        }
        s.push(t!("key.shell").to_string());
        s.push(t!("key.editor").to_string());
        if self.app.has_open() {
            s.push(t!("key.browser").to_string());
        }
        if self.app.has_tasks() {
            s.push(t!("key.task").to_string());
        }
        s.push(t!("key.remove").to_string());
        s.push(t!("key.help").to_string());
        s.push(t!("key.quit").to_string());
        s
    }

    /// Fits the bar to the available width.
    ///
    /// As long as everything fits, the original order is kept. Otherwise we cut from the
    /// end, but `? help` always stays visible — it is what leads to the full list.
    fn shortcut_line(&self, width: u16) -> String {
        const SEP: &str = " · ";
        let help = t!("key.help").to_string();

        let items = self.shortcuts();
        let width = width.saturating_sub(2) as usize; // left/right margins
        let full = items.join(SEP);
        if full.chars().count() <= width {
            return full;
        }

        let reserve = SEP.chars().count() + help.chars().count() + 1; // "… · ? help"
        let mut line = String::new();
        for item in items.iter().filter(|i| *i != &help) {
            let addition = if line.is_empty() {
                item.to_string()
            } else {
                format!("{SEP}{item}")
            };
            if line.chars().count() + addition.chars().count() + reserve > width {
                break;
            }
            line.push_str(&addition);
        }
        if line.is_empty() {
            help
        } else {
            format!("{line}…{SEP}{help}")
        }
    }

    // --------------------------------------------------------------------------------
    // Rendu
    // --------------------------------------------------------------------------------

    fn draw(&mut self, f: &mut Frame) {
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(f.area());

        let title = format!(
            " wt · {} · {} worktree(s) ",
            self.app.project.name(),
            self.rows.len()
        );
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                title,
                Style::default().fg(Color::Black).bg(Color::Cyan),
            ))),
            chunks[0],
        );

        let body = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(chunks[1]);
        self.zones.list = body[0];
        self.draw_list(f, body[0]);
        self.draw_preview(f, body[1]);

        let footer = match &self.message {
            Some(m) => Line::from(Span::styled(
                format!(" {m} "),
                Style::default().fg(Color::Yellow),
            )),
            None => Line::from(Span::styled(
                format!(" {} ", self.shortcut_line(f.area().width)),
                Style::default().fg(Color::DarkGray),
            )),
        };
        f.render_widget(Paragraph::new(footer), chunks[2]);

        if self.output.is_some() {
            self.draw_output(f);
            return;
        }
        self.zones.popup = Rect::default();

        match &self.mode {
            Mode::Help => self.draw_help(f),
            Mode::Pick(_) => self.draw_pick(f),
            Mode::Input { title, buffer, .. } => {
                let (title, buffer) = (title.clone(), buffer.clone());
                self.draw_input(f, &title, &buffer);
            }
            Mode::Confirm { question, .. } => {
                let q = question.clone();
                self.draw_confirm(f, &q);
            }
            Mode::List => {}
        }
    }

    fn draw_list(&mut self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .rows
            .iter()
            .map(|r| {
                let (mark, color) = match r.up {
                    Some(true) => ("●", Color::Green),
                    Some(false) => ("○", Color::DarkGray),
                    None => ("·", Color::DarkGray),
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{mark} "), Style::default().fg(color)),
                    Span::styled(
                        format!("{:<20}", r.slug),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{:<24}", truncate(&r.branch, 24)),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(r.detail.clone(), Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect();

        self.list_state
            .select((!self.rows.is_empty()).then_some(self.sel));
        f.render_stateful_widget(
            List::new(items)
                .block(Block::default().borders(Borders::ALL).title(" worktrees "))
                .highlight_style(Style::default().bg(Color::DarkGray))
                .highlight_symbol("▸"),
            area,
            &mut self.list_state,
        );
    }

    fn draw_preview(&self, f: &mut Frame, area: Rect) {
        let lines: Vec<Line> = self.preview.iter().map(|l| Line::from(l.clone())).collect();
        f.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" {} ", t!("ui.preview"))),
                )
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    fn draw_output(&mut self, f: &mut Frame) {
        let Some(out) = &self.output else {
            return;
        };
        let area = centered(f.area(), 92, 86);
        self.zones.output = area;
        f.render_widget(Clear, area);

        let (mark, color) = if out.running {
            ("…", Color::Cyan)
        } else if out.failed {
            ("✗", Color::Red)
        } else {
            ("✓", Color::Green)
        };

        let inner_height = area.height.saturating_sub(2) as usize;
        // Auto-follow: show the tail unless the user scrolled up (scroll > 0).
        let end = out.lines.len().saturating_sub(out.scroll);
        let start = end.saturating_sub(inner_height);
        let lines: Vec<Line> = out.lines[start..end]
            .iter()
            .map(|m| match m {
                Msg::Info(t) => Line::from(vec![
                    Span::styled("→ ", Style::default().fg(Color::Cyan)),
                    Span::raw(t.clone()),
                ]),
                Msg::Ok(t) => Line::from(vec![
                    Span::styled("✓ ", Style::default().fg(Color::Green)),
                    Span::styled(t.clone(), Style::default().fg(Color::Green)),
                ]),
                Msg::Warn(t) => Line::from(vec![
                    Span::styled("! ", Style::default().fg(Color::Yellow)),
                    Span::styled(t.clone(), Style::default().fg(Color::Yellow)),
                ]),
                // Raw command output is coloured: interpret its ANSI sequences instead
                // of printing them verbatim.
                Msg::Out(t) => {
                    let base = Style::default().fg(Color::Gray);
                    let mut spans = vec![Span::styled("  ", base)];
                    spans.extend(crate::ansi::to_spans(t, base));
                    Line::from(spans)
                }
                Msg::Done(_) => Line::default(),
            })
            .collect();

        let (hint, hint_style) = match (&out.follow, out.running) {
            (_, true) => (
                format!(" {} ", t!("ui.running_hint")),
                Style::default().fg(Color::DarkGray),
            ),
            (Some((question, _)), false) if !out.failed => (
                format!(" {question} [o/n] "),
                Style::default().fg(Color::Yellow),
            ),
            _ => (
                format!(" {} ", t!("ui.close_hint")),
                Style::default().fg(Color::DarkGray),
            ),
        };
        f.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(color))
                    .title(format!(" {mark} {} ", out.title))
                    .title_bottom(Span::styled(hint, hint_style)),
            ),
            area,
        );
    }

    fn draw_help(&self, f: &mut Frame) {
        let mut lines = vec![t!("help.new").to_string()];
        if self.app.has_up() {
            lines.push(t!("help.start").to_string());
        }
        if self.app.has_down() {
            lines.push(t!("help.stop").to_string());
        }
        lines.push(t!("help.shell").to_string());
        lines.push(t!("help.editor").to_string());
        if self.app.has_open() {
            lines.push(t!("help.browser").to_string());
        }
        if self.app.has_tasks() {
            lines.push(t!("help.task").to_string());
        }
        lines.extend([
            t!("help.remove").to_string(),
            t!("help.mouse_toggle").to_string(),
            String::new(),
            t!("help.mouse").to_string(),
            t!("help.actions").to_string(),
            String::new(),
            t!("help.picker").to_string(),
            String::new(),
            t!("help.panel").to_string(),
            t!("help.interactive").to_string(),
            String::new(),
            format!(
                "{}: {}",
                t!("label.config"),
                self.app.project.config_path.display()
            ),
            format!("{}: {}", t!("label.root"), self.app.root.display()),
        ]);
        if self.app.has_tasks() {
            lines.push(String::new());
            lines.push(format!("{}:", t!("ui.tasks")));
            for (name, t) in &self.app.project.config.tasks {
                lines.push(format!("  {name:<14} {}", t.description));
            }
        }
        let area = centered(f.area(), 70, 60);
        f.render_widget(Clear, area);
        f.render_widget(
            Paragraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>())
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" {} ", t!("ui.help"))),
                )
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    fn draw_pick(&mut self, f: &mut Frame) {
        let Mode::Pick(p) = &self.mode else {
            return;
        };
        let (title, multi, filter) = (p.title.clone(), p.multi, p.filter.clone());
        let (shown, total) = (p.view.len(), p.items.len());
        let sel = (!p.view.is_empty()).then_some(p.sel);

        let list: Vec<ListItem> = p
            .view
            .iter()
            .map(|hit| {
                let c = &p.items[hit.idx];
                let style = if c.disabled {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default()
                };
                let mut spans = Vec::new();
                if multi {
                    spans.push(Span::styled(
                        if c.checked { "[×] " } else { "[ ] " },
                        Style::default().fg(if c.checked {
                            Color::Green
                        } else {
                            Color::DarkGray
                        }),
                    ));
                }
                spans.extend(cell(&c.label, &hit.label, style, 34, true));
                spans.extend(cell(
                    &c.detail,
                    &hit.detail,
                    Style::default().fg(Color::DarkGray),
                    40,
                    false,
                ));
                ListItem::new(Line::from(spans))
            })
            .collect();

        // A filter that matches nothing: say so, rather than leave an empty frame that
        // reads like a bug.
        let list = if list.is_empty() && !filter.is_empty() {
            vec![ListItem::new(Line::from(Span::styled(
                format!("  {}", t!("ui.no_match")),
                Style::default().fg(Color::DarkGray),
            )))]
        } else {
            list
        };

        let area = centered(f.area(), 80, 70);
        self.zones.popup = area;
        self.pick_state.select(sel);
        f.render_widget(Clear, area);
        let hint = if multi {
            format!(" — {} ", t!("ui.multi_hint"))
        } else {
            " ".to_string()
        };
        // The query sits at the bottom, fzf-style: the list stays where the eye is.
        let prompt = Line::from(vec![
            Span::styled(" › ", Style::default().fg(Color::Cyan)),
            Span::styled(
                filter.clone(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("▏", Style::default().fg(Color::DarkGray)),
        ]);
        let count = Line::from(Span::styled(
            if filter.is_empty() {
                format!(" {total} ")
            } else {
                format!(" {shown}/{total} ")
            },
            Style::default().fg(Color::DarkGray),
        ))
        .right_aligned();
        f.render_stateful_widget(
            List::new(list)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" {title}{hint}"))
                        .title_bottom(prompt)
                        .title_bottom(count),
                )
                .highlight_style(Style::default().bg(Color::DarkGray))
                .highlight_symbol("▸"),
            area,
            &mut self.pick_state,
        );
    }

    fn draw_input(&self, f: &mut Frame, title: &str, buffer: &str) {
        let area = centered(f.area(), 60, 20);
        f.render_widget(Clear, area);
        f.render_widget(
            Paragraph::new(vec![
                Line::from(format!("> {buffer}▏")),
                Line::from(Span::styled(
                    t!("ui.input_hint").to_string(),
                    Style::default().fg(Color::DarkGray),
                )),
            ])
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {title} ")),
            ),
            area,
        );
    }

    fn draw_confirm(&self, f: &mut Frame, question: &str) {
        let area = centered(f.area(), 60, 20);
        f.render_widget(Clear, area);
        f.render_widget(
            Paragraph::new(vec![
                Line::from(question.to_string()),
                Line::from(Span::styled(
                    t!("ui.confirm_hint").to_string(),
                    Style::default().fg(Color::DarkGray),
                )),
            ])
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} ", t!("ui.confirmation"))),
            )
            .wrap(Wrap { trim: false }),
            area,
        );
    }

    // --------------------------------------------------------------------------------
    // Clavier
    // --------------------------------------------------------------------------------

    fn on_key(&mut self, key: KeyEvent, term: &mut DefaultTerminal) -> Result<()> {
        self.message = None;
        if self.output.is_some() {
            return self.on_key_output(key, term);
        }
        // Dispatch on a copyable discriminant: the arms need mutable access to `self`,
        // which a direct match on `self.mode` would hold.
        match self.mode.kind() {
            ModeKind::List => self.on_key_list(key, term),
            ModeKind::Help => {
                self.mode = Mode::List;
                Ok(())
            }
            ModeKind::Confirm => {
                let confirmed = matches!(key.code, KeyCode::Char('y') | KeyCode::Char('o'));
                let Mode::Confirm { action, .. } = std::mem::replace(&mut self.mode, Mode::List)
                else {
                    return Ok(());
                };
                if confirmed {
                    self.run_confirmed(action, term)?;
                }
                Ok(())
            }
            ModeKind::Input => {
                match key.code {
                    KeyCode::Esc => {
                        if matches!(
                            &self.mode,
                            Mode::Input {
                                kind: InputKind::Prompt { .. },
                                ..
                            }
                        ) {
                            self.pending = None;
                            self.message = Some(t!("ui.cancelled").to_string());
                        }
                        self.mode = Mode::List;
                    }
                    KeyCode::Backspace => {
                        if let Mode::Input { buffer, .. } = &mut self.mode {
                            buffer.pop();
                        }
                    }
                    KeyCode::Char(c) => {
                        if let Mode::Input { buffer, .. } = &mut self.mode {
                            buffer.push(c);
                        }
                    }
                    KeyCode::Enter => return self.submit_input(term),
                    _ => {}
                }
                Ok(())
            }
            ModeKind::Pick => self.on_key_pick(key, term),
        }
    }

    /// Keyboard in a picker.
    ///
    /// Every printable key feeds the filter — a picker is a search box first, the way
    /// fzf is. Navigation therefore lives on the arrows and on the readline-style
    /// controls (`^N`/`^P`, `^J`/`^K`), and TAB is what ticks a box in a multiple
    /// selection.
    fn on_key_pick(&mut self, key: KeyEvent, term: &mut DefaultTerminal) -> Result<()> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let Mode::Pick(p) = &mut self.mode else {
            return Ok(());
        };
        match key.code {
            KeyCode::Enter => return self.submit_pick(term),
            KeyCode::Tab | KeyCode::BackTab => p.toggle(),
            KeyCode::Down => p.move_by(1),
            KeyCode::Up => p.move_by(-1),
            KeyCode::PageDown => p.move_by(10),
            KeyCode::PageUp => p.move_by(-10),
            KeyCode::Char('n' | 'j') if ctrl => p.move_by(1),
            KeyCode::Char('p' | 'k') if ctrl => p.move_by(-1),
            KeyCode::Char('u') if ctrl => p.edit_filter(|f| f.clear()),
            KeyCode::Char('w') if ctrl => p.edit_filter(drop_last_word),
            KeyCode::Backspace => p.edit_filter(|f| {
                f.pop();
            }),
            // ESC clears the query first: a search that went too far is corrected
            // without losing the menu.
            KeyCode::Esc if !p.filter.is_empty() => p.edit_filter(|f| f.clear()),
            KeyCode::Esc => self.cancel_pick(),
            KeyCode::Char('c') if ctrl => self.cancel_pick(),
            KeyCode::Char(c) if !ctrl && !alt => p.edit_filter(|f| f.push(c)),
            _ => {}
        }
        Ok(())
    }

    /// Closes a picker without choosing.
    fn cancel_pick(&mut self) {
        // Giving up on a question means giving up on the action: running it with
        // half-collected answers would be worse than nothing.
        if matches!(
            &self.mode,
            Mode::Pick(Picker {
                kind: PickKind::Prompt { .. },
                ..
            })
        ) {
            self.pending = None;
            self.message = Some(t!("ui.cancelled").to_string());
        }
        self.mode = Mode::List;
    }

    /// Keyboard while the output panel is up: scrolling, and closing once the action is
    /// over. Closing earlier is refused: the panel is the only trace of what is going
    /// on.
    fn on_key_output(&mut self, key: KeyEvent, term: &mut DefaultTerminal) -> Result<()> {
        let Some(out) = &mut self.output else {
            return Ok(());
        };
        let page = 10;
        let max = out.lines.len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => out.scroll = (out.scroll + 1).min(max),
            KeyCode::Down | KeyCode::Char('j') => out.scroll = out.scroll.saturating_sub(1),
            KeyCode::PageUp => out.scroll = (out.scroll + page).min(max),
            KeyCode::PageDown => out.scroll = out.scroll.saturating_sub(page),
            KeyCode::Home => out.scroll = max,
            KeyCode::End => out.scroll = 0,
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('n') => {
                if out.running {
                    self.message = Some(t!("ui.busy").to_string());
                } else {
                    self.output = None;
                    self.refresh();
                }
            }
            // Answer to the offered follow-up ("start now?").
            KeyCode::Char('o') | KeyCode::Char('y') if !out.running && !out.failed => {
                let follow = out.follow.take();
                self.output = None;
                self.refresh();
                if let Some((_, action)) = follow {
                    self.run_confirmed(action, term)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn on_key_list(&mut self, key: KeyEvent, term: &mut DefaultTerminal) -> Result<()> {
        let slug = self.current().map(|r| r.slug.clone());
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Down | KeyCode::Char('j') => {
                if self.sel + 1 < self.rows.len() {
                    self.sel += 1;
                    self.refresh_preview();
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.sel > 0 {
                    self.sel -= 1;
                    self.refresh_preview();
                }
            }
            KeyCode::Char('g') => self.refresh(),
            KeyCode::Char('m') => {
                let on = !self.mouse;
                self.set_mouse(on);
                self.message = Some(if on {
                    t!("ui.mouse_on").to_string()
                } else {
                    t!("ui.mouse_off").to_string()
                });
            }
            KeyCode::Char('?') | KeyCode::Char('h') => self.mode = Mode::Help,
            KeyCode::Char('n') => self.open_branch_picker(),
            KeyCode::Enter => self.open_action_menu(),
            KeyCode::Char('s') => {
                if !self.app.has_up() {
                    self.message = Some(t!("ui.no_services").to_string());
                } else if let Some(s) = slug {
                    self.start(PendingAction::Up, s, term)?;
                }
            }
            KeyCode::Char('S') => {
                if !self.app.has_up() {
                    self.message = Some(t!("ui.no_services").to_string());
                } else if let Some(s) = slug {
                    self.mode = Mode::Input {
                        title: format!("{}", t!("ui.start_options", slug = s)),
                        buffer: String::new(),
                        kind: InputKind::Opts { slug: s },
                    };
                }
            }
            KeyCode::Char('d') => {
                if !self.app.has_down() {
                    self.message = Some(t!("ui.nothing_to_stop").to_string());
                } else if let Some(s) = slug {
                    let title = t!("title.stopping", slug = s).to_string();
                    self.spawn(title, move |app| app.cmd_down(&s));
                }
            }
            KeyCode::Char('c') => {
                if let Some(s) = slug {
                    // The shell takes the terminal: the interface steps aside and comes
                    // back when the session ends, exactly as an interactive task does.
                    self.exec(term, move |app| app.cmd_shell(&s))?;
                }
            }
            KeyCode::Char('e') => self.open_editor_picker(),
            KeyCode::Char('o') => {
                if !self.app.has_open() {
                    self.message = Some(t!("ui.no_url").to_string());
                } else if let Some(s) = slug {
                    self.open_link_picker(&s);
                }
            }
            KeyCode::Char('t') => self.open_task_picker(),
            KeyCode::Char('r') => {
                if let Some(s) = slug {
                    self.mode = Mode::Confirm {
                        question: format!("{}", t!("confirm.remove_long", slug = s)),
                        action: ConfirmAction::Remove(s),
                    };
                }
            }
            _ => {}
        }
        Ok(())
    }

    // --------------------------------------------------------------------------------
    // Project questions (wt.toml -> [[prompt]])
    // --------------------------------------------------------------------------------

    /// Prepares an action and asks the questions that precede it. With no applicable
    /// `[[prompt]]` the action starts immediately — the dialogue is the project's call.
    fn start(
        &mut self,
        action: PendingAction,
        slug: String,
        term: &mut DefaultTerminal,
    ) -> Result<()> {
        let phase = match action {
            PendingAction::New { .. } => Ask::New,
            PendingAction::Up => Ask::Up,
        };
        // Answers already known (previous start) are not asked again but stay in the
        // options passed along: `wt up` reproduces the previous setup.
        let known = crate::state::load(&self.app.root, &slug).opts;
        let queue = self.app.prompts_for(phase, &known).into();
        self.pending = Some(Pending {
            action,
            phase,
            slug,
            opts: known,
            queue,
        });
        self.advance(term)
    }

    /// Asks the next applicable question, or runs the action when none are left.
    fn advance(&mut self, _term: &mut DefaultTerminal) -> Result<()> {
        loop {
            let Some(pending) = &mut self.pending else {
                return Ok(());
            };
            let Some(prompt) = pending.queue.pop_front() else {
                break;
            };
            let slug = pending.slug.clone();
            let opts = pending.opts.clone();
            let phase = pending.phase;
            if !self.app.prompt_applies(&prompt, &slug, &opts, phase) {
                continue;
            }
            self.open_prompt(&prompt, &slug, &opts, phase);
            return Ok(());
        }

        let Some(pending) = self.pending.take() else {
            return Ok(());
        };
        let sets: Vec<String> = pending
            .opts
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let slug = pending.slug;
        match pending.action {
            PendingAction::New { branch, from } => {
                let title = t!("title.creating", slug = slug).to_string();
                // Creating a worktree is almost always about using it: offer to chain —
                // but only if the project has something to start.
                let follow = self.app.has_up().then(|| {
                    (
                        t!("confirm.start_now", slug = slug).to_string(),
                        ConfirmAction::StartAfterNew(slug.clone()),
                    )
                });
                self.spawn_then(title, follow, move |app| {
                    app.cmd_new(&slug, branch.as_deref(), from.as_deref(), &sets)
                });
            }
            PendingAction::Up => {
                let title = t!("title.starting", slug = slug).to_string();
                self.spawn(title, move |app| app.cmd_up(&slug, &sets));
            }
        }
        Ok(())
    }

    fn open_prompt(
        &mut self,
        prompt: &Prompt,
        slug: &str,
        opts: &BTreeMap<String, String>,
        phase: Ask,
    ) {
        if prompt.kind == PromptKind::Text {
            self.mode = Mode::Input {
                title: prompt.title().to_string(),
                buffer: prompt.default.clone().unwrap_or_default(),
                kind: InputKind::Prompt {
                    name: prompt.name.clone(),
                },
            };
            return;
        }

        let multi = prompt.kind == PromptKind::Multi;
        // A `default` pre-checks (multi) or preselects (single choice): that is what
        // lets the common case be confirmed with a single ENTER.
        let preset: Vec<String> = prompt
            .default
            .iter()
            .flat_map(|d| d.split(&prompt.separator))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let items: Vec<Choice> = self
            .app
            .prompt_choices(prompt, slug, opts, phase)
            .into_iter()
            .map(|o| Choice {
                label: if o.label.is_empty() {
                    o.value.clone()
                } else {
                    o.label
                },
                checked: multi && preset.contains(&o.value),
                key: o.value,
                detail: o.detail,
                disabled: false,
            })
            .collect();

        if items.is_empty() {
            // An empty source (database down, no tenant) must not block the action: we
            // leave the question unanswered and move on.
            self.message = Some(t!("ui.no_choices", question = prompt.title()).to_string());
            self.mode = Mode::List;
            return;
        }

        let sel = items
            .iter()
            .position(|c| preset.contains(&c.key))
            .filter(|_| !multi);

        let mut picker = Picker::new(
            prompt.title().to_string(),
            items,
            PickKind::Prompt {
                name: prompt.name.clone(),
                separator: prompt.separator.clone(),
            },
            multi,
        );
        if let Some(sel) = sel {
            picker.select(sel);
        }
        self.mode = Mode::Pick(picker);
    }

    /// The menu only lists what the wt.toml declared: offering "start" to a project
    /// without services would only lead to an error.
    fn open_action_menu(&mut self) {
        let mut items = vec![choice("new", &t!("action.new"), "")];
        if self.app.has_up() {
            items.push(choice("up", &t!("action.start"), "hooks up"));
            items.push(choice(
                "up-opts",
                &t!("action.start_opts"),
                "--set key=value",
            ));
        }
        if self.app.has_down() {
            items.push(choice("down", &t!("action.stop"), "hooks down"));
        }
        items.push(choice(
            "shell",
            &t!("action.shell"),
            &t!("action.shell_detail"),
        ));
        items.push(choice("ide", &t!("action.editor"), ""));
        if self.app.has_open() {
            items.push(choice("open", &t!("action.browser"), ""));
        }
        if self.app.has_tasks() {
            items.push(choice("task", &t!("action.task"), "wt.toml [tasks]"));
        }
        items.push(choice(
            "rm",
            &t!("action.remove"),
            &t!("action.remove_detail"),
        ));
        self.mode = Mode::Pick(Picker::new(
            t!("ui.actions").to_string(),
            items,
            PickKind::Action,
            false,
        ));
    }

    fn open_branch_picker(&mut self) {
        let mut items = vec![choice(
            "__new__",
            &t!("ui.new_branch"),
            &t!("ui.new_branch_hint"),
        )];
        for b in crate::git::branches(&self.app.project.main) {
            let used = b.used_by.is_some();
            items.push(Choice {
                key: b.name.clone(),
                label: if used {
                    format!("⊘ {}", b.name)
                } else {
                    format!("  {}", b.name)
                },
                detail: if used {
                    t!("ui.already_checked_out").to_string()
                } else {
                    format!("{} · {}", b.date, b.subject)
                },
                disabled: used,
                checked: false,
            });
        }
        self.mode = Mode::Pick(Picker::new(
            t!("ui.branch_title").to_string(),
            items,
            PickKind::Branch,
            false,
        ));
    }

    /// Where the branch about to be created starts.
    ///
    /// A worktree is rarely branched off whatever the main repository happens to have
    /// checked out: the question is `dev`, `master`, or the feature branch this one
    /// extends. A branch already checked out elsewhere is a perfectly good start point,
    /// so nothing is greyed out here.
    fn open_base_picker(&mut self) {
        let head = crate::git::current_branch(&self.app.project.main);
        let items: Vec<Choice> = crate::git::branches(&self.app.project.main)
            .into_iter()
            .map(|b| {
                let current = b.name == head;
                Choice {
                    label: if current {
                        format!("● {}", b.name)
                    } else {
                        format!("  {}", b.name)
                    },
                    // The marker stays short: the date and the subject are what tell
                    // two candidate bases apart.
                    detail: if current {
                        format!("{} · {} · {}", t!("ui.head_here"), b.date, b.subject)
                    } else {
                        format!("{} · {}", b.date, b.subject)
                    },
                    key: b.name,
                    disabled: false,
                    checked: false,
                }
            })
            .collect();

        // A repository without a single commit has no branch to offer: HEAD is the only
        // possible start point, and asking would be a dead end.
        if items.is_empty() {
            self.ask_slug(None, None);
            return;
        }

        let mut picker = Picker::new(
            t!("ui.base_title").to_string(),
            items,
            PickKind::BranchBase,
            false,
        );
        if let Some(idx) = picker.items.iter().position(|c| c.key == head) {
            picker.select(idx);
        }
        self.mode = Mode::Pick(picker);
    }

    /// Last step of a creation: the slug, suggested from the branch when there is one.
    fn ask_slug(&mut self, branch: Option<String>, from: Option<String>) {
        let title = match (&branch, &from) {
            (Some(b), _) => t!("ui.slug_for", branch = b).to_string(),
            (None, Some(f)) => t!("ui.slug_from", base = f).to_string(),
            (None, None) => t!("ui.slug_title").to_string(),
        };
        let buffer = match &branch {
            Some(b) => ops::slugify(b.trim_start_matches("origin/")),
            None => String::new(),
        };
        self.mode = Mode::Input {
            title,
            buffer,
            kind: InputKind::Slug { branch, from },
        };
    }

    fn open_task_picker(&mut self) {
        if self.current().is_none() {
            self.message = Some(t!("ui.no_selection").to_string());
            return;
        }
        let items: Vec<Choice> = self
            .app
            .project
            .config
            .tasks
            .iter()
            .map(|(name, t)| choice(name, name, &t.description))
            .collect();
        if items.is_empty() {
            self.message = Some(t!("ui.no_tasks").to_string());
            return;
        }
        self.mode = Mode::Pick(Picker::new(
            t!("ui.task").to_string(),
            items,
            PickKind::Task,
            false,
        ));
    }

    /// A single address: open it. Several (application plus one tenant per mounted
    /// storage, say): ask which one.
    fn open_link_picker(&mut self, slug: &str) {
        let st = crate::state::load(&self.app.root, slug);
        let links = self.app.links(slug, &st);
        match links.len() {
            0 => self.message = Some(t!("ui.no_url").to_string()),
            1 => {
                let s = slug.to_string();
                let title = t!("title.browser", slug = s).to_string();
                self.spawn(title, move |app| app.cmd_open(&s, None, false));
            }
            _ => {
                let items = links
                    .iter()
                    .map(|l| choice(&l.url, &l.label, &l.url))
                    .collect();
                self.mode = Mode::Pick(Picker::new(
                    t!("ui.open_title", slug = slug).to_string(),
                    items,
                    PickKind::OpenLink,
                    false,
                ));
            }
        }
    }

    fn open_editor_picker(&mut self) {
        if self.current().is_none() {
            return;
        }
        let editors = self.app.editors();
        if editors.is_empty() {
            self.message = Some(t!("ui.no_editor").to_string());
            return;
        }
        let items: Vec<Choice> = editors.iter().map(|e| choice(e, e, "")).collect();
        self.mode = Mode::Pick(Picker::new(
            t!("ui.editor").to_string(),
            items,
            PickKind::Editor,
            false,
        ));
    }

    fn submit_pick(&mut self, term: &mut DefaultTerminal) -> Result<()> {
        let Mode::Pick(p) = &self.mode else {
            return Ok(());
        };
        // Nothing under the cursor — a filter that matches nothing. ENTER has nothing to
        // confirm and must not close the menu, unless boxes are already ticked.
        if p.current_index().is_none() && !(p.multi && matches!(p.kind, PickKind::Prompt { .. })) {
            return Ok(());
        }
        let Mode::Pick(p) = std::mem::replace(&mut self.mode, Mode::List) else {
            return Ok(());
        };
        let picked_index = p.current_index();
        let Picker {
            items, kind, multi, ..
        } = p;

        // Answer to a project question: in multi mode the checked boxes are what count,
        // not the hovered row — including those ticked under an earlier filter.
        if let PickKind::Prompt { name, separator } = &kind {
            let value = if multi {
                items
                    .iter()
                    .filter(|c| c.checked)
                    .map(|c| c.key.clone())
                    .collect::<Vec<_>>()
                    .join(separator)
            } else {
                match picked_index.and_then(|i| items.get(i)) {
                    Some(c) => c.key.clone(),
                    None => String::new(),
                }
            };
            self.answer(name.clone(), value);
            return self.advance(term);
        }

        let Some(picked) = picked_index.and_then(|i| items.into_iter().nth(i)) else {
            return Ok(());
        };
        if picked.disabled {
            self.message = Some(t!("ui.branch_taken").to_string());
            return Ok(());
        }
        let slug = self.current().map(|r| r.slug.clone());
        match kind {
            // A branch that exists is its own start point; a new one still has to be
            // told where it begins.
            PickKind::Branch if picked.key == "__new__" => self.open_base_picker(),
            PickKind::Branch => self.ask_slug(Some(picked.key), None),
            PickKind::BranchBase => self.ask_slug(None, Some(picked.key)),
            PickKind::Task => {
                if let Some(s) = slug {
                    let task = picked.key;
                    // A shell or a `logs -f` wants the terminal: the panel can neither
                    // forward keystrokes nor render a full-screen display.
                    if self.app.task_is_interactive(&task) {
                        self.exec(term, |app| app.cmd_run(&task, &s, &[]))?;
                    } else {
                        let title = format!("{task} · {s}");
                        self.spawn(title, move |app| app.cmd_run(&task, &s, &[]));
                    }
                }
            }
            PickKind::OpenLink => {
                if let Some(s) = slug {
                    let url = picked.key;
                    let title = t!("title.browser", slug = s).to_string();
                    self.spawn(title, move |app| app.cmd_open(&s, Some(&url), false));
                }
            }
            PickKind::Editor => {
                if let Some(s) = slug {
                    let editor = picked.key;
                    // A terminal editor replaces the process: nothing can be chained
                    // after it. For an IDE window, on the other hand, a shell already
                    // sitting in the worktree is what is most often missing.
                    let terminal = !self.app.editor_is_terminal(&editor);
                    {
                        let s = s.clone();
                        self.exec(term, move |app| app.cmd_ide(&s, Some(&editor)))?;
                    }
                    if terminal {
                        self.mode = Mode::Confirm {
                            question: t!("confirm.terminal", slug = s).to_string(),
                            action: ConfirmAction::TerminalIn(s),
                        };
                    }
                }
            }
            PickKind::Action => match picked.key.as_str() {
                "new" => self.open_branch_picker(),
                "task" => self.open_task_picker(),
                "ide" => self.open_editor_picker(),
                "shell" => {
                    if let Some(s) = slug {
                        self.exec(term, move |app| app.cmd_shell(&s))?;
                    }
                }
                "up" => {
                    if let Some(s) = slug {
                        self.start(PendingAction::Up, s, term)?;
                    }
                }
                "up-opts" => {
                    if let Some(s) = slug {
                        self.mode = Mode::Input {
                            title: t!("ui.start_options", slug = s).to_string(),
                            buffer: String::new(),
                            kind: InputKind::Opts { slug: s },
                        };
                    }
                }
                "down" => {
                    if let Some(s) = slug {
                        let title = t!("title.stopping", slug = s).to_string();
                        self.spawn(title, move |app| app.cmd_down(&s));
                    }
                }
                "open" => {
                    if let Some(s) = slug {
                        self.open_link_picker(&s);
                    }
                }
                "rm" => {
                    if let Some(s) = slug {
                        self.mode = Mode::Confirm {
                            question: t!("confirm.remove", slug = s).to_string(),
                            action: ConfirmAction::Remove(s),
                        };
                    }
                }
                _ => {}
            },
            // Handled before the match: the value is already recorded.
            PickKind::Prompt { .. } => {}
        }
        Ok(())
    }

    fn submit_input(&mut self, term: &mut DefaultTerminal) -> Result<()> {
        let Mode::Input { buffer, kind, .. } = std::mem::replace(&mut self.mode, Mode::List) else {
            return Ok(());
        };
        match kind {
            InputKind::Slug { branch, from } => {
                let slug = buffer.trim().to_string();
                if slug.is_empty() {
                    self.message = Some(t!("ui.empty_slug").to_string());
                    return Ok(());
                }
                self.start(PendingAction::New { branch, from }, slug, term)?;
            }
            InputKind::Opts { slug } => {
                let sets: Vec<String> = buffer.split_whitespace().map(|s| s.to_string()).collect();
                let title = t!("title.starting", slug = slug).to_string();
                self.spawn(title, move |app| app.cmd_up(&slug, &sets));
            }
            InputKind::Prompt { name } => {
                self.answer(name, buffer.trim().to_string());
                self.advance(term)?;
            }
        }
        Ok(())
    }

    fn run_confirmed(&mut self, action: ConfirmAction, term: &mut DefaultTerminal) -> Result<()> {
        match action {
            ConfirmAction::Remove(slug) => {
                let title = format!("suppression de {slug}");
                self.spawn(title, move |app| app.cmd_rm(&slug, true));
                Ok(())
            }
            ConfirmAction::StartAfterNew(slug) => self.start(PendingAction::Up, slug, term),
            ConfirmAction::TerminalIn(slug) => {
                // Un shell veut le terminal : l'interface s'efface le temps de la session.
                self.exec(term, move |app| app.cmd_shell(&slug))
            }
        }
    }

    /// Records the answer to a question, for the pending action.
    fn answer(&mut self, name: String, value: String) {
        if let Some(pending) = &mut self.pending {
            pending.opts.insert(name, value);
        }
    }

    /// Runs an action in the background and shows its output in a panel.
    ///
    /// The interface stays up: `docker compose`, `npm install` or `git worktree add`
    /// scroll by without leaving the worktree list.
    fn spawn<F>(&mut self, title: impl Into<String>, f: F)
    where
        F: FnOnce(&App) -> Result<()> + Send + 'static,
    {
        self.spawn_then(title, None, f)
    }

    /// Same, with a question asked in the panel if the action succeeds.
    fn spawn_then<F>(
        &mut self,
        title: impl Into<String>,
        follow: Option<(String, ConfirmAction)>,
        f: F,
    ) where
        F: FnOnce(&App) -> Result<()> + Send + 'static,
    {
        if self.output.as_ref().is_some_and(|o| o.running) {
            self.message = Some(t!("ui.action_running").to_string());
            return;
        }
        let (tx, rx) = mpsc::channel();
        let app = Arc::clone(&self.app);
        let sender = tx.clone();
        std::thread::spawn(move || {
            app.set_sink(Some(sender));
            let result = f(&app);
            app.set_sink(None);
            // `Done` last: it is what unlocks the result display.
            let _ = tx.send(Msg::Done(result.err().map(|e| format!("{e:#}"))));
        });
        self.output = Some(Output {
            title: title.into(),
            lines: Vec::new(),
            rx,
            running: true,
            failed: false,
            scroll: 0,
            follow,
        });
    }

    /// Hands the terminal back to the action while it runs.
    ///
    /// Reserved for what genuinely needs the terminal: a shell, a `logs -f`, an editor.
    /// Everything else shows in the panel, without leaving the interface.
    fn exec<F>(&mut self, term: &mut DefaultTerminal, f: F) -> Result<()>
    where
        F: FnOnce(&App) -> Result<()>,
    {
        // Mouse capture must go back to the terminal: otherwise a shell started by an
        // interactive task would receive movement codes instead of the cursor.
        let mouse = self.mouse;
        if mouse {
            self.set_mouse(false);
        }
        ratatui::restore();
        let app = Arc::clone(&self.app);
        let result = f(&app);
        if let Err(e) = &result {
            eprintln!("\x1b[31merreur:\x1b[0m {e:#}");
        }
        print!("\n— {} —", t!("ui.press_enter"));
        let _ = io::stdout().flush();
        let mut line = String::new();
        let _ = io::stdin().read_line(&mut line);

        *term = ratatui::init();
        if mouse {
            self.set_mouse(true);
        }
        // A failed `clear` (slow terminal, script-driven session) is no reason to close
        // the interface: the next frame repaints the whole screen.
        let _ = term.clear();
        self.refresh();
        if let Err(e) = result {
            self.message = Some(t!("ui.failed", error = e).to_string());
        }
        Ok(())
    }
}

fn choice(key: &str, label: &str, detail: &str) -> Choice {
    Choice {
        key: key.to_string(),
        label: label.to_string(),
        detail: detail.to_string(),
        disabled: false,
        checked: false,
    }
}

/// Drops the last word of a search, `^W`-style.
///
/// The cut lands on a character boundary, not a byte one: a non-breaking space —
/// AltGr+space on an AZERTY keyboard — is two bytes wide, and `truncate` panics in the
/// middle of one.
fn drop_last_word(query: &mut String) {
    let kept = query
        .trim_end()
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    query.truncate(kept);
}

/// Spans for one column of a picker row, with what the filter matched picked out.
///
/// The text is truncated to `width` characters, and padded to it when `pad` is set so
/// the next column lines up. Positions past the cut are simply not highlighted.
fn cell(text: &str, hits: &[usize], base: Style, width: usize, pad: bool) -> Vec<Span<'static>> {
    let matched = base
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let chars: Vec<char> = text.chars().collect();
    let cut = chars.len() > width;
    let keep = if cut {
        width.saturating_sub(1)
    } else {
        width.min(chars.len())
    };

    let mut spans = Vec::new();
    let mut run = String::new();
    let mut on = false;
    for (i, c) in chars.iter().take(keep).enumerate() {
        let hit = hits.contains(&i);
        if hit != on {
            if !run.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut run),
                    if on { matched } else { base },
                ));
            }
            on = hit;
        }
        run.push(*c);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, if on { matched } else { base }));
    }
    if cut {
        spans.push(Span::styled("…", base));
    }
    if pad {
        let shown = if cut { keep + 1 } else { keep };
        spans.push(Span::styled(" ".repeat(width.saturating_sub(shown)), base));
    }
    spans
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn centered(area: Rect, pct_x: u16, pct_y: u16) -> Rect {
    let v = Layout::vertical([
        Constraint::Percentage((100 - pct_y) / 2),
        Constraint::Percentage(pct_y),
        Constraint::Percentage((100 - pct_y) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - pct_x) / 2),
        Constraint::Percentage(pct_x),
        Constraint::Percentage((100 - pct_x) / 2),
    ])
    .split(v[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker(items: &[(&str, &str)], multi: bool) -> Picker {
        let items = items
            .iter()
            .map(|(label, detail)| choice(label, label, detail))
            .collect();
        Picker::new(String::new(), items, PickKind::Action, multi)
    }

    fn labels(p: &Picker) -> Vec<&str> {
        p.view
            .iter()
            .map(|h| p.items[h.idx].label.as_str())
            .collect()
    }

    #[test]
    fn an_empty_filter_keeps_the_declared_order() {
        let p = picker(&[("zeta", ""), ("alpha", "")], false);
        assert_eq!(labels(&p), ["zeta", "alpha"]);
    }

    #[test]
    fn filtering_narrows_and_ranks() {
        let mut p = picker(
            &[("prod-acme", ""), ("staging", ""), ("acme-prod", "")],
            false,
        );
        p.edit_filter(|f| f.push_str("acme"));
        // Both survive; the one whose match starts earliest leads.
        assert_eq!(labels(&p), ["acme-prod", "prod-acme"]);
    }

    #[test]
    fn the_filter_reaches_the_detail_column() {
        let mut p = picker(&[("acme", "production"), ("globex", "staging")], false);
        p.edit_filter(|f| f.push_str("acme prod"));
        assert_eq!(labels(&p), ["acme"]);
        // The detail's positions are relative to the detail, not to the joined line.
        assert_eq!(p.view[0].detail, [0, 1, 2, 3]);
        assert_eq!(p.view[0].label, [0, 1, 2, 3]);
    }

    #[test]
    fn the_cursor_goes_back_to_the_best_match() {
        let mut p = picker(&[("alpha", ""), ("beta", ""), ("gamma", "")], false);
        p.sel = 2;
        // Typing aims at the top of the list: ENTER must not confirm the row the
        // cursor happened to sit on before the query.
        p.edit_filter(|f| f.push('a'));
        assert_eq!(p.current_index(), Some(0));
    }

    #[test]
    fn a_filter_that_matches_nothing_leaves_no_selection() {
        let mut p = picker(&[("alpha", "")], false);
        p.edit_filter(|f| f.push_str("zzz"));
        assert!(p.view.is_empty());
        assert_eq!(p.current_index(), None);
        p.move_by(1);
        assert_eq!(p.current_index(), None);
    }

    #[test]
    fn ticked_boxes_survive_the_filter() {
        let mut p = picker(&[("acme", ""), ("globex", "")], true);
        p.toggle();
        p.edit_filter(|f| f.push_str("globex"));
        p.toggle();
        p.edit_filter(|f| f.clear());
        assert!(p.items.iter().all(|c| c.checked));
    }

    #[test]
    fn navigation_stays_inside_the_view() {
        let mut p = picker(&[("a", ""), ("b", ""), ("c", "")], false);
        p.move_by(-1);
        assert_eq!(p.sel, 0);
        p.move_by(10);
        assert_eq!(p.sel, 2);
    }

    #[test]
    fn a_cell_highlights_what_matched() {
        let base = Style::default();
        let spans = cell("acme", &[0, 1], base, 4, false);
        let rendered: Vec<&str> = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(rendered, ["ac", "me"]);
        assert!(spans[0].style.add_modifier.contains(Modifier::UNDERLINED));
        assert!(!spans[1].style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn a_cell_truncates_and_pads_to_its_column() {
        let base = Style::default();
        let width = |spans: Vec<Span>| {
            spans
                .iter()
                .map(|s| s.content.chars().count())
                .sum::<usize>()
        };
        assert_eq!(width(cell("acme", &[], base, 10, true)), 10);
        assert_eq!(width(cell("acme", &[], base, 10, false)), 4);
        assert_eq!(width(cell("acme-production", &[], base, 6, true)), 6);
    }

    #[test]
    fn deleting_a_word_cuts_on_a_character_boundary() {
        let mut f = String::from("acme prod");
        drop_last_word(&mut f);
        assert_eq!(f, "acme ");
        drop_last_word(&mut f);
        assert_eq!(f, "");
        drop_last_word(&mut f);
        assert_eq!(f, "");
        // A non-breaking space is two bytes wide: cutting on a byte index would panic.
        let mut f = String::from("acme\u{a0}prod");
        drop_last_word(&mut f);
        assert_eq!(f, "acme\u{a0}");
    }
}
