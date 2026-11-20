//! Optional registry of component threads hosted by the AgentConsole launcher.
//! Thread selection, input, history and clipboard handling stay in the native TUI.
use super::*;
use crate::pager_overlay::TranscriptOverlay;
use crossterm::event::MouseButton;
use ratatui::buffer::Buffer;
use ratatui::style::Stylize;
use ratatui::widgets::Block;
use ratatui::widgets::BorderType;
use ratatui::widgets::Widget;
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct AgentConsole {
    control: std::path::PathBuf,
    rows: Vec<Component>,
    observer_model: Option<String>,
    #[serde(skip)]
    folded: bool,
    #[serde(skip)]
    shortcut_prefix: bool,
    #[serde(skip)]
    observers: Vec<(ThreadId, ThreadId)>, // (observer, source), in creation order
    #[serde(skip)]
    footer: Rect,
    #[serde(skip)]
    status: std::collections::HashMap<ThreadId, ComponentStatus>,
    #[serde(skip)]
    refreshed: Option<std::time::Instant>,
    #[serde(skip)]
    scroll: usize,
    #[serde(skip)]
    area: Rect,
    #[serde(skip)]
    chat: Rect,
    #[serde(skip)]
    pub(super) detail: Option<(ThreadId, TranscriptOverlay)>,
}

#[derive(Deserialize)]
struct ComponentStatus {
    active: bool,
    waiting: usize,
}

#[derive(Deserialize)]
struct Component {
    name: String,
    id: ThreadId,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConsoleAction {
    Next,
    Trace,
    Queue,
    Observe,
}

impl ConsoleAction {
    fn keys(observer: bool) -> &'static [(char, ConsoleAction, &'static str)] {
        use ConsoleAction::*;
        if observer {
            &[('n', Next, "Next agent"), ('o', Observe, "Back to agent")]
        } else {
            &[
                ('n', Next, "Next agent"),
                ('t', Trace, "Trace"),
                ('q', Queue, "Queue"),
                ('o', Observe, "Observer"),
            ]
        }
    }
}

impl AgentConsole {
    pub(super) fn from_environment() -> Result<Option<Self>> {
        std::env::var_os("CODEX_AGENTCONSOLE")
            .map(|value| serde_json::from_str(&value.to_string_lossy()))
            .transpose()
            .map_err(Into::into)
    }

    pub(super) fn split(&mut self, area: Rect) -> Rect {
        let width = if self.folded || area.width < 55 {
            3
        } else {
            23
        };
        let footer_height = u16::from(area.height >= 5);
        self.footer = Rect::new(
            area.x,
            area.bottom() - footer_height,
            area.width,
            footer_height,
        );
        let area = Rect {
            height: area.height - footer_height,
            ..area
        };
        self.area = Rect::new(area.x, area.y, width.min(area.width), area.height);
        self.chat = Rect::new(
            self.area.right(),
            area.y,
            area.width - self.area.width,
            area.height,
        );
        if self.framed() {
            Self::frame().inner(self.chat)
        } else {
            self.chat
        }
    }

    fn framed(&self) -> bool {
        self.area.width > 3 && self.area.height >= 8
    }

