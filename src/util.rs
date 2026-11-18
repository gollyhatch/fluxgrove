use std::process::{Command, Stdio};

use chrono::{DateTime, Datelike, Local, Utc};
use html2text::render::TextDecorator;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Marks where an image reference was in the rendered article text.
pub const IMAGE_MARKER: char = '\u{fffc}';
/// Separates an image URL from its alt text inside a marker.
const IMAGE_SEPARATOR: char = '|';

/// A piece of the reading view: article text or an embedded image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArticlePart {
    Text(String),
    Image { url: String, alt: String },
}

/// Split rendered article text into text runs and image references.
///
/// [`html_to_text`] replaces every image with a marker holding its source and
/// alt text, so the reader can lay the image out where it appeared.
pub fn article_parts(text: &str) -> Vec<ArticlePart> {
    let mut parts = Vec::new();
    let mut is_image = false;
    for segment in text.split(IMAGE_MARKER) {
        if is_image {
            let (url, alt) =
                segment.split_once(IMAGE_SEPARATOR).unwrap_or((segment, ""));
            if !url.is_empty() {
                parts.push(ArticlePart::Image {
                    url: url.to_string(),
                    alt: alt.to_string(),
                });
            }
        } else if !segment.is_empty() {
            parts.push(ArticlePart::Text(segment.to_string()));
        }
        is_image = !is_image;
    }
    parts
}

