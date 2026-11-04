//! Rendering. Everything here is a pure function of `App`; it never talks to
//! the network.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::*;
use crate::state::*;
use crate::types::*;

const DIM: Style = Style::new().fg(Color::DarkGray);
const ACCENT: Color = Color::Cyan;

pub fn draw(f: &mut Frame, app: &mut App) {
    let [header, body, status, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(1), Constraint::Length(1)]).areas(f.area());
    draw_header(f, app, header);
    match app.screen {
        Screen::List => draw_list(f, app, body),
        Screen::Detail => draw_detail(f, app, body),
    }
    draw_status(f, app, status);
    draw_footer(f, app, footer);
    draw_modal(f, app, body);
}

// ---------- text helpers ----------

/// Word-wraps `text` to `width` columns, keeping explicit newlines.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.trim_end();
        if raw.is_empty() {
            out.push(String::new());
            continue;
        }
        let indent: String = raw.chars().take_while(|c| *c == ' ').collect();
        let mut line = String::new();
        for word in raw.split_whitespace() {
            // Break words longer than a whole line.
            let mut word = word.to_string();
            while word.width() > width {
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                }
                let mut cut = String::new();
                for c in word.chars() {
                    if cut.width() + c.to_string().width() > width {
                        break;
                    }
                    cut.push(c);
                }
                word = word[cut.len()..].to_string();
                out.push(cut);
            }
            let sep = if line.is_empty() { 0 } else { 1 };
            if !line.is_empty() && line.width() + sep + word.width() > width {
                out.push(std::mem::take(&mut line));
            }
            if line.is_empty() {
                line.push_str(&indent);
                if indent.len() + word.width() > width {
                    line.clear();
                }
            } else {
                line.push(' ');
            }
            line.push_str(&word);
        }
        out.push(line);
    }
    out
}

pub fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        if out.width() + c.to_string().width() + 1 > width {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

fn status_style(status: &str) -> Style {
    let c = match status {
        "inbox" => Color::White,
        "grilling" => Color::Yellow,
        "review" => Color::Magenta,
        "ready" | "dispatching" => Color::Blue,
        "dispatched" => Color::Green,
        "failed" => Color::Red,
        _ => Color::DarkGray,
    };
    Style::new().fg(c)
}

fn deploy_pill(d: &Deploy) -> Span<'static> {
    if d.ok() {
        Span::styled(" Deployed ", Style::new().fg(Color::Black).bg(Color::Green))
    } else {
        Span::styled(" Deploy failed ", Style::new().fg(Color::White).bg(Color::Red).bold())
    }
}

fn key_span(k: &str) -> Span<'static> {
    Span::styled(k.to_string(), Style::new().fg(ACCENT).bold())
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

