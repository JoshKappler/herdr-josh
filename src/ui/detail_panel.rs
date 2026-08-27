use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::sidebar::{tab_dashboards, TabDashRow};
use super::status::state_dot;
use super::text::display_width;
use crate::app::state::Palette;
use crate::app::AppState;

/// Right-side drill-in for the focused tab: full summary, then the last
/// prompt and reply from the agent's transcript (Josh 2026-08-27).
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

    let content = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(3),
        area.height.saturating_sub(1),
    );
    let lines = detail_panel_lines(app, content.width);
    let scroll = app.detail_panel_scroll.min(max_scroll_for(&lines, content)) as u16;
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), content);
}

pub(crate) fn detail_panel_max_scroll(app: &AppState, area: Rect) -> usize {
    if area.width < 4 || area.height < 2 {
        return 0;
    }
    let content = Rect::new(0, 0, area.width.saturating_sub(3), area.height.saturating_sub(1));
    max_scroll_for(&detail_panel_lines(app, content.width), content)
}

fn max_scroll_for(lines: &[Line<'_>], content: Rect) -> usize {
    lines.len().saturating_sub(content.height as usize)
}

fn detail_panel_lines(app: &AppState, width: u16) -> Vec<Line<'static>> {
    let p = &app.palette;
    let dim = Style::default().fg(p.subtext0);
    let mut lines = Vec::new();

    let Some(ws) = app.active.and_then(|i| app.workspaces.get(i)) else {
        lines.push(Line::from(Span::styled("no focused tab", dim)));
        return lines;
    };

    let bold = Style::default().fg(p.text).add_modifier(Modifier::BOLD);
    let dash = tab_dashboards(app, ws, width.saturating_add(5))
        .into_iter()
        .find(|dash| dash.tab_idx == ws.active_tab);
    if let Some(dash) = dash {
        let dot = state_dot(dash.state, dash.seen, p);
        for (i, row) in dash.rows.iter().enumerate() {
            match row {
                TabDashRow::Title { text, .. } | TabDashRow::TitleCont { text, .. } => {
                    let mut spans = Vec::new();
                    if i == 0 {
                        spans.push(Span::styled(dot.0.to_string(), dot.1));
                        spans.push(Span::raw(" "));
                    }
                    spans.push(Span::styled(text.clone(), bold));
                    lines.push(Line::from(spans));
                }
                TabDashRow::Counts(text) | TabDashRow::Lane(text) => {
                    lines.push(Line::from(Span::styled(text.clone(), dim)));
                }
            }
        }
    }

    lines.push(Line::default());
    match &app.detail_panel {
        None => lines.push(Line::from(Span::styled("no agent session in this pane", dim))),
        Some(cache) if cache.transcript.is_none() => {
            lines.push(Line::from(Span::styled(
                format!("no transcript view for {} yet", cache.agent),
                dim,
            )));
        }
        Some(cache) => {
            let body = Style::default().fg(p.text);
            lines.push(section_header("last prompt", width, p));
            push_wrapped(&mut lines, &cache.prompt, width, body, dim);
            lines.push(Line::default());
            lines.push(section_header("reply", width, p));
            push_wrapped(&mut lines, &cache.reply, width, body, dim);
        }
    }
    lines
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

    #[test]
    fn wrap_text_wraps_words_and_collapses_blank_runs() {
        let wrapped = wrap_text("one two three four\n\n\nfive", 9);
        assert_eq!(wrapped, ["one two", "three", "four", "", "five"]);
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

        let mut terminal = Terminal::new(TestBackend::new(180, 40))
            .expect("test terminal should initialize");
        terminal
            .draw(|frame| render_detail_panel(&app, frame, rect))
            .expect("detail panel should render");
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(rect.x, 5)].symbol(), "│");
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
