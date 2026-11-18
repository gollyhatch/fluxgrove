use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use miniflux_api::models::{
    Category, CategoryID, Entry, EntryID, EntryStatus, Feed, FeedCounters,
    FeedID, Subscription, UserID,
};
use opaline::Theme;
use ratatui::widgets::ListState;
use tokio::sync::mpsc::UnboundedSender;
use url::Url;

use crate::{
    api::{self, ApiCommand, ApiEvent, BootstrapData, MarkAllScope},
    config::Config,
    util,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Feeds,
    Entries,
    Article,
}

impl Focus {
    pub fn next(self, has_entries: bool) -> Self {
        match self {
            Self::Feeds => Self::Entries,
            Self::Entries if has_entries => Self::Article,
            Self::Entries => Self::Feeds,
            Self::Article => Self::Feeds,
        }
    }

    pub fn previous(self, has_entries: bool) -> Self {
        match self {
            Self::Feeds if has_entries => Self::Article,
            Self::Feeds => Self::Entries,
            Self::Entries => Self::Feeds,
            Self::Article => Self::Entries,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    All,
    Unread,
    Starred,
    Category(CategoryID),
    Feed(FeedID),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidebarRow {
    All,
    Unread,
    Starred,
    Category { id: CategoryID, title: String },
    Feed { id: FeedID, title: String },
}

impl SidebarRow {
    pub fn selection(&self) -> Selection {
        match self {
            Self::All => Selection::All,
            Self::Unread => Selection::Unread,
            Self::Starred => Selection::Starred,
            Self::Category { id, .. } => Selection::Category(*id),
            Self::Feed { id, .. } => Selection::Feed(*id),
        }
    }
}

/// One editable property of a feed, in the order the editor shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedField {
    Title,
    Category,
    FeedUrl,
    SiteUrl,
    UserAgent,
    Username,
    Password,
    ScraperRules,
    RewriteRules,
    BlocklistRules,
    KeeplistRules,
    Crawler,
    Disabled,
    IgnoreHttpCache,
    FetchViaProxy,
}

impl FeedField {
    pub const ALL: [Self; 15] = [
        Self::Title,
        Self::Category,
        Self::FeedUrl,
        Self::SiteUrl,
        Self::UserAgent,
        Self::Username,
        Self::Password,
        Self::ScraperRules,
        Self::RewriteRules,
        Self::BlocklistRules,
        Self::KeeplistRules,
        Self::Crawler,
        Self::Disabled,
        Self::IgnoreHttpCache,
        Self::FetchViaProxy,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Title => "Title",
            Self::Category => "Category",
            Self::FeedUrl => "Feed URL",
            Self::SiteUrl => "Site URL",
            Self::UserAgent => "User agent",
            Self::Username => "Username",
            Self::Password => "Password",
            Self::ScraperRules => "Scraper rules",
            Self::RewriteRules => "Rewrite rules",
            Self::BlocklistRules => "Blocklist rules",
            Self::KeeplistRules => "Keeplist rules",
            Self::Crawler => "Crawler",
            Self::Disabled => "Disabled",
            Self::IgnoreHttpCache => "Ignore HTTP cache",
            Self::FetchViaProxy => "Fetch via proxy",
        }
    }

    pub fn is_toggle(self) -> bool {
        matches!(
            self,
            Self::Crawler
                | Self::Disabled
                | Self::IgnoreHttpCache
                | Self::FetchViaProxy
        )
    }
}

/// Editable text with a caret that can sit anywhere in the string.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextInput {
    text: String,
    cursor: usize,
}

impl TextInput {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            cursor: text.len(),
            text,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    fn prev_boundary(&self, index: usize) -> usize {
        self.text[..index]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next_boundary(&self, index: usize) -> usize {
        self.text[index..]
            .chars()
            .next()
            .map_or(self.text.len(), |ch| index + ch.len_utf8())
    }

    pub fn move_left(&mut self) {
        self.cursor = self.prev_boundary(self.cursor);
    }

    pub fn move_right(&mut self) {
        self.cursor = self.next_boundary(self.cursor);
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.text.len();
    }

    pub fn move_word_left(&mut self) {
        let before = self.text[..self.cursor].trim_end();
        self.cursor = before
            .char_indices()
            .rev()
            .find(|(_, ch)| ch.is_whitespace())
            .map_or(0, |(index, ch)| index + ch.len_utf8());
    }

    pub fn move_word_right(&mut self) {
        let after = &self.text[self.cursor..];
        self.cursor = after
            .char_indices()
            .skip_while(|(_, ch)| ch.is_whitespace())
            .find(|(_, ch)| ch.is_whitespace())
            .map_or(self.text.len(), |(index, _)| self.cursor + index);
    }

    pub fn insert(&mut self, ch: char) {
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.prev_boundary(self.cursor);
        self.text.drain(start..self.cursor);
        self.cursor = start;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        let end = self.next_boundary(self.cursor);
        self.text.drain(self.cursor..end);
    }

    pub fn delete_word_before(&mut self) {
        let end = self.cursor;
        self.move_word_left();
        self.text.drain(self.cursor..end);
    }

    pub fn delete_word_after(&mut self) {
        let start = self.cursor;
        self.move_word_right();
        self.text.drain(start..self.cursor);
        self.cursor = start;
    }

    pub fn kill_to_end(&mut self) {
        self.text.truncate(self.cursor);
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }
}

/// Apply the standard editing keys to a text input.
///
/// Returns false for keys the input does not use, so callers can handle them
/// (Enter and Escape are handled before this is called).
fn apply_text_key(
    input: &mut TextInput,
    key: crossterm::event::KeyEvent,
) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let super_key = key.modifiers.contains(KeyModifiers::SUPER);

    match key.code {
        KeyCode::Left if control || alt => input.move_word_left(),
        KeyCode::Right if control || alt => input.move_word_right(),
        KeyCode::Left => input.move_left(),
        KeyCode::Right => input.move_right(),
        KeyCode::Home => input.move_home(),
        KeyCode::End => input.move_end(),
        KeyCode::Backspace if control || alt => input.delete_word_before(),
        KeyCode::Backspace => input.backspace(),
        KeyCode::Delete if control || alt => input.delete_word_after(),
        KeyCode::Delete => input.delete(),
        KeyCode::Char('a') if control => input.move_home(),
        KeyCode::Char('e') if control => input.move_end(),
        KeyCode::Char('b') if control => input.move_left(),
        KeyCode::Char('f') if control => input.move_right(),
        KeyCode::Char('h') if control => input.backspace(),
        KeyCode::Char('u') if control => input.clear(),
        KeyCode::Char('w') if control => input.delete_word_before(),
        KeyCode::Char('k') if control => input.kill_to_end(),
        KeyCode::Char(ch) if !control && !alt && !super_key => input.insert(ch),
        _ => return false,
    }
    true
}

/// The editable copy of a feed shown by the editor.
#[derive(Clone, Debug)]
pub struct FeedForm {
    pub title: TextInput,
    pub category_id: CategoryID,
    pub feed_url: TextInput,
    pub site_url: TextInput,
    pub user_agent: TextInput,
    pub username: TextInput,
    pub password: TextInput,
    pub scraper_rules: TextInput,
    pub rewrite_rules: TextInput,
    pub blocklist_rules: TextInput,
    pub keeplist_rules: TextInput,
    pub crawler: bool,
    pub disabled: bool,
    pub ignore_http_cache: bool,
    pub fetch_via_proxy: bool,
}

impl FeedForm {
    fn from_feed(feed: &Feed) -> Self {
        Self {
            title: TextInput::new(feed.title.clone()),
            category_id: feed.category.id,
            feed_url: TextInput::new(feed.feed_url.clone()),
            site_url: TextInput::new(feed.site_url.clone()),
            user_agent: TextInput::new(feed.user_agent.clone()),
            username: TextInput::new(feed.username.clone()),
            password: TextInput::new(feed.password.clone()),
            scraper_rules: TextInput::new(feed.scraper_rules.clone()),
            rewrite_rules: TextInput::new(feed.rewrite_rules.clone()),
            blocklist_rules: TextInput::new(feed.blocklist_rules.clone()),
            keeplist_rules: TextInput::new(feed.keeplist_rules.clone()),
            crawler: feed.crawler,
            disabled: feed.disabled,
            ignore_http_cache: feed.ignore_http_cache,
            fetch_via_proxy: feed.fetch_via_proxy,
        }
    }

    pub fn text(&self, field: FeedField) -> Option<&str> {
        match field {
            FeedField::Title => Some(self.title.as_str()),
            FeedField::FeedUrl => Some(self.feed_url.as_str()),
            FeedField::SiteUrl => Some(self.site_url.as_str()),
            FeedField::UserAgent => Some(self.user_agent.as_str()),
            FeedField::Username => Some(self.username.as_str()),
            FeedField::Password => Some(self.password.as_str()),
            FeedField::ScraperRules => Some(self.scraper_rules.as_str()),
            FeedField::RewriteRules => Some(self.rewrite_rules.as_str()),
            FeedField::BlocklistRules => Some(self.blocklist_rules.as_str()),
            FeedField::KeeplistRules => Some(self.keeplist_rules.as_str()),
            FeedField::Category
            | FeedField::Crawler
            | FeedField::Disabled
            | FeedField::IgnoreHttpCache
            | FeedField::FetchViaProxy => None,
        }
    }

    pub fn input(&self, field: FeedField) -> Option<&TextInput> {
        match field {
            FeedField::Title => Some(&self.title),
            FeedField::FeedUrl => Some(&self.feed_url),
            FeedField::SiteUrl => Some(&self.site_url),
            FeedField::UserAgent => Some(&self.user_agent),
            FeedField::Username => Some(&self.username),
            FeedField::Password => Some(&self.password),
            FeedField::ScraperRules => Some(&self.scraper_rules),
            FeedField::RewriteRules => Some(&self.rewrite_rules),
            FeedField::BlocklistRules => Some(&self.blocklist_rules),
            FeedField::KeeplistRules => Some(&self.keeplist_rules),
            FeedField::Category
            | FeedField::Crawler
            | FeedField::Disabled
            | FeedField::IgnoreHttpCache
            | FeedField::FetchViaProxy => None,
        }
    }

    fn text_mut(&mut self, field: FeedField) -> Option<&mut TextInput> {
        match field {
            FeedField::Title => Some(&mut self.title),
            FeedField::FeedUrl => Some(&mut self.feed_url),
            FeedField::SiteUrl => Some(&mut self.site_url),
            FeedField::UserAgent => Some(&mut self.user_agent),
            FeedField::Username => Some(&mut self.username),
            FeedField::Password => Some(&mut self.password),
            FeedField::ScraperRules => Some(&mut self.scraper_rules),
            FeedField::RewriteRules => Some(&mut self.rewrite_rules),
            FeedField::BlocklistRules => Some(&mut self.blocklist_rules),
            FeedField::KeeplistRules => Some(&mut self.keeplist_rules),
            FeedField::Category
            | FeedField::Crawler
            | FeedField::Disabled
            | FeedField::IgnoreHttpCache
            | FeedField::FetchViaProxy => None,
        }
    }

    fn toggle(&mut self, field: FeedField) {
        match field {
            FeedField::Crawler => self.crawler = !self.crawler,
            FeedField::Disabled => self.disabled = !self.disabled,
            FeedField::IgnoreHttpCache => {
                self.ignore_http_cache = !self.ignore_http_cache;
            }
            FeedField::FetchViaProxy => {
                self.fetch_via_proxy = !self.fetch_via_proxy;
            }
            _ => {}
        }
    }

    fn update(&self) -> api::FeedUpdate {
        api::FeedUpdate {
            title: self.title.as_str().to_string(),
            category_id: self.category_id,
            feed_url: self.feed_url.as_str().to_string(),
            site_url: self.site_url.as_str().to_string(),
            user_agent: self.user_agent.as_str().to_string(),
            username: self.username.as_str().to_string(),
            password: self.password.as_str().to_string(),
            scraper_rules: self.scraper_rules.as_str().to_string(),
            rewrite_rules: self.rewrite_rules.as_str().to_string(),
            blocklist_rules: self.blocklist_rules.as_str().to_string(),
            keeplist_rules: self.keeplist_rules.as_str().to_string(),
            crawler: self.crawler,
            disabled: self.disabled,
            ignore_http_cache: self.ignore_http_cache,
            fetch_via_proxy: self.fetch_via_proxy,
        }
    }
}

/// Editor state passed between key handling steps.
struct FeedEditorState {
    feed_id: FeedID,
    form: Box<FeedForm>,
    cursor: usize,
    editing: bool,
    category_picker: Option<usize>,
    confirm_delete: bool,
}

#[derive(Debug)]
pub enum ConfirmAction {
    MarkAllRead(MarkAllScope),
}

/// Which list `/` filters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchScope {
    Entries,
    Feeds,
}

#[derive(Debug, Default)]
pub enum Mode {
    #[default]
    Normal,
    Search {
        input: TextInput,
        scope: SearchScope,
        original: String,
        original_selection: Selection,
    },
    Subscribe {
        input: TextInput,
    },
    FeedPicker {
        subscriptions: Vec<Subscription>,
        cursor: usize,
        category_id: CategoryID,
    },
    Help {
        scroll: u16,
    },
    Confirm {
        prompt: String,
        action: ConfirmAction,
    },
    ThemePicker {
        cursor: usize,
        original: String,
    },
    EditFeed {
        feed_id: FeedID,
        form: Box<FeedForm>,
        cursor: usize,
        editing: bool,
        category_picker: Option<usize>,
        confirm_delete: bool,
    },
    RenameCategory {
        category_id: CategoryID,
        input: TextInput,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Success,
    Warning,
    Error,
}

/// Load state of an image embedded in the reader.
#[derive(Debug)]
pub enum LoadedImage {
    Loading,
    Ready(api::DecodedImage),
    Failed,
}

#[derive(Debug)]
pub struct StatusMessage {
    pub kind: StatusKind,
    pub text: String,
    pub expires: Option<Instant>,
}

pub struct App {
    pub config: Config,
    pub hide_read_feeds: bool,
    pub hide_read_entries: bool,
    pub theme: Theme,
    pub theme_id: String,
    pub theme_choices: Vec<(String, String)>,