// ---------- chrome ----------

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::styled(" ideas ", Style::new().fg(Color::Black).bg(ACCENT).bold()), Span::raw(" ")];
    match (app.screen, &app.current) {
        (Screen::Detail, Some(i)) => {
            spans.push(Span::styled(display_title(i), Style::new().bold()));
        }
        _ => {
            for fl in Filter::ORDER {
                let n = app.ideas.iter().filter(|i| fl.matches(i)).count();
                let label = format!(" {} {} ", fl.label(), n);
                spans.push(if fl == app.filter {
                    Span::styled(label, Style::new().fg(Color::Black).bg(Color::White))
                } else {
                    Span::styled(label, DIM)
                });
            }
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let line = if let Some(s) = &app.status {
        let style = if s.error { Style::new().fg(Color::Red).bold() } else { Style::new().fg(Color::Green) };
        Line::from(Span::styled(format!(" {}", s.text), style))
    } else if let Some(label) = app.current.as_ref().filter(|_| app.screen == Screen::Detail).and_then(|i| app.busy_label(i)) {
        Line::from(Span::styled(format!(" {} {}", app.spinner(), label), Style::new().fg(Color::Yellow)))
    } else if !app.loaded {
        Line::from(Span::styled(format!(" {} Loading…", app.spinner()), DIM))
    } else {
        Line::default()
    };
    f.render_widget(Paragraph::new(line), area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::raw(" ")];
    for (k, label) in hints(app) {
        spans.push(key_span(k));
        spans.push(Span::styled(format!(" {label}  "), DIM));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ---------- list ----------

fn draw_list(f: &mut Frame, app: &mut App, area: Rect) {
    let width = area.width as usize;
    let items: Vec<ListItem> = app
        .visible()
        .iter()
        .map(|i| {
            let pill = i.deploy.as_ref().map(deploy_pill);
            let status = format!("{:<11}", i.status);
            let busy = if app.busy_label(i).is_some() { format!("{} ", app.spinner()) } else { String::new() };
            let right = status.width() + pill.as_ref().map_or(0, |p| p.width() + 1) + busy.width() + 4;
            let title = truncate(&display_title(i), width.saturating_sub(right));
            let pad = width.saturating_sub(right + title.width());
            let mut spans = vec![Span::raw(" "), Span::raw(title), Span::raw(" ".repeat(pad + 1))];
            spans.push(Span::styled(busy, Style::new().fg(Color::Yellow)));
            spans.push(Span::styled(status, status_style(&i.status)));
            if let Some(p) = pill {
                spans.push(Span::raw(" "));
                spans.push(p);
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    if items.is_empty() {
        let msg = if !app.loaded {
            "Loading…".to_string()
        } else if app.filter == Filter::Inbox || app.filter == Filter::Open {
            "Nothing here. Press n to jot down an idea.".to_string()
        } else {
            "Nothing here.".to_string()
        };
        f.render_widget(Paragraph::new(msg).style(DIM).alignment(Alignment::Center), centered(area, area.width, 1));
        return;
    }
    let list = List::new(items).highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, area, &mut app.list_state);
}

// ---------- detail ----------

struct Doc {
    width: usize,
    lines: Vec<Line<'static>>,
    thread_anchor: Option<usize>,
}

impl Doc {
    fn blank(&mut self) {
        self.lines.push(Line::default());
    }
    fn heading(&mut self, text: &str) {
        if !self.lines.is_empty() {
            self.blank();
        }
        self.lines.push(Line::from(Span::styled(text.to_uppercase(), Style::new().fg(ACCENT).bold())));
    }
    fn para(&mut self, text: &str, indent: usize, style: Style) {
        for l in wrap(text, self.width.saturating_sub(indent)) {
            self.lines.push(Line::from(Span::styled(format!("{}{}", " ".repeat(indent), l), style)));
        }
    }
    fn kv(&mut self, key: &str, value: &str) {
        let prefix = format!("{key:<10}");
        let wrapped = wrap(value, self.width.saturating_sub(prefix.len() + 1));
        for (n, l) in wrapped.into_iter().enumerate() {
            let k = if n == 0 { prefix.clone() } else { " ".repeat(prefix.len()) };
            self.lines.push(Line::from(vec![Span::styled(k, DIM), Span::raw(" "), Span::raw(l)]));
        }
    }
}

fn build_doc(app: &App, idea: &Idea, width: usize) -> Doc {
    let mut d = Doc { width, lines: vec![], thread_anchor: None };

    let mut head = vec![Span::styled(format!(" {} ", idea.status), status_style(&idea.status).add_modifier(Modifier::REVERSED))];
    if let Some(dep) = &idea.deploy {
        head.push(Span::raw(" "));
        head.push(deploy_pill(dep));
    }
    if let Some(at) = &idea.created_at {
        head.push(Span::styled(format!("  captured {}", short_time(at)), DIM));
    }
    d.lines.push(Line::from(head));
    let hint = next_step(idea);
    if !hint.is_empty() {
        d.blank();
        d.para(&format!("▶ {hint}"), 0, Style::new().fg(Color::Yellow));
    }

    d.heading("Idea");
    d.para(&idea.text, 0, Style::new());

    if let Some(disp) = &idea.dispatch {
        d.heading("Dispatch");
        let dest = disp.destination.as_deref().unwrap_or("laptop");
        d.kv("To", &match (dest, disp.mode.as_deref()) {
            ("droplet", Some(m)) => format!("droplet ({m})"),
            (dest, _) => dest.to_string(),
        });
        if let (Some(w), Some(t)) = (&disp.workspace_label, &disp.tab_label) {
            d.kv("Herdr", &format!("workspace {w}, tab {t}"));
        }
        if let Some(a) = &disp.agent {
            d.kv("Agent", a);
        }
        if let Some(n) = &disp.note {
            d.kv("Note", n);
        }
        if let Some(e) = disp.error.as_deref().filter(|_| idea.status == "failed") {
            d.lines.push(Line::from(Span::styled(format!("Dispatch failed: {e}"), Style::new().fg(Color::Red).bold())));
        }
    }

    if let Some(job) = &idea.job {
        draw_job_doc(&mut d, job);
    }

    if let Some(dep) = &idea.deploy {
        d.heading("Deploy");
        let (text, style) = if dep.ok() {
            ("Deployed", Style::new().fg(Color::Green).bold())
        } else {
            ("Deploy failed (rolled back)", Style::new().fg(Color::Red).bold())
        };
        let mut meta = text.to_string();
        if let Some(c) = &dep.commit {
            meta.push_str(&format!("  {}", c.chars().take(7).collect::<String>()));
        }
        if let Some(at) = &dep.at {
            meta.push_str(&format!("  {}", short_time(at)));
        }
        d.lines.push(Line::from(Span::styled(meta, style)));
        if let Some(m) = &dep.message {
            d.para(m, 0, Style::new());
        }
        if let Some(log) = dep.log.as_deref().filter(|l| !l.trim().is_empty()) {
            let tail: Vec<&str> = log.lines().collect();
            let tail = &tail[tail.len().saturating_sub(25)..];
            if app.detail.show_log {
                d.lines.push(Line::from(Span::styled("▾ log tail (l to collapse)", DIM)));
                for l in tail {
                    for w in wrap(l, width.saturating_sub(2)) {
                        d.lines.push(Line::from(Span::styled(format!("  {w}"), DIM)));
                    }
                }
            } else {
                d.lines.push(Line::from(Span::styled(format!("▸ log tail, {} lines (l to expand)", tail.len()), DIM)));
            }
        }
    }

    if let Some(h) = app.effective_handoff(idea) {
        d.heading("Handoff");
        if app.drafts.contains_key(&idea.id) {
            d.lines.push(Line::from(Span::styled("edited locally, goes to the server when you send", Style::new().fg(Color::Yellow))));
        }
        d.kv("Title", &h.title);
        let new = app.is_new_project(&h);
        d.kv("Project", &format!("{}{}", h.project, if new { "  (new project)" } else { "" }));
        if new {
            d.kv("Repo", if h.visibility == "public" { "public" } else { "private" });
        }
        d.blank();
        d.para(&h.brief, 0, Style::new());
    }

    if !idea.messages.is_empty() {
        d.heading("Grill");
        d.thread_anchor = Some(d.lines.len());
        let last = idea.messages.len() - 1;
        for (n, m) in idea.messages.iter().enumerate() {
            let (who, color) = if m.role == "me" { ("You", Color::Cyan) } else { ("Claude", Color::Yellow) };
            d.blank();
            d.lines.push(Line::from(Span::styled(who, Style::new().fg(color).bold())));
            if !m.text.is_empty() {
                d.para(&m.text, 2, Style::new());
            }
            if let Some(qs) = &m.questions {
                for (qn, q) in qs.iter().enumerate() {
                    d.para(&format!("{}. {}{}", qn + 1, q.text, if q.is_multi() { " (pick any)" } else { "" }), 2, Style::new().bold());
                    d.para(&q.options.join(" · "), 5, DIM);
                }
                if n == last && open_turn(idea).is_some() && can_chat(idea) {
                    d.lines.push(Line::from(Span::styled("  ◆ waiting for your answers (a)", Style::new().fg(Color::Yellow))));
                }
            }
            if n == last && m.done == Some(true) {
                d.lines.push(Line::from(Span::styled("  ✓ Claude says that's enough (w to write the handoff)", Style::new().fg(Color::Green))));
            }
        }
    }
    d
}

fn draw_job_doc(d: &mut Doc, job: &Job) {
    d.heading(&format!("Droplet job{}", job.mode.as_deref().map(|m| format!(" ({m})")).unwrap_or_default()));
    let tone = match job.status.as_str() {
        "live" | "preview" => Color::Green,
        "failed" | "rejected" => Color::Red,
        _ => Color::Yellow,
    };
    let busy = if job_busy(job) { " …" } else { "" };
    let mut label = format!("{}{busy}", job_label(&job.status));
    if let Some(a) = &job.action {
        label.push_str(&format!(" · {} requested", a.kind));
    }
    if let Some(id) = job.id {
        label.push_str(&format!(" · change {id}"));
    }
    d.lines.push(Line::from(Span::styled(label, Style::new().fg(tone).bold())));
    if let Some(n) = job.note.as_deref().filter(|n| !n.is_empty()) {
        d.para(n, 0, Style::new());
    }
    if job.status == "preview"
        && let Some(u) = &job.preview_url
    {
        d.kv("Preview", u);
    }
    if let Some(b) = &job.branch {
        let c = job.commit.as_deref().map(|c| format!(" · {}", c.chars().take(7).collect::<String>())).unwrap_or_default();
        d.kv("Branch", &format!("{b}{c}"));
    }
    if let Some(t) = &job.tests {
        let ok = match t.ok {
            Some(true) => "passed",
            Some(false) => "failed",
            None => "?",
        };
        d.kv("Tests", &format!("{ok} {}", t.summary.as_deref().unwrap_or("")));
    }
    let acts = job_actions(job);
    if !acts.is_empty() {
        let names: Vec<String> = acts
            .iter()
            .map(|a| match a {
                JobAction::Ship => "S ship it live".to_string(),
                JobAction::Discard => "X discard".to_string(),
                JobAction::Rollback => "B roll back".to_string(),
            })
            .collect();
        d.lines.push(Line::from(vec![Span::styled("Actions: ", DIM), Span::styled(names.join("   "), Style::new().fg(ACCENT).bold())]));
    }
    let skip = job.events.len().saturating_sub(4);
    for e in job.events.iter().skip(skip) {
        let line = format!("{} {} {}", e.at.as_deref().map(short_time).unwrap_or_default(), e.status.as_deref().unwrap_or(""), e.message.as_deref().unwrap_or(""));
        d.lines.push(Line::from(Span::styled(truncate(line.trim(), d.width), DIM)));
    }
}

/// `2026-11-04T16:33:12.123Z` -> `2026-11-04 16:33`.
fn short_time(s: &str) -> String {
    let mut it = s.splitn(2, 'T');
    match (it.next(), it.next()) {
        (Some(d), Some(t)) => format!("{d} {}", t.chars().take(5).collect::<String>()),
        _ => s.to_string(),
    }
}

fn draw_detail(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(idea) = app.current.clone() else { return };
    let doc = build_doc(app, &idea, (area.width as usize).saturating_sub(2));
    let h = area.height as usize;
    if app.detail.jump_to_thread {
        app.detail.jump_to_thread = false;
        if let Some(a) = doc.thread_anchor {
            app.detail.scroll = a.saturating_sub(1);
        }
    }
    let max = doc.lines.len().saturating_sub(h);
    app.detail.scroll = app.detail.scroll.min(max);
    let start = app.detail.scroll;
    let lines: Vec<Line> = doc.lines.iter().skip(start).take(h).cloned().collect();
    let [text, bar] = Layout::horizontal([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    let text = Rect { x: text.x + 1, width: text.width.saturating_sub(1), ..text };
    f.render_widget(Paragraph::new(lines), text);
    if max > 0 {
        let pos = (start * h.saturating_sub(1)) / max.max(1);
        let thumb: Vec<Line> = (0..h).map(|r| Line::from(if r == pos { "█" } else { "│" })).collect();
        f.render_widget(Paragraph::new(thumb).style(DIM), bar);
    }
}

// ---------- modals ----------

fn popup_block(title: &str) -> Block<'static> {
    Block::new()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(ACCENT))
        .title(Span::styled(format!(" {title} "), Style::new().bold()))
}

fn draw_modal(f: &mut Frame, app: &mut App, body: Rect) {
    let spinner = app.spinner();
    let capture_busy = app.capture_busy;
    let projects = app.projects.clone();
    let area = f.area();
    match &mut app.modal {
        Modal::None => {}
        Modal::Help => draw_help(f, area),
        Modal::Capture(ta) => {
            let r = centered(area, area.width.saturating_sub(8).min(100), body.height.saturating_sub(2).clamp(6, 20));
            f.render_widget(Clear, r);
            let title = if capture_busy { format!("New idea  {spinner} saving…") } else { "New idea (Ctrl+S to save)".to_string() };
            let block = popup_block(&title);
            let inner = block.inner(r);
            f.render_widget(block, r);
            ta.set_cursor_style(Style::new().add_modifier(Modifier::REVERSED));
            f.render_widget(&**ta, inner);
        }
        Modal::Reply { area: ta, .. } => {
            let r = centered(area, area.width.saturating_sub(8).min(100), 10.min(area.height));
            f.render_widget(Clear, r);
            let block = popup_block("Reply to Claude (Ctrl+S to send)");
            let inner = block.inner(r);
            f.render_widget(block, r);
            ta.set_cursor_style(Style::new().add_modifier(Modifier::REVERSED));
            f.render_widget(&**ta, inner);
        }
        Modal::Answer(a) => draw_answer(f, a, area),
        Modal::Handoff(h) => draw_handoff_editor(f, h, &projects, area),
        Modal::Send(s) => draw_send(f, s, area),
        Modal::Confirm(c) => {
            let w = area.width.saturating_sub(10).clamp(30, 70);
            let lines = wrap(&c.prompt, w as usize - 4);
            let r = centered(area, w, lines.len() as u16 + 4);
            f.render_widget(Clear, r);
            let mut text: Vec<Line> = lines.into_iter().map(Line::from).collect();
            text.push(Line::default());
            text.push(Line::from(vec![key_span("y"), Span::raw(" yes    "), key_span("n"), Span::raw(" no")]));
            f.render_widget(Paragraph::new(text).block(popup_block("Confirm").border_style(Style::new().fg(Color::Yellow))), r);
        }
    }
}

fn draw_help(f: &mut Frame, area: Rect) {
    let rows: &[(&str, &str)] = &[
        ("Lists", ""),
        ("j/k ↑/↓", "move"),
        ("Enter / l", "open idea"),
        ("n", "new idea (Ctrl+S saves)"),
        ("Tab / S-Tab", "cycle filter (Open, Inbox, Grilling, Sent, Archived, All)"),
        ("A", "toggle archived"),
        ("r", "refresh"),
        ("q", "quit"),
        ("An idea", ""),
        ("j/k Space", "scroll (G end, Home top)"),
        ("g", "grill me (inbox)"),
        ("a", "answer Claude's questions (Space selects, Tab next, e explains)"),
        ("t", "type a free-text reply"),
        ("C", "cancel the grill"),
        ("w", "write handoff"),
        ("e", "edit handoff (Tab fields, Ctrl+N/P projects, Ctrl+S keeps)"),
        ("s", "send (laptop or droplet, then confirm)"),
        ("u", "pull a queued idea back"),
        ("S / X / B", "ship / discard / roll back the droplet job"),
        ("l", "show or hide the deploy log"),
        ("z", "archive or restore"),
        ("D", "delete (inbox or archived only)"),
        ("Esc / q", "back"),
        ("Text boxes", ""),
        ("Ctrl+S", "submit"),
        ("Esc", "cancel"),
        ("Anywhere", ""),
        ("Ctrl+C", "quit"),
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, v)| {
            if v.is_empty() {
                Line::from(Span::styled(*k, Style::new().fg(Color::Yellow).bold()))
            } else {
                Line::from(vec![Span::styled(format!("  {k:<13}"), Style::new().fg(ACCENT).bold()), Span::raw(*v)])
            }
        })
        .collect();
    let r = centered(area, 82, lines.len() as u16 + 2);
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(lines).block(popup_block("Keys")), r);
}

fn draw_answer(f: &mut Frame, a: &AnswerModal, area: Rect) {
    let r = centered(area, area.width.saturating_sub(4).min(110), area.height.saturating_sub(2));
    f.render_widget(Clear, r);
    let title = format!("Answer ({} of {})", a.form.answered_count(), a.form.questions.len());
    let block = popup_block(&title);
    let inner = block.inner(r);
    f.render_widget(block, r);
    let width = inner.width as usize;

    let mut lines: Vec<Line> = Vec::new();
    for l in wrap(&a.comment, width) {
        lines.push(Line::from(Span::styled(l, Style::new().fg(Color::Yellow))));
    }
    lines.push(Line::default());
    let cur = a.form.current();
    let mut cursor_line = 0;
    for (qi, q) in a.form.questions.iter().enumerate() {
        let ans = &a.form.answers[qi];
        let mark = if ans.is_answered() { "✓" } else { " " };
        for (n, l) in wrap(&format!("{}. {}", qi + 1, q.text), width.saturating_sub(4)).into_iter().enumerate() {
            let prefix = if n == 0 { format!("{mark} ") } else { "  ".to_string() };
            lines.push(Line::from(vec![Span::styled(prefix, Style::new().fg(Color::Green)), Span::styled(l, Style::new().bold())]));
        }
        let multi = q.is_multi();
        let (on, off) = if multi { ("[x]", "[ ]") } else { ("(•)", "( )") };
        for (oi, opt) in q.options.iter().enumerate() {
            let selected = ans.sel.contains(&oi);
            let here = cur == Some(Row { q: qi, opt: Some(oi) });
            if here {
                cursor_line = lines.len();
            }
            lines.push(option_line(here, if selected { on } else { off }, opt, selected));
        }
        let here = cur == Some(Row { q: qi, opt: None });
        if here {
            cursor_line = lines.len();
        }
        let other = if a.editing && here {
            format!("Other: {}▏", ans.text)
        } else if ans.other || !ans.text.is_empty() {
            format!("Other: {}", ans.text)
        } else {
            "Other…".to_string()
        };
        lines.push(option_line(here, if ans.other { on } else { off }, &other, ans.other));
        if let Some(e) = a.elab.get(&q.id) {
            for l in wrap(e, width.saturating_sub(6)) {
                lines.push(Line::from(Span::styled(format!("   ⓘ {l}"), Style::new().fg(Color::Blue))));
            }
        } else if a.elab_pending.contains(&q.id) {
            lines.push(Line::from(Span::styled("   ⓘ asking Claude to explain…", DIM)));
        }
        lines.push(Line::default());
    }
    let h = inner.height as usize;
    let scroll = (cursor_line + 2).saturating_sub(h);
    let shown: Vec<Line> = lines.into_iter().skip(scroll).take(h).collect();
    f.render_widget(Paragraph::new(shown), inner);
}

fn option_line(here: bool, mark: &str, text: &str, selected: bool) -> Line<'static> {
    let pointer = if here { "›" } else { " " };
    let style = if here {
        Style::new().add_modifier(Modifier::REVERSED)
    } else if selected {
        Style::new().fg(Color::Green)
    } else {
        Style::new()
    };
    Line::from(vec![Span::raw("  "), Span::styled(pointer.to_string(), Style::new().fg(ACCENT)), Span::styled(format!(" {mark} {text}"), style)])
}

fn draw_handoff_editor(f: &mut Frame, h: &mut HandoffEditor, projects: &[String], area: Rect) {
    let r = centered(area, area.width.saturating_sub(4).min(120), area.height.saturating_sub(1));
    f.render_widget(Clear, r);
    let outer = popup_block("Edit handoff");
    let inner = outer.inner(r);
    f.render_widget(outer, r);
    let [title, project, vis, brief] =
        Layout::vertical([Constraint::Length(3), Constraint::Length(3), Constraint::Length(1), Constraint::Min(4)]).areas(inner);

    let on = Style::new().add_modifier(Modifier::REVERSED);
    let off = Style::new();
    let field = |label: &str, focused: bool| {
        Block::new()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(if focused { ACCENT } else { Color::DarkGray }))
            .title(format!(" {label} "))
    };

    let b = field("Title", h.focus == HField::Title);
    let i = b.inner(title);
    f.render_widget(b, title);
    h.title.set_cursor_style(if h.focus == HField::Title { on } else { off });
    f.render_widget(&h.title, i);

    let name = h.project.lines().join("").trim().to_string();
    let tag = if projects.is_empty() {
        String::new()
    } else if projects.contains(&name) {
        " existing project ".to_string()
    } else {
        " NEW project: folder + GitHub repo ".to_string()
    };
    let b = field(&format!("Project{}  (Ctrl+N/P cycles)", if tag.is_empty() { String::new() } else { format!(" ·{tag}") }), h.focus == HField::Project);
    let i = b.inner(project);
    f.render_widget(b, project);
    h.project.set_cursor_style(if h.focus == HField::Project { on } else { off });
    f.render_widget(&h.project, i);

    let vstyle = if h.focus == HField::Visibility { on } else { off };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" New-repo visibility: "),
            Span::styled(if h.public { " public " } else { " private " }, vstyle),
            Span::styled("  (Space toggles; only matters for new projects)", DIM),
        ])),
        vis,
    );

    let b = field("Brief for Claude Code", h.focus == HField::Brief);
    let i = b.inner(brief);
    f.render_widget(b, brief);
    h.brief.set_cursor_style(if h.focus == HField::Brief { on } else { off });
    f.render_widget(&h.brief, i);
}

