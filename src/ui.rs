//! Job desk. Same ink as tui-design: gold header, role list, pinned facts,
//! description underneath. Sketch: docs/jobs-board.json

use crate::cache;
use crate::Job;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Terminal;
use std::io::{self, Write};
use std::time::Duration;

const C_BG: Color = Color::Rgb(16, 18, 24);
const C_PANEL: Color = Color::Rgb(22, 25, 34);
const C_PANEL_HI: Color = Color::Rgb(32, 37, 50);
const C_HEADER: Color = Color::Rgb(26, 30, 42);
const C_BORDER: Color = Color::Rgb(58, 64, 84);
const C_TEXT: Color = Color::Rgb(236, 234, 228);
const C_MUTED: Color = Color::Rgb(138, 145, 165);
const C_DIM: Color = Color::Rgb(95, 102, 122);
const C_GOLD: Color = Color::Rgb(238, 178, 92);
const C_CYAN: Color = Color::Rgb(112, 202, 228);
const C_INK: Color = Color::Rgb(14, 16, 22);

struct Desk<'a> {
    jobs: &'a [Job],
    keywords: &'a str,
    location: &'a str,
    queried: &'a str,
    cache_path: &'a str,
    selected: usize,
    desc_offset: usize,
    notice: String,
}

pub fn run(
    jobs: &[Job],
    keywords: &str,
    location: &str,
    queried: &str,
    cache_path: &str,
) -> Result<(), String> {
    let mut desk = Desk {
        jobs,
        keywords,
        location,
        queried,
        cache_path,
        selected: 0,
        desc_offset: 0,
        notice: String::new(),
    };

    if let Some(&first) = role_order(jobs).first() {
        desk.selected = first;
    }
    enable_raw_mode().map_err(|e| e.to_string())?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(|e| e.to_string())?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend).map_err(|e| e.to_string())?;
    let _ = term.hide_cursor();

    let result = loop {
        if let Err(e) = term.draw(|f| draw(f, &desk)) {
            break Err(e.to_string());
        }
        if event::poll(Duration::from_millis(200)).unwrap_or(false) {
            match event::read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => {
                    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
                        break Ok(());
                    }
                    match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                        KeyCode::Down | KeyCode::Char('j') => desk.move_sel(1),
                        KeyCode::Up | KeyCode::Char('k') => desk.move_sel(-1),
                        KeyCode::Char('J') | KeyCode::PageDown => desk.scroll_desc(8),
                        KeyCode::Char('K') | KeyCode::PageUp => desk.scroll_desc(-8),
                        KeyCode::Char(' ') => desk.scroll_desc(12),
                        KeyCode::Char('s') => desk.save(),
                        KeyCode::Home | KeyCode::Char('g') => {
                            if let Some(&first) = role_order(desk.jobs).first() {
                                desk.selected = first;
                            }
                            desk.desc_offset = 0;
                        }
                        KeyCode::End | KeyCode::Char('G') => {
                            if let Some(&last) = role_order(desk.jobs).last() {
                                desk.selected = last;
                                desk.desc_offset = 0;
                            }
                        }
                        _ => {}
                    }
                }
                Err(e) => break Err(e.to_string()),
                _ => {}
            }
        }
    };

    disable_raw_mode().ok();
    execute!(term.backend_mut(), LeaveAlternateScreen).ok();
    let _ = term.show_cursor();
    let _ = io::stdout().flush();
    result
}

