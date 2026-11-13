use miniflux_api::models::EntryStatus;
use opaline::{
    Theme,
    names::{gradients, styles, tokens},
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};

use crate::{
    app::{App, Focus, Mode, SidebarRow, StatusKind},
    util,
};

struct Palette {
    theme: Theme,
}

impl Palette {
    fn color(&self, token: &str) -> Color {
        Color::from(self.theme.color(token))
    }

    fn style(&self, name: &str) -> Style {
        Style::from(self.theme.style(name))
    }

    fn base(&self) -> Style {
        Style::default()
            .bg(self.color(tokens::BG_BASE))
            .fg(self.color(tokens::TEXT_PRIMARY))
    }

    fn panel(&self) -> Style {
        Style::default()
            .bg(self.color(tokens::BG_PANEL))
            .fg(self.color(tokens::TEXT_PRIMARY))
    }

    fn text(&self) -> Style {
        Style::default().fg(self.color(tokens::TEXT_PRIMARY))
    }

    fn muted(&self) -> Style {
        Style::default().fg(self.color(tokens::TEXT_MUTED))
    }

    fn dim(&self) -> Style {
        Style::default().fg(self.color(tokens::TEXT_DIM))
    }

    fn strong(&self) -> Style {
        Style::default()
            .fg(self.color(tokens::TEXT_PRIMARY))
            .add_modifier(Modifier::BOLD)
    }

    fn heading(&self) -> Style {
        Style::default()
            .fg(self.color(tokens::ACCENT_PRIMARY))
            .add_modifier(Modifier::BOLD)
    }

    fn accent(&self) -> Style {
        Style::default().fg(self.color(tokens::ACCENT_PRIMARY))
    }

    fn key(&self) -> Style {
        Style::default()
            .fg(self.color(tokens::ACCENT_SECONDARY))
            .add_modifier(Modifier::BOLD)
    }

    fn selected(&self) -> Style {
        self.style(styles::SELECTED)
    }

    fn inactive_selection(&self) -> Style {
        Style::default()
            .bg(self.color(tokens::BG_HIGHLIGHT))
            .fg(self.color(tokens::TEXT_PRIMARY))
    }

    fn border(&self, focused: bool) -> Style {
        if focused {
            self.style(styles::FOCUSED_BORDER)
        } else {
            self.style(styles::UNFOCUSED_BORDER)
        }
    }

    fn title(&self, focused: bool) -> Style {
        if focused {
            self.heading()
        } else {
            self.muted()
        }
    }

    fn status(&self, kind: StatusKind) -> Style {
        let token = match kind {
            StatusKind::Info => tokens::INFO,
            StatusKind::Success => tokens::SUCCESS,
            StatusKind::Warning => tokens::WARNING,
            StatusKind::Error => tokens::ERROR,
        };
        Style::default().fg(self.color(token))
    }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let pal = Palette {
        theme: app.theme.clone(),
    };
    let area = frame.area();
    frame.render_widget(Block::default().style(pal.base()), area);

    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);

    draw_header(frame, app, rows[0], &pal);
    draw_body(frame, app, rows[1], &pal);
    draw_status(frame, app, rows[2], &pal);
    draw_keybar(frame, app, rows[3], &pal);

    if matches!(app.mode, Mode::Help { .. }) {
        draw_help(frame, app, area, &pal);
    }
    if matches!(app.mode, Mode::Confirm { .. }) {
        draw_confirm(frame, app, area, &pal);
    }
    if matches!(app.mode, Mode::ThemePicker { .. }) {
        draw_theme_picker(frame, app, area, &pal);
    }
    if matches!(app.mode, Mode::FeedPicker { .. }) {
        draw_feed_picker(frame, app, area, &pal);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect, pal: &Palette) {
    frame.render_widget(Block::default().style(pal.base()), area);
    let right_width = (area.width / 2).clamp(12, 48);
    let chunks = Layout::horizontal([
        Constraint::Min(10),
        Constraint::Length(right_width),
    ])
    .split(area);

    let brand = if pal.theme.has_gradient(gradients::PRIMARY) {
        pal.theme.gradient_text(gradients::PRIMARY, "fluxgrove")
    } else {
        Line::from(Span::styled("fluxgrove", pal.heading()))
    };

    let mut left_spans = vec![Span::raw(" ")];
    left_spans.extend(brand.spans);
    left_spans.push(Span::styled("  ›  ", pal.dim()));
    left_spans.push(Span::styled(app.selection_label(), pal.strong()));
    if !app.search.is_empty() {
        left_spans.push(Span::styled(
            format!("  /{}", util::truncate(&app.search, 24)),
            pal.muted(),
        ));
    }

    frame.render_widget(
        Paragraph::new(Line::from(left_spans)).style(pal.base()),
        chunks[0],
    );

    let right = format!(
        "{}  {} unread  ·  {} ",
        app.spinner_frame(),
        app.unread_total(),
        util::truncate(app.theme_display_name(), 28)
    );
    frame.render_widget(
        Paragraph::new(
            Line::from(Span::styled(right, pal.muted()))
                .alignment(Alignment::Right),
        )
        .style(pal.base()),
        chunks[1],
    );
}

