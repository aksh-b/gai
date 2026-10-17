//! `gai chat`: a full-screen conversation with an agent.

use std::collections::VecDeque;
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use gai::Gai;
use gai::agent::{AgentSpec, Decision, RunEvent, RunHandle, RunRequest, Runtime};
use gai::core::Usage;
use gai::store::RunStatus;
use ratatui::DefaultTerminal;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

use crate::run::{AgentArgs, resolve_agent};

#[derive(clap::Args)]
pub struct ChatArgs {
    #[command(flatten)]
    agent: AgentArgs,
    /// Continue an existing session
    #[arg(long)]
    session: Option<String>,
    /// Approve every tool call without asking
    #[arg(long, short = 'y')]
    yes: bool,
}

pub async fn chat(gai: &Gai, args: ChatArgs) -> anyhow::Result<()> {
    let (spec, version) = resolve_agent(gai, &args.agent).await?;
    let rt = gai.runtime().await?.clone();
    let model = spec
        .model
        .clone()
        .or_else(|| rt.config().default_model.clone())
        .unwrap_or_else(|| "(no model)".into());
    let mut app = App::new(spec, version, model, args.session, args.yes);
    if let Some(id) = app.session_id.clone() {
        app.load_transcript(&rt, &id).await?;
    }
    let mut terminal = ratatui::init();
    let result = app.run(&mut terminal, &rt).await;
    ratatui::restore();
    if let Some(id) = &app.session_id {
        eprintln!("session {id}  (resume with: gai chat --session {id})");
    }
    result
}

enum Item {
    User(String),
    Assistant(String),
    Reasoning(String),
    Tool {
        id: String,
        name: String,
        args: String,
        result: Option<(bool, String, u64)>,
    },
    Note(String, Color),
}

struct Pending {
    run_id: String,
    call_id: String,
    name: String,
    input: String,
}

struct App {
    spec: AgentSpec,
    version: Option<u32>,
    model: String,
    session_id: Option<String>,
    auto_approve: bool,
    items: Vec<Item>,
    input: String,
    cursor: usize,
    history: Vec<String>,
    history_pos: Option<usize>,
    /// Lines scrolled up from the bottom; 0 follows new output.
    scroll: usize,
    run: Option<RunHandle>,
    pending: VecDeque<Pending>,
    usage: Usage,
    cost: f64,
    quit: bool,
}

impl App {
    fn new(
        spec: AgentSpec,
        version: Option<u32>,
        model: String,
        session_id: Option<String>,
        auto_approve: bool,
    ) -> Self {
        let mut app = Self {
            spec,
            version,
            model,
            session_id,
            auto_approve,
            items: Vec::new(),
            input: String::new(),
            cursor: 0,
            history: Vec::new(),
            history_pos: None,
            scroll: 0,
            run: None,
            pending: VecDeque::new(),
            usage: Usage::default(),
            cost: 0.0,
            quit: false,
        };
        app.note("Enter sends · Alt+Enter new line · Esc stops a run · PgUp/PgDn scroll · /new /help /quit", Color::DarkGray);
        app
    }

    fn note(&mut self, text: impl Into<String>, color: Color) {
        self.items.push(Item::Note(text.into(), color));
    }

