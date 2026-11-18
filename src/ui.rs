use miniflux_api::models::EntryStatus;
use opaline::{
    Theme,
    names::{styles, tokens},
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};

use crate::{
    api,
    app::{
        App, FeedField, FeedForm, Focus, LoadedImage, Mode, SearchScope,
        SidebarRow, StatusKind, TextInput,
    },
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
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);

    draw_body(frame, app, rows[0], &pal);
    draw_status(frame, app, rows[1], &pal);
    draw_keybar(frame, app, rows[2], &pal);

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
    if matches!(app.mode, Mode::EditFeed { .. }) {
        // Keep the status line and key bar visible while editing.
        draw_feed_editor(frame, app, rows[0], &pal);
        draw_category_picker(frame, app, rows[0], &pal);
        draw_feed_delete_confirm(frame, app, rows[0], &pal);
    }
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

    let feeds_counter = if !app.feed_filter.trim().is_empty() {
        format!("/{} · {}", app.feed_filter.trim(), app.unread_total())
    } else if app.hide_read_feeds {
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
            "Everything".to_string(),
            Style::default().fg(pal.color(tokens::ACCENT_PRIMARY)),
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
    let mut entries_heading = app.selection_label();
    if !app.search.is_empty() {
        entries_heading.push_str(&format!(" /{}", app.search));
    }
    let entries_title =
        fitted_title(&entries_heading, Some(&entries_counter), area.width);
    let block =
        pane_block(&entries_title, Some(entries_counter), focused, pal, false);

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
    // A list item taller than the viewport makes ratatui render no items at
    // all, blanking the pane, so no entry may grow past the visible height.
    let max_item_height = inner_height.max(1);
    let items: Vec<ListItem> = app
        .entries
        .iter()
        .map(|entry| {
            let (item, height) =
                entry_item(entry, pal, inner_width, max_item_height);
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

/// Truncate a wrapped title line to `width` columns and mark it as elided.
fn elide(line: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let mut clipped = util::truncate(line, width);
    if clipped.ends_with('…') {
        return clipped;
    }
    if util::display_width(&clipped) >= width {
        clipped.pop();
    }
    clipped.push('…');
    clipped
}

fn entry_item(
    entry: &miniflux_api::models::Entry,
    pal: &Palette,
    width: usize,
    max_lines: usize,
) -> (ListItem<'static>, usize) {
    let unread = entry.status == EntryStatus::Unread;
    let marker = if unread { "●" } else { "○" };
    let marker_style = if unread {
        Style::default().fg(pal.color(tokens::ACCENT_PRIMARY))
    } else {
        pal.dim()
    };
    let star_style = Style::default().fg(pal.color(tokens::WARNING));
    let title_style = if unread { pal.strong() } else { pal.muted() };

    let title = if entry.title.trim().is_empty() {
        "(untitled)".to_string()
    } else {
        entry.title.clone()
    };
    let title_width = width.saturating_sub(2).max(1);
    let max_lines = max_lines.max(1);
    // Reserve the metadata line unless the viewport only has room for a title.
    let show_meta = max_lines > 1;
    let title_budget = if show_meta { max_lines - 1 } else { max_lines };
    let mut title_lines = util::wrap_text(&title, title_width);
    let elided = title_lines.len() > title_budget;
    title_lines.truncate(title_budget);
    if elided && let Some(last) = title_lines.last_mut() {
        *last = elide(last, title_width);
    }

    let mut title_lines = title_lines.into_iter();
    let first_title_line = title_lines.next().unwrap_or_default();
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{marker} "), marker_style),
        Span::styled(first_title_line, title_style),
    ])];

    for title_line in title_lines {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(title_line, title_style),
        ]));
    }

    if show_meta {
        let mut meta = format!(
            "{} · {}",
            entry.feed.title,
            util::relative_time(entry.published_at)
        );
        if entry.reading_time > 0 {
            meta.push_str(&format!(" · {} min", entry.reading_time));
        }
        let star = if entry.starred { " · ★" } else { "" };
        let meta_width =
            width.saturating_sub(2 + util::display_width(star)).max(1);
        let mut spans = vec![
            Span::raw("  "),
            Span::styled(util::truncate(&meta, meta_width), pal.dim()),
        ];
        if entry.starred {
            spans.push(Span::styled(star, star_style));
        }
        lines.push(Line::from(spans));
    }

    let height = lines.len();
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

    // The pane header carries the title; the body starts with the metadata.
    let title = if title.trim().is_empty() {
        "(untitled)".to_string()
    } else {
        title
    };

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

    let mut images: Vec<ImagePlacement> = Vec::new();
    let mut paragraph_starts: Vec<usize> = Vec::new();
    if app.article_parts.is_empty() {
        lines.push(Line::from(Span::styled(
            "No content available. Press o to open the original article.",
            pal.dim(),
        )));
    } else {
        for part in &app.article_parts {
            match part {
                util::ArticlePart::Text(text) => {
                    let mut after_blank = true;
                    for line in util::wrap_text(text, inner_width) {
                        let blank = line.trim().is_empty();
                        if !blank && after_blank {
                            paragraph_starts.push(lines.len());
                        }
                        after_blank = blank;
                        lines.push(Line::from(line));
                    }
                }
                util::ArticlePart::Image { url, alt } => {
                    let label = if alt.trim().is_empty() { url } else { alt };
                    let start = lines.len();
                    paragraph_starts.push(start);
                    if !app.show_images {
                        lines.push(Line::from(Span::styled(
                            util::truncate(
                                &format!("[image] {label}"),
                                inner_width,
                            ),
                            pal.dim(),
                        )));
                        continue;
                    }
                    match app.images.get(url) {
                        Some(LoadedImage::Ready(image)) => {
                            let (width, rows) =
                                image_geometry(image, inner_width);
                            lines.extend(
                                std::iter::repeat_with(|| Line::from(""))
                                    .take(rows),
                            );
                            images.push(ImagePlacement {
                                url: url.clone(),
                                start,
                                width,
                                rows,
                            });
                        }
                        Some(LoadedImage::Loading) => {
                            lines.push(Line::from(Span::styled(
                                "Loading image…",
                                pal.dim(),
                            )));
                        }
                        Some(LoadedImage::Failed) => {
                            lines.push(Line::from(Span::styled(
                                util::truncate(
                                    &format!("[image unavailable] {label}"),
                                    inner_width,
                                ),
                                pal.dim(),
                            )));
                        }
                        None => {
                            lines.push(Line::from(Span::styled(
                                util::truncate(
                                    &format!("[image] {label}"),
                                    inner_width,
                                ),
                                pal.dim(),
                            )));
                        }
                    }
                }
            }
        }
    }

    app.article_paragraph_stops = paragraph_starts;
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
    let percent_label = format!("{percent}%");
    let reader_title = fitted_title(&title, Some(&percent_label), area.width);
    let block =
        pane_block(&reader_title, Some(percent_label), focused, pal, true);

    let paragraph = Paragraph::new(lines)
        .style(pal.panel())
        .scroll((scroll, 0))
        .block(block);

    frame.render_widget(paragraph, area);

    for placement in &images {
        if let Some(LoadedImage::Ready(image)) = app.images.get(&placement.url)
        {
            render_article_image(frame, image, placement, scroll, area, pal);
        }
    }
}

