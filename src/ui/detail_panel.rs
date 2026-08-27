use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::text::{display_width, truncate_end};
use crate::app::detail_panel::Timeline;
use crate::app::state::Palette;
use crate::app::AppState;

/// Right-side drill-in for the focused tab (Josh 2026-08-27): one aggregate
/// line, then the task timeline (done in green, in flight in yellow,
/// inferred next steps in purple), then the conversation view underneath.
/// Clicking a done/in-flight line points the conversation at that exchange.
pub(super) fn render_detail_panel(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.width < 4 || area.height < 2 {
        return;
    }

    let p = &app.palette;
    let sep_style = Style::default().fg(p.text);
    let buf = frame.buffer_mut();
    for y in area.y..area.y + area.height {
        buf[(area.x, y)].set_symbol("│");
        buf[(area.x, y)].set_style(sep_style);
    }

    let content = content_rect(area);
    let lines = detail_panel_lines(app, content.width);
    let scroll = app.detail_panel_scroll.min(max_scroll_for(&lines, content)) as u16;
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), content);
}

fn content_rect(area: Rect) -> Rect {
    Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(3),
        area.height.saturating_sub(1),
    )
}

pub(crate) fn detail_panel_max_scroll(app: &AppState, area: Rect) -> usize {
    if area.width < 4 || area.height < 2 {
        return 0;
    }
    let content = content_rect(area);
    max_scroll_for(&detail_panel_lines(app, content.width), content)
}

/// The timeline item uuid under a click, honoring the current scroll.
pub(crate) fn detail_panel_item_at(app: &AppState, area: Rect, row: u16) -> Option<String> {
    let content = content_rect(area);
    if row < content.y || row >= content.y + content.height {
        return None;
    }
    let timeline = app.detail_panel.as_ref()?.timeline.as_ref()?;
    let scroll = app
        .detail_panel_scroll
        .min(detail_panel_max_scroll(app, area));
    let line_idx = (row - content.y) as usize + scroll;
    let idx = line_idx.checked_sub(1)?;
    timeline
        .done
        .iter()
        .chain(timeline.current.iter())
        .nth(idx)
        .map(|item| item.u.clone())
}

fn max_scroll_for(lines: &[Line<'_>], content: Rect) -> usize {
    lines.len().saturating_sub(content.height as usize)
}

fn detail_panel_lines(app: &AppState, width: u16) -> Vec<Line<'static>> {
    let p = &app.palette;
    let dim = Style::default().fg(p.subtext0);
    let mut lines = Vec::new();

    let Some(cache) = &app.detail_panel else {
        lines.push(Line::from(Span::styled("no agent session in this pane", dim)));
        return lines;
    };
    if cache.transcript.is_none() {
        lines.push(Line::from(Span::styled(
            format!("no transcript view for {} yet", cache.agent),
            dim,
        )));
        return lines;
    }

    let selected = app.detail_panel_selected.as_deref();
    match &cache.timeline {
        None => lines.push(Line::from(Span::styled("timeline pending", dim))),
        Some(tl) => {
            lines.push(Line::from(Span::styled(aggregate_line(tl), dim)));
            let now = now_epoch();
            for item in &tl.done {
                let is_selected = selected == Some(item.u.as_str());
                lines.push(item_line(
                    "✓",
                    &item.label,
                    item.secs.map(fmt_secs),
                    p.green,
                    is_selected,
                    width,
                    p,
                ));
            }
            for item in &tl.current {
                let is_selected = selected == Some(item.u.as_str());
                let running = (item.ts > 0.0).then(|| fmt_secs(now - item.ts));
                lines.push(item_line(
                    "●",
                    &item.label,
                    running,
                    p.yellow,
                    is_selected,
                    width,
                    p,
                ));
            }
            for label in &tl.next {
                lines.push(item_line("◇", label, None, p.mauve, false, width, p));
            }
        }
    }

    lines.push(Line::default());
    let viewed = selected
        .and_then(|uuid| cache.timeline.as_ref().and_then(|tl| tl.item(uuid)))
        .map(|item| item.label.clone());
    let header = match &viewed {
        Some(label) => format!("conversation · {label}"),
        None => "conversation".to_string(),
    };
    lines.push(section_header(&header, width, p));
    let body = Style::default().fg(p.text);
    push_wrapped(&mut lines, &cache.prompt, width, dim, dim);
    lines.push(Line::default());
    push_wrapped(&mut lines, &cache.reply, width, body, dim);
    lines
}

fn aggregate_line(tl: &Timeline) -> String {
    let mut parts = vec![fmt_secs(tl.total_secs), format!("{} tok", fmt_tokens(tl.out_tokens))];
    if !tl.status.is_empty() {
        parts.push(tl.status.clone());
    }
    parts.join(" · ")
}

fn item_line(
    glyph: &str,
    label: &str,
    right: Option<String>,
    color: ratatui::style::Color,
    is_selected: bool,
    width: u16,
    p: &Palette,
) -> Line<'static> {
    let base = Style::default().fg(color);
    let style = if is_selected { base.bg(p.surface0) } else { base };
    let right = right.unwrap_or_default();
    let right_w = display_width(&right);
    let reserve = 2 + if right_w > 0 { right_w + 1 } else { 0 };
    let label_w = (width as usize).saturating_sub(reserve).max(4);
    let label = truncate_end(label, label_w);
    let used = 2 + display_width(&label);
    let pad = (width as usize).saturating_sub(used + right_w).max(1);
    let mut spans = vec![Span::styled(format!("{glyph} {label}"), style)];
    if right_w > 0 {
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(right, Style::default().fg(p.subtext0)));
    }
    Line::from(spans)
}

