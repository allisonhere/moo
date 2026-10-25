//! Pure drawing: `Screen` + palette in, cells out. Nothing here reads a clock, a socket or the terminal.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    symbols::border,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Widget},
};
use serde_json::Value;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::data::{tag, task_progress, AgentStatus};
use super::layout::{Area, Mode};
use super::state::{compact_json, view_tabs, Card, Focus, Link, NavNode, Scope, Screen, View};
use super::theme::{Density, Tide};
use crate::app::state::Palette;

const ASCII_BORDER: border::Set = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// The palette to draw with: a theme picked in this session, else the configured one.
pub fn resolve_palette(screen: &Screen, configured: &Palette) -> Palette {
    screen
        .palette_override()
        .and_then(Palette::from_name)
        .unwrap_or_else(|| configured.clone())
}

pub fn render(buf: &mut Buffer, area: Rect, screen: &Screen, palette: &Palette) {
    let tide = Tide::from_palette(palette, screen.options);
    Clear.render(area, buf);
    buf.set_style(area, Style::default().bg(tide.bg).fg(tide.text));
    // Lay out the area we were given, so drawing never depends on a stale remembered size.
    let frame = super::layout::compute(area, screen.visibility());
    if let Some(header) = frame.header {
        draw_header(buf, header, screen, &tide, frame.mode);
    }
    if frame.mode != Mode::Single || screen.active == Area::Main {
        draw_main(buf, frame.main, screen, &tide);
    }
    // Side areas last: in collapsed mode they are drawers that float over the workspace.
    if let Some(rect) = frame.navigator {
        Clear.render(rect, buf);
        draw_navigator(buf, rect, screen, &tide);
    }
    if let Some(rect) = frame.inspector {
        Clear.render(rect, buf);
        draw_inspector(buf, rect, screen, &tide);
    }
    if let Some(footer) = frame.footer {
        draw_footer(buf, footer, screen, &tide);
    }
    if screen.help {
        draw_help(buf, area, &tide);
    }
    if let Some(builder) = &screen.builder {
        draw_builder(buf, area, builder, &tide);
    }
    if screen.palette.is_some() {
        draw_palette(buf, area, screen, &tide);
    }
    if screen.modal.is_some() {
        draw_modal(buf, area, screen, &tide);
    }
    if let Some(toast) = &screen.toast {
        draw_toast(buf, area, toast, &tide);
    }
}

// ---- text helpers ---------------------------------------------------------------------------

pub fn fit(text: &str, width: usize, ellipsis: &str) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let room = width.saturating_sub(ellipsis.width());
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > room {
            break;
        }
        out.push(ch);
        used += w;
    }
    if width >= ellipsis.width() {
        out.push_str(ellipsis);
    }
    out
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(4);
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        if paragraph.width() <= width {
            lines.push(paragraph.to_owned());
            continue;
        }
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let word = fit_hard(word, width);
            if !line.is_empty() && line.width() + 1 + word.width() > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&word);
        }
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// A word longer than the line is cut rather than allowed to overflow.
fn fit_hard(word: &str, width: usize) -> String {
    if word.width() <= width {
        return word.to_owned();
    }
    fit(word, width, "~")
}

fn pad(text: &str, width: usize) -> String {
    let used = text.width();
    format!("{text}{}", " ".repeat(width.saturating_sub(used)))
}

fn put(buf: &mut Buffer, rect: Rect, row: u16, line: Line<'_>) {
    if row < rect.height {
        buf.set_line(rect.x, rect.y + row, &line, rect.width);
    }
}

fn panel(buf: &mut Buffer, rect: Rect, title: &str, focused: bool, tide: &Tide) -> Rect {
    if rect.width < 3 || rect.height < 3 {
        return Rect::new(rect.x, rect.y, 0, 0);
    }
    let color = if focused {
        tide.border_focus
    } else {
        tide.border
    };
    let mut title_style = Style::default().fg(if focused {
        tide.border_focus
    } else {
        tide.muted
    });
    if focused {
        title_style = title_style.add_modifier(Modifier::BOLD);
        if tide.options.high_contrast {
            title_style = title_style.add_modifier(Modifier::REVERSED);
        }
    }
    let set = if tide.options.ascii {
        ASCII_BORDER
    } else {
        border::ROUNDED
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(set)
        .border_style(Style::default().fg(color))
        .title(Span::styled(
            format!(
                " {} ",
                fit(
                    title,
                    rect.width.saturating_sub(6) as usize,
                    tide.glyphs.ellipsis
                )
            ),
            title_style,
        ))
        .style(Style::default().bg(tide.panel));
    let inner = block.inner(rect);
    block.render(rect, buf);
    inner
}

fn highlight_mentions<'a>(text: &str, base: Style, mention: Style) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    let mut first = true;
    for word in text.split(' ') {
        if !first {
            spans.push(Span::styled(" ".to_owned(), base));
        }
        first = false;
        let style = if word.starts_with('@') && word.len() > 1 {
            mention
        } else {
            base
        };
        spans.push(Span::styled(word.to_owned(), style));
    }
    spans
}

// ---- header and footer ----------------------------------------------------------------------

fn draw_header(buf: &mut Buffer, rect: Rect, screen: &Screen, tide: &Tide, mode: Mode) {
    let snap = &screen.snapshot;
    let mut right: Vec<Span> = Vec::new();
    let pending = snap.attention.pending_approvals.len();
    let conflicts = snap.attention.open_conflicts.len();
    if pending > 0 {
        right.push(Span::styled(
            format!("{} {pending} awaiting you  ", tide.glyphs.approval),
            Style::default().fg(tide.warn),
        ));
    }
    let unknown = snap.attention.merge_unknown.len();
    if unknown > 0 {
        right.push(Span::styled(
            format!("{} {unknown} merge unknown  ", tide.glyphs.blocked),
            Style::default().fg(tide.danger),
        ));
    }
    if conflicts > 0 {
        right.push(Span::styled(
            format!("{} {conflicts} conflicts  ", tide.glyphs.blocked),
            Style::default().fg(tide.danger),
        ));
    }
    let (state, color) = match &screen.link {
        Link::Loading => ("loading".to_owned(), tide.muted),
        Link::Live => (format!("live  rev {}", snap.revision), tide.good),
        Link::Reconnecting(why) => (format!("reconnecting - {why}"), tide.warn),
    };
    if let Some(theme) = screen.palette_override() {
        right.push(Span::styled(
            format!("theme {theme}  "),
            Style::default().fg(tide.muted),
        ));
    }
    right.push(Span::styled(state, Style::default().fg(color)));
    let left = if mode == Mode::Single {
        // One area at a time: say which, and what the others are called.
        let tab = |name: &str, area: Area| {
            if screen.active == area {
                format!("[{name}]")
            } else {
                format!(" {name} ")
            }
        };
        format!(
            " {}{}{}",
            tab("teams", Area::Navigator),
            tab("workspace", Area::Main),
            tab("info", Area::Inspector)
        )
    } else {
        format!(
            " moo teams  {}  {}",
            tide.glyphs.rail.trim(),
            screen.scope_title()
        )
    };
    // Status text yields before the title does; the single-pane strip is never cut.
    let mut right_width: usize = right.iter().map(|s| s.content.width()).sum();
    while right_width + left.width() + 1 > rect.width as usize && !right.is_empty() {
        let dropped = right.remove(0);
        right_width -= dropped.content.width();
    }
    let room = (rect.width as usize).saturating_sub(right_width + 1);
    let left = fit(&left, room, tide.glyphs.ellipsis);
    let mut spans = vec![Span::styled(
        pad(&left, room),
        Style::default()
            .fg(tide.accent)
            .add_modifier(Modifier::BOLD),
    )];
    spans.extend(right);
    put(buf, rect, 0, Line::from(spans));
}