/// Where an image sits inside the reader's line-based content.
struct ImagePlacement {
    url: String,
    start: usize,
    width: usize,
    rows: usize,
}

/// Guard against pathological images filling the line list.
const MAX_IMAGE_ROWS: usize = 4096;

/// Scale an image to the full reader width, keeping its aspect ratio.
fn image_geometry(
    image: &api::DecodedImage,
    max_width: usize,
) -> (usize, usize) {
    let max_width = max_width.max(1);
    let src_w = image.width.max(1) as f64;
    let src_h = image.height.max(1) as f64;
    // Two source pixels per cell row: terminal cells are about twice as tall
    // as they are wide.
    let rows = (max_width as f64 * src_h / src_w / 2.0).ceil();
    let width = max_width;
    let rows = (rows.round() as usize).clamp(1, MAX_IMAGE_ROWS);
    (width, rows)
}

fn render_article_image(
    frame: &mut Frame,
    image: &api::DecodedImage,
    placement: &ImagePlacement,
    scroll: u16,
    area: Rect,
    pal: &Palette,
) {
    let inner_height = area.height.saturating_sub(2) as usize;
    let offset = placement.start as isize - scroll as isize;
    let first = offset.max(0) as usize;
    let last = (offset + placement.rows as isize)
        .clamp(0, inner_height as isize) as usize;
    if last <= first {
        return;
    }

    let background = pal.color(tokens::BG_PANEL);
    let buffer = frame.buffer_mut();
    for screen_row in first..last {
        let image_row = (screen_row as isize - offset) as usize;
        let y = area.y + 1 + screen_row as u16;
        for x in 0..placement.width {
            let top = blend_pixel(
                sample_image(image, placement, x, image_row * 2),
                background,
            );
            let bottom = blend_pixel(
                sample_image(image, placement, x, image_row * 2 + 1),
                background,
            );
            if let Some(cell) = buffer.cell_mut((area.x + 1 + x as u16, y)) {
                cell.set_symbol("▀");
                cell.set_fg(top);
                cell.set_bg(bottom);
            }
        }
    }
}

/// Nearest-neighbour sample of the source image at a destination pixel.
fn sample_image(
    image: &api::DecodedImage,
    placement: &ImagePlacement,
    x: usize,
    y: usize,
) -> [u8; 4] {
    let src_w = image.width.max(1) as u64;
    let src_h = image.height.max(1) as u64;
    let dst_w = placement.width.max(1) as u64;
    let dst_h = (placement.rows * 2).max(1) as u64;
    let sx = (x as u64 * src_w / dst_w).min(src_w - 1);
    let sy = (y as u64 * src_h / dst_h).min(src_h - 1);
    let index = ((sy * src_w + sx) * 4) as usize;
    image
        .rgba
        .get(index..index + 4)
        .map_or([0, 0, 0, 0], |pixel| {
            [pixel[0], pixel[1], pixel[2], pixel[3]]
        })
}

/// Blend a pixel over the panel background when it is translucent.
fn blend_pixel(pixel: [u8; 4], background: Color) -> Color {
    let [red, green, blue, alpha] = pixel;
    if alpha == 255 {
        return Color::Rgb(red, green, blue);
    }
    let (bg_red, bg_green, bg_blue) = match background {
        Color::Rgb(red, green, blue) => (red, green, blue),
        _ => (0, 0, 0),
    };
    let alpha = u16::from(alpha);
    let mix = |foreground: u8, backdrop: u8| {
        ((u16::from(foreground) * alpha + u16::from(backdrop) * (255 - alpha))
            / 255) as u8
    };
    Color::Rgb(mix(red, bg_red), mix(green, bg_green), mix(blue, bg_blue))
}