fn draw_body(frame: &mut Frame, app: &mut App, area: Rect, pal: &Palette) {
    if area.width >= 100 {
        let sidebar_width = (area.width / 4).clamp(22, 30);
        let remaining = area.width - sidebar_width;
        let entries_width = ((remaining as f32) * 0.38) as u16;
        let entries_width =
            entries_width.clamp(28, (remaining.saturating_sub(26)).max(28));
        let chunks = Layout::horizontal([
            Constraint::Length(sidebar_width),
            Constraint::Length(entries_width),
            Constraint::Min(26),
        ])
        .split(area);

        draw_feeds(frame, app, chunks[0], pal);
        draw_entries(frame, app, chunks[1], pal);
        draw_article(frame, app, chunks[2], pal);
    } else if area.width >= 66 {
        if app.focus == Focus::Feeds {
            let sidebar_width = 24.min(area.width / 3);
            let chunks = Layout::horizontal([
                Constraint::Length(sidebar_width),
                Constraint::Min(24),
            ])
            .split(area);
            draw_feeds(frame, app, chunks[0], pal);
            draw_entries(frame, app, chunks[1], pal);
        } else {
            let entries_width = ((area.width as f32) * 0.40) as u16;
            let chunks = Layout::horizontal([
                Constraint::Length(entries_width),
                Constraint::Min(24),
            ])
            .split(area);
            draw_entries(frame, app, chunks[0], pal);
            draw_article(frame, app, chunks[1], pal);
        }
    } else {
        match app.focus {
            Focus::Feeds => draw_feeds(frame, app, area, pal),
            Focus::Entries => draw_entries(frame, app, area, pal),
            Focus::Article if app.selected_entry().is_some() => {
                draw_article(frame, app, area, pal)
            }
            Focus::Article => draw_entries(frame, app, area, pal),
        }
    }
}

fn draw_feeds(frame: &mut Frame, app: &mut App, area: Rect, pal: &Palette) {
    let focused = app.focus == Focus::Feeds;
    let rows = app.sidebar_rows();
    let inner_width = area.width.saturating_sub(4) as usize;
    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| sidebar_item(app, row, pal, inner_width))
        .collect();

    let feeds_counter = if app.hide_read_feeds {
        format!("{} · filtered", app.unread_total())
    } else {
        format!("{} unread", app.unread_total())
    };
    let block = pane_block("Feeds", Some(feeds_counter), focused, pal, true);

    app.sidebar_state.select(Some(app.sidebar_cursor));
    app.sidebar_page = area.height.saturating_sub(2) as usize;

    let highlight = if focused {
        pal.selected()
    } else {
        pal.inactive_selection()
    };

    let list = List::new(items)
        .block(block)
        .highlight_style(highlight)
        .highlight_symbol("▸ ");

    frame.render_stateful_widget(list, area, &mut app.sidebar_state);
}