impl Desk<'_> {
    fn move_sel(&mut self, delta: isize) {
        let roles = role_order(self.jobs);
        if roles.is_empty() {
            return;
        }
        let pos = roles.iter().position(|i| *i == self.selected).unwrap_or(0);
        let next = (pos as isize + delta).clamp(0, roles.len() as isize - 1) as usize;
        if roles[next] != self.selected {
            self.selected = roles[next];
            self.desc_offset = 0;
        }
    }

    fn scroll_desc(&mut self, delta: isize) {
        if delta < 0 {
            self.desc_offset = self.desc_offset.saturating_sub((-delta) as usize);
        } else {
            self.desc_offset = self.desc_offset.saturating_add(delta as usize);
        }
    }

    fn save(&mut self) {
        if self.cache_path.is_empty() {
            self.notice = "cache off; nothing written".into();
            return;
        }
        let path = std::path::Path::new(self.cache_path);
        let store = match cache::Cache::open(path) {
            Ok(store) => store,
            Err(err) => {
                self.notice = format!("save failed: {err}");
                return;
            }
        };
        let mut n = 0usize;
        for job in self.jobs {
            if !cache::has_detail(job) {
                continue;
            }
            if let Err(err) = store.put(job) {
                self.notice = format!("save failed: {err}");
                return;
            }
            n += 1;
        }
        self.notice = format!("saved {n} → {}", self.cache_path);
    }
}

fn draw(f: &mut ratatui::Frame<'_>, desk: &Desk<'_>) {
    let area = f.area();
    f.render_widget(Block::default().style(Style::default().bg(C_BG).fg(C_TEXT)), area);
    if area.height < 10 || area.width < 56 {
        f.render_widget(
            Paragraph::new("need a wider terminal").style(Style::default().fg(C_MUTED)),
            area,
        );
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(6),
            Constraint::Length(1),
        ])
        .split(area);
    draw_header(f, desk, chunks[0]);
    draw_body(f, desk, chunks[1]);
    draw_status(f, desk, chunks[2]);
}

fn draw_header(f: &mut ratatui::Frame<'_>, desk: &Desk<'_>, area: Rect) {
    let place = if desk.location.is_empty() {
        "anywhere".to_string()
    } else {
        desk.location.to_string()
    };
    let left = Line::from(vec![
        Span::styled(
            " ◆ JOBS ",
            Style::default().fg(C_INK).bg(C_GOLD).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {} ", desk.keywords),
            Style::default().fg(C_TEXT).bg(C_HEADER).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{place}  "), Style::default().fg(C_GOLD).bg(C_HEADER)),
        Span::styled(
            format!("past 7d  {}", desk.queried),
            Style::default().fg(C_MUTED).bg(C_HEADER),
        ),
    ]);
    f.render_widget(
        Paragraph::new(left).style(Style::default().bg(C_HEADER)),
        area,
    );
}

fn draw_body(f: &mut ratatui::Frame<'_>, desk: &Desk<'_>, area: Rect) {
    let list_w = if area.width >= 140 {
        52
    } else if area.width >= 100 {
        44
    } else {
        34
    };
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(list_w), Constraint::Min(24)])
        .split(area);
    draw_list(f, desk, cols[0]);
    draw_detail(f, desk, cols[1]);
}