/// Fit a pane title next to its right-hand label.
fn fitted_title(title: &str, right: Option<&str>, width: u16) -> String {
    let reserved = right.map_or(0, |right| util::display_width(right) + 4);
    let available = (width as usize).saturating_sub(reserved).max(1);
    util::truncate(title, available)
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

    if let Mode::Search { input, scope, .. } = &app.mode {
        let prefix = match scope {
            SearchScope::Entries => "/",
            SearchScope::Feeds => "Feeds: ",
        };
        draw_input_prompt(frame, area, prefix, input, pal);
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

/// The slice of `text` to show so the caret stays visible, and its column.
fn input_window(text: &str, cursor: usize, width: usize) -> (usize, usize) {
    let width = width.max(1);
    let mut used = 0;
    let mut start = cursor;
    for (index, ch) in text[..cursor].char_indices().rev() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > width.saturating_sub(1) {
            break;
        }
        used += ch_width;
        start = index;
    }
    (start, used)
}

fn draw_input_prompt(
    frame: &mut Frame,
    area: Rect,
    prefix: &str,
    input: &TextInput,
    pal: &Palette,
) {
    let prefix_width = util::display_width(prefix);
    let available = (area.width as usize).saturating_sub(prefix_width + 1);
    let (start, caret) =
        input_window(input.as_str(), input.cursor(), available);
    let shown = &input.as_str()[start..];

    let x = area.x
        + (prefix_width + caret).min(area.width.saturating_sub(1) as usize)
            as u16;
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(prefix.to_string(), pal.key()),
            Span::styled(shown.to_string(), pal.text()),
        ]))
        .style(pal.base()),
        area,
    );
    frame.set_cursor_position((x, area.y));
}

fn draw_keybar(frame: &mut Frame, app: &App, area: Rect, pal: &Palette) {
    frame.render_widget(Block::default().style(pal.panel()), area);

    let hints: &[(&str, &str)] = match &app.mode {
        Mode::Search {
            scope: SearchScope::Feeds,
            ..
        } => &[("Enter", "filter"), ("Esc", "cancel"), ("Ctrl-u", "clear")],
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
        // The feed editor shows its hints inside the dialog.
        Mode::EditFeed { .. } => &[],
        Mode::Normal => match app.focus {
            Focus::Feeds => &[
                ("j/k", "select"),
                ("l", "entries"),
                ("n", "new feed"),
                ("E", "edit feed"),
                ("/", "filter"),
                (
                    "f",
                    if app.hide_read_feeds {
                        "show read"
                    } else {
                        "hide read"
                    },
                ),
                ("m", "read"),
                ("M", "view read"),
                ("s", "star"),
                ("S", "save"),
                ("Ctrl-o", "XDG"),
                ("a", "all read"),
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
                ("M", "view read"),
                ("p", "feed"),
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
                ("Space", "paragraph"),
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
                ("M", "view read"),
                ("p", "feed"),
                ("i", "images"),
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

    let inner_width = area.width.saturating_sub(2) as usize;
    let inner_height = area.height.saturating_sub(2) as usize;
    let lines = help_lines(inner_width, pal);
    let total = lines.len();
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

enum HelpRow {
    Section(&'static str),
    Key(&'static str, &'static str),
    Note(&'static str),
    Tip(&'static str),
    Blank,
}

/// Lay the help out for the modal's inner width.
///
/// Descriptions wrap under their key column, so the content always fits the
/// modal and the scroll limit matches the rendered line count.
fn help_lines(width: usize, pal: &Palette) -> Vec<Line<'static>> {
    const INDENT: usize = 2;
    const KEY_WIDTH: usize = 16;
    let indent = " ".repeat(INDENT);

    let mut lines = Vec::new();
    for row in help_rows() {
        match row {
            HelpRow::Blank => lines.push(Line::from("")),
            HelpRow::Section(title) => {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!("{indent}{title}"),
                    pal.heading(),
                )));
            }
            HelpRow::Note(text) => {
                help_paragraph(text, width, INDENT, pal.muted(), &mut lines);
            }
            HelpRow::Tip(text) => {
                help_paragraph(text, width, INDENT, pal.dim(), &mut lines);
            }
            HelpRow::Key(key, description) => {
                let available = width.saturating_sub(INDENT + KEY_WIDTH).max(8);
                let mut wrapped = util::wrap_text(description, available);
                if wrapped.is_empty() {
                    wrapped.push(String::new());
                }
                let mut wrapped = wrapped.into_iter();
                let first = wrapped.next().unwrap_or_default();
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{indent}{key:<KEY_WIDTH$}"),
                        pal.key(),
                    ),
                    Span::styled(first, pal.text()),
                ]));
                for continuation in wrapped {
                    lines.push(Line::from(vec![
                        Span::raw(" ".repeat(INDENT + KEY_WIDTH)),
                        Span::styled(continuation, pal.text()),
                    ]));
                }
            }
        }
    }
    lines
}

fn help_paragraph(
    text: &str,
    width: usize,
    indent: usize,
    style: Style,
    lines: &mut Vec<Line<'static>>,
) {
    let padding = " ".repeat(indent);
    let available = width.saturating_sub(indent).max(1);
    for line in util::wrap_text(text, available) {
        lines.push(Line::from(Span::styled(format!("{padding}{line}"), style)));
    }
}