fn sidebar_item(
    app: &App,
    row: &SidebarRow,
    pal: &Palette,
    width: usize,
) -> ListItem<'static> {
    let (icon, label, style, count) = match row {
        SidebarRow::All => (
            "◆",
            "All entries".to_string(),
            pal.strong(),
            Some(app.unread_total()),
        ),
        SidebarRow::Unread => (
            "●",
            "Unread".to_string(),
            Style::default().fg(pal.color(tokens::ACCENT_PRIMARY)),
            Some(app.unread_total()),
        ),
        SidebarRow::Starred => (
            "★",
            "Starred".to_string(),
            Style::default().fg(pal.color(tokens::WARNING)),
            None,
        ),
        SidebarRow::Category { id, title } => (
            "▾",
            util::truncate(title, width.saturating_sub(8)),
            pal.heading(),
            Some(app.category_unread(*id)),
        ),
        SidebarRow::Feed { id, title } => {
            let unread = app.feed_unread(*id);
            (
                "·",
                util::truncate(title, width.saturating_sub(8)),
                if unread > 0 { pal.text() } else { pal.muted() },
                Some(unread),
            )
        }
    };

    let count_text = match count {
        Some(0) => String::new(),
        Some(value) => value.to_string(),
        None => String::new(),
    };
    let count_width = util::display_width(&count_text);
    let label_width = width.saturating_sub(2 + count_width + 1);
    let label = util::truncate(&label, label_width);
    let padding =
        width.saturating_sub(2 + util::display_width(&label) + count_width);

    let line = Line::from(vec![
        Span::styled(format!("{icon} "), style),
        Span::styled(label, style),
        Span::raw(" ".repeat(padding)),
        Span::styled(count_text, pal.dim()),
    ]);

    ListItem::new(line)
}

fn draw_entries(frame: &mut Frame, app: &mut App, area: Rect, pal: &Palette) {
    let focused = app.focus == Focus::Entries;
    let entries_counter = if app.hide_read_entries {
        format!("{} · filtered", app.entry_counter_label())
    } else {
        app.entry_counter_label()
    };
    let block =
        pane_block("Entries", Some(entries_counter), focused, pal, false);

    let inner_height = area.height.saturating_sub(2) as usize;

    if app.entries.is_empty() {
        app.entry_page = (inner_height / 3).max(1);
        let message = if app.loading {
            "Loading…"
        } else if !app.search.is_empty() {
            "No entries match this search"
        } else {
            "No entries in this view"
        };
        let paragraph = Paragraph::new(message)
            .alignment(Alignment::Center)
            .style(pal.dim())
            .block(block);
        frame.render_widget(paragraph, area);
        return;
    }

    let inner_width = area.width.saturating_sub(4) as usize;
    let mut total_height = 0usize;
    let items: Vec<ListItem> = app
        .entries
        .iter()
        .map(|entry| {
            let (item, height) = entry_item(entry, pal, inner_width);
            total_height += height;
            item
        })
        .collect();

    // Items have different heights because titles wrap, so estimate how many
    // entries fit into the viewport for half/full-page navigation.
    app.entry_page = (inner_height.saturating_mul(items.len())
        / total_height.max(1))
    .clamp(1, items.len().max(1));

    app.entry_state.select(Some(app.entry_cursor));
    let highlight = if focused {
        pal.selected()
    } else {
        pal.inactive_selection()
    };

    let list = List::new(items)
        .block(block)
        .highlight_style(highlight)
        .highlight_symbol("▸ ");

    frame.render_stateful_widget(list, area, &mut app.entry_state);
}