fn draw_footer(buf: &mut Buffer, rect: Rect, screen: &Screen, tide: &Tide) {
    // Highest priority first; whatever does not fit is dropped from the end, never clipped.
    let mut hints: Vec<(&str, &str)> = Vec::new();
    if screen.modal.is_some() {
        hints.extend([("enter", "confirm"), ("esc", "cancel")]);
    } else if let Some(builder) = &screen.builder {
        match builder.step {
            super::state::BuilderStep::Edit => {
                hints.extend([
                    ("tab", "next field"),
                    ("enter", "next / review"),
                    ("esc", "cancel"),
                ]);
            }
            super::state::BuilderStep::Review => {
                hints.extend([
                    ("enter", "create and launch"),
                    ("e", "edit"),
                    ("esc", "edit"),
                ]);
            }
        }
    } else if screen.palette.is_some() {
        hints.extend([("enter", "run"), ("up/down", "choose"), ("esc", "close")]);
    } else if screen.focus == Focus::Composer {
        hints.extend([("enter", "send"), ("esc", "back")]);
    } else {
        match screen.focus {
            Focus::Navigator => hints.extend([("j/k", "move"), ("enter", "open"), ("h/l", "fold")]),
            _ => match screen.view {
                View::Terminals => {
                    hints.extend([("j/k", "move"), ("enter", "focus pane"), ("b", "bind pane")]);
                }
                View::Chat => hints.extend([("j/k", "scroll"), ("c", "write")]),
                View::Tasks => hints.extend([("j/k", "move")]),
                View::Decisions => hints.extend([
                    ("j/k", "move"),
                    ("a", "approve"),
                    ("d", "deny"),
                    ("x", "resolve"),
                ]),
            },
        }
        hints.extend([
            ("tab", "area"),
            ("1-4", "view"),
            ("n", "nav"),
            ("i", "info"),
            (":", "commands"),
            ("?", "help"),
            ("q", "quit"),
        ]);
    }
    let width = rect.width as usize;
    let mut spans: Vec<Span> = Vec::new();
    let mut used = 1;
    spans.push(Span::raw(" "));
    for (key, what) in hints {
        let cost = key.width() + what.width() + 4;
        if used + cost > width {
            break;
        }
        spans.push(Span::styled(
            key.to_owned(),
            Style::default().fg(tide.accent),
        ));
        spans.push(Span::styled(
            format!(" {what}   "),
            Style::default().fg(tide.muted),
        ));
        used += cost;
    }
    put(buf, rect, 0, Line::from(spans));
}

// ---- navigator ------------------------------------------------------------------------------

fn draw_navigator(buf: &mut Buffer, rect: Rect, screen: &Screen, tide: &Tide) {
    let focused = screen.focus == Focus::Navigator;
    let inner = panel(buf, rect, "teams", focused, tide);
    if inner.width == 0 {
        return;
    }
    let rows = screen.nav_rows();
    let offset = screen.nav_offset(rect.height);
    let snap = &screen.snapshot;
    if !screen.loaded {
        put(
            buf,
            inner,
            0,
            Line::styled("loading...", Style::default().fg(tide.muted)),
        );
        return;
    }
    for (idx, row) in rows
        .iter()
        .skip(offset)
        .take(inner.height as usize)
        .enumerate()
    {
        let selected = offset + idx == screen.nav_cursor;
        let current = match &row.node {
            NavNode::AllTeams => screen.scope == Scope::All,
            NavNode::Project(id) => screen.scope == Scope::Project(id.clone()),
            NavNode::Team(id) => screen.scope == Scope::Team(id.clone()),
            NavNode::Agent { id, .. } => screen.agent.as_deref() == Some(id.as_str()),
        };
        let indent = "  ".repeat(usize::from(row.depth));
        let fold = if row.branch {
            if row.collapsed {
                tide.glyphs.closed
            } else {
                tide.glyphs.open
            }
        } else {
            " "
        };
        let (label, suffix, suffix_style): (String, String, Style) = match &row.node {
            NavNode::AllTeams => {
                let pending = snap.pending_for(None);
                (
                    "All teams".into(),
                    if pending > 0 {
                        format!("{}{pending}", tide.glyphs.approval)
                    } else {
                        String::new()
                    },
                    Style::default().fg(tide.warn),
                )
            }
            NavNode::Project(id) => (
                snap.projects
                    .iter()
                    .find(|p| &p.id == id)
                    .map_or(id.clone(), |p| p.name.clone()),
                String::new(),
                Style::default(),
            ),
            NavNode::Team(id) => {
                let state = snap.team(id).map_or("", |t| tag(&t.status));
                let marker = if state == "active" || state.is_empty() {
                    String::new()
                } else {
                    format!("{} ", tide.glyphs.paused)
                };
                let pending = snap.pending_for(Some(id));
                (
                    snap.team_label(id),
                    format!(
                        "{marker}{}{}",
                        snap.open_work(id),
                        if pending > 0 {
                            format!(" {}{pending}", tide.glyphs.approval)
                        } else {
                            String::new()
                        }
                    ),
                    Style::default().fg(if pending > 0 { tide.warn } else { tide.muted }),
                )
            }
            NavNode::Agent { id, .. } => {
                let status = snap
                    .actor(id)
                    .map_or(AgentStatus::Idle, |a| snap.agent_status(a));
                (
                    id.clone(),
                    format!("{} {}", tide.status_glyph(status), status.word()),
                    Style::default().fg(tide.status_color(status)),
                )
            }
        };
        let width = inner.width as usize;
        let rail = if selected { tide.glyphs.rail } else { " " };
        // Leaves have nothing to fold, so they give that column back to the name.
        let head = if row.branch {
            format!("{rail}{indent}{fold} ")
        } else {
            format!("{rail}{indent}")
        };
        let suffix_room = suffix.width().min(width.saturating_sub(head.width() + 3));
        let suffix = fit(&suffix, suffix_room, tide.glyphs.ellipsis);
        let label_room = width.saturating_sub(head.width() + suffix.width() + 1);
        let label = fit(&label, label_room, tide.glyphs.ellipsis);
        let gap = width.saturating_sub(head.width() + label.width() + suffix.width());
        let mut base = Style::default().fg(if current { tide.text } else { tide.muted });
        if current {
            base = base.add_modifier(Modifier::BOLD);
        }
        let mut row_style = Style::default();
        if selected {
            row_style = row_style.bg(tide.selected_bg);
            if focused && tide.options.high_contrast {
                row_style = row_style.add_modifier(Modifier::REVERSED);
            }
        }
        let line = Line::from(vec![
            Span::styled(head, Style::default().fg(tide.border_focus)),
            Span::styled(label, base),
            Span::raw(" ".repeat(gap)),
            Span::styled(suffix, suffix_style),
        ])
        .style(row_style);
        put(buf, inner, idx as u16, line);
    }
}