fn help_rows() -> Vec<HelpRow> {
    use HelpRow::{Blank, Key, Note, Section, Tip};

    vec![
        Note("fluxgrove · vim keys for Miniflux"),
        Section("Navigation"),
        Key("j / k", "move cursor, scroll article"),
        Key("h / l", "focus pane left / right"),
        Key("Tab", "cycle panes"),
        Key("1 / 2 / 3", "focus feeds / entries / reader"),
        Key("g / G", "go to top / bottom"),
        Key("n", "subscribe to a website (feeds are detected)"),
        Key("E", "edit the selected feed"),
        Key("f (Feeds)", "hide / show feeds without unread entries"),
        Key("f (Entries)", "hide / show read articles"),
        Key("Ctrl-d / Ctrl-u", "half page down / up"),
        Key("PgUp / PgDn", "page up / down"),
        Key("Space", "scroll to the next paragraph"),
        Key("Ctrl-f / Ctrl-b", "full page down / up"),
        Key("Enter", "open entry in the reader"),
        Key("Esc", "clear the search or feed filter, or go back"),
        Section("Reading"),
        Key("m", "mark read (and advance) / unread"),
        Key("M", "mark every entry in the current view as read"),
        Key("J / K", "next / previous article (marks it read)"),
        Key("s", "toggle star / bookmark"),
        Key("S", "save the article to a Miniflux integration"),
        Key("F", "fetch and display the original article content"),
        Key("a", "mark everything in view as read"),
        Key("o", "open the original article in a browser"),
        Key(
            "Ctrl-o",
            "open the original article in the XDG default browser",
        ),
        Key("r", "ask Miniflux to refresh feeds"),
        Key("i", "show or hide embedded images"),
        Section("Text input"),
        Key("← / →", "move the caret (Ctrl/Alt by word)"),
        Key("Home / End", "start / end (also Ctrl-A / Ctrl-E)"),
        Key("Delete", "delete under the caret"),
        Key("Ctrl-W / Ctrl-K", "delete word / to the end"),
        Section("Searching"),
        Key("/", "search entries, or filter feeds as you type"),
        Key("Enter", "apply the search"),
        Key("Esc", "cancel the search"),
        Key("c", "clear the active search"),
        Section("Appearance"),
        Key("t", "choose a theme (live preview)"),
        Key("q", "quit"),
        Key("?", "toggle this help"),
        Blank,
        Tip("Tips: feeds and categories are selectable in the sidebar;"),
        Tip("opening an unread entry marks it as read automatically."),
    ]
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

    // Two content lines per feed plus the border and titles. Never let the
    // popup exceed the frame: a short terminal must still render.
    let max_height = area.height.saturating_sub(2).max(1).min(area.height);
    let desired_height =
        subscriptions.len().saturating_mul(2).saturating_add(6);
    let height = u16::try_from(desired_height)
        .unwrap_or(u16::MAX)
        .min(max_height);
    let width = ((u32::from(area.width) * 68 / 100) as u16)
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

fn checkbox(value: bool) -> String {
    if value {
        "[x]".to_string()
    } else {
        "[ ]".to_string()
    }
}

fn feed_field_value(
    app: &App,
    form: &FeedForm,
    field: FeedField,
    reveal_password: bool,
) -> (String, bool) {
    match field {
        FeedField::Category => {
            let title = app
                .categories
                .iter()
                .find(|category| category.id == form.category_id)
                .map(|category| category.title.clone())
                .unwrap_or_else(|| format!("category {}", form.category_id));
            return (title, false);
        }
        FeedField::Crawler => return (checkbox(form.crawler), false),
        FeedField::Disabled => return (checkbox(form.disabled), false),
        FeedField::IgnoreHttpCache => {
            return (checkbox(form.ignore_http_cache), false);
        }
        FeedField::FetchViaProxy => {
            return (checkbox(form.fetch_via_proxy), false);
        }
        FeedField::Password
            if !reveal_password && !form.password.is_empty() =>
        {
            return ("••••••".to_string(), false);
        }
        _ => {}
    }
    let text = form.text(field).unwrap_or_default();
    if text.is_empty() {
        ("(empty)".to_string(), true)
    } else {
        (text.to_string(), false)
    }
}

fn draw_feed_editor(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    pal: &Palette,
) {
    let Mode::EditFeed {
        form,
        cursor,
        editing,
        category_picker,
        confirm_delete,
        ..
    } = &app.mode
    else {
        return;
    };
    let cursor = (*cursor).min(FeedField::ALL.len() - 1);
    let editing = *editing;
    let overlay_open = category_picker.is_some() || *confirm_delete;

    let area = centered_rect(80, 92, area);
    frame.render_widget(Clear, area);

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(pal.border(true))
        .style(pal.panel())
        .title_top(Line::from(Span::styled(" Edit feed ", pal.heading())));

    // The dialog carries its own key hints; overlays show theirs.
    if !overlay_open {
        let hints = if editing {
            " Enter done · ←/→ move · Ctrl-W delete · Ctrl-u clear "
        } else {
            " Enter edit · Ctrl-s save · d unsubscribe · Esc cancel "
        };
        block = block.title_bottom(
            Line::from(Span::styled(hints, pal.muted()))
                .alignment(Alignment::Center),
        );
    }

    let inner = block.inner(area);
    let inner_width = inner.width as usize;
    let label_width = 19.min(inner_width.saturating_sub(1));
    let value_width = inner_width.saturating_sub(label_width + 1);

    let lines: Vec<Line<'static>> = FeedField::ALL
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let selected = index == cursor;
            let is_editing = selected && editing;
            let (value, muted) =
                feed_field_value(app, form, *field, is_editing);
            let shown = match (is_editing, form.input(*field)) {
                (true, Some(input)) => {
                    let (start, _) = input_window(
                        input.as_str(),
                        input.cursor(),
                        value_width,
                    );
                    input.as_str()[start..].to_string()
                }
                _ => util::truncate(&value, value_width),
            };
            let value_style = if muted {
                pal.dim()
            } else if selected {
                pal.strong()
            } else {
                pal.text()
            };
            let mut line = Line::from(vec![
                Span::styled(
                    format!("{:<label_width$}", field.label()),
                    if selected { pal.heading() } else { pal.muted() },
                ),
                Span::styled(shown, value_style),
            ]);
            if selected {
                line = line.style(pal.selected());
            }
            line
        })
        .collect();

    let inner_height = inner.height as usize;
    let offset = cursor.saturating_sub(inner_height.saturating_sub(1));
    frame.render_widget(
        Paragraph::new(lines)
            .style(pal.panel())
            .scroll((offset as u16, 0))
            .block(block),
        area,
    );

    if inner.width > 0 && inner.height > 0 && !overlay_open && editing {
        let field = FeedField::ALL[cursor];
        if let Some(input) = form.input(field) {
            let (_, caret) =
                input_window(input.as_str(), input.cursor(), value_width);
            let offset_x =
                (label_width + caret).min(inner_width.saturating_sub(1));
            let x = inner.x + offset_x as u16;
            let y = inner.y + (cursor - offset) as u16;
            frame.set_cursor_position((x, y));
        }
    }
}