    pub feeds: Vec<Feed>,
    pub categories: Vec<Category>,
    pub counters: FeedCounters,
    pub user_id: Option<UserID>,

    pub selection: Selection,
    pub sidebar_cursor: usize,
    pub sidebar_state: ListState,
    pub sidebar_page: usize,

    pub entries: Vec<Entry>,
    pub entry_cursor: usize,
    pub entry_state: ListState,
    pub entry_page: usize,
    pub total_entries: Option<i64>,

    pub search: String,
    pub feed_filter: String,
    pub focus: Focus,
    pub mode: Mode,
    pub status: Option<StatusMessage>,
    pub loading: bool,
    pub bootstrapped: bool,

    pub article_text: String,
    pub article_parts: Vec<util::ArticlePart>,
    pub images: HashMap<String, LoadedImage>,
    pub show_images: bool,
    pub article_entry: Option<EntryID>,
    pub article_scroll: u16,
    pub article_max_scroll: u16,
    pub article_page: u16,
    /// First line of every paragraph, in reader content order.
    pub article_paragraph_stops: Vec<usize>,

    pub should_quit: bool,
    pub dirty: bool,
    startup_warning: Option<String>,
    pending_select_feed: Option<FeedID>,

    api_tx: UnboundedSender<ApiCommand>,
    generation: u64,
}

impl App {
    pub fn new(
        config: Config,
        theme: Theme,
        theme_id: String,
        api_tx: UnboundedSender<ApiCommand>,
    ) -> Self {
        let mut theme_choices: Vec<(String, String)> =
            opaline::builtins::builtin_names()
                .iter()
                .map(|(id, name)| (id.to_string(), name.to_string()))
                .collect();
        theme_choices.sort_by_key(|(_, name)| name.to_lowercase());
        let hide_read_feeds = config.hide_read_feeds;
        let hide_read_entries = config.hide_read_entries;
        let show_images = config.images;

        let mut app = Self {
            config,
            hide_read_feeds,
            hide_read_entries,
            theme,
            theme_id,
            theme_choices,
            feeds: Vec::new(),
            categories: Vec::new(),
            counters: FeedCounters::default(),
            user_id: None,
            selection: Selection::Unread,
            sidebar_cursor: 0,
            sidebar_state: ListState::default(),
            sidebar_page: 10,
            entries: Vec::new(),
            entry_cursor: 0,
            entry_state: ListState::default(),
            entry_page: 10,
            total_entries: None,
            search: String::new(),
            feed_filter: String::new(),
            focus: Focus::Feeds,
            mode: Mode::Normal,
            status: None,
            loading: false,
            bootstrapped: false,
            article_text: String::new(),
            article_parts: Vec::new(),
            images: HashMap::new(),
            show_images,
            article_entry: None,
            article_scroll: 0,
            article_max_scroll: 0,
            article_page: 10,
            article_paragraph_stops: Vec::new(),
            should_quit: false,
            dirty: true,
            startup_warning: None,
            pending_select_feed: None,
            api_tx,
            generation: 0,
        };
        app.request_bootstrap("Connecting to Miniflux…");
        app
    }

    // ── Data loading ─────────────────────────────────────────────────

    fn request_bootstrap(&mut self, message: &str) {
        self.loading = true;
        self.set_status(StatusKind::Info, message);
        self.send(ApiCommand::Bootstrap);
    }

    fn send(&mut self, command: ApiCommand) {
        if self.api_tx.send(command).is_err() {
            self.loading = false;
            self.set_status(
                StatusKind::Error,
                "The Miniflux worker stopped unexpectedly",
            );
        }
    }

    pub fn load_entries(&mut self, clear: bool) {
        if !self.bootstrapped {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        let filter = api::entry_filter(
            &self.selection,
            &self.search,
            self.config.limit,
            self.hide_read_entries,
        );
        if clear {
            self.entries.clear();
            self.entry_cursor = 0;
            self.entry_state.select(None);
            self.total_entries = None;
            self.article_text.clear();
            self.article_parts.clear();
            self.images.clear();
            self.article_paragraph_stops.clear();
            self.article_entry = None;
            self.article_scroll = 0;
        }
        self.loading = true;
        self.send(ApiCommand::LoadEntries {
            filter: Box::new(filter),
            generation: self.generation,
        });
    }

    fn apply_selection(&mut self, selection: Selection) {
        if self.selection == selection {
            return;
        }
        self.selection = selection;
        self.sync_sidebar_cursor();
        self.focus = Focus::Feeds;
        self.load_entries(true);
    }

    fn sync_sidebar_cursor(&mut self) {
        let rows = self.sidebar_rows();
        if let Some(index) = rows
            .iter()
            .position(|row| row.selection() == self.selection)
        {
            self.sidebar_cursor = index;
        } else {
            self.selection = Selection::Unread;
            self.sidebar_cursor = rows
                .iter()
                .position(|row| row.selection() == Selection::Unread)
                .unwrap_or(0);
        }
        self.sidebar_state.select(Some(self.sidebar_cursor));
    }

    // ── Derived data ─────────────────────────────────────────────────

    pub fn sidebar_rows(&self) -> Vec<SidebarRow> {
        let mut rows =
            vec![SidebarRow::Unread, SidebarRow::All, SidebarRow::Starred];

        let filter = self.feed_filter.trim().to_lowercase();
        let matches = |title: &str| {
            filter.is_empty() || title.to_lowercase().contains(&filter)
        };

        let mut categories: Vec<&Category> = self.categories.iter().collect();
        categories.sort_by_key(|category| category.title.to_lowercase());

        for category in categories {
            let mut feeds: Vec<&Feed> = self
                .feeds
                .iter()
                .filter(|feed| feed.category.id == category.id)
                .collect();
            feeds.sort_by_key(|feed| feed.title.to_lowercase());

            let category_selected =
                self.selection == Selection::Category(category.id);
            if self.hide_read_feeds {
                feeds.retain(|feed| {
                    self.feed_unread(feed.id) > 0
                        || self.selection == Selection::Feed(feed.id)
                });
            }
            if !matches(&category.title) {
                feeds.retain(|feed| matches(&feed.title));
            }
            // Keep a selected category visible even when its last feed is
            // filtered out, so the panel does not jump under the cursor.
            if feeds.is_empty()
                && !category_selected
                && (self.hide_read_feeds || !filter.is_empty())
            {
                continue;
            }

            rows.push(SidebarRow::Category {
                id: category.id,
                title: category.title.clone(),
            });
            for feed in feeds {
                rows.push(SidebarRow::Feed {
                    id: feed.id,
                    title: feed.title.clone(),
                });
            }
        }

        rows
    }

    pub fn unread_total(&self) -> i64 {
        self.counters.unreads.values().sum()
    }

    pub fn feed_unread(&self, id: FeedID) -> i64 {
        self.counters.unreads.get(&id).copied().unwrap_or(0)
    }

    pub fn category_unread(&self, id: CategoryID) -> i64 {
        self.feeds
            .iter()
            .filter(|feed| feed.category.id == id)
            .map(|feed| self.feed_unread(feed.id))
            .sum()
    }

    pub fn selection_label(&self) -> String {
        match &self.selection {
            Selection::All => "Everything".to_string(),
            Selection::Unread => "Unread".to_string(),
            Selection::Starred => "Starred".to_string(),
            Selection::Category(id) => self
                .categories
                .iter()
                .find(|category| category.id == *id)
                .map(|category| category.title.clone())
                .unwrap_or_else(|| "Category".to_string()),
            Selection::Feed(id) => self
                .feeds
                .iter()
                .find(|feed| feed.id == *id)
                .map(|feed| feed.title.clone())
                .unwrap_or_else(|| "Feed".to_string()),
        }
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.entry_cursor)
    }

    pub fn entry_counter_label(&self) -> String {
        match self.total_entries {
            Some(total) if total as usize != self.entries.len() => {
                format!("{} / {}", self.entries.len(), total)
            }
            _ => self.entries.len().to_string(),
        }
    }

    // ── Key handling ─────────────────────────────────────────────────