    async fn load_transcript(&mut self, rt: &Runtime, session: &str) -> anyhow::Result<()> {
        for m in rt.store().list_messages(session).await? {
            for b in m.message.content {
                match b {
                    gai::core::ContentBlock::Text { text } => {
                        self.items.push(match m.message.role {
                            gai::core::Role::User => Item::User(text),
                            gai::core::Role::Assistant => Item::Assistant(text),
                        })
                    }
                    gai::core::ContentBlock::ToolUse { id, name, input } => {
                        self.items.push(Item::Tool {
                            id,
                            name,
                            args: input.to_string(),
                            result: None,
                        })
                    }
                    gai::core::ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        let text: String = content.iter().filter_map(|c| c.as_text()).collect();
                        self.set_tool_result(&tool_use_id, !is_error, text, 0);
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    async fn run(&mut self, terminal: &mut DefaultTerminal, rt: &Runtime) -> anyhow::Result<()> {
        let mut keys = EventStream::new();
        let mut tick = tokio::time::interval(Duration::from_millis(40));
        let mut dirty = true;
        while !self.quit {
            if dirty {
                terminal.draw(|f| self.draw(f))?;
                dirty = false;
            }
            tokio::select! {
                key = keys.next() => match key {
                    Some(Ok(Event::Key(k))) if k.kind != KeyEventKind::Release => {
                        self.on_key(k, rt).await;
                        dirty = true;
                    }
                    Some(Ok(Event::Resize(..))) => dirty = true,
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.into()),
                    None => break,
                },
                ev = next_run_event(&mut self.run) => {
                    match ev {
                        Some(e) => self.on_run_event(e),
                        None => self.run = None,
                    }
                    // Coalesce bursts of deltas into one redraw.
                    dirty = true;
                }
                _ = tick.tick(), if dirty => {}
            }
        }
        if let Some(h) = &self.run {
            rt.cancel(&h.run_id);
        }
        Ok(())
    }

    async fn on_key(&mut self, k: KeyEvent, rt: &Runtime) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        if ctrl && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('d')) {
            self.quit = true;
            return;
        }
        if let Some((run_id, call_id)) = self
            .pending
            .front()
            .map(|p| (p.run_id.clone(), p.call_id.clone()))
        {
            let decision = match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => Some(Decision::approve()),
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    Some(Decision::reject("the user declined"))
                }
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    self.auto_approve = true;
                    self.note(
                        "Auto-approving tool calls for the rest of this chat.",
                        Color::Yellow,
                    );
                    Some(Decision::approve())
                }
                _ => None,
            };
            if let Some(d) = decision {
                if let Err(e) = rt.resolve_approval(&run_id, &call_id, d) {
                    self.note(e, Color::Red);
                }
                self.pending.pop_front();
            }
            return;
        }
        match k.code {
            KeyCode::Esc => {
                if let Some(h) = &self.run {
                    rt.cancel(&h.run_id);
                    self.note("Stopping…", Color::Yellow);
                }
            }
            KeyCode::Enter if alt || ctrl => self.insert('\n'),
            KeyCode::Enter => self.submit(rt).await,
            KeyCode::Char('j') if ctrl => self.insert('\n'),
            KeyCode::Char('u') if ctrl => {
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Char(c) => self.insert(c),
            KeyCode::Backspace if self.cursor > 0 => {
                let prev = self.input[..self.cursor]
                    .chars()
                    .next_back()
                    .map_or(0, char::len_utf8);
                self.cursor -= prev;
                self.input.remove(self.cursor);
            }
            KeyCode::Delete if self.cursor < self.input.len() => {
                self.input.remove(self.cursor);
            }
            KeyCode::Left => {
                self.cursor -= self.input[..self.cursor]
                    .chars()
                    .next_back()
                    .map_or(0, char::len_utf8)
            }
            KeyCode::Right => {
                self.cursor += self.input[self.cursor..]
                    .chars()
                    .next()
                    .map_or(0, char::len_utf8)
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),
            KeyCode::Up if self.input.is_empty() || self.history_pos.is_some() => self.recall(-1),
            KeyCode::Down if self.history_pos.is_some() => self.recall(1),
            KeyCode::PageUp => self.scroll += 10,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Up if ctrl => self.scroll += 1,
            KeyCode::Down if ctrl => self.scroll = self.scroll.saturating_sub(1),
            _ => {}
        }
    }

    fn insert(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    fn recall(&mut self, dir: i32) {
        if self.history.is_empty() {
            return;
        }
        let pos = match (self.history_pos, dir) {
            (None, _) => self.history.len() - 1,
            (Some(0), -1) => 0,
            (Some(p), -1) => p - 1,
            (Some(p), _) if p + 1 >= self.history.len() => {
                self.history_pos = None;
                self.input.clear();
                self.cursor = 0;
                return;
            }
            (Some(p), _) => p + 1,
        };
        self.history_pos = Some(pos);
        self.input = self.history[pos].clone();
        self.cursor = self.input.len();
    }

    async fn submit(&mut self, rt: &Runtime) {
        let text = self.input.trim().to_owned();
        if text.is_empty() {
            return;
        }
        self.input.clear();
        self.cursor = 0;
        self.history_pos = None;
        self.history.push(text.clone());
        self.scroll = 0;
        match text.as_str() {
            "/quit" | "/exit" => {
                self.quit = true;
                return;
            }
            "/new" => {
                self.session_id = None;
                self.items.clear();
                self.usage = Usage::default();
                self.cost = 0.0;
                self.note("New session.", Color::DarkGray);
                return;
            }
            "/help" => {
                self.note(
                    "/new starts a fresh session · /quit exits · Esc stops the current run · y/n/a answer approvals (a = allow all)",
                    Color::DarkGray,
                );
                return;
            }
            _ => {}
        }
        if self.run.is_some() {
            self.note(
                "Still working on the last message; press Esc to stop it first.",
                Color::Yellow,
            );
            self.input = text;
            self.cursor = self.input.len();
            return;
        }
        self.items.push(Item::User(text.clone()));
        let mut req = RunRequest::new(self.spec.clone(), text);
        req.agent_version = self.version;
        req.session_id = self.session_id.clone();
        req.auto_approve = self.auto_approve;
        match rt.start(req).await {
            Ok(h) => {
                self.session_id = Some(h.session_id.clone());
                self.run = Some(h);
            }
            Err(e) => self.note(format!("Could not start: {e}"), Color::Red),
        }
    }

    fn set_tool_result(&mut self, id: &str, ok: bool, text: String, ms: u64) {
        for item in self.items.iter_mut().rev() {
            if let Item::Tool {
                id: tid, result, ..
            } = item
                && tid == id
            {
                *result = Some((ok, text, ms));
                return;
            }
        }
    }

    fn on_run_event(&mut self, env: gai::agent::EventEnvelope) {
        let run_id = env.run_id.clone();
        match env.event {
            RunEvent::RunStarted { model, .. } => self.model = model,
            RunEvent::TextDelta { text } => match self.items.last_mut() {
                Some(Item::Assistant(s)) => s.push_str(&text),
                _ => self.items.push(Item::Assistant(text)),
            },
            RunEvent::ReasoningDelta { text } => match self.items.last_mut() {
                Some(Item::Reasoning(s)) => s.push_str(&text),
                _ => self.items.push(Item::Reasoning(text)),
            },
            RunEvent::ToolCall {
                id, name, input, ..
            } => self.items.push(Item::Tool {
                id,
                name,
                args: input.to_string(),
                result: None,
            }),
            RunEvent::ToolResult {
                id,
                output,
                duration_ms,
                ..
            } => self.set_tool_result(&id, !output.is_error, output.as_text(), duration_ms),
            RunEvent::ApprovalRequired { id, name, input } => self.pending.push_back(Pending {
                run_id,
                call_id: id,
                name,
                input: serde_json::to_string_pretty(&input).unwrap_or_default(),
            }),
            RunEvent::ApprovalResolved {
                approved: false,
                reason,
                ..
            } => self.note(
                format!(
                    "Declined{}",
                    reason.map(|r| format!(": {r}")).unwrap_or_default()
                ),
                Color::Yellow,
            ),
            RunEvent::Usage { usage, cost, .. } => {
                self.usage += usage;
                self.cost += cost.unwrap_or(0.0);
            }
            RunEvent::Warning { message } => self.note(message, Color::Yellow),
            RunEvent::Compacted {
                summarized_messages,
                ..
            } => self.note(
                format!("Summarized {summarized_messages} earlier messages to free context."),
                Color::DarkGray,
            ),
            RunEvent::RunFinished { status, error, .. } => {
                if status != RunStatus::Completed {
                    let msg = match error {
                        Some(e) => format!("Run {}: {e}", status.as_str()),
                        None => format!("Run {}.", status.as_str()),
                    };
                    self.note(
                        msg,
                        if status == RunStatus::Cancelled {
                            Color::Yellow
                        } else {
                            Color::Red
                        },
                    );
                }
                self.pending.retain(|p| p.run_id != run_id);
            }
            _ => {}
        }
    }

    fn draw(&self, f: &mut ratatui::Frame) {
        let area = f.area();
        let width = area.width.saturating_sub(2).max(10) as usize;
        let (input_lines, cursor) = wrap_input(&self.input, self.cursor, width.saturating_sub(2));
        let input_height = if self.pending.is_empty() {
            (input_lines.len() as u16).clamp(1, 8) + 2
        } else {
            3
        };
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(input_height),
        ])
        .areas(area);

        // Header: agent · model · session · usage
        let status = if self.run.is_some() {
            Span::styled(" ● working ", Style::new().fg(Color::Yellow))
        } else {
            Span::raw(" ")
        };
        let version = self.version.map(|v| format!(" v{v}")).unwrap_or_default();
        let tokens = format!(
            "{} in · {} out",
            self.usage.total_input(),
            self.usage.output_tokens
        );
        let cost = if self.cost > 0.0 {
            format!(" · ${:.4}", self.cost)
        } else {
            String::new()
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!(" {}{version} ", self.spec.name),
                    Style::new().bold().fg(Color::Black).bg(Color::LightYellow),
                ),
                Span::styled(format!(" {} ", self.model), Style::new().fg(Color::Cyan)),
                status,
                Span::styled(format!("{tokens}{cost}"), Style::new().fg(Color::DarkGray)),
            ])),
            header,
        );

        // Transcript, bottom-anchored.
        let lines = self.transcript_lines(body.width.saturating_sub(2) as usize);
        let visible = body.height as usize;
        let max_scroll = lines.len().saturating_sub(visible);
        let scroll = self.scroll.min(max_scroll);
        let start = lines.len().saturating_sub(visible + scroll);
        let shown: Vec<Line> = lines.into_iter().skip(start).take(visible).collect();
        f.render_widget(
            Paragraph::new(shown),
            Rect {
                x: body.x + 1,
                width: body.width.saturating_sub(2),
                ..body
            },
        );

        // Input, or the approval prompt.
        if let Some(p) = self.pending.front() {
            let block = Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(Color::Yellow))
                .title(" approval needed ");
            let args = p.input.replace('\n', " ");
            let line = Line::from(vec![
                Span::styled(format!("Run {} ", p.name), Style::new().bold()),
                Span::styled(
                    clip(&args, width.saturating_sub(40)),
                    Style::new().fg(Color::DarkGray),
                ),
                Span::styled("  [y]es [n]o [a]llow all", Style::new().fg(Color::Yellow)),
            ]);
            f.render_widget(Paragraph::new(line).block(block), footer);
        } else {
            let hint = if self.run.is_some() {
                " working… Esc to stop "
            } else {
                " message "
            };
            let block = Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(Color::DarkGray))
                .title(hint);
            let text: Vec<Line> = input_lines.into_iter().map(Line::from).collect();
            let inner_h = footer.height.saturating_sub(2) as usize;
            let skip = (cursor.1 + 1).saturating_sub(inner_h);
            f.render_widget(
                Paragraph::new(text.into_iter().skip(skip).collect::<Vec<_>>()).block(block),
                footer,
            );
            f.set_cursor_position(Position::new(
                footer.x + 1 + cursor.0 as u16,
                footer.y + 1 + (cursor.1 - skip) as u16,
            ));
        }
    }

    fn transcript_lines(&self, width: usize) -> Vec<Line<'static>> {
        let width = width.max(10);
        let mut out: Vec<Line> = Vec::new();
        for item in &self.items {
            match item {
                Item::User(t) => {
                    out.push(Line::default());
                    push_wrapped(
                        &mut out,
                        t,
                        width,
                        "› ",
                        Style::new()
                            .fg(Color::LightBlue)
                            .add_modifier(Modifier::BOLD),
                    );
                }
                Item::Assistant(t) => {
                    out.push(Line::default());
                    push_wrapped(&mut out, t, width, "", Style::new());
                }
                Item::Reasoning(t) => {
                    push_wrapped(
                        &mut out,
                        t,
                        width,
                        "┆ ",
                        Style::new()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    );
                }
                Item::Tool {
                    name, args, result, ..
                } => {
                    let (mark, color) = match result {
                        None => ("◌", Color::Yellow),
                        Some((true, ..)) => ("✓", Color::Green),
                        Some((false, ..)) => ("✗", Color::Red),
                    };
                    out.push(Line::from(vec![
                        Span::styled(format!("{mark} "), Style::new().fg(color)),
                        Span::styled(name.clone(), Style::new().fg(Color::Cyan)),
                        Span::styled(
                            format!(" {}", clip(args, width.saturating_sub(name.len() + 4))),
                            Style::new().fg(Color::DarkGray),
                        ),
                    ]));
                    if let Some((_, text, ms)) = result {
                        let first = text
                            .lines()
                            .find(|l| !l.trim().is_empty())
                            .unwrap_or("")
                            .trim();
                        let timing = if *ms > 0 {
                            format!(" ({ms} ms)")
                        } else {
                            String::new()
                        };
                        out.push(Line::from(Span::styled(
                            format!("  └ {}{timing}", clip(first, width.saturating_sub(16))),
                            Style::new().fg(Color::DarkGray),
                        )));
                    }
                }
                Item::Note(t, color) => {
                    push_wrapped(&mut out, t, width, "", Style::new().fg(*color))
                }
            }
        }
        out
    }
}