fn entry_item(
    entry: &miniflux_api::models::Entry,
    pal: &Palette,
    width: usize,
) -> (ListItem<'static>, usize) {
    let unread = entry.status == EntryStatus::Unread;
    let marker = if unread { "●" } else { "○" };
    let marker_style = if unread {
        Style::default().fg(pal.color(tokens::ACCENT_PRIMARY))
    } else {
        pal.dim()
    };
    let star = if entry.starred { "★" } else { " " };
    let star_style = Style::default().fg(pal.color(tokens::WARNING));
    let title_style = if unread { pal.strong() } else { pal.muted() };

    let title = if entry.title.trim().is_empty() {
        "(untitled)".to_string()
    } else {
        entry.title.clone()
    };
    let title_width = width.saturating_sub(4).max(1);
    let mut title_lines = util::wrap_text(&title, title_width).into_iter();
    let first_title_line = title_lines.next().unwrap_or_default();

    let title_line = Line::from(vec![
        Span::styled(format!("{marker} "), marker_style),
        Span::styled(format!("{star} "), star_style),
        Span::styled(first_title_line, title_style),
    ]);

    let mut meta = format!(
        "{} · {}",
        entry.feed.title,
        util::relative_time(entry.published_at)
    );
    if entry.reading_time > 0 {
        meta.push_str(&format!(" · {} min", entry.reading_time));
    }
    let mut lines = vec![title_line];

    for title_line in title_lines {
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(title_line, title_style),
        ]));
    }
    let mut height = lines.len();

    let meta_line = Line::from(vec![
        Span::raw("  "),
        Span::styled(util::truncate(&meta, width.saturating_sub(4)), pal.dim()),
    ]);
    lines.push(meta_line);
    height += 1;

    (ListItem::new(lines), height)
}

fn draw_article(frame: &mut Frame, app: &mut App, area: Rect, pal: &Palette) {
    let focused = app.focus == Focus::Article;
    let inner_width = area.width.saturating_sub(2).max(1) as usize;
    let inner_height = area.height.saturating_sub(2) as usize;

    let Some((
        title,
        author,
        feed,
        published,
        reading_time,
        url,
        tags,
        starred,
        unread,
    )) = app.selected_entry().map(|entry| {
        (
            entry.title.clone(),
            entry.author.clone(),
            entry.feed.title.clone(),
            entry.published_at,
            entry.reading_time,
            entry.url.clone(),
            entry.tags.clone(),
            entry.starred,
            entry.status == EntryStatus::Unread,
        )
    })
    else {
        let block = pane_block("Reader", None, focused, pal, true);
        let paragraph = Paragraph::new("Select an entry to read")
            .alignment(Alignment::Center)
            .style(pal.dim())
            .block(block);
        frame.render_widget(paragraph, area);
        app.article_max_scroll = 0;
        app.article_scroll = 0;
        app.article_page = inner_height as u16;
        return;
    };

    let mut lines: Vec<Line<'static>> = Vec::new();

    let title = if title.trim().is_empty() {
        "(untitled)".to_string()
    } else {
        title
    };
    for line in util::wrap_text(&title, inner_width) {
        lines.push(Line::from(Span::styled(line, pal.heading())));
    }
    lines.push(Line::from(""));

    let mut meta_parts: Vec<String> = Vec::new();
    if !author.trim().is_empty() {
        meta_parts.push(author);
    }
    meta_parts.push(feed);
    meta_parts.push(util::format_datetime(published));
    if reading_time > 0 {
        meta_parts.push(format!("{reading_time} min read"));
    }
    if starred {
        meta_parts.push("★ bookmarked".to_string());
    }
    if unread {
        meta_parts.push("● unread".to_string());
    }

    lines.push(Line::from(Span::styled(
        util::truncate(&meta_parts.join("  ·  "), inner_width),
        pal.muted(),
    )));
    lines.push(Line::from(Span::styled(
        util::truncate(&url, inner_width),
        pal.dim(),
    )));

    if !tags.is_empty() {
        let tags = tags
            .iter()
            .map(|tag| format!("#{tag}"))
            .collect::<Vec<_>>()
            .join("  ");
        lines.push(Line::from(Span::styled(
            util::truncate(&tags, inner_width),
            Style::default().fg(pal.color(tokens::ACCENT_TERTIARY)),
        )));
    }

    lines.push(Line::from(Span::styled(
        "─".repeat(inner_width),
        Style::default().fg(pal.color(tokens::BORDER_UNFOCUSED)),
    )));
    lines.push(Line::from(""));

    if app.article_text.trim().is_empty() {
        lines.push(Line::from(Span::styled(
            "No content available. Press o to open the original article.",
            pal.dim(),
        )));
    } else {
        for line in util::wrap_text(&app.article_text, inner_width) {
            lines.push(Line::from(line));
        }
    }

    let total = lines.len();
    let max_scroll = total.saturating_sub(inner_height);
    app.article_max_scroll = max_scroll.min(u16::MAX as usize) as u16;
    app.article_page = inner_height.min(u16::MAX as usize) as u16;
    let scroll = app.article_scroll.min(app.article_max_scroll);
    app.article_scroll = scroll;

    let percent = if app.article_max_scroll == 0 {
        100
    } else {
        (scroll as usize * 100 / app.article_max_scroll as usize).min(100)
    };
    let block =
        pane_block("Reader", Some(format!("{percent}%")), focused, pal, true);

    let paragraph = Paragraph::new(lines)
        .style(pal.panel())
        .scroll((scroll, 0))
        .block(block);

    frame.render_widget(paragraph, area);
}