// ---- main workspace -------------------------------------------------------------------------

/// The view tabs: folder-style tabs with rounded tops. The active one is open at the bottom so it
/// joins the workspace below it, and the others sit on a line that runs to the edge.
fn draw_view_tabs(buf: &mut Buffer, rect: Rect, inner: Rect, screen: &Screen, tide: &Tide) {
    let (tl, tr, side, bl, br, bar) = if tide.options.ascii {
        ("+", "+", "|", "+", "+", "-")
    } else {
        ("╭", "╮", "│", "┘", "└", "─")
    };
    let line = Style::default().fg(tide.border);
    let tabs = view_tabs(rect);
    let Some(base_y) = tabs.first().map(|(r, _)| r.y + 1) else {
        return;
    };
    if base_y >= inner.bottom() {
        return;
    }
    // The line the tabs sit on, across the whole workspace.
    for x in inner.x..inner.right() {
        buf[(x, base_y)].set_symbol(bar).set_style(line);
    }
    // ...joined to the panel's side borders.
    let (left_join, right_join) = if tide.options.ascii {
        ("+", "+")
    } else {
        ("├", "┤")
    };
    buf[(rect.x, base_y)].set_symbol(left_join).set_style(line);
    buf[(rect.right() - 1, base_y)]
        .set_symbol(right_join)
        .set_style(line);
    for (n, (tab, view)) in tabs.iter().enumerate() {
        let active = *view == screen.view;
        let label = format!(" {} {} ", n + 1, view.label());
        let label_width = label.chars().count() as u16;
        if active {
            let edge = Style::default().fg(tide.accent);
            let cap_y = tab.y - 1;
            buf[(tab.x, cap_y)].set_symbol(tl).set_style(edge);
            for x in tab.x + 1..tab.right() - 1 {
                buf[(x, cap_y)].set_symbol(bar).set_style(edge);
            }
            buf[(tab.right() - 1, cap_y)].set_symbol(tr).set_style(edge);
            buf[(tab.x, tab.y)].set_symbol(side).set_style(edge);
            buf[(tab.right() - 1, tab.y)]
                .set_symbol(side)
                .set_style(edge);
            buf.set_stringn(
                tab.x + 1,
                tab.y,
                &label,
                label_width as usize,
                Style::default()
                    .fg(tide.accent)
                    .add_modifier(Modifier::BOLD),
            );
            // Open at the bottom: the line turns up into the tab and back out.
            buf[(tab.x, base_y)].set_symbol(bl).set_style(edge);
            for x in tab.x + 1..tab.right() - 1 {
                buf[(x, base_y)].set_symbol(" ").set_style(line);
            }
            buf[(tab.right() - 1, base_y)]
                .set_symbol(br)
                .set_style(edge);
        } else {
            buf.set_stringn(
                tab.x + 1,
                tab.y,
                &label,
                label_width as usize,
                Style::default().fg(tide.muted),
            );
        }
    }
}

fn draw_main(buf: &mut Buffer, rect: Rect, screen: &Screen, tide: &Tide) {
    let focused = matches!(screen.focus, Focus::Main | Focus::Composer);
    let inner = panel(buf, rect, &screen.scope_title(), focused, tide);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    draw_view_tabs(buf, rect, inner, screen, tide);
    let body = Rect::new(
        inner.x,
        inner.y + 3,
        inner.width,
        inner.height.saturating_sub(3),
    );
    if body.height == 0 {
        return;
    }
    if !screen.loaded {
        put(
            buf,
            body,
            0,
            Line::styled("  loading teams...", Style::default().fg(tide.muted)),
        );
        return;
    }
    if screen.snapshot.teams.is_empty() {
        let lines = [
            "no teams yet",
            "press : and choose \"new team\" to build one, or describe a goal and let Teams propose one",
            "? lists every key, q closes this screen",
        ];
        for (i, text) in lines.iter().enumerate() {
            put(
                buf,
                body,
                i as u16,
                Line::styled(
                    format!(
                        "  {}",
                        fit(text, body.width as usize - 2, tide.glyphs.ellipsis)
                    ),
                    Style::default().fg(tide.muted),
                ),
            );
        }
        return;
    }
    match screen.view {
        View::Terminals => draw_terminals(buf, body, screen, tide),
        View::Chat => draw_chat(buf, body, screen, tide),
        View::Tasks => draw_tasks(buf, body, screen, tide),
        View::Decisions => draw_decisions(buf, body, screen, tide),
    }
}

fn spacing(tide: &Tide) -> u16 {
    if tide.options.density == Density::Comfortable {
        1
    } else {
        0
    }
}

fn row_style(selected: bool, tide: &Tide) -> Style {
    if selected {
        Style::default().bg(tide.selected_bg)
    } else {
        Style::default()
    }
}