fn draw_list(f: &mut ratatui::Frame<'_>, desk: &Desk<'_>, area: Rect) {
    let block = panel("roles");
    let inner = block.inner(area);
    f.render_widget(block, area);
    if desk.jobs.is_empty() || inner.width == 0 || inner.height == 0 {
        f.render_widget(
            Paragraph::new("no cards").style(Style::default().fg(C_DIM)),
            inner,
        );
        return;
    }
    let slots = list_slots(desk.jobs);
    let height = inner.height as usize;
    let width = inner.width as usize;
    let sel_at = slots
        .iter()
        .position(|slot| matches!(slot, Slot::Role { index, .. } if *index == desk.selected))
        .unwrap_or(0);
    let mut sel_line = 0usize;
    for slot in slots.iter().take(sel_at) {
        sel_line += slot.lines();
    }
    let sel_span = slots.get(sel_at).map(Slot::lines).unwrap_or(2);
    let mut offset = 0usize;
    if height > 0 && sel_line + sel_span > height {
        // Pin the selected company to the last panel line. The top slot may be
        // clipped; a slot-aligned window leaves a blank row under the company.
        offset = sel_line + sel_span - height;
    }

    let mut line_no = 0usize;
    let mut lines = Vec::new();
    for slot in &slots {
        let start = line_no;
        let span = slot.lines();
        line_no += span;
        if start + span <= offset || start >= offset + height {
            continue;
        }
        let skip = offset.saturating_sub(start);
        match slot {
            Slot::Day(day) => {
                if skip > 0 {
                    continue;
                }
                let label = format!(" {day}");
                lines.push(Line::from(Span::styled(
                    pad(&trunc(&label, width), width),
                    Style::default()
                        .fg(C_INK)
                        .bg(C_GOLD)
                        .add_modifier(Modifier::BOLD),
                )));
            }
            Slot::Role { index, visual } => {
                let job = &desk.jobs[*index];
                let on = *index == desk.selected;
                let bg = if on { C_PANEL_HI } else { C_PANEL };
                let title_fg = if on { C_TEXT } else { C_MUTED };
                let caret = if on { "▸" } else { " " };
                let idx = format!("{visual:>3}");
                let idx_fg = if on { C_GOLD } else { C_DIM };
                let prefix = format!("{caret}{idx} ");
                let prefix_w = disp(&prefix);
                let title = trunc(&job.title, width.saturating_sub(prefix_w));
                let title_pad = pad(&title, width.saturating_sub(prefix_w));
                let indent = " ".repeat(prefix_w);
                let co = trunc(empty(&job.company), width.saturating_sub(prefix_w));
                let co_pad = pad(&format!("{indent}{co}"), width);
                if skip == 0 {
                    lines.push(Line::from(vec![
                        Span::styled(
                            prefix,
                            Style::default().fg(idx_fg).bg(bg).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            title_pad,
                            Style::default()
                                .fg(title_fg)
                                .bg(bg)
                                .add_modifier(if on { Modifier::BOLD } else { Modifier::empty() }),
                        ),
                    ]));
                }
                if skip <= 1 {
                    lines.push(Line::from(Span::styled(
                        co_pad,
                        Style::default()
                            .fg(if on { C_CYAN } else { C_GOLD })
                            .bg(bg),
                    )));
                }
            }
        }
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_detail(f: &mut ratatui::Frame<'_>, desk: &Desk<'_>, area: Rect) {
    let title = desk
        .jobs
        .get(desk.selected)
        .map(|j| trunc(&j.title, 48))
        .unwrap_or_else(|| "empty".into());
    let block = panel(&title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if desk.jobs.is_empty() || inner.height < 4 {
        f.render_widget(
            Paragraph::new("No guest cards for this search.").style(Style::default().fg(C_MUTED)),
            inner,
        );
        return;
    }
    let job = &desk.jobs[desk.selected];
    let fact_h = 8.min(inner.height.saturating_sub(2));
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(fact_h), Constraint::Min(1)])
        .split(inner);
    draw_facts(f, job, split[0]);
    draw_description(f, desk, job, split[1]);
}

fn draw_facts(f: &mut ratatui::Frame<'_>, job: &Job, area: Rect) {
    let mut lines = vec![
        fact("company", &job.company),
        fact("where", &job.location),
        fact("listed", &job.listed),
    ];
    if !job.seniority.is_empty() {
        lines.push(fact("level", &job.seniority));
    }
    if !job.employment.is_empty() {
        lines.push(fact("type", &job.employment));
    }
    if !job.applicants.is_empty() {
        lines.push(fact("applied", &job.applicants));
    }
    if !job.industries.is_empty() {
        lines.push(fact("field", &job.industries));
    }
    if !job.benefits.is_empty() {
        lines.push(fact("card", &job.benefits));
    }
    lines.push(Line::from(vec![
        Span::styled(" url      ", Style::default().fg(C_DIM)),
        Span::styled(
            trunc(&job.url, (area.width as usize).saturating_sub(12)),
            Style::default().fg(C_CYAN),
        ),
    ]));
    f.render_widget(Paragraph::new(lines), area);
}

fn fact(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {label:<8} "), Style::default().fg(C_DIM)),
        Span::styled(empty(value).to_string(), Style::default().fg(C_TEXT)),
    ])
}