    fn frame() -> Block<'static> {
        Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(crate::style::accent_style())
    }

    fn capacity(&self) -> usize {
        usize::from(
            self.area
                .height
                .saturating_sub(if self.framed() { 2 } else { 1 }),
        )
    }

    fn source(&self, id: ThreadId) -> ThreadId {
        self.observers
            .iter()
            .find_map(|(observer, source)| (*observer == id).then_some(*source))
            .unwrap_or(id)
    }

    fn shortcut_hint(&self) -> &'static str {
        if self.shortcut_prefix {
            " Press key: "
        } else {
            " Ctrl+X → "
        }
    }

    fn buttons(&self, selected: Option<ThreadId>) -> Vec<(Rect, String, ConsoleAction)> {
        let observer = selected.is_some_and(|id| self.source(id) != id);
        let mut x = self
            .footer
            .x
            .saturating_add(self.shortcut_hint().chars().count() as u16);
        ConsoleAction::keys(observer)
            .iter()
            .filter_map(|(key, action, label)| {
                let key = key.to_ascii_uppercase();
                let label = if self.footer.width < 55 {
                    label.split_whitespace().next().unwrap_or(label)
                } else {
                    label
                };
                let text = format!(" {key} {label} ");
                let width = u16::try_from(text.len()).ok()?;
                if width > self.footer.right().saturating_sub(x) {
                    return None;
                }
                let rect = Rect::new(x, self.footer.y, width, self.footer.height);
                x += width;
                Some((rect, text, *action))
            })
            .collect()
    }

    pub(super) fn paint(&mut self, buffer: &mut Buffer, selected: Option<ThreadId>) {
        let area = self.area;
        let framed = self.framed();
        let inset = u16::from(framed);
        let line = |row: u16| {
            Rect::new(
                area.x + inset,
                area.y + row,
                area.width.saturating_sub(2 * inset),
                1,
            )
        };
        if area.is_empty() {
            return;
        }
        self.shortcut_hint().cyan().render(self.footer, buffer);
        for (rect, text, _) in self.buttons(selected) {
            if let Some((key, label)) = text.trim_start().split_once(' ') {
                ratatui::text::Line::from(vec![
                    format!(" {key} ").cyan().bold(),
                    label.to_owned().into(),
                ])
                .render(rect, buffer);
            }
        }
        if area.width <= 3 {
            "[>]".bold().render(line(0), buffer);
            return;
        }
        if self
            .refreshed
            .is_none_or(|time| time.elapsed() >= std::time::Duration::from_millis(900))
        {
            self.refreshed = Some(std::time::Instant::now());
            if let Ok(bytes) = std::fs::read(self.control.with_file_name("status.json"))
                && let Ok(status) = serde_json::from_slice(&bytes)
            {
                self.status = status;
            }
        }
        if framed {
            Self::frame()
                .title(" System agents [<] ")
                .render(area, buffer);
            let name = self
                .rows
                .iter()
                .find(|c| Some(c.id) == selected.map(|id| self.source(id)))
                .map(|c| c.name.as_str());
            let mode = if selected.is_some_and(|id| self.source(id) != id) {
                "Observer"
            } else {
                "Chat"
            };
            let title = name.map_or_else(|| " Chat ".to_owned(), |n| format!(" {mode} · {n} "));
            Self::frame().title(title).render(self.chat, buffer);
        } else {
            "System agents [<]".bold().render(line(0), buffer);
        }
        for (n, component) in self
            .rows
            .iter()
            .skip(self.scroll)
            .take(self.capacity())
            .enumerate()
        {
            let status = self.status.get(&component.id);
            let marker = if status.is_some_and(|s| s.active) {
                "●"
            } else {
                "·"
            };
            let waiting = status
                .filter(|s| s.waiting > 0)
                .map(|s| format!(" +{}", s.waiting))
                .unwrap_or_default();
            let label = format!("{marker} {}{waiting}", component.name);
            if Some(component.id) == selected.map(|id| self.source(id)) {
                label.reversed().render(line(1 + n as u16), buffer);
            } else {
                label.as_str().render(line(1 + n as u16), buffer);
            }
        }
    }

    async fn request(
        &self,
        action: &str,
        id: ThreadId,
        text: Option<String>,
    ) -> Result<serde_json::Value> {
        #[cfg(unix)]
        {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
            let request = serde_json::json!({"action":action,"id":id,"text":text});
            let exchange = async {
                let mut stream = tokio::net::UnixStream::connect(&self.control).await?;
                stream.write_all(request.to_string().as_bytes()).await?;
                stream.write_all(b"\n").await?;
                let mut response = String::new();
                BufReader::new(stream).read_line(&mut response).await?;
                let envelope: serde_json::Value = serde_json::from_str(&response)?;
                if let Some(message) = envelope["error"].as_str() {
                    color_eyre::eyre::bail!("{message}");
                }
                Ok(envelope["value"].clone())
            };
            tokio::time::timeout(std::time::Duration::from_secs(15), exchange).await?
        }
        #[cfg(not(unix))]
        {
            let _ = (action, id, text);
            color_eyre::eyre::bail!("AgentConsole requires a local Unix socket");
        }
    }
}