fn draw_terminals(buf: &mut Buffer, body: Rect, screen: &Screen, tide: &Tide) {
    let snap = &screen.snapshot;
    // Wide enough for the longest agent id here, within reason, so ids are not cut for no cause.
    let id_width = screen
        .scope_team_ids()
        .iter()
        .flat_map(|team| snap.team_agents(team))
        .map(|actor| actor.id.width())
        .max()
        .unwrap_or(8)
        .clamp(8, 24);
    let cursor = screen.cursor_of(View::Terminals);
    let mut row = 0u16;
    let mut index = 0usize;
    let width = body.width as usize;
    for team_id in screen.scope_team_ids() {
        if matches!(screen.scope, Scope::All | Scope::Project(_)) {
            let name = fit(
                &snap.team_label(team_id),
                width.saturating_sub(2),
                tide.glyphs.ellipsis,
            );
            put(
                buf,
                body,
                row,
                Line::styled(
                    format!(" {name}"),
                    Style::default().fg(tide.muted).add_modifier(Modifier::BOLD),
                ),
            );
            row += 1;
        }
        for actor in snap.team_agents(team_id) {
            let status = snap.agent_status(actor);
            let selected = index == cursor && screen.focus == Focus::Main;
            let task = snap
                .tasks
                .iter()
                .find(|t| {
                    t.developer_id == actor.id
                        && !matches!(tag(&t.status), "finalized" | "cancelled")
                })
                .map(|t| format!("{} {}", t.id, t.title))
                .unwrap_or_default();
            let rail = if index == cursor {
                tide.glyphs.rail
            } else {
                " "
            };
            let fixed = format!(
                "{rail}{} {:<id_width$} {:<9} {:<9} ",
                tide.status_glyph(status),
                fit(&actor.id, id_width, tide.glyphs.ellipsis),
                actor.role,
                status.word()
            );
            let task = fit(
                &task,
                width.saturating_sub(fixed.width()),
                tide.glyphs.ellipsis,
            );
            let line = Line::from(vec![
                Span::styled(fixed, Style::default().fg(tide.status_color(status))),
                Span::styled(task, Style::default().fg(tide.text)),
            ])
            .style(row_style(selected, tide));
            put(buf, body, row, line);
            row += 1 + spacing(tide);
            index += 1;
            if row >= body.height {
                return;
            }
        }
    }
    if index == 0 {
        put(
            buf,
            body,
            0,
            Line::styled("  no agents in this scope", Style::default().fg(tide.muted)),
        );
    }
}