fn pane_block(
    title: &str,
    right: Option<String>,
    focused: bool,
    pal: &Palette,
    panel: bool,
) -> Block<'static> {
    let left =
        Line::from(Span::styled(format!(" {title} "), pal.title(focused)));
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(pal.border(focused))
        .style(if panel { pal.panel() } else { pal.base() })
        .title_top(left);

    if let Some(right) = right {
        block = block.title_top(
            Line::from(Span::styled(format!(" {right} "), pal.muted()))
                .alignment(Alignment::Right),
        );
    }

    block
}

fn draw_status(frame: &mut Frame, app: &mut App, area: Rect, pal: &Palette) {
    frame.render_widget(Block::default().style(pal.base()), area);

    if let Mode::Search { input } = &app.mode {
        draw_input_prompt(frame, area, "/", input, pal);
        return;
    }

    if let Mode::Subscribe { input } = &app.mode {
        draw_input_prompt(frame, area, "Website: ", input, pal);
        return;
    }

    if let Mode::Confirm { prompt, .. } = &app.mode {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("? ", pal.status(StatusKind::Warning)),
                Span::styled(prompt.clone(), pal.text()),
                Span::styled("  [y/n]", pal.muted()),
            ]))
            .style(pal.base()),
            area,
        );
        return;
    }

    if let Some(status) = &app.status {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    util::truncate(
                        &status.text,
                        area.width.saturating_sub(2) as usize,
                    ),
                    pal.status(status.kind),
                ),
            ]))
            .style(pal.base()),
            area,
        );
        return;
    }

    let hint = app
        .selected_entry()
        .map(|entry| entry.url.clone())
        .unwrap_or_else(|| "Press ? for help".to_string());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                util::truncate(&hint, area.width.saturating_sub(2) as usize),
                pal.dim(),
            ),
        ]))
        .style(pal.base()),
        area,
    );
}

fn draw_input_prompt(
    frame: &mut Frame,
    area: Rect,
    prefix: &str,
    input: &str,
    pal: &Palette,
) {
    let prefix_width = util::display_width(prefix);
    let available = (area.width as usize).saturating_sub(prefix_width + 1);
    let shown = if util::display_width(input) <= available {
        input.to_string()
    } else {
        let mut result = String::new();
        let mut width = 0;
        for ch in input.chars().rev() {
            let ch_width =
                unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if width + ch_width > available {
                break;
            }
            result.insert(0, ch);
            width += ch_width;
        }
        result
    };

    let x = area.x
        + (prefix_width + util::display_width(&shown))
            .min(area.width.saturating_sub(1) as usize) as u16;
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(prefix.to_string(), pal.key()),
            Span::styled(shown, pal.text()),
        ]))
        .style(pal.base()),
        area,
    );
    frame.set_cursor_position((x, area.y));
}