async fn next_run_event(run: &mut Option<RunHandle>) -> Option<gai::agent::EventEnvelope> {
    match run {
        Some(h) => h.next_event().await,
        None => std::future::pending().await,
    }
}

fn push_wrapped(
    out: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    prefix: &str,
    style: Style,
) {
    let indent = " ".repeat(prefix.chars().count());
    let opts = textwrap::Options::new(width.max(prefix.len() + 5))
        .initial_indent(prefix)
        .subsequent_indent(&indent);
    for para in text.split('\n') {
        if para.is_empty() {
            out.push(Line::default());
            continue;
        }
        for l in textwrap::wrap(para, &opts) {
            out.push(Line::styled(l.into_owned(), style));
        }
    }
}

/// Hard-wrap the input box text and find the cursor's (column, row).
fn wrap_input(input: &str, cursor: usize, width: usize) -> (Vec<String>, (usize, usize)) {
    let width = width.max(1);
    let mut lines = vec![String::new()];
    let mut pos = (0, 0);
    for (i, c) in input.char_indices() {
        if i == cursor {
            pos = (
                lines.last().map_or(0, |l| l.chars().count()),
                lines.len() - 1,
            );
        }
        if c == '\n' {
            lines.push(String::new());
            continue;
        }
        if lines.last().is_some_and(|l| l.chars().count() >= width) {
            lines.push(String::new());
        }
        lines.last_mut().expect("non-empty").push(c);
    }
    if cursor >= input.len() {
        let last = lines.last().map_or(0, |l| l.chars().count());
        pos = if last >= width {
            (0, lines.len())
        } else {
            (last, lines.len() - 1)
        };
        if pos.1 == lines.len() {
            lines.push(String::new());
        }
    }
    (lines, pos)
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        format!(
            "{}…",
            s.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_wrapping_tracks_the_cursor() {
        assert_eq!(wrap_input("", 0, 10), (vec![String::new()], (0, 0)));
        let (lines, pos) = wrap_input("hello\nworld", 8, 10);
        assert_eq!(lines, ["hello", "world"]);
        assert_eq!(pos, (2, 1));
        let (lines, pos) = wrap_input("abcdef", 6, 3);
        assert_eq!(lines, ["abc", "def", ""]);
        assert_eq!(pos, (0, 2));
    }
}