fn draw_chat(buf: &mut Buffer, body: Rect, screen: &Screen, tide: &Tide) {
    let snap = &screen.snapshot;
    let writable = matches!(screen.scope, Scope::Team(_));
    let composer_height = if writable {
        3.min(body.height)
    } else {
        1.min(body.height)
    };
    let log = Rect::new(
        body.x,
        body.y,
        body.width,
        body.height.saturating_sub(composer_height),
    );
    let messages: Vec<_> = snap
        .messages
        .iter()
        .filter(|m| screen.in_scope(&m.team_id))
        .collect();
    let width = log.width.saturating_sub(2) as usize;
    let selected_from_end = screen.cursor_of(View::Chat);
    // Build every message as blocks of lines, then show the window that ends at the selection.
    let mut blocks: Vec<Vec<Line>> = Vec::new();
    for message in &messages {
        let kind = message
            .kind
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("text");
        let label = match kind {
            "text" => String::new(),
            "task_assignment" => " assigned task".to_owned(),
            "handoff" => " handoff".to_owned(),
            "qa_decision" => " QA review".to_owned(),
            "outcome" => " outcome".to_owned(),
            "cancellation" => " cancelled".to_owned(),
            other => format!(" {other}"),
        };
        let label = match snap.audience_label(&message.audience) {
            Some(audience) => format!("{label}  {} {audience}", tide.glyphs.rail),
            None => label,
        };
        let scoped = snap.audience_label(&message.audience).is_some();
        let mut lines = vec![Line::from(vec![
            Span::styled(
                format!(" {}", message.sender_id),
                Style::default()
                    .fg(tide.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                label,
                Style::default().fg(if scoped {
                    tide.accent
                } else if kind == "text" {
                    tide.muted
                } else {
                    tide.warn
                }),
            ),
            Span::styled(
                message
                    .task_id
                    .as_ref()
                    .map(|t| format!("  [{t}]"))
                    .unwrap_or_default(),
                Style::default().fg(tide.dim),
            ),
        ])];
        for text in wrap(&message.body, width.saturating_sub(2)) {
            lines.push(Line::from(highlight_mentions(
                &format!("   {text}"),
                Style::default().fg(tide.text),
                Style::default().fg(tide.warn).add_modifier(Modifier::BOLD),
            )));
        }
        if kind == "handoff" {
            if let Some(handoff) = snap
                .handoffs
                .iter()
                .find(|h| Some(&h.task_id) == message.task_id.as_ref() && h.summary == message.body)
            {
                for item in &handoff.evidence {
                    lines.push(Line::styled(
                        format!(
                            "   {} {}: {}",
                            tide.glyphs.rail,
                            item.kind,
                            fit(
                                &item.reference,
                                width.saturating_sub(12),
                                tide.glyphs.ellipsis
                            )
                        ),
                        Style::default().fg(tide.muted),
                    ));
                }
            }
        }
        if kind == "qa_decision" {
            if let Some(decision) = message
                .kind
                .get("decision_id")
                .and_then(Value::as_str)
                .and_then(|id| snap.decisions.iter().find(|d| d.id == id))
            {
                let verdict = decision
                    .kind
                    .get("verdict")
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                lines.push(Line::styled(
                    format!("   verdict: {verdict}"),
                    Style::default().fg(if verdict == "accept" {
                        tide.good
                    } else {
                        tide.danger
                    }),
                ));
            }
        }
        if spacing(tide) > 0 {
            lines.push(Line::raw(""));
        }
        blocks.push(lines);
    }
    if screen.scope != Scope::All && messages.is_empty() {
        put(
            buf,
            log,
            0,
            Line::styled("  no messages yet", Style::default().fg(tide.muted)),
        );
    }
    // Pick the blocks that end at the selected message and fit, newest at the bottom.
    let end = blocks.len().saturating_sub(selected_from_end);
    let mut shown: Vec<&Vec<Line>> = Vec::new();
    let mut height = 0usize;
    for block in blocks[..end].iter().rev() {
        if height + block.len() > log.height as usize && !shown.is_empty() {
            break;
        }
        height += block.len();
        shown.push(block);
    }
    let mut row = log.height.saturating_sub(height as u16);
    for block in shown.iter().rev() {
        for line in block.iter() {
            put(buf, log, row, line.clone());
            row += 1;
        }
    }
    let composer = Rect::new(
        body.x,
        body.y + body.height - composer_height,
        body.width,
        composer_height,
    );
    if writable {
        let inner = panel(
            buf,
            composer,
            &format!(
                "write to {}{}",
                screen.scope_title(),
                screen
                    .composer_audience_label()
                    .map(|audience| format!("  {} {audience}", tide.glyphs.rail))
                    .unwrap_or_default()
            ),
            screen.focus == Focus::Composer,
            tide,
        );
        if inner.width > 0 {
            let text = &screen.composer;
            let room = inner.width as usize - 1;
            let tail: String = if text.width() > room {
                text.chars()
                    .rev()
                    .take(room)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect()
            } else {
                text.clone()
            };
            let (hint, color) = if text.is_empty() && screen.focus != Focus::Composer {
                (
                    "press c to write; start with @lead, @dev, @qa or @agent-id to address one"
                        .to_owned(),
                    tide.dim,
                )
            } else {
                (tail, tide.text)
            };
            let cursor = if screen.focus == Focus::Composer {
                if tide.options.ascii {
                    "_"
                } else {
                    "▏"
                }
            } else {
                ""
            };
            put(
                buf,
                inner,
                0,
                Line::from(vec![
                    Span::styled(hint, Style::default().fg(color)),
                    Span::styled(cursor.to_owned(), Style::default().fg(tide.accent)),
                ]),
            );
        }
    } else if composer_height > 0 {
        put(
            buf,
            composer,
            0,
            Line::styled(
                " select one team to write in its chat",
                Style::default().fg(tide.dim),
            ),
        );
    }
}

fn draw_tasks(buf: &mut Buffer, body: Rect, screen: &Screen, tide: &Tide) {
    let snap = &screen.snapshot;
    let tasks: Vec<_> = snap
        .tasks
        .iter()
        .filter(|t| screen.in_scope(&t.team_id))
        .collect();
    if tasks.is_empty() {
        put(
            buf,
            body,
            0,
            Line::styled("  no tasks yet", Style::default().fg(tide.muted)),
        );
        return;
    }
    let cursor = screen.cursor_of(View::Tasks);
    let width = body.width as usize;
    let mut row = 0u16;
    for (index, task) in tasks.iter().enumerate() {
        if row >= body.height {
            break;
        }
        let selected = index == cursor && screen.focus == Focus::Main;
        let (done, total, label) = task_progress(task);
        let bar: String = (0..total)
            .map(|i| {
                if i < done {
                    tide.glyphs.bar_full
                } else {
                    tide.glyphs.bar_empty
                }
            })
            .collect();
        let blocker = snap.blocked_by(task);
        let state_color = match tag(&task.status) {
            "finalized" => tide.good,
            "cancelled" => tide.muted,
            "rework_requested" => tide.danger,
            _ if blocker.is_some() => tide.danger,
            _ => tide.accent,
        };
        let rail = if index == cursor {
            tide.glyphs.rail
        } else {
            " "
        };
        let right = format!("{bar} {done}/{total} {label}");
        let left = format!("{rail} {} {}", task.id, task.title);
        let room = width.saturating_sub(right.width() + 2);
        let left = fit(&left, room, tide.glyphs.ellipsis);
        let gap = width.saturating_sub(left.width() + right.width());
        let line = Line::from(vec![
            Span::styled(left, Style::default().fg(tide.text)),
            Span::raw(" ".repeat(gap)),
            Span::styled(right, Style::default().fg(state_color)),
        ])
        .style(row_style(selected, tide));
        put(buf, body, row, line);
        row += 1;
        let mut detail = format!("   owner {}", task.developer_id);
        if !task.dependencies.is_empty() {
            detail.push_str(&format!("  needs {}", task.dependencies.join(", ")));
        }
        if let Some(blocker) = blocker {
            detail.push_str(&format!("  {} blocked by {blocker}", tide.glyphs.blocked));
        }
        if row < body.height {
            put(
                buf,
                body,
                row,
                Line::styled(
                    fit(&detail, width, tide.glyphs.ellipsis),
                    Style::default().fg(tide.muted),
                ),
            );
            row += 1;
        }
        if selected || tide.options.density == Density::Comfortable {
            if let Some(handoff) = snap.handoff_for(task) {
                for item in &handoff.evidence {
                    if row >= body.height {
                        break;
                    }
                    put(
                        buf,
                        body,
                        row,
                        Line::styled(
                            fit(
                                &format!(
                                    "   {} evidence {}: {}",
                                    tide.glyphs.rail, item.kind, item.reference
                                ),
                                width,
                                tide.glyphs.ellipsis,
                            ),
                            Style::default().fg(tide.good),
                        ),
                    );
                    row += 1;
                }
            } else if selected && row < body.height {
                put(
                    buf,
                    body,
                    row,
                    Line::styled(
                        "   no verification evidence yet",
                        Style::default().fg(tide.dim),
                    ),
                );
                row += 1;
            }
        }
        row += spacing(tide);
    }
}

fn draw_decisions(buf: &mut Buffer, body: Rect, screen: &Screen, tide: &Tide) {
    let snap = &screen.snapshot;
    let cards = screen.cards();
    if cards.is_empty() {
        put(
            buf,
            body,
            0,
            Line::styled(
                "  nothing needs a decision",
                Style::default().fg(tide.muted),
            ),
        );
        return;
    }
    let cursor = screen.cursor_of(View::Decisions);
    let width = body.width as usize;
    let owner = screen.is_owner();
    let mut row = 0u16;
    for (index, card) in cards.iter().enumerate() {
        if row >= body.height {
            break;
        }
        let selected = index == cursor && screen.focus == Focus::Main;
        let rail = if index == cursor {
            tide.glyphs.rail
        } else {
            " "
        };
        let mut lines: Vec<Line> = Vec::new();
        match card {
            Card::Approval(i) => {
                let a = &snap.approvals[*i];
                let state = tag(&a.status);
                let (glyph, color) = match state {
                    "pending" => (tide.glyphs.approval, tide.warn),
                    "approved" => (tide.glyphs.working, tide.good),
                    "used" => (tide.glyphs.idle, tide.muted),
                    _ => (tide.glyphs.blocked, tide.danger),
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{rail}{glyph} "), Style::default().fg(color)),
                    Span::styled(
                        format!("approval {}", a.id),
                        Style::default().fg(tide.text).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {}  {state}", snap.team_label(&a.team_id)),
                        Style::default().fg(color),
                    ),
                ]));
                lines.push(Line::styled(
                    format!("   action  {} -> {}", a.action.kind, a.action.target),
                    Style::default().fg(tide.text),
                ));
                lines.push(Line::styled(
                    format!("   scope   {}", compact_json(&a.action.parameters)),
                    Style::default().fg(tide.muted),
                ));
                lines.push(Line::styled(
                    format!("   why     {}  (by {})", a.justification, a.requested_by),
                    Style::default().fg(tide.muted),
                ));
                for (what, items, color) in [
                    ("else", &a.alternatives, tide.muted),
                    ("risk", &a.risks, tide.warn),
                ] {
                    for item in items {
                        lines.push(Line::styled(
                            format!("   {what:<6}  {item}"),
                            Style::default().fg(color),
                        ));
                    }
                }
                if state == "pending" && selected {
                    let (style, note) = if owner {
                        (Style::default().fg(tide.accent), "")
                    } else {
                        (Style::default().fg(tide.dim), "  (owner only)")
                    };
                    lines.push(Line::styled(
                        format!("   [a] approve   [d] deny{note}"),
                        style,
                    ));
                } else if state == "approved" && selected && owner {
                    lines.push(Line::styled(
                        "   [v] revoke",
                        Style::default().fg(tide.accent),
                    ));
                }
            }
            Card::Conflict(i) => {
                let c = &snap.conflicts[*i];
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{rail}{} ", tide.glyphs.blocked),
                        Style::default().fg(tide.danger),
                    ),
                    Span::styled(
                        format!("conflict {}", c.kind),
                        Style::default().fg(tide.text).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {}", tag(&c.status)),
                        Style::default().fg(tide.danger),
                    ),
                ]));
                lines.push(Line::styled(
                    format!(
                        "   between {}",
                        c.team_ids
                            .iter()
                            .map(|t| snap.team_label(t))
                            .collect::<Vec<_>>()
                            .join(" / ")
                    ),
                    Style::default().fg(tide.text),
                ));
                lines.push(Line::styled(
                    format!("   detail  {}", c.detail.join(", ")),
                    Style::default().fg(tide.muted),
                ));
                if selected {
                    let (style, note) = if owner {
                        (Style::default().fg(tide.accent), "")
                    } else {
                        (Style::default().fg(tide.dim), "  (owner only)")
                    };
                    lines.push(Line::styled(format!("   [x] resolve{note}"), style));
                }
            }
            Card::Decision(i) => {
                let d = &snap.decisions[*i];
                let kind = d
                    .kind
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("decision");
                let verdict = d.kind.get("verdict").and_then(Value::as_str).unwrap_or("");
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{rail}{} ", tide.glyphs.idle),
                        Style::default().fg(tide.muted),
                    ),
                    Span::styled(
                        format!("{kind} {verdict}").trim().to_owned(),
                        Style::default().fg(tide.text).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {} on {}", d.actor_id, d.task_id),
                        Style::default().fg(tide.muted),
                    ),
                ]));
                lines.push(Line::styled(
                    format!("   {}", d.body),
                    Style::default().fg(tide.muted),
                ));
            }
        }
        for line in lines {
            if row >= body.height {
                break;
            }
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let clipped = if text.width() > width {
                Line::styled(
                    fit(&text, width, tide.glyphs.ellipsis),
                    line.spans.first().map_or(Style::default(), |s| s.style),
                )
            } else {
                line
            };
            put(buf, body, row, clipped.style(row_style(selected, tide)));
            row += 1;
        }
        row += 1;
    }
}