fn draw_keybar(frame: &mut Frame, app: &App, area: Rect, pal: &Palette) {
    frame.render_widget(Block::default().style(pal.panel()), area);

    let hints: &[(&str, &str)] = match &app.mode {
        Mode::Search { .. } => {
            &[("Enter", "search"), ("Esc", "cancel"), ("Ctrl-u", "clear")]
        }
        Mode::Subscribe { .. } => &[
            ("Enter", "discover"),
            ("Esc", "cancel"),
            ("Ctrl-u", "clear"),
        ],
        Mode::FeedPicker { .. } => {
            &[("j/k", "select"), ("Enter", "subscribe"), ("Esc", "cancel")]
        }
        Mode::Help { .. } => &[("j/k", "scroll"), ("q", "close")],
        Mode::Confirm { .. } => &[("y", "confirm"), ("n", "cancel")],
        Mode::ThemePicker { .. } => {
            &[("j/k", "preview"), ("Enter", "apply"), ("Esc", "cancel")]
        }
        Mode::Normal => match app.focus {
            Focus::Feeds => &[
                ("j/k", "select"),
                ("l", "entries"),
                ("n", "new feed"),
                (
                    "f",
                    if app.hide_read_feeds {
                        "show read"
                    } else {
                        "hide read"
                    },
                ),
                ("m", "read"),
                ("M", "feed read"),
                ("s", "star"),
                ("S", "save"),
                ("Ctrl-o", "XDG"),
                ("a", "all read"),
                ("/", "search"),
                ("r", "refresh"),
                ("t", "theme"),
                ("?", "help"),
                ("q", "quit"),
            ],
            Focus::Entries => &[
                ("j/k", "move"),
                ("Enter", "read"),
                (
                    "f",
                    if app.hide_read_entries {
                        "show read"
                    } else {
                        "hide read"
                    },
                ),
                ("m", "read"),
                ("M", "feed read"),
                ("s", "star"),
                ("S", "save"),
                ("F", "fetch"),
                ("o", "browser"),
                ("Ctrl-o", "XDG"),
                ("/", "search"),
                ("t", "theme"),
                ("?", "help"),
                ("q", "quit"),
            ],
            Focus::Article => &[
                ("j/k", "scroll"),
                ("J/K", "next/prev"),
                ("g/G", "top/end"),
                ("Ctrl-d/u", "half page"),
                ("PgUp/Dn", "page"),
                ("h", "entries"),
                ("o", "browser"),
                ("Ctrl-o", "XDG"),
                ("S", "save"),
                ("F", "fetch"),
                ("m", "read"),
                ("M", "feed read"),
                ("q", "quit"),
            ],
        },
    };

    let mut spans = vec![Span::raw(" ")];
    for (index, (key, label)) in hints.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  ", pal.dim()));
        }
        spans.push(Span::styled(*key, pal.key()));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(*label, pal.muted()));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(pal.panel()),
        area,
    );
}