fn draw_category_picker(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    pal: &Palette,
) {
    let Mode::EditFeed {
        category_picker: Some(cursor),
        form,
        ..
    } = &app.mode
    else {
        return;
    };

    let categories = app.sorted_categories();
    let cursor = (*cursor).min(categories.len().saturating_sub(1));

    let area = centered_rect(60, 70, area);
    frame.render_widget(Clear, area);

    let items: Vec<ListItem> = categories
        .iter()
        .map(|category| {
            let marker = if category.id == form.category_id {
                "● "
            } else {
                "  "
            };
            ListItem::new(Line::from(vec![
                Span::styled(marker, pal.accent()),
                Span::styled(category.title.clone(), pal.text()),
            ]))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(pal.border(true))
        .style(pal.panel())
        .title_top(Line::from(Span::styled(" Choose category ", pal.heading())))
        .title_bottom(
            Line::from(Span::styled(
                " j/k select · Enter choose · Esc cancel ",
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

fn draw_feed_delete_confirm(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    pal: &Palette,
) {
    let Mode::EditFeed {
        form,
        confirm_delete: true,
        ..
    } = &app.mode
    else {
        return;
    };

    let width = ((u32::from(area.width) * 60 / 100) as u16)
        .max(24)
        .min(area.width.saturating_sub(2));
    let height = 7_u16.min(area.height.saturating_sub(2));
    let area = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, area);

    let warning = Style::default().fg(pal.color(tokens::WARNING));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(warning)
        .style(pal.panel())
        .title_top(Line::from(Span::styled(
            " Unsubscribe ",
            warning.add_modifier(Modifier::BOLD),
        )));

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            util::truncate(
                &format!("Delete “{}” and its entries?", form.title.as_str()),
                area.width.saturating_sub(4) as usize,
            ),
            pal.text(),
        ))
        .alignment(Alignment::Center),
        Line::from(""),
        Line::from(vec![
            Span::styled("[y]", pal.key()),
            Span::raw(" unsubscribe    "),
            Span::styled("[n]", pal.key()),
            Span::raw(" cancel"),
        ])
        .alignment(Alignment::Center),
    ];

    frame.render_widget(
        Paragraph::new(text).style(pal.panel()).block(block),
        area,
    );
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
    // Percentages are computed in u32: `u16 * u16` overflows on ultrawide
    // terminals (911+ columns), which panics in debug builds.
    let width = (u32::from(area.width) * u32::from(percent_x) / 100) as u16;
    let width = width.max(24).min(area.width.saturating_sub(2));
    let height = (u32::from(area.height) * u32::from(percent_y) / 100) as u16;
    let height = height.max(7).min(area.height.saturating_sub(2));
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{FeedField, FeedForm, Mode, Selection},
        config::{Auth, Config},
    };
    use miniflux_api::models::{
        Category, CategoryID, FeedID, Subscription, UserID,
    };
    use ratatui::{Terminal, backend::TestBackend};
    use url::Url;

    fn test_app() -> App {
        let config = Config {
            url: Url::parse("http://localhost/").expect("test URL"),
            auth: Auth::Token("test".to_string()),
            theme: "catppuccin-mocha".to_string(),
            limit: 100,
            mark_read: true,
            hide_read_feeds: false,
            hide_read_entries: false,
            images: false,
        };
        let theme = opaline::builtins::load_by_name("catppuccin-mocha")
            .expect("builtin theme");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        App::new(config, theme, "catppuccin-mocha".to_string(), tx)
    }

    fn render_at(app: &mut App, width: u16, height: u16) {
        let mut terminal = Terminal::new(TestBackend::new(width, height))
            .expect("test terminal");
        terminal
            .draw(|frame| draw(frame, app))
            .expect("draw the frame");
    }

    #[test]
    fn feed_picker_renders_in_a_short_terminal() {
        let mut app = test_app();
        app.mode = Mode::FeedPicker {
            subscriptions: vec![
                Subscription {
                    url: "https://example.com/rss.xml".to_string(),
                    title: "RSS".to_string(),
                    kind: "rss".to_string(),
                },
                Subscription {
                    url: "https://example.com/atom.xml".to_string(),
                    title: "Atom".to_string(),
                    kind: "atom".to_string(),
                },
            ],
            cursor: 0,
            category_id: CategoryID(1),
        };

        for height in 1..=12 {
            render_at(&mut app, 80, height);
        }
    }

    fn probe_entry() -> miniflux_api::models::Entry {
        let long_title = "A very long entry title that has to wrap ".repeat(8);
        serde_json::from_value(serde_json::json!({
            "id": 1,
            "user_id": 1,
            "feed_id": 7,
            "status": "unread",
            "hash": "hash-1",
            "title": long_title,
            "url": "https://example.com/1",
            "author": "Author",
            "content": format!("<p>{}</p>", "Body text ".repeat(200)),
            "published_at": "2026-01-01T00:00:00Z",
            "created_at": "2026-01-01T00:00:00Z",
            "share_code": "",
            "starred": true,
            "reading_time": 9,
            "enclosures": [],
            "tags": ["one", "two"],
            "feed": {
                "id": 7,
                "user_id": 1,
                "title": "A feed with a rather long title for truncation",
                "site_url": "https://example.com",
                "feed_url": "https://example.com/feed.xml",
                "category": {"id": 3, "user_id": 1, "title": "Category"}
            }
        }))
        .expect("valid test entry")
    }

    fn starred_entry() -> miniflux_api::models::Entry {
        serde_json::from_value(serde_json::json!({
            "id": 2,
            "user_id": 1,
            "feed_id": 7,
            "status": "unread",
            "hash": "hash-2",
            "title": "Starred entry",
            "url": "https://example.com/2",
            "author": "Author",
            "content": "<p>Body</p>",
            "published_at": "2026-01-01T00:00:00Z",
            "created_at": "2026-01-01T00:00:00Z",
            "share_code": "",
            "starred": true,
            "reading_time": 3,
            "enclosures": [],
            "tags": [],
            "feed": {
                "id": 7,
                "user_id": 1,
                "title": "Feed X",
                "site_url": "https://example.com",
                "feed_url": "https://example.com/feed.xml",
                "category": {"id": 3, "user_id": 1, "title": "Category"}
            }
        }))
        .expect("valid test entry")
    }

    #[test]
    fn layouts_render_across_terminal_sizes() {
        let mut app = test_app();
        app.entries = vec![probe_entry(), probe_entry()];
        app.entry_state.select(Some(0));
        app.categories = vec![category(1, "News"), category(2, "Tech")];
        app.ensure_article();
        let widths = [
            1u16, 2, 3, 10, 21, 22, 40, 65, 66, 99, 100, 101, 140, 911, 1300,
        ];
        let heights = [1u16, 2, 3, 4, 6, 7, 9, 10, 11, 24, 40];
        let modes = [
            Mode::Normal,
            Mode::Help { scroll: u16::MAX },
            Mode::Confirm {
                prompt: "Mark every entry in this view as read?".to_string(),
                action: crate::app::ConfirmAction::MarkAllRead(
                    crate::api::MarkAllScope::User(
                        miniflux_api::models::UserID(1),
                    ),
                ),
            },
            Mode::ThemePicker {
                cursor: 3,
                original: "catppuccin-mocha".to_string(),
            },
            Mode::FeedPicker {
                subscriptions: vec![
                    Subscription {
                        url: "https://example.com/rss.xml".to_string(),
                        title: "RSS".to_string(),
                        kind: "rss".to_string(),
                    },
                    Subscription {
                        url: "https://example.com/atom.xml".to_string(),
                        title: "Atom".to_string(),
                        kind: "atom".to_string(),
                    },
                ],
                cursor: 1,
                category_id: CategoryID(1),
            },
            Mode::EditFeed {
                feed_id: FeedID(7),
                form: Box::new(editor_form()),
                cursor: 8,
                editing: true,
                category_picker: None,
                confirm_delete: false,
            },
            Mode::EditFeed {
                feed_id: FeedID(7),
                form: Box::new(editor_form()),
                cursor: 1,
                editing: false,
                category_picker: Some(1),
                confirm_delete: false,
            },
            Mode::EditFeed {
                feed_id: FeedID(7),
                form: Box::new(editor_form()),
                cursor: 0,
                editing: false,
                category_picker: None,
                confirm_delete: true,
            },
            Mode::Search {
                input: TextInput::new("rust"),
                scope: SearchScope::Feeds,
                original: String::new(),
                original_selection: Selection::Unread,
            },
        ];
        for mode in modes {
            app.mode = mode;
            for width in widths {
                for height in heights {
                    render_at(&mut app, width, height);
                }
            }
        }
    }

    fn render_text(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height))
            .expect("test terminal");
        terminal
            .draw(|frame| draw(frame, app))
            .expect("draw the frame");
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..buffer.area().height {
            for x in 0..buffer.area().width {
                text.push_str(
                    buffer.cell((x, y)).map_or(" ", |cell| cell.symbol()),
                );
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn entries_keep_the_selected_row_visible_in_a_short_pane() {
        let mut app = test_app();
        app.entries = vec![probe_entry(), probe_entry()];
        app.focus = Focus::Entries;
        app.entry_cursor = 0;
        app.entry_state.select(Some(0));

        let text = render_text(&mut app, 80, 8);

        assert!(
            text.contains("▸"),
            "the entries pane rendered no selected row: {text:?}"
        );
    }

    #[test]
    fn help_wraps_its_descriptions_at_eighty_columns() {
        let mut app = test_app();
        app.mode = Mode::Help { scroll: 0 };

        let text = render_text(&mut app, 80, 24);

        assert!(
            text.contains("detected)"),
            "help text was clipped instead of wrapped: {text:?}"
        );
    }

    fn category(id: i64, title: &str) -> Category {
        Category {
            id: CategoryID(id),
            user_id: UserID(1),
            title: title.to_string(),
            hide_globally: false,
            feed_count: None,
            total_unread: None,
        }
    }

    fn editor_form() -> FeedForm {
        FeedForm {
            title: TextInput::new(
                "A feed with a rather long title for the editor",
            ),
            category_id: CategoryID(3),
            feed_url: TextInput::new("https://example.com/feed.xml"),
            site_url: TextInput::new("https://example.com"),
            user_agent: TextInput::new(String::new()),
            username: TextInput::new("user"),
            password: TextInput::new("secret"),
            scraper_rules: TextInput::new(String::new()),
            rewrite_rules: TextInput::new(String::new()),
            blocklist_rules: TextInput::new(String::new()),
            keeplist_rules: TextInput::new(String::new()),
            crawler: true,
            disabled: false,
            ignore_http_cache: true,
            fetch_via_proxy: false,
        }
    }

    #[test]
    fn feed_editor_renders_every_option() {
        let mut app = test_app();
        app.mode = Mode::EditFeed {
            feed_id: FeedID(7),
            form: Box::new(editor_form()),
            cursor: 0,
            editing: false,
            category_picker: None,
            confirm_delete: false,
        };

        let text = render_text(&mut app, 100, 30);

        for field in FeedField::ALL {
            assert!(
                text.contains(field.label()),
                "missing field {}: {text:?}",
                field.label()
            );
        }
        assert!(text.contains("A feed with a rather"));
        assert!(text.contains("[x]"));
        assert!(text.contains("••••••"));
    }

    #[test]
    fn category_picker_renders_the_categories() {
        let mut app = test_app();
        app.categories = vec![category(1, "News"), category(2, "Tech")];
        app.mode = Mode::EditFeed {
            feed_id: FeedID(7),
            form: Box::new(editor_form()),
            cursor: 1,
            editing: false,
            category_picker: Some(1),
            confirm_delete: false,
        };

        let text = render_text(&mut app, 80, 24);

        assert!(text.contains("Choose category"), "{text:?}");
        assert!(text.contains("Enter choose"), "{text:?}");
        assert!(text.contains("News"), "{text:?}");
        assert!(text.contains("Tech"), "{text:?}");
    }

    #[test]
    fn unsubscribe_confirm_renders_the_prompt() {
        let mut app = test_app();
        let mut form = editor_form();
        form.title = TextInput::new("Example feed");
        app.mode = Mode::EditFeed {
            feed_id: FeedID(7),
            form: Box::new(form),
            cursor: 0,
            editing: false,
            category_picker: None,
            confirm_delete: true,
        };

        let text = render_text(&mut app, 80, 24);

        assert!(text.contains("Unsubscribe"), "{text:?}");
        assert!(text.contains("and its entries?"), "{text:?}");
        assert!(text.contains("[y]"), "{text:?}");
        assert!(text.contains("[n]"), "{text:?}");
    }

    #[test]
    fn feed_editor_hints_live_in_the_dialog() {
        let mut app = test_app();
        app.mode = Mode::EditFeed {
            feed_id: FeedID(7),
            form: Box::new(editor_form()),
            cursor: 0,
            editing: false,
            category_picker: None,
            confirm_delete: false,
        };

        let text = render_text(&mut app, 80, 30);
        let keybar = text.lines().last().unwrap_or_default();
        assert!(
            keybar.trim().is_empty(),
            "the key bar still shows hints: {keybar:?}"
        );
        assert!(text.contains("Ctrl-s save"), "{text:?}");
        assert!(text.contains("d unsubscribe"), "{text:?}");

        app.mode = Mode::EditFeed {
            feed_id: FeedID(7),
            form: Box::new(editor_form()),
            cursor: 0,
            editing: true,
            category_picker: None,
            confirm_delete: false,
        };

        let text = render_text(&mut app, 80, 30);
        assert!(text.contains("Ctrl-u clear"), "{text:?}");
        assert!(!text.contains("Ctrl-s save"), "{text:?}");
    }

    #[test]
    fn feed_filter_prompt_and_title_render() {
        let mut app = test_app();
        app.focus = Focus::Feeds;
        app.mode = Mode::Search {
            input: TextInput::new("rust"),
            scope: SearchScope::Feeds,
            original: String::new(),
            original_selection: Selection::Unread,
        };

        let text = render_text(&mut app, 80, 24);
        assert!(text.contains("Feeds: rust"), "{text:?}");

        app.mode = Mode::Normal;
        app.feed_filter = "rust".to_string();

        let text = render_text(&mut app, 80, 24);
        assert!(text.contains("/rust"), "{text:?}");
    }

    #[test]
    fn reader_renders_images_when_enabled() {
        let mut app = test_app();
        app.entries = vec![probe_entry()];
        app.entry_cursor = 0;
        app.focus = Focus::Article;
        app.article_parts = vec![util::ArticlePart::Image {
            url: "https://example.com/pic.png".to_string(),
            alt: "Pic".to_string(),
        }];

        app.show_images = false;
        let text = render_text(&mut app, 80, 24);
        assert!(text.contains("[image] Pic"), "{text:?}");

        app.show_images = true;
        app.images.insert(
            "https://example.com/pic.png".to_string(),
            LoadedImage::Ready(api::DecodedImage {
                width: 4,
                height: 4,
                rgba: [255, 0, 0, 255].repeat(16),
            }),
        );
        let text = render_text(&mut app, 80, 24);
        assert!(text.contains("▀"), "the image was not drawn: {text:?}");
    }

    fn feed(
        id: i64,
        title: &str,
        category_id: i64,
    ) -> miniflux_api::models::Feed {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "user_id": 1,
            "title": title,
            "site_url": "https://example.com",
            "feed_url": "https://example.com/feed.xml",
            "category": {
                "id": category_id,
                "user_id": 1,
                "title": "Category"
            }
        }))
        .expect("valid test feed")
    }

    #[test]
    fn the_star_sits_on_the_metadata_line() {
        let mut app = test_app();
        app.entries = vec![starred_entry()];
        app.entry_cursor = 0;
        app.focus = Focus::Feeds;

        // 80 columns with the Feeds pane focused draws feeds and entries only,
        // so the reader header cannot supply a star of its own.
        let text = render_text(&mut app, 80, 24);
        let title_line = text
            .lines()
            .find(|line| line.contains("Starred entry"))
            .expect("title line");
        let meta_line = text
            .lines()
            .find(|line| line.contains("Feed X"))
            .expect("meta line");
        assert!(!title_line.contains('★'), "{title_line:?}");
        assert!(meta_line.contains('★'), "{meta_line:?}");
        assert!(meta_line.contains("3 min"), "{meta_line:?}");

        app.entries[0].starred = false;
        let text = render_text(&mut app, 80, 24);
        let meta_line = text
            .lines()
            .find(|line| line.contains("Feed X"))
            .expect("meta line");
        assert!(!meta_line.contains('★'), "{meta_line:?}");
    }

    #[test]
    fn images_fill_the_reading_width() {
        let image = |width, height| api::DecodedImage {
            width,
            height,
            rgba: vec![255; 0],
        };

        assert_eq!(image_geometry(&image(300, 600), 80), (80, 80));
        assert_eq!(image_geometry(&image(4, 4), 80), (80, 40));
        assert_eq!(image_geometry(&image(1000, 10), 46), (46, 1));
        assert_eq!(image_geometry(&image(1, 8192), 10), (10, MAX_IMAGE_ROWS));
    }

    #[test]
    fn pane_titles_use_the_selection_and_entry_titles() {
        let mut app = test_app();
        app.categories = vec![category(1, "News")];
        app.feeds = vec![feed(7, "My Feed", 1)];
        app.selection = Selection::Feed(FeedID(7));
        app.focus = Focus::Entries;
        app.entries = vec![starred_entry()];
        app.entry_cursor = 0;

        // At 80 columns with the Entries pane focused only the entries and
        // reader panes are drawn, so the sidebar cannot supply these titles.
        let text = render_text(&mut app, 80, 24);
        let borders = text.lines().next().expect("pane borders");

        assert!(borders.contains("My Feed"), "{borders:?}");
        assert!(borders.contains("Starred entry"), "{borders:?}");
        assert!(!borders.contains("Entries"), "{borders:?}");
        assert!(!borders.contains("Reader"), "{borders:?}");

        app.selection = Selection::Unread;
        app.entries.clear();
        let text = render_text(&mut app, 80, 24);
        let borders = text.lines().next().expect("pane borders");
        assert!(borders.contains("Unread"), "{borders:?}");
        assert!(borders.contains("Reader"), "{borders:?}");

        app.search = "rust".to_string();
        let text = render_text(&mut app, 80, 24);
        let borders = text.lines().next().expect("pane borders");
        assert!(borders.contains("Unread /rust"), "{borders:?}");
    }

    #[test]
    fn space_moves_to_the_next_rendered_paragraph() {
        let mut app = test_app();
        let mut entry = starred_entry();
        let paragraph = "Body text that keeps the article taller ".repeat(10);
        entry.content =
            format!("<p>{paragraph}</p><p>{paragraph}</p><p>{paragraph}</p>");
        app.entries = vec![entry];
        app.entry_cursor = 0;
        app.focus = Focus::Article;
        app.ensure_article();

        let _ = render_text(&mut app, 80, 24);

        let stops = app.article_paragraph_stops.clone();
        assert!(stops.len() >= 3, "{stops:?}");
        assert!(stops.windows(2).all(|pair| pair[0] < pair[1]));
        let first = stops[0];
        assert!(first > 0, "{stops:?}");
        assert!(
            (first as u16) <= app.article_max_scroll,
            "first stop {first} beyond max {}",
            app.article_max_scroll
        );

        app.handle_event(crossterm::event::Event::Key(
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(' '),
                crossterm::event::KeyModifiers::NONE,
            ),
        ));

        assert_eq!(app.article_scroll as usize, first);
    }

    #[test]
    fn input_windows_keep_the_caret_visible() {
        assert_eq!(input_window("short", 5, 10), (0, 5));

        let text = "0123456789";
        let (start, caret) = input_window(text, text.len(), 5);
        assert_eq!((start, caret), (6, 4));
        assert_eq!(&text[start..], "6789");

        let (start, caret) = input_window(text, 5, 4);
        assert_eq!((start, caret), (2, 3));
    }

    #[test]
    fn long_fields_scroll_to_keep_the_caret_visible() {
        let mut app = test_app();
        let mut form = editor_form();
        form.title.move_end();
        app.mode = Mode::EditFeed {
            feed_id: FeedID(7),
            form: Box::new(form),
            cursor: 0,
            editing: true,
            category_picker: None,
            confirm_delete: false,
        };

        let text = render_text(&mut app, 80, 24);
        let title_line = text
            .lines()
            .find(|line| line.contains("Title"))
            .expect("title row");

        assert!(!title_line.contains("A feed with"), "{title_line:?}");
        assert!(title_line.contains("for the editor"), "{title_line:?}");
    }

    #[test]
    fn the_sidebar_shows_everything_instead_of_all_entries() {
        let mut app = test_app();

        let text = render_text(&mut app, 80, 24);

        assert!(text.contains("Everything"), "{text:?}");
        assert!(!text.contains("All entries"), "{text:?}");
    }

    #[test]
    fn everything_is_highlighted_like_unread() {
        let mut app = test_app();
        // Highlight the Starred row so both icons keep their item styles.
        app.sidebar_cursor = 2;
        let mut terminal =
            Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw the frame");
        let buffer = terminal.backend().buffer();

        let mut unread = None;
        let mut everything = None;
        for y in 0..buffer.area().height {
            for x in 0..buffer.area().width {
                if let Some(cell) = buffer.cell((x, y)) {
                    match cell.symbol() {
                        "●" => unread = Some(cell.fg),
                        "◆" => everything = Some(cell.fg),
                        _ => {}
                    }
                }
            }
        }

        assert!(unread.is_some(), "the Unread icon was not drawn");
        assert_eq!(everything, unread);
    }
}