/// The article as plain text, ignoring images.
pub fn article_plain_text(parts: &[ArticlePart]) -> String {
    parts
        .iter()
        .filter_map(|part| match part {
            ArticlePart::Text(text) => Some(text.as_str()),
            ArticlePart::Image { .. } => None,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Decode HTML entities in feed-provided text such as titles.
pub fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    html_escape::decode_html_entities(text).into_owned()
}

/// Convert sanitized feed HTML to readable plain text.
pub fn html_to_text(html: &str) -> String {
    if html.trim().is_empty() {
        return String::new();
    }

    // A very large render width keeps html2text from hard-wrapping the article;
    // the reader pane does its own wrapping so it can follow terminal resizes.
    let text = html2text::from_read_with_decorator(
        html.as_bytes(),
        1_000,
        ArticleDecorator,
    )
    .unwrap_or_else(|_| {
        html.replace("<br>", "\n")
            .replace("<br/>", "\n")
            .replace("<br />", "\n")
            .replace("</p>", "\n\n")
    });

    compact_blank_lines(&text)
}

/// Decorator for article text: keep list and quote structure, but strip
/// markdown emphasis and heading prefixes that would look like noise in a
/// reader pane.
#[derive(Clone, Debug)]
struct ArticleDecorator;

impl TextDecorator for ArticleDecorator {
    type Annotation = ();

    fn decorate_link_start(
        &mut self,
        _url: &str,
    ) -> (String, Self::Annotation) {
        (String::new(), ())
    }

    fn decorate_link_end(&mut self) -> String {
        String::new()
    }

    fn decorate_em_start(&self) -> (String, Self::Annotation) {
        (String::new(), ())
    }

    fn decorate_em_end(&self) -> String {
        String::new()
    }

    fn decorate_strong_start(&self) -> (String, Self::Annotation) {
        (String::new(), ())
    }

    fn decorate_strong_end(&self) -> String {
        String::new()
    }

    fn decorate_strikeout_start(&self) -> (String, Self::Annotation) {
        (String::new(), ())
    }

    fn decorate_strikeout_end(&self) -> String {
        String::new()
    }

    fn decorate_code_start(&self) -> (String, Self::Annotation) {
        (String::new(), ())
    }

    fn decorate_code_end(&self) -> String {
        String::new()
    }

    fn decorate_preformat_first(&self) -> Self::Annotation {}
    fn decorate_preformat_cont(&self) -> Self::Annotation {}

    fn decorate_image(
        &mut self,
        src: &str,
        title: &str,
    ) -> (String, Self::Annotation) {
        (
            format!(
                "{IMAGE_MARKER}{src}{IMAGE_SEPARATOR}{title}{IMAGE_MARKER}"
            ),
            (),
        )
    }

    fn header_prefix(&self, _level: usize) -> String {
        String::new()
    }

    fn quote_prefix(&self) -> String {
        "> ".to_string()
    }

    fn unordered_item_prefix(&self) -> String {
        "* ".to_string()
    }

    fn ordered_item_prefix(&self, i: i64) -> String {
        format!("{i}. ")
    }

    fn make_subblock_decorator(&self) -> Self {
        Self
    }
}

pub fn compact_blank_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

/// Collapse any whitespace (including newlines) into single spaces.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Truncate to at most `max` terminal columns, appending an ellipsis when cut.
pub fn truncate(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let text = one_line(text);
    if display_width(&text) <= max {
        return text;
    }
    if max == 1 {
        return "…".to_string();
    }

    let mut out = String::new();
    let mut width = 0;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > max - 1 {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out.push('…');
    out
}

/// Wrap plain text to a terminal width, preserving blank lines and indentation.
///
/// This intentionally does not join hard-wrapped lines: the input from
/// html2text uses newlines for paragraph and list structure, and joining them
/// would destroy bullets.
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();

    for raw in text.lines() {
        let expanded = raw.replace('\t', "    ");
        let raw = expanded.trim_end();
        if raw.trim().is_empty() {
            lines.push(String::new());
            continue;
        }

        let indent_len = raw.chars().take_while(|ch| *ch == ' ').count();
        let indent = " ".repeat(indent_len.min(width.saturating_sub(1)));
        let content = raw[indent_len..].trim_start();
        let available = width.saturating_sub(indent_len).max(1);

        let mut current = String::new();
        let mut current_width = 0usize;

        for word in content.split_whitespace() {
            let word_width = display_width(word);

            if current_width > 0 && current_width + 1 + word_width > available {
                lines.push(format!("{indent}{current}"));
                current.clear();
                current_width = 0;
            }

            if word_width > available {
                if !current.is_empty() {
                    lines.push(format!("{indent}{current}"));
                    current.clear();
                }

                let mut chunk = String::new();
                let mut chunk_width = 0usize;
                for ch in word.chars() {
                    let ch_width =
                        UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
                    if chunk_width > 0 && chunk_width + ch_width > available {
                        lines.push(format!("{indent}{chunk}"));
                        chunk.clear();
                        chunk_width = 0;
                    }
                    chunk.push(ch);
                    chunk_width += ch_width;
                }
                current = chunk;
                current_width = chunk_width;
            } else {
                if current_width > 0 {
                    current.push(' ');
                    current_width += 1;
                }
                current.push_str(word);
                current_width += word_width;
            }
        }

        if !current.is_empty() {
            lines.push(format!("{indent}{current}"));
        }
    }

    lines
}

pub fn relative_time(when: DateTime<Utc>) -> String {
    let now = Utc::now();
    let seconds = now.signed_duration_since(when).num_seconds();

    if seconds < 0 {
        return when.with_timezone(&Local).format("%b %d").to_string();
    }
    if seconds < 60 {
        return "now".to_string();
    }
    if seconds < 3_600 {
        return format!("{}m", seconds / 60);
    }
    if seconds < 86_400 {
        return format!("{}h", seconds / 3_600);
    }
    if seconds < 7 * 86_400 {
        return format!("{}d", seconds / 86_400);
    }

    let local = when.with_timezone(&Local);
    if local.year() == Local::now().year() {
        local.format("%b %d").to_string()
    } else {
        local.format("%b %d, %Y").to_string()
    }
}

pub fn format_datetime(when: DateTime<Utc>) -> String {
    when.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// Open a URL in the user's preferred browser without blocking the TUI.
///
/// `$BROWSER` is honoured when set; otherwise the platform opener is used.
pub fn open_in_browser(url: &str) -> std::io::Result<()> {
    if let Some(browser) = std::env::var_os("BROWSER")
        && let Some((program, args)) = browser_command(&browser, url)
    {
        match spawn_detached(&program, &args) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }

    let (program, args) = platform_opener(url);
    spawn_detached_strs(program, &args)
}

/// Open a URL through the XDG desktop environment's default web browser.
///
/// On Linux this means `xdg-open` and its MIME/default-browser associations.
/// Other platforms fall back to their native opener.
pub fn open_in_xdg_browser(url: &str) -> std::io::Result<()> {
    if cfg!(target_os = "linux") && !is_wsl() {
        let openers: [(&str, Vec<&str>); 3] = [
            ("xdg-open", vec![url]),
            ("gio", vec!["open", url]),
            ("sensible-browser", vec![url]),
        ];

        for (program, args) in openers {
            match spawn_detached_strs(program, &args) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }

        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no XDG browser opener found (tried xdg-open, gio and sensible-browser)",
        ));
    }

    open_in_browser(url)
}

fn platform_opener(url: &str) -> (&'static str, Vec<&str>) {
    if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(target_os = "windows") {
        ("cmd", vec!["/C", "start", "", url])
    } else if is_wsl() {
        ("wslview", vec![url])
    } else {
        ("xdg-open", vec![url])
    }
}

fn is_wsl() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::fs::read_to_string("/proc/version")
            .map(|version| version.to_ascii_lowercase().contains("microsoft"))
            .unwrap_or(false)
}