fn draw_help(frame: &mut Frame, app: &mut App, area: Rect, pal: &Palette) {
    let area = centered_rect(72, 82, area);
    frame.render_widget(Clear, area);

    let lines = help_lines(pal);
    let total = lines.len();
    let inner_height = area.height.saturating_sub(2) as usize;
    let max_scroll =
        total.saturating_sub(inner_height).min(u16::MAX as usize) as u16;

    let scroll = if let Mode::Help { scroll } = &mut app.mode {
        *scroll = (*scroll).min(max_scroll);
        *scroll
    } else {
        0
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(pal.border(true))
        .style(pal.panel())
        .title_top(Line::from(Span::styled(" Help ", pal.heading())))
        .title_top(
            Line::from(Span::styled(" q / Esc close ", pal.muted()))
                .alignment(Alignment::Right),
        );

    frame.render_widget(
        Paragraph::new(lines)
            .style(pal.panel())
            .scroll((scroll, 0))
            .block(block),
        area,
    );
}

fn help_lines(pal: &Palette) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let section = |lines: &mut Vec<Line<'static>>, title: &str| {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  {title}"),
            pal.heading(),
        )));
    };

    lines.push(Line::from(Span::styled(
        "  fluxgrove · vim keys for Miniflux",
        pal.muted(),
    )));

    section(&mut lines, "Navigation");
    lines.push(help_key("j / k", "move cursor, scroll article", pal));
    lines.push(help_key("h / l", "focus pane left / right", pal));
    lines.push(help_key("Tab", "cycle panes", pal));
    lines.push(help_key("1 / 2 / 3", "focus feeds / entries / reader", pal));
    lines.push(help_key("g / G", "go to top / bottom", pal));
    lines.push(help_key(
        "n",
        "subscribe to a website (feeds are detected)",
        pal,
    ));
    lines.push(help_key(
        "f (Feeds)",
        "hide / show feeds without unread entries",
        pal,
    ));
    lines.push(help_key("f (Entries)", "hide / show read articles", pal));
    lines.push(help_key("Ctrl-d / Ctrl-u", "half page down / up", pal));
    lines.push(help_key("PgUp / PgDn", "page up / down", pal));
    lines.push(help_key("Ctrl-f / Ctrl-b", "full page down / up", pal));
    lines.push(help_key("Enter", "open entry in the reader", pal));
    lines.push(help_key("Esc", "go back one pane", pal));

    section(&mut lines, "Reading");
    lines.push(help_key("m", "mark read (and advance) / unread", pal));
    lines.push(help_key(
        "M",
        "mark every entry in the current feed as read",
        pal,
    ));
    lines.push(help_key(
        "J / K",
        "next / previous article (marks it read)",
        pal,
    ));
    lines.push(help_key("s", "toggle star / bookmark", pal));
    lines.push(help_key(
        "S",
        "save the article to a Miniflux integration",
        pal,
    ));
    lines.push(help_key(
        "F",
        "fetch and display the original article content",
        pal,
    ));
    lines.push(help_key("a", "mark everything in view as read", pal));
    lines.push(help_key("o", "open the original article in a browser", pal));
    lines.push(help_key(
        "Ctrl-o",
        "open the original article in the XDG default browser",
        pal,
    ));
    lines.push(help_key("r", "ask Miniflux to refresh feeds", pal));

    section(&mut lines, "Searching");
    lines.push(help_key("/", "search entries", pal));
    lines.push(help_key("Enter", "apply the search", pal));
    lines.push(help_key("Esc", "cancel the search", pal));
    lines.push(help_key("c", "clear the active search", pal));

    section(&mut lines, "Appearance");
    lines.push(help_key("t", "choose a theme (live preview)", pal));
    lines.push(help_key("q", "quit", pal));
    lines.push(help_key("?", "toggle this help", pal));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Tips: feeds and categories are selectable in the sidebar;",
        pal.dim(),
    )));
    lines.push(Line::from(Span::styled(
        "  opening an unread entry marks it as read automatically.",
        pal.dim(),
    )));
    lines
}

fn help_key(key: &str, description: &str, pal: &Palette) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {key:<16}"), pal.key()),
        Span::styled(description.to_string(), pal.text()),
    ])
}