// ---- inspector ------------------------------------------------------------------------------

fn draw_inspector(buf: &mut Buffer, rect: Rect, screen: &Screen, tide: &Tide) {
    let snap = &screen.snapshot;
    let focused = screen.focus == Focus::Inspector;
    let agent = screen.agent.as_ref().and_then(|id| snap.actor(id));
    let title = agent.map_or("inspector".to_owned(), |a| a.id.clone());
    let inner = panel(buf, rect, &title, focused, tide);
    if inner.width == 0 || !screen.loaded {
        return;
    }
    let width = inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    let kv = |k: &str, v: String, color| {
        Line::from(vec![
            Span::styled(format!("{k:<11}"), Style::default().fg(tide.dim)),
            Span::styled(
                fit(&v, width.saturating_sub(11), tide.glyphs.ellipsis),
                Style::default().fg(color),
            ),
        ])
    };
    if let Some(actor) = agent {
        let status = snap.agent_status(actor);
        lines.push(kv("role", actor.role.clone(), tide.text));
        lines.push(kv(
            "status",
            format!("{} {}", tide.status_glyph(status), status.word()),
            tide.status_color(status),
        ));
        match snap.binding(&actor.id) {
            Some(binding) => {
                lines.push(kv(
                    "provider",
                    binding
                        .provider
                        .clone()
                        .unwrap_or_else(|| "not reported".into()),
                    tide.text,
                ));
                lines.push(kv("pane", binding.pane_id.clone(), tide.text));
            }
            None => {
                lines.push(kv("provider", "not reported".into(), tide.dim));
                lines.push(kv("pane", "none bound (b)".into(), tide.dim));
            }
        }
        lines.push(kv(
            "run",
            actor.run_generation.map_or("-".into(), |g| format!("#{g}")),
            tide.muted,
        ));
        if let Some(team) = &actor.team_id {
            lines.push(kv("team", snap.team_label(team), tide.text));
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "tasks",
            Style::default().fg(tide.muted).add_modifier(Modifier::BOLD),
        ));
        let owned: Vec<_> = snap
            .tasks
            .iter()
            .filter(|t| t.developer_id == actor.id)
            .collect();
        if owned.is_empty() {
            lines.push(Line::styled("none", Style::default().fg(tide.dim)));
        }
        for task in owned {
            let (done, total, label) = task_progress(task);
            lines.push(Line::styled(
                fit(
                    &format!("{} {done}/{total} {label}", task.id),
                    width,
                    tide.glyphs.ellipsis,
                ),
                Style::default().fg(tide.text),
            ));
            if let Some(blocker) = snap.blocked_by(task) {
                lines.push(Line::styled(
                    fit(
                        &format!("{} blocked by {blocker}", tide.glyphs.blocked),
                        width,
                        tide.glyphs.ellipsis,
                    ),
                    Style::default().fg(tide.danger),
                ));
            }
        }
    } else {
        lines.push(kv("scope", screen.scope_title(), tide.text));
        let ids = screen.scope_team_ids();
        lines.push(kv("teams", ids.len().to_string(), tide.text));
        let open: usize = ids.iter().map(|id| snap.open_work(id)).sum();
        lines.push(kv("open work", open.to_string(), tide.text));
        if let Scope::Team(id) = &screen.scope {
            if let Some(team) = snap.team(id) {
                lines.push(kv(
                    "state",
                    tag(&team.status).to_owned(),
                    if tag(&team.status) == "active" {
                        tide.good
                    } else {
                        tide.warn
                    },
                ));
                for (role, who) in [
                    ("lead", &team.lead_id),
                    ("dev", &team.developer_id),
                    ("qa", &team.qa_id),
                ] {
                    let status = snap
                        .actor(who)
                        .map_or(AgentStatus::Idle, |a| snap.agent_status(a));
                    lines.push(Line::from(vec![
                        Span::styled(format!("{role:<11}"), Style::default().fg(tide.dim)),
                        Span::styled(
                            format!("{} ", tide.status_glyph(status)),
                            Style::default().fg(tide.status_color(status)),
                        ),
                        Span::styled(
                            fit(who, width.saturating_sub(11), tide.glyphs.ellipsis),
                            Style::default().fg(tide.text),
                        ),
                    ]));
                }
            }
        }
    }
    // Blockers: what is holding this scope up, always as words.
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "blockers",
        Style::default().fg(tide.muted).add_modifier(Modifier::BOLD),
    ));
    let mut blockers = Vec::new();
    for approval in snap
        .approvals
        .iter()
        .filter(|a| tag(&a.status) == "pending" && screen.in_scope(&a.team_id))
    {
        blockers.push((
            tide.glyphs.approval,
            tide.warn,
            format!("awaiting owner: {}", approval.id),
        ));
    }
    for conflict in snap
        .conflicts
        .iter()
        .filter(|c| tag(&c.status) == "open" && c.team_ids.iter().any(|t| screen.in_scope(t)))
    {
        blockers.push((
            tide.glyphs.blocked,
            tide.danger,
            format!("conflict: {}", conflict.kind),
        ));
    }
    for notice in snap.notices.iter().filter(|n| {
        tag(&n.status) == "open"
            && (screen.in_scope(&n.to_team_id) || screen.in_scope(&n.from_team_id))
    }) {
        blockers.push((
            tide.glyphs.waiting,
            tide.warn,
            format!(
                "notice {} -> {}",
                snap.team_label(&notice.from_team_id),
                snap.team_label(&notice.to_team_id)
            ),
        ));
    }
    for recruitment in snap
        .recruitments
        .iter()
        .filter(|r| tag(&r.status) == "queued" && screen.in_scope(&r.team_id))
    {
        blockers.push((
            tide.glyphs.waiting,
            tide.muted,
            format!("{} specialist queued", recruitment.specialty),
        ));
    }
    for team in snap
        .teams
        .iter()
        .filter(|t| screen.in_scope(&t.id) && tag(&t.status) != "active")
    {
        blockers.push((
            tide.glyphs.paused,
            tide.warn,
            format!("{} is {}", team.name, tag(&team.status)),
        ));
    }
    if blockers.is_empty() {
        lines.push(Line::styled("none", Style::default().fg(tide.good)));
    }
    for (glyph, color, text) in blockers {
        lines.push(Line::styled(
            fit(&format!("{glyph} {text}"), width, tide.glyphs.ellipsis),
            Style::default().fg(color),
        ));
    }
    for (row, line) in lines.into_iter().enumerate() {
        put(buf, inner, row as u16, line);
    }
}