fn spawn_detached(program: &str, args: &[String]) -> std::io::Result<()> {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

fn spawn_detached_strs(program: &str, args: &[&str]) -> std::io::Result<()> {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    spawn_detached(program, &args)
}

/// Parse a `$BROWSER` value such as `firefox`, `firefox %s`, or
/// `firefox:chromium`. Returns the executable and arguments.
fn browser_command(
    browser: &std::ffi::OsStr,
    url: &str,
) -> Option<(String, Vec<String>)> {
    let browser = browser.to_string_lossy();
    let first = browser.split(':').find(|part| !part.trim().is_empty())?;
    let mut tokens = first.split_whitespace();
    let program = tokens.next()?.to_string();
    let mut args: Vec<String> =
        tokens.map(|token| token.replace("%s", url)).collect();

    if !args.iter().any(|arg| arg.contains(url)) {
        args.push(url.to_string());
    }

    Some((program, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_long_lines() {
        let lines = wrap_text("hello world this is a test", 10);
        assert_eq!(lines, vec!["hello", "world this", "is a test"]);
    }

    #[test]
    fn preserves_indentation_and_blank_lines() {
        let lines = wrap_text("  * one two three\n\nnext", 8);
        assert_eq!(lines, vec!["  * one", "  two", "  three", "", "next"]);
    }

    #[test]
    fn truncates_wide_text() {
        assert_eq!(truncate("hello", 3), "he…");
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn splits_images_out_of_the_article_text() {
        let text = html_to_text(
            "<p>Before</p><img src=\"https://example.com/a.png\" \
             alt=\"A picture\"><p>After</p>",
        );
        let parts = article_parts(&text);

        assert!(
            matches!(&parts[0], ArticlePart::Text(text) if text.contains("Before")),
            "{parts:?}"
        );
        assert!(
            parts.iter().any(|part| matches!(
                part,
                ArticlePart::Image { url, alt }
                    if url == "https://example.com/a.png"
                        && alt == "A picture"
            )),
            "{parts:?}"
        );
        assert!(
            parts.iter().any(
                |part| matches!(part, ArticlePart::Text(text) if text.contains("After"))
            ),
            "{parts:?}"
        );
        assert!(!article_plain_text(&parts).contains(IMAGE_MARKER));
    }

    #[test]
    fn decodes_html_entities() {
        assert_eq!(
            decode_entities(
                "Rock &#34;n&#34; Roll &amp; &#x27;Jazz&#x27; &mdash; live"
            ),
            "Rock \"n\" Roll & 'Jazz' — live"
        );
        assert_eq!(decode_entities("AT&T stays"), "AT&T stays");
        assert_eq!(decode_entities("Plain title"), "Plain title");
    }

    #[test]
    fn html_rendering_strips_markdown_noise() {
        let text = html_to_text(
            "<p>Hello <strong>world</strong></p><h2>Title</h2>\
             <ul><li>One</li><li>Two</li></ul><blockquote>Quote</blockquote>",
        );
        assert!(text.contains("Hello world"));
        assert!(text.contains("Title"));
        assert!(text.contains("* One"));
        assert!(text.contains("> Quote"));
        assert!(!text.contains("**"));
        assert!(!text.contains("##"));
    }

    #[test]
    fn parses_browser_overrides() {
        let (program, args) = browser_command(
            std::ffi::OsStr::new("firefox --new-window %s"),
            "https://example.com",
        )
        .expect("browser command");
        assert_eq!(program, "firefox");
        assert_eq!(args, vec!["--new-window", "https://example.com"]);

        let (program, args) = browser_command(
            std::ffi::OsStr::new("firefox:chromium"),
            "https://example.com",
        )
        .expect("browser command");
        assert_eq!(program, "firefox");
        assert_eq!(args, vec!["https://example.com"]);
    }

    #[test]
    fn collapses_excess_blank_lines() {
        assert_eq!(compact_blank_lines("a\n\n\n\nb\n"), "a\n\nb");
    }
}