impl App {
    async fn detach_excerpt(
        &mut self,
        server: &mut AppServerSession,
        id: ThreadId,
        text: String,
    ) -> Result<()> {
        let console = self.agent_console.as_mut().expect("agent console is open");
        let source = console
            .rows
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.name.as_str())
            .unwrap_or("thread");
        let started = server
            .start_excerpt_observer(
                &self.local_settings,
                &self.config,
                console
                    .observer_model
                    .clone()
                    .unwrap_or_else(|| self.chat_widget.current_model().to_owned()),
                format!("Excerpt from {source}:\n{text}"),
            )
            .await?;
        let child = started.session.thread_id;
        console.observers.push((child, id));
        self.install_thread_attachment(
            server,
            child,
            super::thread_attachment::PreparedAttachment::Live(Box::new(started)),
            super::thread_attachment::ChannelMode::ReplaceLive,
        )
        .await;
        self.app_event_tx.send(AppEvent::SelectAgentThread(child));
        Ok(())
    }

    pub(super) async fn agent_console_detail(
        &mut self,
        tui: &mut tui::Tui,
        server: &mut AppServerSession,
        event: TuiEvent,
    ) -> Result<()> {
        let console = self.agent_console.as_mut().expect("agent console is open");
        let Some((id, view)) = console.detail.as_mut() else {
            return Ok(());
        };
        if matches!(&event, TuiEvent::Key(key) if key.kind == KeyEventKind::Press
            && key.code == KeyCode::Esc && key.modifiers.is_empty())
        {
            console.detail = None;
        } else if matches!(&event,TuiEvent::Key(key) if key.kind==KeyEventKind::Press && key.code==KeyCode::Char('d') && key.modifiers.is_empty())
            && !view.is_search_editing()
            && let Some(text) = view.selected_excerpt()
        {
            let source = *id;
            console.detail = None;
            self.detach_excerpt(server, source, text).await?;
        } else {
            view.handle_event(tui, event)?;
            if !view.is_done() {
                return Ok(());
            }
            console.detail = None;
        }
        tui.frame_requester().schedule_frame();
        Ok(())
    }

    pub(super) async fn agent_console_input(
        &mut self,
        tui: &mut tui::Tui,
        server: &mut AppServerSession,
        event: &TuiEvent,
    ) -> Result<bool> {
        let Some(console) = self.agent_console.as_mut() else {
            return Ok(false);
        };
        let selected = self.active_thread_id;
        if let TuiEvent::Key(key) = event {
            if key.code == KeyCode::Char('x') && key.modifiers == KeyModifiers::CONTROL {
                if key.kind == KeyEventKind::Press {
                    console.shortcut_prefix = !console.shortcut_prefix;
                    tui.frame_requester().schedule_frame();
                }
                return Ok(true);
            }
            if console.shortcut_prefix && key.kind != KeyEventKind::Press {
                return Ok(true);
            }
        }
        let pending = console.shortcut_prefix;
        if pending
            && matches!(
                event,
                TuiEvent::Key(_)
                    | TuiEvent::Paste(_)
                    | TuiEvent::FocusLost
                    | TuiEvent::Mouse(crossterm::event::MouseEvent {
                        kind: MouseEventKind::Down(_),
                        ..
                    })
            )
        {
            console.shortcut_prefix = false;
            tui.frame_requester().schedule_frame();
        }
        let action = match event {
            TuiEvent::Key(key) if pending && key.code == KeyCode::Esc => return Ok(true),
            TuiEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && (key.modifiers == KeyModifiers::ALT
                        || (pending && key.modifiers.is_empty())) =>
            {
                let KeyCode::Char(ch) = key.code else {
                    return Ok(false);
                };
                let observer = selected.is_some_and(|id| console.source(id) != id);
                ConsoleAction::keys(observer)
                    .iter()
                    .find_map(|(k, a, _)| (*k == ch.to_ascii_lowercase()).then_some(*a))
            }
            TuiEvent::Mouse(mouse) => {
                let pos = (mouse.column, mouse.row).into();
                if console.footer.contains(pos) {
                    if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
                        return Ok(true);
                    }
                    console
                        .buttons(selected)
                        .into_iter()
                        .find_map(|(rect, _, action)| rect.contains(pos).then_some(action))
                } else if console.area.contains(pos) {
                    let row = usize::from(mouse.row - console.area.y);
                    match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            console.scroll = console.scroll.saturating_sub(1)
                        }
                        MouseEventKind::ScrollDown => {
                            console.scroll = (console.scroll + 1)
                                .min(console.rows.len().saturating_sub(console.capacity()))
                        }
                        MouseEventKind::Down(MouseButton::Left) if row == 0 => {
                            console.folded = !console.folded
                        }
                        MouseEventKind::Down(MouseButton::Left)
                            if console.area.width > 3 && row <= console.capacity() =>
                        {
                            if let Some(c) = console.rows.get(console.scroll + row - 1) {
                                self.app_event_tx.send(AppEvent::SelectAgentThread(c.id));
                            }
                        }
                        _ => {}
                    }
                    tui.frame_requester().schedule_frame();
                    return Ok(true);
                } else {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        };
        let (Some(action), Some(id)) = (action, selected) else {
            return Ok(false);
        };
        let source = console.source(id);
        match action {
            ConsoleAction::Next => {
                if !console.rows.is_empty() {
                    let next = console
                        .rows
                        .iter()
                        .position(|r| r.id == source)
                        .map_or(0, |i| (i + 1) % console.rows.len());
                    self.app_event_tx
                        .send(AppEvent::SelectAgentThread(console.rows[next].id));
                    if next < console.scroll || next >= console.scroll + console.capacity() {
                        console.scroll = next;
                    }
                }
            }
            ConsoleAction::Observe if source != id => {
                self.app_event_tx.send(AppEvent::SelectAgentThread(source))
            }
            ConsoleAction::Observe => {
                if let Some(text) = self.transcript_view.selected_text(&self.transcript_cells) {
                    self.detach_excerpt(server, id, text).await?;
                } else if let Some((observer, _)) =
                    console.observers.iter().rev().find(|(_, s)| *s == id)
                {
                    self.app_event_tx
                        .send(AppEvent::SelectAgentThread(*observer));
                } else {
                    self.chat_widget.add_info_message(
                        "Select transcript text to start an observer.".into(),
                        None,
                    );
                }
            }
            ConsoleAction::Trace | ConsoleAction::Queue => {
                let action = if action == ConsoleAction::Trace {
                    "Trace"
                } else {
                    "Queue"
                };
                let response = console.request(action, id, None).await?;
                let lines = response
                    .as_str()
                    .unwrap_or_default()
                    .lines()
                    .map(|s| ratatui::text::Line::from(s.to_owned()))
                    .collect();
                let cell = crate::history_cell::PlainHistoryCell::new(lines);
                let mut view = TranscriptOverlay::new(
                    vec![std::sync::Arc::new(cell)],
                    self.keymap.pager.clone(),
                );
                let name = console
                    .rows
                    .iter()
                    .find(|component| component.id == id)
                    .map(|component| component.name.as_str())
                    .unwrap_or("thread");
                view.panel_title = Some(format!(" {action} · {name} "));
                view.set_keymap_bindings(&self.keymap);
                view.browsing_footer = Some(
                    format!(
                        "{action}   ·   drag to select   ·   d discuss selection   ·   esc back"
                    )
                    .into(),
                );
                console.detail = Some((id, view));
            }
        }
        tui.frame_requester().schedule_frame();
        Ok(true)
    }
}