// ---- overlays -------------------------------------------------------------------------------

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn draw_modal(buf: &mut Buffer, area: Rect, screen: &Screen, tide: &Tide) {
    let Some(modal) = &screen.modal else { return };
    let width = 70.min(area.width.saturating_sub(2)).max(20);
    let rect = centered(area, width, (modal.lines.len() as u16 + 7).min(area.height));
    Clear.render(rect, buf);
    let inner = panel(buf, rect, &modal.title, true, tide);
    if inner.width == 0 {
        return;
    }
    let room = inner.width as usize;
    let mut row = 0u16;
    for text in &modal.lines {
        for part in wrap(text, room) {
            put(
                buf,
                inner,
                row,
                Line::styled(part, Style::default().fg(tide.text)),
            );
            row += 1;
        }
    }
    row += 1;
    let label = match modal.action {
        super::state::Action::Bind => "pane",
        super::state::Action::Rename => "name",
        super::state::Action::Goal => "goal",
        _ => "reason",
    };
    let cursor = if tide.options.ascii { "_" } else { "▏" };
    put(
        buf,
        inner,
        row,
        Line::from(vec![
            Span::styled(format!("{label}: "), Style::default().fg(tide.dim)),
            Span::styled(
                fit(
                    &format!("{}{cursor}", modal.input),
                    room.saturating_sub(label.len() + 2),
                    tide.glyphs.ellipsis,
                ),
                Style::default().fg(tide.text),
            ),
        ]),
    );
    put(
        buf,
        inner,
        row + 1,
        Line::styled(
            "enter confirm   esc cancel",
            Style::default().fg(tide.muted),
        ),
    );
}

fn draw_help(buf: &mut Buffer, area: Rect, tide: &Tide) {
    let lines = [
        (
            "tab / shift-tab",
            "move between navigator, workspace and inspector",
        ),
        ("j k / arrows", "move; h l fold and unfold in the navigator"),
        ("enter", "open the row; on an agent, show its terminal"),
        ("1 2 3 4  [ ]", "terminals, chat, tasks, decisions"),
        ("c", "write in the team chat (enter sends, esc leaves)"),
        ("a d v x", "decisions: approve, deny, revoke, resolve"),
        ("n  i", "show or hide the navigator and the inspector"),
        ("D  A  H", "density, ascii glyphs, high-contrast focus"),
        ("r  q", "refresh now; quit (sessions keep running)"),
    ];
    let rect = centered(
        area,
        78.min(area.width),
        (lines.len() as u16 + 4).min(area.height),
    );
    Clear.render(rect, buf);
    let inner = panel(buf, rect, "keys", true, tide);
    for (row, (keys, what)) in lines.iter().enumerate() {
        put(
            buf,
            inner,
            row as u16,
            Line::from(vec![
                Span::styled(format!("{keys:<17}"), Style::default().fg(tide.accent)),
                Span::styled(
                    fit(
                        what,
                        (inner.width as usize).saturating_sub(17),
                        tide.glyphs.ellipsis,
                    ),
                    Style::default().fg(tide.text),
                ),
            ]),
        );
    }
    put(
        buf,
        inner,
        lines.len() as u16 + 1,
        Line::styled("any key closes", Style::default().fg(tide.dim)),
    );
}