    pub fn handle_event(&mut self, event: crossterm::event::Event) {
        use crossterm::event::{Event, KeyEventKind};

        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                self.dirty = true;
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL)
                    && key.code == crossterm::event::KeyCode::Char('c')
                {
                    self.should_quit = true;
                    return;
                }
                let mode = std::mem::take(&mut self.mode);
                match mode {
                    Mode::Normal => self.handle_normal(key),
                    Mode::Search {
                        input,
                        scope,
                        original,
                        original_selection,
                    } => self.handle_search(
                        key,
                        input,
                        scope,
                        original,
                        original_selection,
                    ),
                    Mode::Subscribe { input } => {
                        self.handle_subscribe(key, input)
                    }
                    Mode::FeedPicker {
                        subscriptions,
                        cursor,
                        category_id,
                    } => self.handle_feed_picker(
                        key,
                        subscriptions,
                        cursor,
                        category_id,
                    ),
                    Mode::Help { scroll } => self.handle_help(key, scroll),
                    Mode::Confirm { prompt, action } => {
                        self.handle_confirm(key, prompt, action)
                    }
                    Mode::ThemePicker { cursor, original } => {
                        self.handle_theme_picker(key, cursor, original)
                    }
                    Mode::EditFeed {
                        feed_id,
                        form,
                        cursor,
                        editing,
                        category_picker,
                        confirm_delete,
                    } => self.handle_feed_editor(
                        key,
                        FeedEditorState {
                            feed_id,
                            form,
                            cursor,
                            editing,
                            category_picker,
                            confirm_delete,
                        },
                    ),
                    Mode::RenameCategory { category_id, input } => {
                        self.handle_category_rename(key, category_id, input)
                    }
                }
            }
            Event::Resize(_, _) => self.dirty = true,
            _ => {}
        }
    }

    fn handle_normal(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyModifiers};

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('d') => self.half_page_down(),
                KeyCode::Char('u') => self.half_page_up(),
                KeyCode::Char('f') => self.full_page_down(),
                KeyCode::Char('b') => self.full_page_up(),
                KeyCode::Char('o') => self.open_selected_xdg(),
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('g') => self.goto_top(),
            KeyCode::Char('G') => self.goto_bottom(),

            KeyCode::Char('j') | KeyCode::Down => self.move_down(),
            KeyCode::Char('k') | KeyCode::Up => self.move_up(),
            KeyCode::PageDown => self.full_page_down(),
            KeyCode::PageUp => self.full_page_up(),

            KeyCode::Char('h') | KeyCode::Left => self.focus_left(),
            KeyCode::Char('l') | KeyCode::Right => self.focus_right(),
            KeyCode::Tab => {
                self.focus = self.focus.next(!self.entries.is_empty());
                self.dirty = true;
            }
            KeyCode::BackTab => {
                self.focus = self.focus.previous(!self.entries.is_empty());
                self.dirty = true;
            }
            KeyCode::Char('1') => self.focus = Focus::Feeds,
            KeyCode::Char('2') => self.focus = Focus::Entries,
            KeyCode::Char('3') => {
                if self.selected_entry().is_some() {
                    self.focus = Focus::Article;
                }
            }

            KeyCode::Enter => self.activate(),
            KeyCode::Esc => {
                if self.focus == Focus::Feeds && !self.feed_filter.is_empty() {
                    self.clear_feed_filter();
                } else if self.search.is_empty() {
                    self.focus_left();
                } else {
                    self.clear_search();
                }
            }

            KeyCode::Char('f') => match self.focus {
                Focus::Feeds => self.toggle_hide_read_feeds(),
                Focus::Entries => self.toggle_hide_read_entries(),
                Focus::Article => {}
            },
            KeyCode::Char('m') => self.toggle_read(),
            KeyCode::Char('M') => self.mark_view_read(),
            KeyCode::Char('E') => self.edit_selected(),
            KeyCode::Char('p') => self.jump_to_entry_feed(),
            KeyCode::Char('i') => self.toggle_images(),
            KeyCode::Char(' ') if self.focus == Focus::Article => {
                self.scroll_to_next_paragraph();
            }
            KeyCode::Char('s') => self.toggle_star(),
            KeyCode::Char('J') if self.focus == Focus::Article => {
                self.open_relative_article(1);
            }
            KeyCode::Char('K') if self.focus == Focus::Article => {
                self.open_relative_article(-1);
            }
            KeyCode::Char('S') => self.save_selected(),
            KeyCode::Char('F')
                if matches!(self.focus, Focus::Entries | Focus::Article) =>
            {
                self.fetch_selected_content();
            }
            KeyCode::Char('a') => self.confirm_mark_all(),
            KeyCode::Char('o') => self.open_selected(),
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('/') => {
                let (input, scope, original) = if self.focus == Focus::Feeds {
                    (
                        TextInput::new(self.feed_filter.clone()),
                        SearchScope::Feeds,
                        self.feed_filter.clone(),
                    )
                } else {
                    (
                        TextInput::new(self.search.clone()),
                        SearchScope::Entries,
                        String::new(),
                    )
                };
                self.mode = Mode::Search {
                    input,
                    scope,
                    original,
                    original_selection: self.selection.clone(),
                };
            }
            KeyCode::Char('c') => self.clear_search(),
            KeyCode::Char('n') => {
                self.mode = Mode::Subscribe {
                    input: TextInput::default(),
                };
            }
            KeyCode::Char('t') => self.open_theme_picker(),
            KeyCode::Char('?') => {
                self.mode = Mode::Help { scroll: 0 };
            }
            KeyCode::Char('q') => self.should_quit = true,
            _ => {}
        }
    }

    fn handle_search(
        &mut self,
        key: crossterm::event::KeyEvent,
        mut input: TextInput,
        scope: SearchScope,
        original: String,
        original_selection: Selection,
    ) {
        use crossterm::event::KeyCode;

        match key.code {
            KeyCode::Esc => {
                if scope == SearchScope::Feeds {
                    // Undo the live filtering and its selection jump.
                    self.feed_filter = original;
                    if self.selection != original_selection {
                        self.apply_selection(original_selection);
                    } else {
                        self.sync_sidebar_cursor();
                    }
                    self.dirty = true;
                }
                self.mode = Mode::Normal;
                return;
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                let value = input.as_str().trim().to_string();
                match scope {
                    SearchScope::Entries => {
                        self.search = value;
                        self.focus = Focus::Entries;
                        self.load_entries(true);
                        if !self.search.is_empty() {
                            self.set_status(
                                StatusKind::Info,
                                format!("Search: {}", self.search),
                            );
                        }
                    }
                    SearchScope::Feeds => {
                        self.feed_filter = value;
                        self.focus = Focus::Feeds;
                        self.sync_sidebar_cursor();
                        self.set_status(
                            StatusKind::Info,
                            if self.feed_filter.is_empty() {
                                "Feed filter cleared".to_string()
                            } else {
                                format!("Feed filter: {}", self.feed_filter)
                            },
                        );
                    }
                }
                return;
            }
            _ => {
                apply_text_key(&mut input, key);
            }
        }

        if scope == SearchScope::Feeds {
            self.apply_live_feed_filter(input.as_str());
        }
        self.mode = Mode::Search {
            input,
            scope,
            original,
            original_selection,
        };
    }

    /// Apply the feed filter as the user types and follow the first match.
    fn apply_live_feed_filter(&mut self, input: &str) {
        let value = input.trim().to_string();
        if value == self.feed_filter {
            return;
        }
        self.feed_filter = value;
        self.dirty = true;

        if self.feed_filter.is_empty() {
            // Nothing is filtered out, so leave the view alone.
            self.sync_sidebar_cursor();
            return;
        }
        let target = self.first_matching_feed().unwrap_or(Selection::Unread);
        if self.selection != target {
            self.apply_selection(target);
        } else {
            self.sync_sidebar_cursor();
        }
    }

    fn first_matching_feed(&self) -> Option<Selection> {
        self.sidebar_rows().into_iter().find_map(|row| match row {
            SidebarRow::Feed { id, .. } => Some(Selection::Feed(id)),
            _ => None,
        })
    }

    fn handle_subscribe(
        &mut self,
        key: crossterm::event::KeyEvent,
        mut input: TextInput,
    ) {
        use crossterm::event::KeyCode;

        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                return;
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                self.discover_feeds(input.as_str().to_string());
                return;
            }
            _ => {
                apply_text_key(&mut input, key);
            }
        }
        self.mode = Mode::Subscribe { input };
    }

    fn discover_feeds(&mut self, input: String) {
        let raw = input.trim();
        if raw.is_empty() {
            self.set_status(StatusKind::Warning, "Website URL is required");
            return;
        }

        let candidate = if raw.contains("://") {
            raw.to_string()
        } else {
            format!("https://{raw}")
        };
        let url = match Url::parse(&candidate) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => url,
            _ => {
                self.set_status(
                    StatusKind::Warning,
                    "Enter a valid HTTP(S) website URL",
                );
                return;
            }
        };

        let Some(category_id) = self.subscribe_category() else {
            self.set_status(
                StatusKind::Error,
                "Create a category in Miniflux before subscribing",
            );
            return;
        };

        self.set_status(
            StatusKind::Info,
            format!("Looking for feeds at {url}…"),
        );
        self.send(ApiCommand::DiscoverFeeds { url, category_id });
    }

    fn subscribe_feed(
        &mut self,
        feed_url: &str,
        category_id: CategoryID,
        title: &str,
    ) {
        let url = match Url::parse(feed_url) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => url,
            _ => {
                self.set_status(
                    StatusKind::Error,
                    format!(
                        "Miniflux returned an invalid feed URL: {feed_url}"
                    ),
                );
                return;
            }
        };

        let title = if title.trim().is_empty() {
            url.as_str()
        } else {
            title
        };
        self.set_status(StatusKind::Info, format!("Subscribing to {title}…"));
        self.send(ApiCommand::CreateFeed { url, category_id });
    }

    fn handle_feed_picker(
        &mut self,
        key: crossterm::event::KeyEvent,
        subscriptions: Vec<Subscription>,
        mut cursor: usize,
        category_id: CategoryID,
    ) {
        use crossterm::event::KeyCode;

        let max = subscriptions.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = Mode::Normal;
                self.set_status(StatusKind::Info, "Subscription cancelled");
            }
            KeyCode::Enter => {
                let Some(subscription) = subscriptions.get(cursor) else {
                    self.mode = Mode::Normal;
                    return;
                };
                let feed_url = subscription.url.clone();
                let title = subscription.title.clone();
                self.mode = Mode::Normal;
                self.subscribe_feed(&feed_url, category_id, &title);
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if cursor < max {
                    cursor += 1;
                }
                self.mode = Mode::FeedPicker {
                    subscriptions,
                    cursor,
                    category_id,
                };
            }
            KeyCode::Char('k') | KeyCode::Up => {
                cursor = cursor.saturating_sub(1);
                self.mode = Mode::FeedPicker {
                    subscriptions,
                    cursor,
                    category_id,
                };
            }
            KeyCode::Char('g') => {
                cursor = 0;
                self.mode = Mode::FeedPicker {
                    subscriptions,
                    cursor,
                    category_id,
                };
            }
            KeyCode::Char('G') => {
                cursor = max;
                self.mode = Mode::FeedPicker {
                    subscriptions,
                    cursor,
                    category_id,
                };
            }
            _ => {
                self.mode = Mode::FeedPicker {
                    subscriptions,
                    cursor,
                    category_id,
                };
            }
        }
    }

    fn subscribe_category(&self) -> Option<CategoryID> {
        match self.selection {
            Selection::Category(id) => Some(id),
            Selection::Feed(id) => self
                .feeds
                .iter()
                .find(|feed| feed.id == id)
                .map(|feed| feed.category.id),
            _ => {
                let mut categories: Vec<&Category> =
                    self.categories.iter().collect();
                categories
                    .sort_by_key(|category| category.title.to_lowercase());
                categories
                    .first()
                    .map(|category| category.id)
                    .or_else(|| self.feeds.first().map(|feed| feed.category.id))
            }
        }
    }

    fn handle_help(
        &mut self,
        key: crossterm::event::KeyEvent,
        mut scroll: u16,
    ) {
        use crossterm::event::{KeyCode, KeyModifiers};

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
                self.mode = Mode::Normal;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                scroll = scroll.saturating_add(1);
                self.mode = Mode::Help { scroll };
            }
            KeyCode::Char('k') | KeyCode::Up => {
                scroll = scroll.saturating_sub(1);
                self.mode = Mode::Help { scroll };
            }
            KeyCode::Char('g') => {
                self.mode = Mode::Help { scroll: 0 };
            }
            KeyCode::Char('G') => {
                self.mode = Mode::Help { scroll: u16::MAX };
            }
            KeyCode::Char('d')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                scroll = scroll.saturating_add(8);
                self.mode = Mode::Help { scroll };
            }
            KeyCode::Char('u')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                scroll = scroll.saturating_sub(8);
                self.mode = Mode::Help { scroll };
            }
            _ => {
                self.mode = Mode::Help { scroll };
            }
        }
    }

    fn handle_confirm(
        &mut self,
        key: crossterm::event::KeyEvent,
        prompt: String,
        action: ConfirmAction,
    ) {
        use crossterm::event::KeyCode;

        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                self.mode = Mode::Normal;
                self.execute_confirm(action);
            }
            KeyCode::Char('n')
            | KeyCode::Char('N')
            | KeyCode::Esc
            | KeyCode::Char('q') => {
                self.mode = Mode::Normal;
                self.set_status(StatusKind::Info, "Cancelled");
            }
            _ => {
                self.mode = Mode::Confirm { prompt, action };
            }
        }
    }

    fn execute_confirm(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::MarkAllRead(scope) => {
                self.set_status(StatusKind::Info, "Marking all as read…");
                self.send(ApiCommand::MarkAllRead { scope });
            }
        }
    }

    fn handle_theme_picker(
        &mut self,
        key: crossterm::event::KeyEvent,
        mut cursor: usize,
        original: String,
    ) {
        use crossterm::event::KeyCode;

        let max = self.theme_choices.len().saturating_sub(1);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if cursor < max {
                    cursor += 1;
                    self.preview_theme(cursor);
                }
                self.mode = Mode::ThemePicker { cursor, original };
            }
            KeyCode::Char('k') | KeyCode::Up => {
                cursor = cursor.saturating_sub(1);
                self.preview_theme(cursor);
                self.mode = Mode::ThemePicker { cursor, original };
            }
            KeyCode::Char('g') => {
                cursor = 0;
                self.preview_theme(cursor);
                self.mode = Mode::ThemePicker { cursor, original };
            }
            KeyCode::Char('G') => {
                cursor = max;
                self.preview_theme(cursor);
                self.mode = Mode::ThemePicker { cursor, original };
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                let name = self.theme_choices[cursor].1.clone();
                self.set_status(StatusKind::Success, format!("Theme: {name}"));
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.apply_theme(&original);
                self.mode = Mode::Normal;
            }
            _ => {
                self.mode = Mode::ThemePicker { cursor, original };
            }
        }
    }

    fn preview_theme(&mut self, cursor: usize) {
        if let Some((id, _)) = self.theme_choices.get(cursor).cloned() {
            self.apply_theme(&id);
        }
    }

    pub fn apply_theme(&mut self, id: &str) {
        if let Some(theme) = opaline::builtins::load_by_name(id) {
            self.theme = theme;
            self.theme_id = id.to_string();
            self.dirty = true;
        }
    }

    fn open_theme_picker(&mut self) {
        let cursor = self
            .theme_choices
            .iter()
            .position(|(id, _)| id == &self.theme_id)
            .unwrap_or(0);
        self.mode = Mode::ThemePicker {
            cursor,
            original: self.theme_id.clone(),
        };
    }

    fn edit_selected(&mut self) {
        if let Selection::Category(id) = self.selection {
            self.open_category_rename(id);
            return;
        }
        // The selected entry wins, so `E` works from any view. Fall back to a
        // feed row when the entries pane is empty.
        let feed_id = self.selected_entry().map(|entry| entry.feed_id).or(
            match self.selection {
                Selection::Feed(id) => Some(id),
                _ => None,
            },
        );
        let Some(feed_id) = feed_id else {
            self.set_status(
                StatusKind::Warning,
                "Select an entry or feed to edit",
            );
            return;
        };
        let Some(feed) = self.feeds.iter().find(|feed| feed.id == feed_id)
        else {
            self.set_status(StatusKind::Warning, "Feed is no longer available");
            return;
        };
        self.mode = Mode::EditFeed {
            feed_id,
            form: Box::new(FeedForm::from_feed(feed)),
            cursor: 0,
            editing: false,
            category_picker: None,
            confirm_delete: false,
        };
        self.set_status(StatusKind::Info, "Editing the feed; Ctrl-s saves");
    }

    /// Select the feed of the entry under the cursor in the Feeds pane.
    fn jump_to_entry_feed(&mut self) {
        let Some(feed_id) = self.selected_entry().map(|entry| entry.feed_id)
        else {
            self.set_status(StatusKind::Warning, "No entry selected");
            return;
        };
        if !self.feeds.iter().any(|feed| feed.id == feed_id) {
            self.set_status(StatusKind::Warning, "Feed is no longer available");
            return;
        }

        let selection = Selection::Feed(feed_id);
        if self.selection == selection {
            self.focus = Focus::Entries;
            self.dirty = true;
            return;
        }

        self.apply_selection(selection);
        // `apply_selection` focuses the sidebar; the reader keeps the entries
        // in view after jumping.
        self.focus = Focus::Entries;
        self.set_status(
            StatusKind::Info,
            format!("Showing “{}”", self.selection_label()),
        );
    }

    fn open_category_rename(&mut self, id: CategoryID) {
        let Some(category) =
            self.categories.iter().find(|category| category.id == id)
        else {
            self.set_status(StatusKind::Warning, "Category is not loaded");
            return;
        };
        self.mode = Mode::RenameCategory {
            category_id: id,
            input: TextInput::new(category.title.clone()),
        };
        self.set_status(StatusKind::Info, "Renaming the category; Enter saves");
    }

    fn handle_category_rename(
        &mut self,
        key: crossterm::event::KeyEvent,
        category_id: CategoryID,
        mut input: TextInput,
    ) {
        use crossterm::event::KeyCode;

        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.set_status(StatusKind::Info, "Category rename cancelled");
                return;
            }
            KeyCode::Enter => {
                let title = input.as_str().trim().to_string();
                if title.is_empty() {
                    self.set_status(
                        StatusKind::Warning,
                        "Give the category a name",
                    );
                } else {
                    self.set_status(StatusKind::Info, "Renaming category…");
                    self.send(ApiCommand::UpdateCategory {
                        id: category_id,
                        title,
                    });
                }
            }
            _ => {
                apply_text_key(&mut input, key);
            }
        }
        self.mode = Mode::RenameCategory { category_id, input };
        self.dirty = true;
    }

    fn handle_feed_editor(
        &mut self,
        key: crossterm::event::KeyEvent,
        state: FeedEditorState,
    ) {
        use crossterm::event::{KeyCode, KeyModifiers};

        let FeedEditorState {
            feed_id,
            mut form,
            mut cursor,
            mut editing,
            mut category_picker,
            mut confirm_delete,
        } = state;
        let last = FeedField::ALL.len() - 1;
        cursor = cursor.min(last);
        let field = FeedField::ALL[cursor];

        if confirm_delete {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    self.mode = Mode::Normal;
                    self.set_status(StatusKind::Info, "Unsubscribing…");
                    self.send(ApiCommand::DeleteFeed { id: feed_id });
                    self.dirty = true;
                    return;
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    confirm_delete = false;
                }
                _ => {}
            }
            self.mode = Mode::EditFeed {
                feed_id,
                form,
                cursor,
                editing,
                category_picker,
                confirm_delete,
            };
            self.dirty = true;
            return;
        }

        if let Some(mut picker) = category_picker {
            let categories = self.sorted_categories();
            let last = categories.len().saturating_sub(1);
            let mut close = false;
            match key.code {
                KeyCode::Esc => close = true,
                KeyCode::Enter => {
                    if let Some(category) = categories.get(picker) {
                        form.category_id = category.id;
                    }
                    close = true;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    picker = (picker + 1).min(last);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    picker = picker.saturating_sub(1);
                }
                KeyCode::Char('g') => picker = 0,
                KeyCode::Char('G') => picker = last,
                _ => {}
            }
            self.mode = Mode::EditFeed {
                feed_id,
                form,
                cursor,
                editing,
                category_picker: if close { None } else { Some(picker) },
                confirm_delete,
            };
            self.dirty = true;
            return;
        }

        if editing {
            match key.code {
                KeyCode::Enter | KeyCode::Esc => editing = false,
                _ => {
                    if let Some(input) = form.text_mut(field) {
                        apply_text_key(input, key);
                    }
                }
            }
            self.mode = Mode::EditFeed {
                feed_id,
                form,
                cursor,
                editing,
                category_picker: None,
                confirm_delete: false,
            };
            self.dirty = true;
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.set_status(StatusKind::Info, "Feed edit cancelled");
                return;
            }
            KeyCode::Char('s')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.save_feed(feed_id, &form);
            }
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab => {
                cursor = (cursor + 1).min(last);
            }
            KeyCode::Char('k') | KeyCode::Up | KeyCode::BackTab => {
                cursor = cursor.saturating_sub(1);
            }
            KeyCode::Char('g') => cursor = 0,
            KeyCode::Char('G') => cursor = last,
            KeyCode::Enter => match field {
                FeedField::Category => {
                    let categories = self.sorted_categories();
                    if categories.is_empty() {
                        self.set_status(
                            StatusKind::Warning,
                            "No categories to choose from",
                        );
                    } else {
                        let index = categories
                            .iter()
                            .position(|category| {
                                category.id == form.category_id
                            })
                            .unwrap_or(0);
                        category_picker = Some(index);
                    }
                }
                _ if field.is_toggle() => form.toggle(field),
                _ => editing = true,
            },
            KeyCode::Char('d') => confirm_delete = true,
            KeyCode::Char('h') | KeyCode::Left => {
                self.adjust_feed_field(&mut form, field, false);
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.adjust_feed_field(&mut form, field, true);
            }
            _ => {}
        }

        self.mode = Mode::EditFeed {
            feed_id,
            form,
            cursor,
            editing,
            category_picker,
            confirm_delete,
        };
        self.dirty = true;
    }

    fn adjust_feed_field(
        &self,
        form: &mut FeedForm,
        field: FeedField,
        forward: bool,
    ) {
        match field {
            FeedField::Crawler => form.crawler = forward,
            FeedField::Disabled => form.disabled = forward,
            FeedField::IgnoreHttpCache => form.ignore_http_cache = forward,
            FeedField::FetchViaProxy => form.fetch_via_proxy = forward,
            _ => {}
        }
    }

    /// Categories in the order the sidebar and the pickers show them.
    pub fn sorted_categories(&self) -> Vec<&Category> {
        let mut categories: Vec<&Category> = self.categories.iter().collect();
        categories.sort_by_key(|category| category.title.to_lowercase());
        categories
    }

    fn save_feed(&mut self, feed_id: FeedID, form: &FeedForm) {
        self.set_status(StatusKind::Info, "Saving feed…");
        self.send(ApiCommand::UpdateFeed {
            id: feed_id,
            changes: Box::new(form.update()),
        });
    }

    // ── Navigation ───────────────────────────────────────────────────

    fn move_down(&mut self) {
        match self.focus {
            Focus::Feeds => self.move_sidebar(1),
            Focus::Entries => self.move_entry(1),
            Focus::Article => self.scroll_article(1),
        }
    }

    fn move_up(&mut self) {
        match self.focus {
            Focus::Feeds => self.move_sidebar(-1),
            Focus::Entries => self.move_entry(-1),
            Focus::Article => self.scroll_article(-1),
        }
    }

    fn half_page_down(&mut self) {
        match self.focus {
            Focus::Feeds => {
                let step = (self.sidebar_page / 2).max(1) as isize;
                self.move_sidebar(step);
            }
            Focus::Entries => {
                let step = (self.entry_page / 2).max(1) as isize;
                self.move_entry(step);
            }
            Focus::Article => {
                self.scroll_article((self.article_page / 2).max(1) as i32);
            }
        }
    }

    fn half_page_up(&mut self) {
        match self.focus {
            Focus::Feeds => {
                let step = (self.sidebar_page / 2).max(1) as isize;
                self.move_sidebar(-step);
            }
            Focus::Entries => {
                let step = (self.entry_page / 2).max(1) as isize;
                self.move_entry(-step);
            }
            Focus::Article => {
                self.scroll_article(-((self.article_page / 2).max(1) as i32));
            }
        }
    }

    fn full_page_down(&mut self) {
        match self.focus {
            Focus::Feeds => {
                let step = self.sidebar_page.max(1) as isize;
                self.move_sidebar(step);
            }
            Focus::Entries => {
                let step = self.entry_page.max(1) as isize;
                self.move_entry(step);
            }
            Focus::Article => {
                self.scroll_article(self.article_page.max(1) as i32);
            }
        }
    }

    fn full_page_up(&mut self) {
        match self.focus {
            Focus::Feeds => {
                let step = self.sidebar_page.max(1) as isize;
                self.move_sidebar(-step);
            }
            Focus::Entries => {
                let step = self.entry_page.max(1) as isize;
                self.move_entry(-step);
            }
            Focus::Article => {
                self.scroll_article(-(self.article_page.max(1) as i32));
            }
        }
    }

    fn move_sidebar(&mut self, delta: isize) {
        let rows = self.sidebar_rows();
        if rows.is_empty() {
            return;
        }
        let current = self.sidebar_cursor.min(rows.len() - 1);
        let next = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current.saturating_add(delta as usize).min(rows.len() - 1)
        };
        if next != current {
            let selection = rows[next].selection();
            self.apply_selection(selection);
        }
    }

    fn move_entry(&mut self, delta: isize) {
        self.try_move_entry(delta);
    }

    /// Move the entry cursor, returning whether it actually changed position.
    fn try_move_entry(&mut self, delta: isize) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        let max = self.entries.len() - 1;
        let next = if delta < 0 {
            self.entry_cursor.saturating_sub(delta.unsigned_abs())
        } else {
            self.entry_cursor.saturating_add(delta as usize).min(max)
        };
        if next == self.entry_cursor {
            return false;
        }
        self.entry_cursor = next;
        self.entry_state.select(Some(next));
        self.article_scroll = 0;
        self.ensure_article();
        self.dirty = true;
        true
    }

    /// Switch to the next/previous article in the reader, treating it like
    /// opening the article from the list.
    fn open_relative_article(&mut self, delta: isize) {
        if self.try_move_entry(delta) {
            self.mark_selected_if_unread();
        }
    }

    fn scroll_to_next_paragraph(&mut self) {
        let current = self.article_scroll as usize;
        let target = self
            .article_paragraph_stops
            .iter()
            .copied()
            .find(|&start| start > current)
            .unwrap_or(self.article_max_scroll as usize);
        self.article_scroll =
            target.min(self.article_max_scroll as usize) as u16;
        self.dirty = true;
    }

    fn scroll_article(&mut self, delta: i32) {
        if delta < 0 {
            self.article_scroll = self.article_scroll.saturating_sub(
                delta.unsigned_abs().min(u16::MAX as u32) as u16,
            );
        } else {
            self.article_scroll = self
                .article_scroll
                .saturating_add(delta.min(u16::MAX as i32) as u16)
                .min(self.article_max_scroll);
        }
        self.dirty = true;
    }

    fn goto_top(&mut self) {
        match self.focus {
            Focus::Feeds => {
                if self.sidebar_cursor != 0 {
                    let rows = self.sidebar_rows();
                    if let Some(row) = rows.first() {
                        let selection = row.selection();
                        self.apply_selection(selection);
                    }
                }
            }
            Focus::Entries => {
                if self.entry_cursor != 0 {
                    self.entry_cursor = 0;
                    self.entry_state.select(Some(0));
                    self.article_scroll = 0;
                    self.ensure_article();
                    self.dirty = true;
                }
            }
            Focus::Article => {
                self.article_scroll = 0;
                self.dirty = true;
            }
        }
    }

    fn goto_bottom(&mut self) {
        match self.focus {
            Focus::Feeds => {
                let rows = self.sidebar_rows();
                if let Some(index) = rows.len().checked_sub(1)
                    && index != self.sidebar_cursor
                    && let Some(row) = rows.get(index)
                {
                    let selection = row.selection();
                    self.apply_selection(selection);
                }
            }
            Focus::Entries => {
                if let Some(index) = self.entries.len().checked_sub(1)
                    && index != self.entry_cursor
                {
                    self.entry_cursor = index;
                    self.entry_state.select(Some(index));
                    self.article_scroll = 0;
                    self.ensure_article();
                    self.dirty = true;
                }
            }
            Focus::Article => {
                self.article_scroll = self.article_max_scroll;
                self.dirty = true;
            }
        }
    }

    fn focus_left(&mut self) {
        self.focus = match self.focus {
            Focus::Article => Focus::Entries,
            Focus::Entries => Focus::Feeds,
            Focus::Feeds => Focus::Feeds,
        };
        self.dirty = true;
    }

    fn focus_right(&mut self) {
        match self.focus {
            Focus::Feeds => {
                self.focus = Focus::Entries;
                self.dirty = true;
            }
            Focus::Entries => self.activate(),
            Focus::Article => {}
        }
    }

    fn activate(&mut self) {
        match self.focus {
            Focus::Feeds => {
                self.focus = Focus::Entries;
                self.dirty = true;
            }
            Focus::Entries => {
                if self.selected_entry().is_some() {
                    self.focus = Focus::Article;
                    self.article_scroll = 0;
                    self.mark_selected_if_unread();
                    self.dirty = true;
                }
            }
            Focus::Article => {}
        }
    }

    // ── Actions ──────────────────────────────────────────────────────

    fn mark_selected_if_unread(&mut self) {
        if !self.config.mark_read {
            return;
        }
        if let Some(entry) = self.selected_entry()
            && entry.status == EntryStatus::Unread
        {
            let id = entry.id;
            self.apply_read_locally(id, true);
            self.send(ApiCommand::SetRead { id, read: true });
        }
    }

    fn toggle_read(&mut self) {
        let Some(entry) = self.selected_entry() else {
            return;
        };
        let id = entry.id;
        let read = entry.status == EntryStatus::Unread;
        self.apply_read_locally(id, read);
        self.send(ApiCommand::SetRead { id, read });
        self.set_status(
            StatusKind::Info,
            if read {
                "Marking as read…"
            } else {
                "Marking as unread…"
            },
        );
        if read {
            // Marking read is a reading-flow action: keep the entry in the
            // list and move on to the next one. The entry only disappears on
            // the next reload when read entries are filtered out.
            self.move_entry(1);
        }
    }

    /// The API scope that covers every entry in the current view.
    ///
    /// Searches and the starred view exist only on the client, so those mark
    /// exactly the loaded results; every other view maps onto a server-side
    /// scope, including entries beyond the fetch limit.
    fn view_scope(&self) -> Result<(MarkAllScope, String), &'static str> {
        let loaded_only = !self.search.is_empty()
            || matches!(self.selection, Selection::Starred);
        if loaded_only {
            let ids: Vec<EntryID> =
                self.entries.iter().map(|entry| entry.id).collect();
            if ids.is_empty() {
                return Err("No loaded entries to mark");
            }
            let description = if self.search.is_empty() {
                format!("the {} loaded starred entries", ids.len())
            } else {
                format!("the {} loaded search results", ids.len())
            };
            return Ok((MarkAllScope::Entries(ids), description));
        }

        let scope = match &self.selection {
            Selection::All | Selection::Unread => {
                let id = self.user_id.ok_or("Still connecting to Miniflux")?;
                MarkAllScope::User(id)
            }
            Selection::Category(id) => MarkAllScope::Category(*id),
            Selection::Feed(id) => MarkAllScope::Feed(*id),
            Selection::Starred => unreachable!(),
        };
        Ok((
            scope,
            format!("all entries in “{}”", self.selection_label()),
        ))
    }

    fn mark_view_read(&mut self) {
        match self.view_scope() {
            Ok((scope, description)) => {
                self.set_status(
                    StatusKind::Info,
                    format!("Marking {description} as read…"),
                );
                self.send(ApiCommand::MarkAllRead { scope });
            }
            Err(reason) => self.set_status(StatusKind::Warning, reason),
        }
    }

    fn toggle_star(&mut self) {
        let Some(entry) = self.selected_entry() else {
            return;
        };
        let id = entry.id;
        let starred = !entry.starred;
        if let Some(entry) =
            self.entries.iter_mut().find(|entry| entry.id == id)
        {
            entry.starred = starred;
        }
        self.send(ApiCommand::ToggleStarred { id });
        self.set_status(
            StatusKind::Info,
            if starred {
                "Adding bookmark…"
            } else {
                "Removing bookmark…"
            },
        );
    }

    fn open_selected(&mut self) {
        let Some(entry) = self.selected_entry() else {
            self.set_status(StatusKind::Info, "No entry selected");
            return;
        };
        let url = entry.url.clone();
        match util::open_in_browser(&url) {
            Ok(()) => {
                self.set_status(StatusKind::Success, "Opened in your browser")
            }
            Err(error) => self.set_status(
                StatusKind::Error,
                format!("Could not open the browser: {error}"),
            ),
        }
    }

    fn open_selected_xdg(&mut self) {
        let Some(entry) = self.selected_entry() else {
            self.set_status(StatusKind::Info, "No entry selected");
            return;
        };
        let url = entry.url.clone();
        match util::open_in_xdg_browser(&url) {
            Ok(()) => self.set_status(
                StatusKind::Success,
                "Opened in the XDG default browser",
            ),
            Err(error) => self.set_status(
                StatusKind::Error,
                format!("Could not open the XDG browser: {error}"),
            ),
        }
    }

    fn save_selected(&mut self) {
        let Some(entry) = self.selected_entry() else {
            self.set_status(StatusKind::Info, "No entry selected");
            return;
        };
        let id = entry.id;
        self.set_status(StatusKind::Info, "Saving to integration…");
        self.send(ApiCommand::SaveEntry { id });
    }

    fn fetch_selected_content(&mut self) {
        let Some(entry) = self.selected_entry() else {
            self.set_status(StatusKind::Info, "No entry selected");
            return;
        };
        let id = entry.id;
        self.set_status(StatusKind::Info, "Fetching original content…");
        self.send(ApiCommand::FetchContent { id });
    }

    fn refresh(&mut self) {
        self.loading = true;
        self.set_status(StatusKind::Info, "Refreshing feeds…");
        self.send(ApiCommand::Refresh);
    }

    fn confirm_mark_all(&mut self) {
        match self.view_scope() {
            Ok((scope, description)) => {
                self.mode = Mode::Confirm {
                    prompt: format!("Mark {description} as read?"),
                    action: ConfirmAction::MarkAllRead(scope),
                };
            }
            Err(reason) => self.set_status(StatusKind::Warning, reason),
        }
    }

    fn toggle_hide_read_feeds(&mut self) {
        self.hide_read_feeds = !self.hide_read_feeds;

        if self.hide_read_feeds {
            let selection_hidden = match self.selection {
                Selection::Feed(id) => self.feed_unread(id) == 0,
                Selection::Category(id) => self.category_unread(id) == 0,
                _ => false,
            };
            if selection_hidden {
                self.selection = Selection::Unread;
                self.load_entries(true);
            }
        }

        self.sync_sidebar_cursor();
        self.set_status(
            StatusKind::Info,
            if self.hide_read_feeds {
                "Hiding feeds without unread entries"
            } else {
                "Showing all feeds"
            },
        );
    }

    fn toggle_hide_read_entries(&mut self) {
        self.hide_read_entries = !self.hide_read_entries;
        self.load_entries(true);
        self.set_status(
            StatusKind::Info,
            if self.hide_read_entries {
                "Hiding read entries"
            } else {
                "Showing all entries"
            },
        );
    }

    fn clear_feed_filter(&mut self) {
        if self.feed_filter.is_empty() {
            return;
        }
        self.feed_filter.clear();
        self.sync_sidebar_cursor();
        self.set_status(StatusKind::Info, "Feed filter cleared");
    }

    fn clear_search(&mut self) {
        if self.search.is_empty() {
            return;
        }
        self.search.clear();
        self.load_entries(true);
        self.set_status(StatusKind::Info, "Search cleared");
    }

    // ── API events ───────────────────────────────────────────────────

    pub fn handle_api(&mut self, event: ApiEvent) {
        self.dirty = true;
        match event {
            ApiEvent::Bootstrap(data) => self.on_bootstrap(*data),
            ApiEvent::Entries {
                generation,
                entries,
                total,
            } => {
                if generation != self.generation {
                    return;
                }
                self.loading = false;
                self.entries = entries;
                self.total_entries = Some(total);
                if self.entries.is_empty() {
                    self.entry_cursor = 0;
                    self.entry_state.select(None);
                } else {
                    self.entry_cursor =
                        self.entry_cursor.min(self.entries.len() - 1);
                    self.entry_state.select(Some(self.entry_cursor));
                }
                self.article_scroll = 0;
                self.ensure_article();
                if self.entries.is_empty() {
                    self.set_status(
                        StatusKind::Info,
                        "No entries in this view",
                    );
                }
            }
            ApiEvent::ReadUpdated { id, read } => {
                self.update_read(id, read);
            }
            ApiEvent::StarredUpdated { id, starred } => {
                if let Some(entry) =
                    self.entries.iter_mut().find(|entry| entry.id == id)
                {
                    entry.starred = starred;
                }
                self.set_status(
                    StatusKind::Success,
                    if starred {
                        "Bookmarked"
                    } else {
                        "Bookmark removed"
                    },
                );
            }
            ApiEvent::Saved => {
                self.set_status(
                    StatusKind::Success,
                    "Saved to your Miniflux integration",
                );
            }
            ApiEvent::FeedsDiscovered {
                subscriptions,
                category_id,
                source_url,
            } => {
                if subscriptions.is_empty() {
                    self.set_status(
                        StatusKind::Warning,
                        format!("No feeds found at {source_url}"),
                    );
                } else if subscriptions.len() == 1 {
                    let subscription = &subscriptions[0];
                    let feed_url = subscription.url.clone();
                    let title = subscription.title.clone();
                    self.subscribe_feed(&feed_url, category_id, &title);
                } else {
                    self.set_status(
                        StatusKind::Info,
                        format!(
                            "Found {} feeds at {source_url}",
                            subscriptions.len()
                        ),
                    );
                    self.mode = Mode::FeedPicker {
                        subscriptions,
                        cursor: 0,
                        category_id,
                    };
                }
            }
            ApiEvent::FeedCreated { feed_id } => {
                self.pending_select_feed = Some(feed_id);
                self.loading = true;
                self.set_status(StatusKind::Success, "Subscribed to feed");
                self.send(ApiCommand::Bootstrap);
            }
            ApiEvent::FetchedContent {
                id,
                content,
                reading_time,
            } => {
                if let Some(entry) =
                    self.entries.iter_mut().find(|entry| entry.id == id)
                {
                    entry.content = content;
                    if reading_time > 0 {
                        entry.reading_time = reading_time;
                    }
                }
                if self.selected_entry().map(|entry| entry.id) == Some(id) {
                    self.article_entry = None;
                    self.article_scroll = 0;
                    self.ensure_article();
                }
                self.set_status(
                    StatusKind::Success,
                    "Fetched original content",
                );
            }
            ApiEvent::FeedUpdated(feed) => {
                let id = feed.id;
                let title = feed.title.clone();
                if let Some(existing) =
                    self.feeds.iter_mut().find(|feed| feed.id == id)
                {
                    *existing = *feed;
                }
                if matches!(
                    &self.mode,
                    Mode::EditFeed { feed_id, .. } if *feed_id == id
                ) {
                    self.mode = Mode::Normal;
                }
                self.sync_sidebar_cursor();
                self.set_status(
                    StatusKind::Success,
                    format!("Updated “{title}”"),
                );
            }
            ApiEvent::FeedDeleted { feed_id } => {
                let title = self
                    .feeds
                    .iter()
                    .find(|feed| feed.id == feed_id)
                    .map(|feed| feed.title.clone())
                    .unwrap_or_else(|| "feed".to_string());
                self.feeds.retain(|feed| feed.id != feed_id);
                self.counters.unreads.remove(&feed_id);
                self.counters.reads.remove(&feed_id);
                if matches!(
                    &self.mode,
                    Mode::EditFeed { feed_id: open, .. } if *open == feed_id
                ) {
                    self.mode = Mode::Normal;
                }
                self.sync_sidebar_cursor();
                self.set_status(
                    StatusKind::Success,
                    format!("Unsubscribed from “{title}”"),
                );
                self.load_entries(true);
            }
            ApiEvent::MarkedAllRead => {
                self.loading = true;
                self.set_status(
                    StatusKind::Success,
                    "Marked all entries as read",
                );
                self.send(ApiCommand::Bootstrap);
            }
            ApiEvent::CategoryUpdated(category) => {
                let id = category.id;
                let title = category.title.clone();
                if let Some(existing) = self
                    .categories
                    .iter_mut()
                    .find(|category| category.id == id)
                {
                    existing.title = title.clone();
                }
                if matches!(
                    &self.mode,
                    Mode::RenameCategory { category_id, .. } if *category_id == id
                ) {
                    self.mode = Mode::Normal;
                }
                self.sync_sidebar_cursor();
                self.set_status(
                    StatusKind::Success,
                    format!("Renamed category to “{title}”"),
                );
            }
            ApiEvent::ImageLoaded { url, result } => {
                if self.images.contains_key(&url) {
                    let state = match result {
                        Ok(image) => LoadedImage::Ready(image),
                        Err(_) => LoadedImage::Failed,
                    };
                    self.images.insert(url, state);
                }
            }
            ApiEvent::Error { context, message } => {
                self.loading = false;
                self.set_status(
                    StatusKind::Error,
                    format!("{context}: {message}"),
                );
            }
            ApiEvent::Notice(message) => {
                self.set_status(StatusKind::Info, message);
            }
        }
    }

    fn on_bootstrap(&mut self, data: BootstrapData) {
        let first = !self.bootstrapped;
        self.feeds = data.feeds;
        self.categories = data.categories;
        self.counters = data.counters;
        self.user_id = Some(data.user_id);
        self.bootstrapped = true;
        self.loading = false;
        self.sync_sidebar_cursor();

        if first {
            if let Some(warning) = self.startup_warning.take() {
                self.set_status(StatusKind::Warning, warning);
            } else {
                self.set_status(
                    StatusKind::Success,
                    format!(
                        "Connected · {} feeds · {} unread",
                        self.feeds.len(),
                        self.unread_total()
                    ),
                );
            }
        }

        let new_feed = self
            .pending_select_feed
            .take()
            .filter(|id| self.feeds.iter().any(|feed| feed.id == *id));
        if let Some(feed_id) = new_feed {
            self.selection = Selection::Feed(feed_id);
            self.focus = Focus::Feeds;
            self.sync_sidebar_cursor();
            self.load_entries(true);
        } else {
            self.load_entries(false);
        }
    }

    fn update_read(&mut self, id: EntryID, read: bool) {
        self.apply_read_locally(id, read);
        self.set_status(
            StatusKind::Success,
            if read { "Marked read" } else { "Marked unread" },
        );
    }

    /// Apply a read-state change to the loaded entry and unread counters.
    /// Returns false when the entry was already in the requested state.
    fn apply_read_locally(&mut self, id: EntryID, read: bool) -> bool {
        let Some(index) = self.entries.iter().position(|entry| entry.id == id)
        else {
            return false;
        };
        let feed_id = self.entries[index].feed_id;
        let old = self.entries[index].status;
        let new = if read {
            EntryStatus::Read
        } else {
            EntryStatus::Unread
        };
        if old == new {
            return false;
        }
        self.entries[index].status = new;

        let unread = self.counters.unreads.entry(feed_id).or_insert(0);
        if new == EntryStatus::Unread {
            *unread += 1;
        } else {
            *unread = (*unread - 1).max(0);
        }
        true
    }

    fn toggle_images(&mut self) {
        self.show_images = !self.show_images;
        if self.show_images {
            self.request_article_images();
        }
        self.set_status(
            StatusKind::Info,
            if self.show_images {
                "Showing images"
            } else {
                "Hiding images"
            },
        );
        self.dirty = true;
    }

    fn request_article_images(&mut self) {
        if !self.show_images {
            return;
        }
        let urls: Vec<String> = self
            .article_parts
            .iter()
            .filter_map(|part| match part {
                util::ArticlePart::Image { url, .. } => Some(url.clone()),
                util::ArticlePart::Text(_) => None,
            })
            .collect();
        for url in urls {
            if self.images.contains_key(&url) {
                continue;
            }
            self.images.insert(url.clone(), LoadedImage::Loading);
            self.send(ApiCommand::LoadImage { url });
        }
    }

    pub fn ensure_article(&mut self) {
        let selected = self
            .selected_entry()
            .map(|entry| (entry.id, entry.content.clone()));

        match selected {
            Some((id, _content)) if self.article_entry == Some(id) => {}
            Some((id, content)) => {
                self.article_entry = Some(id);
                let text = util::html_to_text(&content);
                self.article_parts = util::article_parts(&text);
                self.article_text =
                    util::article_plain_text(&self.article_parts);
                self.images.clear();
                self.article_scroll = 0;
                self.request_article_images();
            }
            None => {
                self.article_entry = None;
                self.article_text.clear();
                self.article_parts.clear();
                self.images.clear();
                self.article_paragraph_stops.clear();
                self.article_scroll = 0;
            }
        }
    }

    // ── Misc ─────────────────────────────────────────────────────────

    pub fn set_startup_warning(&mut self, text: impl Into<String>) {
        self.startup_warning = Some(text.into());
    }

    pub fn set_status(&mut self, kind: StatusKind, text: impl Into<String>) {
        let expires = match kind {
            StatusKind::Error => None,
            _ => Some(Instant::now() + Duration::from_secs(6)),
        };
        self.status = Some(StatusMessage {
            kind,
            text: text.into(),
            expires,
        });
        self.dirty = true;
    }

    pub fn on_tick(&mut self) {
        if let Some(status) = &self.status
            && status
                .expires
                .is_some_and(|expires| Instant::now() >= expires)
        {
            self.status = None;
            self.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Auth, Config};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use url::Url;

    fn test_config() -> Config {
        Config {
            url: Url::parse("http://localhost/").unwrap(),
            auth: Auth::Token("test".to_string()),
            theme: "catppuccin-mocha".to_string(),
            limit: 100,
            mark_read: true,
            hide_read_feeds: false,
            hide_read_entries: false,
            images: false,
        }
    }

    fn test_app_from(
        config: Config,
    ) -> (App, tokio::sync::mpsc::UnboundedReceiver<ApiCommand>) {
        let theme = opaline::builtins::load_by_name("catppuccin-mocha")
            .expect("builtin theme");
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let app = App::new(config, theme, "catppuccin-mocha".to_string(), tx);
        (app, rx)
    }

    fn test_app() -> (App, tokio::sync::mpsc::UnboundedReceiver<ApiCommand>) {
        test_app_from(test_config())
    }

    fn render_category(id: i64, title: &str) -> Category {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "user_id": 1,
            "title": title,
            "hide_globally": false
        }))
        .expect("valid test category")
    }

    fn render_feed(id: i64, title: &str, category_id: i64) -> Feed {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "user_id": 1,
            "title": title,
            "site_url": "https://example.com",
            "feed_url": "https://example.com/feed.xml",
            "category": {
                "id": category_id,
                "user_id": 1,
                "title": format!("Category {category_id}")
            }
        }))
        .expect("valid test feed")
    }

    fn render_entry(id: i64, status: &str) -> Entry {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "user_id": 1,
            "feed_id": 7,
            "title": format!("Entry {id}"),
            "url": format!("https://example.com/{id}"),
            "author": "Author",
            "content": "<p>Body</p>",
            "hash": format!("hash-{id}"),
            "published_at": "2026-01-01T00:00:00Z",
            "created_at": "2026-01-01T00:00:00Z",
            "status": status,
            "share_code": "",
            "starred": false,
            "reading_time": 1,
            "enclosures": [],
            "tags": [],
            "feed": {
                "id": 7,
                "user_id": 1,
                "title": "Test Feed",
                "site_url": "https://example.com",
                "feed_url": "https://example.com/feed.xml",
                "category": {"id": 3, "user_id": 1, "title": "Test"}
            }
        }))
        .expect("valid test entry")
    }

    fn test_app_with_feed()
    -> (App, tokio::sync::mpsc::UnboundedReceiver<ApiCommand>) {
        let (mut app, rx) = test_app();
        app.categories =
            vec![render_category(1, "News"), render_category(2, "Tech")];
        app.feeds = vec![render_feed(7, "Example", 1)];
        app.selection = Selection::Feed(FeedID(7));
        app.focus = Focus::Feeds;
        (app, rx)
    }

    fn press_with(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        app.handle_event(crossterm::event::Event::Key(KeyEvent::new(
            code, modifiers,
        )));
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_event(crossterm::event::Event::Key(KeyEvent::new(
            code,
            KeyModifiers::NONE,
        )));
    }

    #[test]
    fn starts_on_the_unread_view() {
        let (app, _rx) = test_app();
        assert_eq!(app.selection, Selection::Unread);

        let rows = app.sidebar_rows();
        assert!(matches!(rows.first(), Some(SidebarRow::Unread)));
    }

    #[test]
    fn n_starts_the_subscribe_prompt() {
        let (mut app, _rx) = test_app();
        press(&mut app, KeyCode::Char('n'));
        assert!(matches!(app.mode, Mode::Subscribe { .. }));
    }

    #[test]
    fn subscribe_discovers_feeds_from_a_website_url() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap
        app.categories = vec![render_category(2, "Engineering")];
        app.selection = Selection::Category(CategoryID(2));
        app.mode = Mode::Subscribe {
            input: TextInput::new("https://example.com"),
        };

        press(&mut app, KeyCode::Enter);

        let command = rx.try_recv().expect("discover command");
        assert!(matches!(
            command,
            ApiCommand::DiscoverFeeds { url, category_id }
                if url.as_str() == "https://example.com/"
                    && category_id == CategoryID(2)
        ));
    }

    #[test]
    fn one_discovered_feed_is_subscribed_automatically() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap

        app.handle_api(ApiEvent::FeedsDiscovered {
            subscriptions: vec![Subscription {
                url: "https://example.com/feed.xml".to_string(),
                title: "Example".to_string(),
                kind: "rss".to_string(),
            }],
            category_id: CategoryID(2),
            source_url: Url::parse("https://example.com/").unwrap(),
        });

        let command = rx.try_recv().expect("create-feed command");
        assert!(matches!(
            command,
            ApiCommand::CreateFeed { url, category_id }
                if url.as_str() == "https://example.com/feed.xml"
                    && category_id == CategoryID(2)
        ));
    }

    #[test]
    fn multiple_discovered_feeds_open_a_picker() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap

        app.handle_api(ApiEvent::FeedsDiscovered {
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
            category_id: CategoryID(3),
            source_url: Url::parse("https://example.com/").unwrap(),
        });

        assert!(matches!(app.mode, Mode::FeedPicker { .. }));
        assert!(rx.try_recv().is_err());

        press(&mut app, KeyCode::Enter);

        let command = rx.try_recv().expect("create-feed command");
        assert!(matches!(
            command,
            ApiCommand::CreateFeed { url, category_id }
                if url.as_str() == "https://example.com/rss.xml"
                    && category_id == CategoryID(3)
        ));
    }

    #[test]
    fn no_discovered_feeds_does_not_subscribe() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap

        app.handle_api(ApiEvent::FeedsDiscovered {
            subscriptions: Vec::new(),
            category_id: CategoryID(1),
            source_url: Url::parse("https://example.com/").unwrap(),
        });

        assert!(rx.try_recv().is_err());
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn subscribe_rejects_invalid_urls() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap
        app.categories = vec![render_category(1, "Category")];
        app.mode = Mode::Subscribe {
            input: TextInput::new("not a url"),
        };

        press(&mut app, KeyCode::Enter);

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn feed_created_reloads_and_selects_the_new_feed() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap
        app.categories = vec![render_category(1, "Category")];
        app.feeds = vec![render_feed(9, "New Feed", 1)];
        app.pending_select_feed = Some(FeedID(9));
        app.bootstrapped = true;

        app.on_bootstrap(BootstrapData {
            feeds: app.feeds.clone(),
            categories: app.categories.clone(),
            counters: FeedCounters::default(),
            user_id: UserID(1),
        });

        assert_eq!(app.selection, Selection::Feed(FeedID(9)));
        assert_eq!(app.focus, Focus::Feeds);
        let _ = rx.try_recv(); // LoadEntries from the reload
    }

    #[test]
    fn marking_read_advances_to_the_next_entry() {
        let (mut app, _rx) = test_app();
        app.entries =
            vec![render_entry(1, "unread"), render_entry(2, "unread")];
        app.entry_cursor = 0;
        app.focus = Focus::Entries;

        press(&mut app, KeyCode::Char('m'));

        assert_eq!(app.entries.len(), 2);
        assert_eq!(app.entries[0].status, EntryStatus::Read);
        assert_eq!(app.entry_cursor, 1);
    }

    #[test]
    fn marking_read_keeps_the_entry_when_read_entries_are_hidden() {
        let (mut app, _rx) = test_app();
        app.hide_read_entries = true;
        app.entries =
            vec![render_entry(1, "unread"), render_entry(2, "unread")];
        app.entry_cursor = 0;
        app.entry_state.select(Some(0));
        app.total_entries = Some(2);
        app.focus = Focus::Entries;

        press(&mut app, KeyCode::Char('m'));

        assert_eq!(app.entries.len(), 2);
        assert_eq!(app.entries[0].status, EntryStatus::Read);
        assert_eq!(app.entry_cursor, 1);
        assert_eq!(app.total_entries, Some(2));
    }

    #[test]
    fn marking_unread_does_not_advance() {
        let (mut app, _rx) = test_app();
        app.entries = vec![render_entry(1, "read"), render_entry(2, "unread")];
        app.entry_cursor = 0;
        app.focus = Focus::Entries;

        press(&mut app, KeyCode::Char('m'));

        assert_eq!(app.entry_cursor, 0);
        assert_eq!(app.entries[0].status, EntryStatus::Unread);
    }

    #[test]
    fn shift_j_and_k_move_between_articles_in_the_reader() {
        let (mut app, _rx) = test_app();
        app.entries = vec![
            render_entry(1, "unread"),
            render_entry(2, "unread"),
            render_entry(3, "unread"),
        ];
        app.entry_cursor = 0;
        app.focus = Focus::Article;

        press(&mut app, KeyCode::Char('J'));
        assert_eq!(app.entry_cursor, 1);
        assert_eq!(app.entries[1].status, EntryStatus::Read);

        press(&mut app, KeyCode::Char('J'));
        assert_eq!(app.entry_cursor, 2);
        assert_eq!(app.entries[2].status, EntryStatus::Read);

        press(&mut app, KeyCode::Char('K'));
        assert_eq!(app.entry_cursor, 1);
    }

    #[test]
    fn page_keys_scroll_the_reader() {
        let (mut app, _rx) = test_app();
        app.focus = Focus::Article;
        app.article_page = 10;
        app.article_max_scroll = 100;
        app.article_scroll = 0;

        press(&mut app, KeyCode::PageDown);
        assert_eq!(app.article_scroll, 10);

        press(&mut app, KeyCode::PageUp);
        assert_eq!(app.article_scroll, 0);
    }

    #[test]
    fn single_g_jumps_to_the_top_of_the_list() {
        let (mut app, _rx) = test_app();
        app.entries = vec![
            render_entry(1, "unread"),
            render_entry(2, "unread"),
            render_entry(3, "unread"),
        ];
        app.entry_cursor = 2;
        app.focus = Focus::Entries;

        press(&mut app, KeyCode::Char('g'));

        assert_eq!(app.entry_cursor, 0);
    }

    #[test]
    fn hide_read_feeds_filters_the_sidebar() {
        let (mut app, _rx) = test_app();
        app.categories = vec![
            render_category(1, "Read Category"),
            render_category(2, "Unread Category"),
        ];
        app.feeds = vec![
            render_feed(1, "Read Feed", 1),
            render_feed(2, "Unread Feed", 2),
        ];
        app.counters.unreads.insert(FeedID(2), 2);

        let rows = app.sidebar_rows();
        assert!(rows.iter().any(
            |row| matches!(row, SidebarRow::Feed { id, .. } if *id == FeedID(1))
        ));

        app.toggle_hide_read_feeds();
        let rows = app.sidebar_rows();
        assert!(!rows.iter().any(
            |row| matches!(row, SidebarRow::Feed { id, .. } if *id == FeedID(1))
        ));
        assert!(rows.iter().any(
            |row| matches!(row, SidebarRow::Feed { id, .. } if *id == FeedID(2))
        ));
        assert!(!rows.iter().any(|row| matches!(
            row,
            SidebarRow::Category { id, .. } if *id == CategoryID(1)
        )));
        assert!(rows.iter().any(|row| matches!(
            row,
            SidebarRow::Category { id, .. } if *id == CategoryID(2)
        )));
    }

    #[test]
    fn hiding_read_feeds_moves_away_from_a_hidden_selection() {
        let (mut app, _rx) = test_app();
        app.categories = vec![render_category(1, "Read Category")];
        app.feeds = vec![render_feed(1, "Read Feed", 1)];
        app.selection = Selection::Feed(FeedID(1));

        app.toggle_hide_read_feeds();

        assert!(app.hide_read_feeds);
        assert_eq!(app.selection, Selection::Unread);
    }

    #[test]
    fn selected_feed_stays_visible_while_its_last_entry_is_read() {
        let (mut app, _rx) = test_app();
        app.categories = vec![render_category(1, "Category")];
        app.feeds = vec![render_feed(1, "Feed", 1)];
        app.counters.unreads.insert(FeedID(1), 1);
        app.selection = Selection::Feed(FeedID(1));
        app.toggle_hide_read_feeds();

        // Mark the last entry read while the feed is selected.
        app.counters.unreads.insert(FeedID(1), 0);
        let rows = app.sidebar_rows();
        assert!(rows.iter().any(
            |row| matches!(row, SidebarRow::Feed { id, .. } if *id == FeedID(1))
        ));

        // Moving away lets the now-read feed disappear.
        app.selection = Selection::All;
        let rows = app.sidebar_rows();
        assert!(!rows.iter().any(
            |row| matches!(row, SidebarRow::Feed { id, .. } if *id == FeedID(1))
        ));
    }

    #[test]
    fn hide_read_feeds_default_comes_from_config() {
        let mut config = test_config();
        config.hide_read_feeds = true;
        let (app, _rx) = test_app_from(config);
        assert!(app.hide_read_feeds);
    }

    #[test]
    fn hide_read_entries_default_comes_from_config() {
        let mut config = test_config();
        config.hide_read_entries = true;
        let (app, _rx) = test_app_from(config);
        assert!(app.hide_read_entries);
    }

    #[test]
    fn f_toggles_entries_filter_only_from_the_entries_panel() {
        let (mut app, _rx) = test_app();
        app.focus = Focus::Entries;

        press(&mut app, KeyCode::Char('f'));
        assert!(app.hide_read_entries);

        press(&mut app, KeyCode::Char('f'));
        assert!(!app.hide_read_entries);

        app.focus = Focus::Article;
        press(&mut app, KeyCode::Char('f'));
        assert!(!app.hide_read_entries);
    }

    #[test]
    fn uppercase_m_marks_the_unread_view_read() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap
        app.user_id = Some(UserID(1));
        app.selection = Selection::Unread;
        // The focused pane and the selected entry must not narrow the scope.
        app.focus = Focus::Feeds;
        app.entries = vec![render_entry(42, "unread")];

        press(&mut app, KeyCode::Char('M'));

        let command = rx.try_recv().expect("mark-view command");
        assert!(matches!(
            command,
            ApiCommand::MarkAllRead {
                scope: MarkAllScope::User(UserID(1))
            }
        ));
    }

    #[test]
    fn uppercase_m_marks_the_whole_category_read() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap
        app.selection = Selection::Category(CategoryID(3));

        press(&mut app, KeyCode::Char('M'));

        let command = rx.try_recv().expect("mark-view command");
        assert!(matches!(
            command,
            ApiCommand::MarkAllRead {
                scope: MarkAllScope::Category(CategoryID(3))
            }
        ));
    }

    #[test]
    fn uppercase_m_marks_the_selected_feed_read() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap
        app.selection = Selection::Feed(FeedID(42));

        press(&mut app, KeyCode::Char('M'));

        let command = rx.try_recv().expect("mark-view command");
        assert!(matches!(
            command,
            ApiCommand::MarkAllRead {
                scope: MarkAllScope::Feed(FeedID(42))
            }
        ));
    }

    #[test]
    fn uppercase_m_marks_only_the_loaded_starred_entries() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap
        app.selection = Selection::Starred;
        app.entries =
            vec![render_entry(42, "unread"), render_entry(43, "unread")];

        press(&mut app, KeyCode::Char('M'));

        let command = rx.try_recv().expect("mark-view command");
        assert!(matches!(
            command,
            ApiCommand::MarkAllRead {
                scope: MarkAllScope::Entries(ids)
            } if ids == vec![EntryID(42), EntryID(43)]
        ));
    }

    #[test]
    fn uppercase_m_marks_only_the_loaded_search_results() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap
        app.user_id = Some(UserID(1));
        app.selection = Selection::Unread;
        app.search = "rust".to_string();
        app.entries =
            vec![render_entry(42, "unread"), render_entry(43, "unread")];

        press(&mut app, KeyCode::Char('M'));

        let command = rx.try_recv().expect("mark-view command");
        assert!(matches!(
            command,
            ApiCommand::MarkAllRead {
                scope: MarkAllScope::Entries(ids)
            } if ids == vec![EntryID(42), EntryID(43)]
        ));
    }

    #[test]
    fn uppercase_m_needs_a_connection_for_server_wide_views() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap
        app.selection = Selection::Unread;
        app.entries = vec![render_entry(42, "unread")];

        press(&mut app, KeyCode::Char('M'));

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn uppercase_m_needs_loaded_entries_in_a_client_side_view() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap
        app.selection = Selection::Starred;

        press(&mut app, KeyCode::Char('M'));

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn confirm_mark_all_uses_the_current_view() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap
        app.user_id = Some(UserID(1));

        press(&mut app, KeyCode::Char('a'));

        match &app.mode {
            Mode::Confirm { prompt, action } => {
                assert_eq!(prompt, "Mark all entries in “Unread” as read?");
                assert!(matches!(
                    action,
                    ConfirmAction::MarkAllRead(MarkAllScope::User(UserID(1)))
                ));
            }
            other => panic!("expected the confirm prompt, got {other:?}"),
        }
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn uppercase_f_requests_the_original_content_in_the_reader() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap
        app.entries = vec![render_entry(42, "unread")];
        app.entry_cursor = 0;
        app.focus = Focus::Article;

        press(&mut app, KeyCode::Char('F'));

        let command = rx.try_recv().expect("fetch-content command");
        assert!(
            matches!(command, ApiCommand::FetchContent { id } if id == EntryID(42))
        );
    }

    #[test]
    fn fetched_content_replaces_the_reader_text() {
        let (mut app, _rx) = test_app();
        app.entries = vec![render_entry(42, "unread")];
        app.entry_cursor = 0;
        app.ensure_article();

        app.handle_api(ApiEvent::FetchedContent {
            id: EntryID(42),
            content: "<p>Fetched original body</p>".to_string(),
            reading_time: 9,
        });

        assert_eq!(app.entries[0].reading_time, 9);
        assert!(app.article_text.contains("Fetched original body"));
    }

    #[test]
    fn shift_s_requests_the_save_integration() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap
        app.entries = vec![render_entry(42, "unread")];
        app.entry_cursor = 0;

        press(&mut app, KeyCode::Char('S'));

        let command = rx.try_recv().expect("save command");
        assert!(
            matches!(command, ApiCommand::SaveEntry { id } if id == EntryID(42))
        );
    }

    #[test]
    fn shift_j_and_k_do_nothing_outside_the_reader() {
        let (mut app, _rx) = test_app();
        app.entries =
            vec![render_entry(1, "unread"), render_entry(2, "unread")];
        app.entry_cursor = 0;
        app.focus = Focus::Entries;

        press(&mut app, KeyCode::Char('J'));
        assert_eq!(app.entry_cursor, 0);
    }

    #[test]
    fn uppercase_e_opens_the_editor_for_the_selected_feed() {
        let (mut app, _rx) = test_app_with_feed();

        press(&mut app, KeyCode::Char('E'));

        match &app.mode {
            Mode::EditFeed { feed_id, form, .. } => {
                assert_eq!(*feed_id, FeedID(7));
                assert_eq!(form.title.as_str(), "Example");
                assert_eq!(
                    form.feed_url.as_str(),
                    "https://example.com/feed.xml"
                );
                assert_eq!(form.site_url.as_str(), "https://example.com");
                assert_eq!(form.category_id, CategoryID(1));
            }
            other => panic!("expected the feed editor, got {other:?}"),
        }
    }

    #[test]
    fn uppercase_e_opens_the_editor_for_the_selected_entry_feed() {
        let (mut app, _rx) = test_app_with_feed();
        app.selection = Selection::Starred;
        app.entries = vec![render_entry(42, "unread")];
        app.entry_cursor = 0;
        app.focus = Focus::Entries;

        press(&mut app, KeyCode::Char('E'));

        match &app.mode {
            Mode::EditFeed { feed_id, form, .. } => {
                assert_eq!(*feed_id, FeedID(7));
                assert_eq!(form.title.as_str(), "Example");
            }
            other => panic!("expected the feed editor, got {other:?}"),
        }
    }

    #[test]
    fn uppercase_e_with_an_entry_from_an_unknown_feed_shows_a_warning() {
        let (mut app, _rx) = test_app();
        app.selection = Selection::Unread;
        app.entries = vec![render_entry(42, "unread")];
        app.entry_cursor = 0;
        app.focus = Focus::Entries;

        press(&mut app, KeyCode::Char('E'));

        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn p_jumps_to_the_entry_feed() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        app.bootstrapped = true;
        app.selection = Selection::Starred;
        app.focus = Focus::Article;
        app.entries = vec![render_entry(42, "unread")];
        app.entry_cursor = 0;

        press(&mut app, KeyCode::Char('p'));

        assert_eq!(app.selection, Selection::Feed(FeedID(7)));
        assert_eq!(app.focus, Focus::Entries);
        let rows = app.sidebar_rows();
        assert_eq!(
            rows[app.sidebar_cursor].selection(),
            Selection::Feed(FeedID(7))
        );
        assert!(app.entries.is_empty());

        let command = rx.try_recv().expect("load-entries command");
        assert!(matches!(
            command,
            ApiCommand::LoadEntries { filter, .. }
                if filter.feed_id == Some(FeedID(7))
        ));
    }

    #[test]
    fn p_without_an_entry_shows_a_warning() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        app.selection = Selection::Unread;

        press(&mut app, KeyCode::Char('p'));

        assert_eq!(app.selection, Selection::Unread);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn p_keeps_the_entries_pane_focused_when_the_feed_is_selected() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        app.bootstrapped = true;
        app.focus = Focus::Article;
        app.entries = vec![render_entry(42, "unread")];
        app.entry_cursor = 0;

        press(&mut app, KeyCode::Char('p'));

        assert_eq!(app.focus, Focus::Entries);
        assert_eq!(app.selection, Selection::Feed(FeedID(7)));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn p_with_an_entry_from_an_unknown_feed_shows_a_warning() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap
        app.selection = Selection::Unread;
        app.entries = vec![render_entry(42, "unread")];

        press(&mut app, KeyCode::Char('p'));

        assert_eq!(app.selection, Selection::Unread);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn lowercase_e_does_not_open_the_feed_editor() {
        let (mut app, _rx) = test_app_with_feed();

        press(&mut app, KeyCode::Char('e'));

        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn uppercase_e_without_a_target_shows_a_warning() {
        let (mut app, _rx) = test_app();

        press(&mut app, KeyCode::Char('E'));

        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn feed_editor_opens_a_category_picker() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        press(&mut app, KeyCode::Char('E'));
        press(&mut app, KeyCode::Char('j')); // Category
        press(&mut app, KeyCode::Enter);

        let (picker, category) = match &app.mode {
            Mode::EditFeed {
                category_picker,
                form,
                ..
            } => (*category_picker, form.category_id),
            other => panic!("expected the feed editor, got {other:?}"),
        };
        // The picker starts on the feed's current category.
        assert_eq!(picker, Some(0));
        assert_eq!(category, CategoryID(1));

        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Enter);

        let (picker, category) = match &app.mode {
            Mode::EditFeed {
                category_picker,
                form,
                ..
            } => (*category_picker, form.category_id),
            other => panic!("expected the feed editor, got {other:?}"),
        };
        assert_eq!(picker, None);
        assert_eq!(category, CategoryID(2));
    }

    #[test]
    fn feed_editor_category_picker_cancels() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        press(&mut app, KeyCode::Char('E'));
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Esc);

        let (picker, category) = match &app.mode {
            Mode::EditFeed {
                category_picker,
                form,
                ..
            } => (*category_picker, form.category_id),
            other => panic!("expected the feed editor, got {other:?}"),
        };
        assert_eq!(picker, None);
        assert_eq!(category, CategoryID(1));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn feed_editor_category_ignores_h_and_l() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        press(&mut app, KeyCode::Char('E'));
        press(&mut app, KeyCode::Char('j'));

        press(&mut app, KeyCode::Char('l'));

        let (picker, category) = match &app.mode {
            Mode::EditFeed {
                category_picker,
                form,
                ..
            } => (*category_picker, form.category_id),
            other => panic!("expected the feed editor, got {other:?}"),
        };
        assert_eq!(picker, None);
        assert_eq!(category, CategoryID(1));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn feed_editor_toggles_booleans_with_enter_and_h_l() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        press(&mut app, KeyCode::Char('E'));

        press(&mut app, KeyCode::Char('G'));
        press(&mut app, KeyCode::Char('l'));
        let form = match &app.mode {
            Mode::EditFeed { form, cursor, .. } => {
                assert_eq!(*cursor, FeedField::ALL.len() - 1);
                form
            }
            other => panic!("expected the feed editor, got {other:?}"),
        };
        assert!(form.fetch_via_proxy);

        press(&mut app, KeyCode::Char('h'));
        let form = match &app.mode {
            Mode::EditFeed { form, .. } => form,
            other => panic!("expected the feed editor, got {other:?}"),
        };
        assert!(!form.fetch_via_proxy);

        press(&mut app, KeyCode::Enter);
        let form = match &app.mode {
            Mode::EditFeed { form, .. } => form,
            other => panic!("expected the feed editor, got {other:?}"),
        };
        assert!(form.fetch_via_proxy);
    }

    #[test]
    fn feed_editor_edits_text_and_saves_every_option() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        press(&mut app, KeyCode::Char('E'));

        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::EditFeed { editing: true, .. }));
        press_with(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        for ch in "Fresh title".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Enter);
        press_with(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);

        let command = rx.try_recv().expect("update-feed command");
        let ApiCommand::UpdateFeed { id, changes } = command else {
            panic!("expected an update-feed command");
        };
        assert_eq!(id, FeedID(7));
        assert_eq!(changes.title, "Fresh title");
        assert_eq!(changes.category_id, CategoryID(1));
        assert_eq!(changes.feed_url, "https://example.com/feed.xml");
        assert_eq!(changes.site_url, "https://example.com");
        assert!(!changes.crawler);
        assert!(!changes.disabled);
        assert!(!changes.ignore_http_cache);
        assert!(!changes.fetch_via_proxy);
    }

    #[test]
    fn feed_editor_unsubscribe_requires_confirmation() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        press(&mut app, KeyCode::Char('E'));

        press(&mut app, KeyCode::Char('d'));
        assert!(matches!(
            app.mode,
            Mode::EditFeed {
                confirm_delete: true,
                ..
            }
        ));

        press(&mut app, KeyCode::Char('n'));
        assert!(matches!(
            app.mode,
            Mode::EditFeed {
                confirm_delete: false,
                ..
            }
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn feed_editor_unsubscribe_sends_delete() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        press(&mut app, KeyCode::Char('E'));

        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('y'));

        assert!(matches!(app.mode, Mode::Normal));
        let command = rx.try_recv().expect("delete-feed command");
        assert!(matches!(
            command,
            ApiCommand::DeleteFeed { id } if id == FeedID(7)
        ));
    }

    #[test]
    fn feed_deleted_removes_the_feed_and_refreshes_the_view() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        app.bootstrapped = true;
        app.counters.unreads.insert(FeedID(7), 2);
        app.selection = Selection::Feed(FeedID(7));
        press(&mut app, KeyCode::Char('E'));

        app.handle_api(ApiEvent::FeedDeleted { feed_id: FeedID(7) });

        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.feeds.is_empty());
        assert!(!app.counters.unreads.contains_key(&FeedID(7)));
        assert_eq!(app.selection, Selection::Unread);
        let command = rx.try_recv().expect("load-entries command");
        assert!(matches!(command, ApiCommand::LoadEntries { .. }));
    }

    #[test]
    fn feed_deleted_keeps_another_feeds_editor_open() {
        let (mut app, _rx) = test_app_with_feed();
        app.feeds.push(render_feed(9, "Other", 1));
        app.selection = Selection::Feed(FeedID(7));
        press(&mut app, KeyCode::Char('E'));

        app.handle_api(ApiEvent::FeedDeleted { feed_id: FeedID(9) });

        assert!(matches!(
            app.mode,
            Mode::EditFeed { feed_id, .. } if feed_id == FeedID(7)
        ));
        assert!(app.feeds.iter().any(|feed| feed.id == FeedID(7)));
    }

    #[test]
    fn feed_editor_escape_cancels_without_saving() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        press(&mut app, KeyCode::Char('E'));

        press(&mut app, KeyCode::Esc);

        assert!(matches!(app.mode, Mode::Normal));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn feed_updated_replaces_the_feed_and_closes_the_editor() {
        let (mut app, _rx) = test_app_with_feed();
        press(&mut app, KeyCode::Char('E'));

        app.handle_api(ApiEvent::FeedUpdated(Box::new(render_feed(
            7, "Renamed", 1,
        ))));

        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.feeds[0].title, "Renamed");
    }

    #[test]
    fn feed_update_error_keeps_the_editor_open() {
        let (mut app, _rx) = test_app_with_feed();
        press(&mut app, KeyCode::Char('E'));

        app.handle_api(ApiEvent::Error {
            context: "Could not update the feed".to_string(),
            message: "HTTP 400".to_string(),
        });

        assert!(matches!(app.mode, Mode::EditFeed { .. }));
    }

    #[test]
    fn escape_clears_an_active_search() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap
        app.bootstrapped = true;
        app.search = "rust".to_string();
        app.focus = Focus::Entries;
        app.entries = vec![render_entry(1, "unread")];

        press(&mut app, KeyCode::Esc);

        assert!(app.search.is_empty());
        assert_eq!(app.focus, Focus::Entries);
        assert!(app.entries.is_empty());
        let command = rx.try_recv().expect("load-entries command");
        assert!(matches!(
            command,
            ApiCommand::LoadEntries { filter, .. } if filter.search.is_none()
        ));
    }

    #[test]
    fn escape_goes_back_a_pane_without_a_search() {
        let (mut app, _rx) = test_app();
        app.focus = Focus::Article;

        press(&mut app, KeyCode::Esc);

        assert_eq!(app.focus, Focus::Entries);
    }

    #[test]
    fn slash_in_feeds_opens_the_feed_filter() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        app.focus = Focus::Feeds;

        press(&mut app, KeyCode::Char('/'));
        assert!(matches!(
            app.mode,
            Mode::Search {
                scope: SearchScope::Feeds,
                ..
            }
        ));
        for ch in "exam".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Enter);

        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.feed_filter, "exam");
        assert!(app.search.is_empty());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn slash_outside_feeds_searches_entries() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        app.bootstrapped = true;
        app.focus = Focus::Entries;

        press(&mut app, KeyCode::Char('/'));
        assert!(matches!(
            app.mode,
            Mode::Search {
                scope: SearchScope::Entries,
                ..
            }
        ));
        for ch in "rust".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Enter);

        assert_eq!(app.search, "rust");
        assert!(app.feed_filter.is_empty());
        assert_eq!(app.focus, Focus::Entries);
        assert!(matches!(
            rx.try_recv().expect("load-entries command"),
            ApiCommand::LoadEntries { filter, .. }
                if filter.search.as_deref() == Some("rust")
        ));
    }

    #[test]
    fn feed_filter_narrows_the_sidebar() {
        let (mut app, _rx) = test_app_with_feed();
        app.feeds.push(render_feed(9, "Cooking", 2));
        app.feed_filter = "exam".to_string();

        let rows = app.sidebar_rows();

        assert!(rows.iter().any(
            |row| matches!(row, SidebarRow::Feed { id, .. } if *id == FeedID(7))
        ));
        assert!(!rows.iter().any(
            |row| matches!(row, SidebarRow::Feed { id, .. } if *id == FeedID(9))
        ));
        assert!(rows.iter().any(|row| matches!(
            row,
            SidebarRow::Category { id, .. } if *id == CategoryID(1)
        )));
        assert!(!rows.iter().any(|row| matches!(
            row,
            SidebarRow::Category { id, .. } if *id == CategoryID(2)
        )));
        assert!(matches!(rows.first(), Some(SidebarRow::Unread)));
    }

    #[test]
    fn feed_filter_matches_category_titles() {
        let (mut app, _rx) = test_app_with_feed();
        app.feeds.push(render_feed(9, "Cooking", 2));
        app.feed_filter = "news".to_string();

        let rows = app.sidebar_rows();

        // The category title matches, so its feed is shown even though the
        // feed title does not contain the filter.
        assert!(rows.iter().any(
            |row| matches!(row, SidebarRow::Feed { id, .. } if *id == FeedID(7))
        ));
        assert!(!rows.iter().any(
            |row| matches!(row, SidebarRow::Feed { id, .. } if *id == FeedID(9))
        ));
    }

    #[test]
    fn escape_in_feeds_clears_the_feed_filter() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        app.focus = Focus::Feeds;
        app.feed_filter = "exam".to_string();

        press(&mut app, KeyCode::Esc);

        assert!(app.feed_filter.is_empty());
        assert_eq!(app.focus, Focus::Feeds);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn escape_in_entries_clears_the_search_not_the_feed_filter() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        app.bootstrapped = true;
        app.focus = Focus::Entries;
        app.search = "rust".to_string();
        app.feed_filter = "exam".to_string();

        press(&mut app, KeyCode::Esc);

        assert!(app.search.is_empty());
        assert_eq!(app.feed_filter, "exam");
        assert_eq!(app.focus, Focus::Entries);
        assert!(matches!(
            rx.try_recv().expect("load-entries command"),
            ApiCommand::LoadEntries { filter, .. } if filter.search.is_none()
        ));
    }

    #[test]
    fn a_filtered_out_feed_disappears_from_the_sidebar() {
        let (mut app, _rx) = test_app_with_feed();
        app.feeds.push(render_feed(9, "Cooking", 2));
        app.selection = Selection::Feed(FeedID(9));
        app.feed_filter = "exam".to_string();

        let rows = app.sidebar_rows();

        assert!(rows.iter().any(
            |row| matches!(row, SidebarRow::Feed { id, .. } if *id == FeedID(7))
        ));
        assert!(!rows.iter().any(
            |row| matches!(row, SidebarRow::Feed { id, .. } if *id == FeedID(9))
        ));
    }

    #[test]
    fn feed_filter_applies_while_typing() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        app.bootstrapped = true;
        app.feeds.push(render_feed(9, "Cooking", 2));
        app.selection = Selection::Unread;
        app.focus = Focus::Feeds;
        press(&mut app, KeyCode::Char('/'));

        press(&mut app, KeyCode::Char('e'));
        assert_eq!(app.feed_filter, "e");
        assert_eq!(app.selection, Selection::Feed(FeedID(7)));
        assert!(matches!(
            rx.try_recv().expect("load-entries command"),
            ApiCommand::LoadEntries { filter, .. }
                if filter.feed_id == Some(FeedID(7))
        ));

        press(&mut app, KeyCode::Char('x'));
        assert_eq!(app.feed_filter, "ex");
        assert_eq!(app.selection, Selection::Feed(FeedID(7)));
        assert!(rx.try_recv().is_err());
        assert!(matches!(app.mode, Mode::Search { .. }));
        let rows = app.sidebar_rows();
        assert!(rows.iter().any(
            |row| matches!(row, SidebarRow::Feed { id, .. } if *id == FeedID(7))
        ));
        assert!(!rows.iter().any(
            |row| matches!(row, SidebarRow::Feed { id, .. } if *id == FeedID(9))
        ));

        press_with(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert!(app.feed_filter.is_empty());
        // An empty filter no longer removes anything, so the view stays put.
        assert_eq!(app.selection, Selection::Feed(FeedID(7)));
    }

    #[test]
    fn typing_the_filter_selects_the_first_matching_feed() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        app.bootstrapped = true;
        app.feeds.push(render_feed(9, "Cooking", 2));
        app.selection = Selection::Unread;
        app.focus = Focus::Feeds;
        press(&mut app, KeyCode::Char('/'));

        for ch in "coo".chars() {
            press(&mut app, KeyCode::Char(ch));
        }

        assert_eq!(app.selection, Selection::Feed(FeedID(9)));
        assert!(matches!(
            rx.try_recv().expect("load-entries command"),
            ApiCommand::LoadEntries { filter, .. }
                if filter.feed_id == Some(FeedID(9))
        ));
    }

    #[test]
    fn a_filter_with_no_matches_selects_unread() {
        let (mut app, _rx) = test_app_with_feed();
        app.selection = Selection::Feed(FeedID(7));
        app.focus = Focus::Feeds;
        press(&mut app, KeyCode::Char('/'));

        for ch in "zzz".chars() {
            press(&mut app, KeyCode::Char(ch));
        }

        assert_eq!(app.selection, Selection::Unread);
        let rows = app.sidebar_rows();
        assert!(
            !rows
                .iter()
                .any(|row| matches!(row, SidebarRow::Feed { .. }))
        );
    }

    #[test]
    fn feed_filter_escape_restores_the_previous_filter() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        app.feed_filter = "exam".to_string();
        app.selection = Selection::Feed(FeedID(7));
        app.focus = Focus::Feeds;

        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('z'));
        assert_eq!(app.feed_filter, "examz");
        assert_eq!(app.selection, Selection::Unread);

        press(&mut app, KeyCode::Esc);

        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.feed_filter, "exam");
        assert_eq!(app.selection, Selection::Feed(FeedID(7)));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn feed_filter_enter_keeps_the_typed_filter() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        app.focus = Focus::Feeds;

        press(&mut app, KeyCode::Char('/'));
        for ch in "exam".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Enter);

        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.feed_filter, "exam");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn i_toggles_images_and_requests_them() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap
        app.article_parts = vec![util::ArticlePart::Image {
            url: "https://example.com/a.png".to_string(),
            alt: "A".to_string(),
        }];

        press(&mut app, KeyCode::Char('i'));

        assert!(app.show_images);
        assert!(matches!(
            app.images.get("https://example.com/a.png"),
            Some(LoadedImage::Loading)
        ));
        let command = rx.try_recv().expect("load-image command");
        assert!(matches!(
            command,
            ApiCommand::LoadImage { url }
                if url == "https://example.com/a.png"
        ));

        press(&mut app, KeyCode::Char('i'));
        assert!(!app.show_images);
    }

    #[test]
    fn images_start_from_the_config() {
        let mut config = test_config();
        config.images = true;
        let (app, _rx) = test_app_from(config);

        assert!(app.show_images);
    }

    #[test]
    fn an_loaded_image_is_kept() {
        let (mut app, _rx) = test_app();
        let url = "https://example.com/a.png".to_string();
        app.images.insert(url.clone(), LoadedImage::Loading);

        app.handle_api(ApiEvent::ImageLoaded {
            url: url.clone(),
            result: Ok(api::DecodedImage {
                width: 1,
                height: 1,
                rgba: vec![255, 0, 0, 255],
            }),
        });

        assert!(matches!(
            app.images.get(&url),
            Some(LoadedImage::Ready(image)) if image.width == 1
        ));
    }

    #[test]
    fn preparing_an_article_collects_its_images() {
        let (mut app, mut rx) = test_app();
        let _ = rx.try_recv(); // initial Bootstrap
        app.show_images = true;
        let mut entry = render_entry(42, "unread");
        entry.content = "<p>Before</p>\
             <img src=\"https://example.com/pic.png\" alt=\"Pic\">\
             <p>After</p>"
            .to_string();
        app.entries = vec![entry];
        app.entry_cursor = 0;

        app.ensure_article();

        assert!(app.article_text.contains("Before"));
        assert!(app.article_parts.iter().any(|part| matches!(
            part,
            util::ArticlePart::Image { url, alt }
                if url == "https://example.com/pic.png" && alt == "Pic"
        )));
        let command = rx.try_recv().expect("load-image command");
        assert!(matches!(
            command,
            ApiCommand::LoadImage { url }
                if url == "https://example.com/pic.png"
        ));
    }

    #[test]
    fn space_scrolls_to_the_next_paragraph() {
        let (mut app, _rx) = test_app();
        app.focus = Focus::Article;
        app.article_max_scroll = 100;
        app.article_paragraph_stops = vec![10, 30, 60];
        app.article_scroll = 0;

        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.article_scroll, 10);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.article_scroll, 30);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.article_scroll, 60);
        // Past the last paragraph, settle at the bottom.
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.article_scroll, 100);
    }

    #[test]
    fn space_does_nothing_outside_the_reader() {
        let (mut app, _rx) = test_app();
        app.focus = Focus::Entries;
        app.article_max_scroll = 100;
        app.article_paragraph_stops = vec![10];
        app.article_scroll = 0;

        press(&mut app, KeyCode::Char(' '));

        assert_eq!(app.article_scroll, 0);
    }

    #[test]
    fn text_input_moves_the_caret() {
        let mut input = TextInput::new("hello world");

        input.move_left();
        input.insert('!');
        assert_eq!(input.as_str(), "hello worl!d");

        input.move_home();
        input.insert('>');
        assert_eq!(input.as_str(), ">hello worl!d");

        input.move_end();
        input.insert('<');
        assert_eq!(input.as_str(), ">hello worl!d<");
    }

    #[test]
    fn text_input_moves_and_deletes_by_word() {
        let mut input = TextInput::new("one two three");
        input.move_word_left();
        assert_eq!(input.cursor(), 8);
        input.move_word_left();
        assert_eq!(input.cursor(), 4);
        input.move_word_right();
        assert_eq!(input.cursor(), 7);

        input.delete_word_before();
        assert_eq!(input.as_str(), "one  three");
        assert_eq!(input.cursor(), 4);

        input.delete_word_after();
        assert_eq!(input.as_str(), "one ");
    }

    #[test]
    fn text_input_deletes_around_the_caret() {
        let mut input = TextInput::new("hello world");
        input.move_home();
        input.delete();
        assert_eq!(input.as_str(), "ello world");

        input.move_end();
        input.backspace();
        assert_eq!(input.as_str(), "ello worl");

        input.kill_to_end();
        assert_eq!(input.as_str(), "ello worl");

        input.clear();
        assert_eq!(input.as_str(), "");
        assert_eq!(input.cursor(), 0);
    }

    #[test]
    fn feed_editor_edits_at_the_caret() {
        let (mut app, _rx) = test_app_with_feed();
        press(&mut app, KeyCode::Char('E'));
        press(&mut app, KeyCode::Enter);

        press_with(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
        for ch in "X ".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Char('!'));
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Char('?'));

        let form = match &app.mode {
            Mode::EditFeed { form, .. } => form,
            other => panic!("expected the feed editor, got {other:?}"),
        };
        assert_eq!(form.title.as_str(), "X Example?!");
    }

    #[test]
    fn search_input_edits_at_the_caret() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        app.bootstrapped = true;
        app.focus = Focus::Entries;

        press(&mut app, KeyCode::Char('/'));
        for ch in "rust".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press_with(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
        press(&mut app, KeyCode::Char('c'));
        press(&mut app, KeyCode::Delete);
        press(&mut app, KeyCode::Enter);

        assert_eq!(app.search, "cust");
    }

    #[test]
    fn the_all_view_is_called_everything() {
        let (mut app, _rx) = test_app();
        app.selection = Selection::All;

        assert_eq!(app.selection_label(), "Everything");
    }

    #[test]
    fn uppercase_e_renames_the_selected_category() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        app.selection = Selection::Category(CategoryID(1));
        app.focus = Focus::Feeds;
        // The category selection wins over the loaded entry.
        app.entries = vec![render_entry(42, "unread")];
        app.entry_cursor = 0;

        press(&mut app, KeyCode::Char('E'));

        match &app.mode {
            Mode::RenameCategory { category_id, input } => {
                assert_eq!(*category_id, CategoryID(1));
                assert_eq!(input.as_str(), "News");
            }
            other => panic!("expected the rename dialog, got {other:?}"),
        }

        press_with(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        for ch in "Fresh".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Enter);

        assert!(matches!(app.mode, Mode::RenameCategory { .. }));
        let command = rx.try_recv().expect("update-category command");
        assert!(matches!(
            command,
            ApiCommand::UpdateCategory { id, title }
                if id == CategoryID(1) && title == "Fresh"
        ));

        app.handle_api(ApiEvent::CategoryUpdated(Box::new(render_category(
            1, "Fresh",
        ))));

        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.categories[0].title, "Fresh");
        assert_eq!(app.selection, Selection::Category(CategoryID(1)));
    }

    #[test]
    fn category_rename_rejects_an_empty_name() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        app.selection = Selection::Category(CategoryID(1));
        press(&mut app, KeyCode::Char('E'));

        press_with(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        press(&mut app, KeyCode::Enter);

        assert!(matches!(app.mode, Mode::RenameCategory { .. }));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn escape_closes_the_category_rename_dialog() {
        let (mut app, mut rx) = test_app_with_feed();
        let _ = rx.try_recv(); // initial Bootstrap
        app.selection = Selection::Category(CategoryID(1));
        press(&mut app, KeyCode::Char('E'));

        press(&mut app, KeyCode::Esc);

        assert!(matches!(app.mode, Mode::Normal));
        assert!(rx.try_recv().is_err());
    }
}