fn draw_send(f: &mut Frame, s: &SendDialog, area: Rect) {
    let radio = |on: bool, label: String, enabled: bool| {
        let style = if !enabled { DIM } else if on { Style::new().fg(Color::Green).bold() } else { Style::new() };
        Line::from(Span::styled(format!("  {} {}", if on { "(•)" } else { "( )" }, label), style))
    };
    let mut lines = vec![
        Line::from(vec![Span::styled("Title    ", DIM), Span::raw(s.handoff.title.clone())]),
        Line::from(vec![
            Span::styled("Project  ", DIM),
            Span::raw(s.handoff.project.clone()),
            Span::styled(if s.new_project { "  (new project)" } else { "" }, Style::new().fg(Color::Yellow)),
        ]),
        Line::default(),
        Line::from(Span::styled("Where", Style::new().bold())),
        radio(s.dest == Dest::Laptop, "1  Laptop (dispatcher + herdr)".into(), true),
        radio(
            s.dest == Dest::Droplet,
            if s.new_project { "2  Droplet (not available for new projects)".into() } else { "2  Droplet".into() },
            !s.new_project,
        ),
    ];
    if s.dest == Dest::Droplet {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("Then", Style::new().bold())));
        lines.push(radio(!s.live, "p  Preview first, ship or discard it after".into(), true));
        lines.push(radio(s.live, "v  Go live (tests must pass; rolls back on failure)".into(), true));
    }
    let r = centered(area, 70, lines.len() as u16 + 2);
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(lines).block(popup_block("Send (Enter to continue)")), r);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Client;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use tokio::sync::mpsc::unbounded_channel;

    fn app() -> App {
        let (tx, _rx) = unbounded_channel();
        let mut a = App::new(Client::with_base("http://127.0.0.1:9".into(), "t".into()), tx);
        a.loaded = true;
        a
    }

    fn render(a: &mut App, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| draw(f, a)).unwrap();
        text(t.backend().buffer())
    }

    fn text(b: &Buffer) -> String {
        let mut out = String::new();
        for y in 0..b.area.height {
            for x in 0..b.area.width {
                out.push_str(b[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn wraps_words_and_long_tokens() {
        assert_eq!(wrap("aaa bbb ccc", 8), vec!["aaa bbb", "ccc"]);
        assert_eq!(wrap("a\n\nb", 20), vec!["a", "", "b"]);
        let w = wrap(&"x".repeat(20), 8);
        assert!(w.iter().all(|l| l.width() <= 8) && w.len() == 3);
        assert_eq!(truncate("hello world", 6), "hello…");
    }

    #[tokio::test]
    async fn list_shows_title_status_and_deploy_pill() {
        let mut a = app();
        a.ideas = vec![
            Idea { id: "1".into(), text: "Build a thing\nmore".into(), status: "dispatched".into(), deploy: Some(Deploy { status: "ok".into(), ..Default::default() }), ..Default::default() },
            Idea { id: "2".into(), text: "Broken one".into(), status: "dispatched".into(), deploy: Some(Deploy { status: "failed".into(), ..Default::default() }), ..Default::default() },
            Idea { id: "3".into(), text: "Fresh".into(), status: "inbox".into(), ..Default::default() },
        ];
        a.list_state.select(Some(0));
        let s = render(&mut a, 80, 14);
        assert!(s.contains("Build a thing") && s.contains("Deployed"), "{s}");
        assert!(s.contains("Deploy failed") && s.contains("inbox"), "{s}");
        assert!(s.contains("Open 3"), "{s}");
        assert!(s.contains("n new idea"), "footer lists keys: {s}");
    }

    #[tokio::test]
    async fn detail_shows_job_deploy_and_actions() {
        let mut a = app();
        let idea = Idea {
            id: "1".into(),
            text: "Make it faster".into(),
            status: "dispatched".into(),
            dispatch: Some(Dispatch { destination: Some("droplet".into()), mode: Some("preview".into()), ..Default::default() }),
            job: Some(Job { id: Some(7), status: "preview".into(), note: Some("Preview is up".into()), preview_url: Some("https://p.example".into()), ..Default::default() }),
            deploy: Some(Deploy { status: "failed".into(), message: Some("tests broke".into()), log: Some("line1\nline2".into()), commit: Some("abcdef123".into()), ..Default::default() }),
            handoff: Some(Handoff { title: "Faster".into(), project: "p".into(), brief: "Do it.".into(), ..Default::default() }),
            ..Default::default()
        };
        a.open_idea(idea);
        let s = render(&mut a, 90, 40);
        for want in ["Preview ready", "https://p.example", "S ship it live", "X discard", "Deploy failed", "tests broke", "log tail", "abcdef1", "Faster", "Do it."] {
            assert!(s.contains(want), "missing {want:?} in:\n{s}");
        }
        assert!(!s.contains("line2"), "log is collapsed by default");
        a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('l')));
        assert!(render(&mut a, 90, 40).contains("line2"));
    }

    #[tokio::test]
    async fn answer_modal_renders_options_and_cursor() {
        let mut a = app();
        let idea = Idea {
            id: "1".into(),
            status: "grilling".into(),
            grill_session: Some("s".into()),
            messages: vec![Message {
                role: "claude".into(),
                text: "A comment".into(),
                questions: Some(vec![Question { id: "q1".into(), text: "Pick one?".into(), kind: "single".into(), options: vec!["alpha".into(), "beta".into()] }]),
                done: Some(false),
                at: Some("t".into()),
            }],
            ..Default::default()
        };
        a.open_idea(idea);
        a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('a')));
        let s = render(&mut a, 80, 24);
        for want in ["A comment", "Pick one?", "( ) alpha", "( ) beta", "Other…", "Answer (0 of 1)"] {
            assert!(s.contains(want), "missing {want:?} in:\n{s}");
        }
    }

    #[tokio::test]
    async fn tiny_terminals_do_not_panic() {
        let mut a = app();
        a.ideas = vec![Idea { id: "1".into(), text: "x".into(), status: "inbox".into(), ..Default::default() }];
        a.list_state.select(Some(0));
        for (w, h) in [(1, 1), (10, 3), (20, 6)] {
            render(&mut a, w, h);
        }
        a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('?')));
        render(&mut a, 20, 6);
        a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Esc));
        a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('n')));
        render(&mut a, 20, 6);
    }
}