fn draw_toast(buf: &mut Buffer, area: Rect, text: &str, tide: &Tide) {
    if area.height < 3 {
        return;
    }
    let text = fit(
        &format!(" {text} "),
        area.width as usize,
        tide.glyphs.ellipsis,
    );
    let rect = Rect::new(area.x, area.y + area.height - 2, area.width, 1);
    put(
        buf,
        rect,
        0,
        Line::styled(
            pad(&text, area.width as usize),
            Style::default().fg(tide.bg).bg(tide.warn),
        ),
    );
}

fn draw_builder(buf: &mut Buffer, area: Rect, builder: &super::state::Builder, tide: &Tide) {
    use super::state::{BuilderStep, BUILDER_FIELDS};
    let draft = &builder.draft;
    let rect = centered(area, 78.min(area.width), 17.min(area.height));
    Clear.render(rect, buf);
    let title = match builder.step {
        BuilderStep::Edit => "new team",
        BuilderStep::Review => "review team - nothing is created until you launch",
    };
    let inner = panel(buf, rect, title, true, tide);
    if inner.width == 0 {
        return;
    }
    let width = inner.width as usize;
    let cursor = if tide.options.ascii { "_" } else { "▏" };
    put(
        buf,
        inner,
        0,
        Line::styled(
            format!("source: {}", draft.source),
            Style::default().fg(tide.dim),
        ),
    );
    match builder.step {
        BuilderStep::Edit => {
            let values = [
                draft.name.clone(),
                if draft.project_id.is_empty() {
                    "< a new default project >".to_owned()
                } else {
                    format!("< {} >", draft.project_id)
                },
                draft.lead.clone(),
                draft.developer.clone(),
                draft.qa.clone(),
            ];
            for (index, (label, value)) in BUILDER_FIELDS.iter().zip(values).enumerate() {
                let selected = index == builder.field;
                let rail = if selected { tide.glyphs.rail } else { " " };
                let shown = if selected && index != 1 {
                    format!("{value}{cursor}")
                } else {
                    value
                };
                let line = Line::from(vec![
                    Span::styled(
                        format!("{rail} {label:<13}"),
                        Style::default().fg(if selected { tide.accent } else { tide.muted }),
                    ),
                    Span::styled(
                        fit(&shown, width.saturating_sub(16), tide.glyphs.ellipsis),
                        Style::default().fg(tide.text),
                    ),
                ])
                .style(row_style(selected, tide));
                put(buf, inner, 2 + index as u16, line);
            }
            let (text, color) = match (&builder.preview, builder.checking) {
                (_, true) => (
                    "asking the server to check the draft...".to_owned(),
                    tide.muted,
                ),
                (Some(preview), false) if !preview.valid => (
                    preview
                        .problems
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "the draft is not valid".to_owned()),
                    tide.warn,
                ),
                _ => (
                    "enter on the last field has the server check it".to_owned(),
                    tide.dim,
                ),
            };
            put(
                buf,
                inner,
                8,
                Line::styled(
                    fit(&text, width, tide.glyphs.ellipsis),
                    Style::default().fg(color),
                ),
            );
            put(
                buf,
                inner,
                10,
                Line::styled(
                    "left/right changes the project",
                    Style::default().fg(tide.dim),
                ),
            );
        }
        BuilderStep::Review => {
            let team_id = builder
                .preview
                .as_ref()
                .map_or_else(String::new, |p| p.team_id.clone());
            let credential_dir = builder
                .preview
                .as_ref()
                .map_or_else(String::new, |p| p.credential_dir.clone());
            let rows = [
                ("name", draft.name.clone()),
                ("team id", team_id.clone()),
                (
                    "project",
                    if draft.project_id.is_empty() {
                        "default (created)".to_owned()
                    } else {
                        draft.project_id.clone()
                    },
                ),
                ("lead", draft.lead.clone()),
                ("developer", draft.developer.clone()),
                ("qa", draft.qa.clone()),
                (
                    "keys",
                    format!("{credential_dir}/ in the session data dir, mode 0700"),
                ),
            ];
            for (index, (label, value)) in rows.iter().enumerate() {
                put(
                    buf,
                    inner,
                    2 + index as u16,
                    Line::from(vec![
                        Span::styled(format!("  {label:<10}"), Style::default().fg(tide.dim)),
                        Span::styled(
                            fit(value, width.saturating_sub(12), tide.glyphs.ellipsis),
                            Style::default().fg(tide.text),
                        ),
                    ]),
                );
            }
            let note = "Launching creates 3 credentials, the team spec, enrolls the agents and starts the team.";
            for (offset, part) in wrap(note, width).into_iter().enumerate() {
                put(
                    buf,
                    inner,
                    10 + offset as u16,
                    Line::styled(part, Style::default().fg(tide.muted)),
                );
            }
        }
    }
}

fn draw_palette(buf: &mut Buffer, area: Rect, screen: &Screen, tide: &Tide) {
    let Some(state) = &screen.palette else { return };
    let matches = screen.palette_matches();
    let rows = matches.len().min(12);
    let rect = centered(area, 64.min(area.width), (rows as u16 + 5).min(area.height));
    Clear.render(rect, buf);
    let inner = panel(buf, rect, "commands", true, tide);
    if inner.width == 0 {
        return;
    }
    let cursor = if tide.options.ascii { "_" } else { "▏" };
    put(
        buf,
        inner,
        0,
        Line::from(vec![
            Span::styled("> ", Style::default().fg(tide.accent)),
            Span::styled(
                format!("{}{cursor}", state.query),
                Style::default().fg(tide.text),
            ),
        ]),
    );
    if matches.is_empty() {
        put(
            buf,
            inner,
            2,
            Line::styled("no command matches", Style::default().fg(tide.muted)),
        );
        return;
    }
    let selected = state.cursor.min(matches.len() - 1);
    // Keep the highlighted row on screen when there are more matches than rows.
    let first = selected.saturating_sub(rows.saturating_sub(1));
    for (offset, (label, _)) in matches.iter().skip(first).take(rows).enumerate() {
        let is_selected = first + offset == selected;
        let rail = if is_selected { tide.glyphs.rail } else { " " };
        let line = Line::styled(
            fit(
                &format!("{rail} {label}"),
                inner.width as usize,
                tide.glyphs.ellipsis,
            ),
            Style::default().fg(if is_selected { tide.text } else { tide.muted }),
        )
        .style(row_style(is_selected, tide));
        put(buf, inner, 2 + offset as u16, line);
    }
}