fn now_epoch() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn fmt_secs(secs: f64) -> String {
    let secs = secs.max(0.0) as u64;
    if secs >= 3600 {
        format!("{}h{:02}", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}

fn section_header(title: &str, width: u16, p: &Palette) -> Line<'static> {
    let style = Style::default().fg(p.overlay0);
    let label = format!("─ {title} ");
    let fill = (width as usize).saturating_sub(display_width(&label));
    Line::from(Span::styled(format!("{label}{}", "─".repeat(fill)), style))
}

fn push_wrapped(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: u16,
    body: Style,
    dim: Style,
) {
    if text.is_empty() {
        lines.push(Line::from(Span::styled("(none)", dim)));
        return;
    }
    for line in wrap_text(text, width.max(8) as usize) {
        lines.push(Line::from(Span::styled(line, body)));
    }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for para in text.split('\n') {
        if para.trim().is_empty() {
            if !matches!(out.last(), Some(last) if last.is_empty()) {
                out.push(String::new());
            }
            continue;
        }
        let mut line = String::new();
        for word in para.split_whitespace() {
            if line.is_empty() {
                line = word.to_string();
            } else if display_width(&line) + 1 + display_width(word) <= width {
                line.push(' ');
                line.push_str(word);
            } else {
                out.push(std::mem::take(&mut line));
                line = word.to_string();
            }
        }
        if !line.is_empty() {
            out.push(line);
        }
    }
    while matches!(out.last(), Some(last) if last.is_empty()) {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::detail_panel::TimelineItem;

    #[test]
    fn wrap_text_wraps_words_and_collapses_blank_runs() {
        let wrapped = wrap_text("one two three four\n\n\nfive", 9);
        assert_eq!(wrapped, ["one two", "three", "four", "", "five"]);
    }

    #[test]
    fn durations_and_tokens_format_compactly() {
        assert_eq!(fmt_secs(42.0), "42s");
        assert_eq!(fmt_secs(754.0), "12m");
        assert_eq!(fmt_secs(6135.0), "1h42");
        assert_eq!(fmt_tokens(96_512), "96k");
        assert_eq!(fmt_tokens(1_240_000), "1.2M");
    }

    fn test_timeline() -> Timeline {
        let item = |u: &str, label: &str, secs| TimelineItem {
            u: u.into(),
            label: label.into(),
            ts: 100.0,
            secs,
            off: 0,
        };
        Timeline {
            status: "working".into(),
            total_secs: 754.0,
            out_tokens: 96_512,
            done: vec![item("u1", "restored the configs", Some(300.0))],
            current: vec![item("u2", "building the panel", None)],
            next: vec!["ship the teardown".into()],
        }
    }

    #[test]
    fn clicks_resolve_timeline_items_through_scroll() {
        let mut app = crate::app::state::AppState::test_new();
        app.workspaces = vec![crate::workspace::Workspace::test_new("one")];
        app.active = Some(0);
        app.detail_panel_open = true;
        crate::ui::compute_view(&mut app, Rect::new(0, 0, 180, 40));
        let rect = app.view.detail_panel_rect;
        app.detail_panel = Some(crate::app::detail_panel::DetailPanelCache::test_with_timeline(
            test_timeline(),
        ));

        let content = content_rect(rect);
        assert_eq!(detail_panel_item_at(&app, rect, content.y), None);
        assert_eq!(
            detail_panel_item_at(&app, rect, content.y + 1).as_deref(),
            Some("u1")
        );
        assert_eq!(
            detail_panel_item_at(&app, rect, content.y + 2).as_deref(),
            Some("u2")
        );
        assert_eq!(detail_panel_item_at(&app, rect, content.y + 3), None);
    }

    #[test]
    fn open_detail_panel_reserves_a_right_column_and_renders() {
        use ratatui::{backend::TestBackend, Terminal};

        let mut app = crate::app::state::AppState::test_new();
        app.workspaces = vec![crate::workspace::Workspace::test_new("one")];
        app.active = Some(0);
        app.detail_panel_open = true;
        let area = Rect::new(0, 0, 180, 40);
        crate::ui::compute_view(&mut app, area);

        let rect = app.view.detail_panel_rect;
        assert_eq!(rect.width, 46);
        assert_eq!(rect.x + rect.width, 180);
        assert!(app.view.terminal_area.width >= 60);

        app.detail_panel = Some(crate::app::detail_panel::DetailPanelCache::test_with_timeline(
            test_timeline(),
        ));
        let mut terminal = Terminal::new(TestBackend::new(180, 40))
            .expect("test terminal should initialize");
        terminal
            .draw(|frame| render_detail_panel(&app, frame, rect))
            .expect("detail panel should render");
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(rect.x, 5)].symbol(), "│");
        let content = content_rect(rect);
        assert_eq!(buffer[(content.x, content.y + 1)].symbol(), "✓");
        assert_eq!(
            buffer[(content.x, content.y + 1)].style().fg,
            Some(app.palette.green)
        );
        assert_eq!(
            buffer[(content.x, content.y + 2)].style().fg,
            Some(app.palette.yellow)
        );
        assert_eq!(
            buffer[(content.x, content.y + 3)].style().fg,
            Some(app.palette.mauve)
        );
    }

    #[test]
    fn closed_or_narrow_layouts_reserve_no_detail_column() {
        let mut app = crate::app::state::AppState::test_new();
        app.workspaces = vec![crate::workspace::Workspace::test_new("one")];
        app.active = Some(0);
        crate::ui::compute_view(&mut app, Rect::new(0, 0, 180, 40));
        assert_eq!(app.view.detail_panel_rect, Rect::default());

        app.detail_panel_open = true;
        crate::ui::compute_view(&mut app, Rect::new(0, 0, 100, 40));
        assert_eq!(app.view.detail_panel_rect, Rect::default());
    }
}