fn draw_description(f: &mut ratatui::Frame<'_>, desk: &Desk<'_>, job: &Job, area: Rect) {
    let width = area.width as usize;
    let body = if job.description.is_empty() {
        "Description was not on the guest page. The url above is the public posting.".to_string()
    } else {
        job.description.clone()
    };
    let mut wrapped = Vec::new();
    for para in body.split("\n\n") {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        for raw in para.split('\n') {
            let chunk = wrap_width(raw, width.max(8));
            if chunk.is_empty() {
                wrapped.push(String::new());
            } else {
                wrapped.extend(chunk);
            }
        }
        wrapped.push(String::new());
    }
    let view = area.height.saturating_sub(1) as usize;
    let max_off = wrapped.len().saturating_sub(view.max(1));
    let off = desk.desc_offset.min(max_off);
    let end = (off + view).min(wrapped.len());
    let more = if wrapped.len() > view {
        format!(" description  {}–{end} / {} ", off + 1, wrapped.len())
    } else {
        " description ".into()
    };
    let mut lines = vec![Line::from(Span::styled(more, Style::default().fg(C_GOLD)))];
    for line in wrapped.iter().skip(off).take(view) {
        lines.push(Line::from(Span::styled(line.clone(), Style::default().fg(C_TEXT))));
    }
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_status(f: &mut ratatui::Frame<'_>, desk: &Desk<'_>, area: Rect) {
    let order = role_order(desk.jobs);
    let here = if order.is_empty() {
        "—".into()
    } else {
        let pos = order.iter().position(|i| *i == desk.selected).unwrap_or(0) + 1;
        format!("{pos} / {}", order.len())
    };
    let hint = if desk.notice.is_empty() {
        "  j/k role    J/K description    space page    s save    q quit".to_string()
    } else {
        format!("  {}", desk.notice)
    };
    let line = Line::from(vec![
        Span::styled(
            " desk ",
            Style::default().fg(C_INK).bg(C_CYAN).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {here} "),
            Style::default().fg(C_GOLD).bg(C_INK),
        ),
        Span::styled(hint, Style::default().fg(C_MUTED).bg(C_INK)),
    ]);
    f.render_widget(Paragraph::new(line).style(Style::default().bg(C_INK)), area);
}

fn panel(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(C_BORDER))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(C_GOLD).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(C_PANEL).fg(C_TEXT))
}

enum Slot {
    Day(String),
    Role { index: usize, visual: usize },
}

impl Slot {
    fn lines(&self) -> usize {
        match self {
            Slot::Day(_) => 1,
            Slot::Role { .. } => 2,
        }
    }
}

fn role_order(jobs: &[Job]) -> Vec<usize> {
    list_slots(jobs)
        .into_iter()
        .filter_map(|slot| match slot {
            Slot::Role { index, .. } => Some(index),
            Slot::Day(_) => None,
        })
        .collect()
}

fn list_slots(jobs: &[Job]) -> Vec<Slot> {
    let mut order: Vec<usize> = (0..jobs.len()).collect();
    order.sort_by(|&a, &b| {
        listed_day(&jobs[b].listed)
            .cmp(&listed_day(&jobs[a].listed))
            .then(a.cmp(&b))
    });
    let mut slots = Vec::new();
    let mut current = String::new();
    for (visual, i) in order.into_iter().enumerate() {
        let day = listed_day(&jobs[i].listed);
        if day != current {
            current = day.clone();
            slots.push(Slot::Day(day));
        }
        slots.push(Slot::Role {
            index: i,
            visual: visual + 1,
        });
    }
    slots
}