fn draw_confirm(frame: &mut Frame, app: &App, area: Rect, pal: &Palette) {
    let area = centered_rect(62, 24, area);
    frame.render_widget(Clear, area);

    let prompt = if let Mode::Confirm { prompt, .. } = &app.mode {
        prompt.clone()
    } else {
        String::new()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(pal.color(tokens::WARNING)))
        .style(pal.panel())
        .title_top(Line::from(Span::styled(
            " Confirm ",
            Style::default()
                .fg(pal.color(tokens::WARNING))
                .add_modifier(Modifier::BOLD),
        )));

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            util::truncate(&prompt, area.width.saturating_sub(4) as usize),
            pal.text(),
        ))
        .alignment(Alignment::Center),
        Line::from(""),
        Line::from(vec![
            Span::styled("[y]", pal.key()),
            Span::raw(" yes    "),
            Span::styled("[n]", pal.key()),
            Span::raw(" no"),
        ])
        .alignment(Alignment::Center),
    ];

    frame.render_widget(
        Paragraph::new(text).style(pal.panel()).block(block),
        area,
    );
}

fn draw_feed_picker(frame: &mut Frame, app: &App, area: Rect, pal: &Palette) {
    let Mode::FeedPicker {
        subscriptions,
        cursor,
        ..
    } = &app.mode
    else {
        return;
    };

    let max_height = area.height.saturating_sub(2).max(7);
    let height = (subscriptions.len() as u16 * 2 + 6).clamp(9, max_height);
    let width = (area.width * 68 / 100)
        .max(24)
        .min(area.width.saturating_sub(2).max(1));
    let area = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, area);

    let inner_width = area.width.saturating_sub(4) as usize;
    let items: Vec<ListItem> = subscriptions
        .iter()
        .map(|subscription| {
            let title = if subscription.title.trim().is_empty() {
                "Untitled feed".to_string()
            } else {
                subscription.title.clone()
            };
            let kind = if subscription.kind.trim().is_empty() {
                "feed".to_string()
            } else {
                subscription.kind.clone()
            };
            ListItem::new(vec![
                Line::from(Span::styled(
                    util::truncate(&title, inner_width),
                    pal.strong(),
                )),
                Line::from(Span::styled(
                    util::truncate(
                        &format!("{kind} · {}", subscription.url),
                        inner_width,
                    ),
                    pal.dim(),
                )),
            ])
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(pal.border(true))
        .style(pal.panel())
        .title_top(Line::from(Span::styled(" Choose feed ", pal.heading())))
        .title_top(
            Line::from(Span::styled(
                format!(" {} found ", subscriptions.len()),
                pal.muted(),
            ))
            .alignment(Alignment::Right),
        )
        .title_bottom(
            Line::from(Span::styled(
                " j/k select · Enter subscribe · Esc cancel ",
                pal.muted(),
            ))
            .alignment(Alignment::Center),
        );

    let mut state = ListState::default().with_selected(Some(*cursor));
    let list = List::new(items)
        .block(block)
        .highlight_style(pal.selected())
        .highlight_symbol("▸ ");

    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_theme_picker(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    pal: &Palette,
) {
    let area = centered_rect(54, 74, area);
    frame.render_widget(Clear, area);

    let cursor = if let Mode::ThemePicker { cursor, .. } = &app.mode {
        *cursor
    } else {
        0
    };

    let items: Vec<ListItem> = app
        .theme_choices
        .iter()
        .map(|(id, name)| {
            let marker = if id == &app.theme_id { "● " } else { "  " };
            ListItem::new(Line::from(vec![
                Span::styled(marker, pal.accent()),
                Span::styled(name.clone(), pal.text()),
            ]))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(pal.border(true))
        .style(pal.panel())
        .title_top(Line::from(Span::styled(" Theme ", pal.heading())))
        .title_bottom(
            Line::from(Span::styled(
                " j/k preview · Enter apply · Esc cancel ",
                pal.muted(),
            ))
            .alignment(Alignment::Center),
        );

    let mut state = ListState::default().with_selected(Some(cursor));
    let list = List::new(items)
        .block(block)
        .highlight_style(pal.selected())
        .highlight_symbol("▸ ");

    frame.render_stateful_widget(list, area, &mut state);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let width = (area.width * percent_x / 100)
        .max(24)
        .min(area.width.saturating_sub(2));
    let height = (area.height * percent_y / 100)
        .max(7)
        .min(area.height.saturating_sub(2));
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}