fn listed_day(listed: &str) -> String {
    let head = listed.split_whitespace().next().unwrap_or("");
    let bytes = head.as_bytes();
    if bytes.len() >= 10 && bytes[4] == b'-' && bytes[7] == b'-' && head[..10].chars().all(|c| c.is_ascii_digit() || c == '-')
    {
        return head[..10].to_string();
    }
    if listed.trim().is_empty() {
        "undated".into()
    } else {
        listed.trim().to_string()
    }
}

fn empty(s: &str) -> &str {
    if s.is_empty() {
        "—"
    } else {
        s
    }
}

fn disp(s: &str) -> usize {
    s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
}

fn trunc(s: &str, width: usize) -> String {
    if disp(s) <= width {
        return s.to_string();
    }
    if width <= 1 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = if c.is_ascii() { 1 } else { 2 };
        if w + cw > width.saturating_sub(1) {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

fn pad(s: &str, width: usize) -> String {
    let mut out = s.to_string();
    let w = disp(&out);
    if w < width {
        out.push_str(&" ".repeat(width - w));
    }
    out
}

fn wrap_width(s: &str, width: usize) -> Vec<String> {
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in words {
        let extra = if cur.is_empty() { 0 } else { 1 };
        if !cur.is_empty() && disp(&cur) + extra + disp(word) > width {
            lines.push(cur);
            cur = String::new();
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Job;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn job(title: &str, company: &str, listed: &str) -> Job {
        Job {
            id: title.into(),
            title: title.into(),
            company: company.into(),
            location: "Hong Kong".into(),
            keywords: String::new(),
            search_location: String::new(),
            listed: listed.into(),
            benefits: String::new(),
            url: "https://example.com".into(),
            applicants: String::new(),
            seniority: String::new(),
            employment: String::new(),
            industries: String::new(),
            description: "desc".into(),
        }
    }

    fn list_col(buf: &ratatui::buffer::Buffer, y: u16, n: u16) -> String {
        (0..n)
            .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
            .collect()
    }

    #[test]
    fn last_selected_role_shows_company_on_its_row() {
        let mut jobs: Vec<Job> = (0..169)
            .map(|i| {
                job(
                    "Software Engineer",
                    "dtcpay",
                    &format!("2026-04-{:02}", 1 + (i / 30)),
                )
            })
            .collect();
        let selected = role_order(&jobs).pop().unwrap();
        jobs[selected].company = "talisman".into();
        jobs[selected].title = "Software Engineer".into();
        let desk = Desk {
            jobs: &jobs,
            keywords: "devops",
            location: "Hong Kong",
            queried: "now",
            cache_path: "",
            selected,
            desc_offset: 0,
            notice: String::new(),
        };
        for (w, h) in [(80u16, 12), (100, 24), (160, 40), (200, 50)] {
            let backend = TestBackend::new(w, h);
            let mut term = Terminal::new(backend).unwrap();
            term.draw(|f| draw(f, &desk)).unwrap();
            let buf = term.backend().buffer().clone();
            let title_y = (0..h)
                .find(|y| list_col(&buf, *y, w).contains('▸'))
                .unwrap_or_else(|| panic!("no caret at {w}x{h}"));
            let title_row = list_col(&buf, title_y, w.min(70));
            let company_row = list_col(&buf, title_y + 1, w.min(70));
            assert!(
                title_row.contains("Software"),
                "{w}x{h} title missing on {title_y}: {title_row}"
            );
            assert!(
                !title_row.contains("talisman"),
                "{w}x{h} company should be the line under the title: {title_row}"
            );
            assert!(
                company_row.contains("talisman"),
                "{w}x{h} company not under selected row {title_y}: {company_row}"
            );
            let below = list_col(&buf, title_y + 2, 8);
            assert!(
                below.contains('─') || below.contains('└'),
                "{w}x{h} blank row under company at {}: {below:?}",
                title_y + 2
            );
        }
    }
}
