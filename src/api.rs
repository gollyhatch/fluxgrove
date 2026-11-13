use std::time::Duration;

use anyhow::{Context, Result};
use miniflux_api::{
    MinifluxApi,
    models::{
        Category, CategoryID, Entry, EntryFilter, EntryID, EntryStatus, Feed,
        FeedCounters, FeedID, OrderBy, OrderDirection, Subscription, UserID,
    },
};
use reqwest::Client;
use tokio::sync::mpsc;
use url::Url;

use crate::config::{Auth, Config};

#[derive(Debug)]
pub struct BootstrapData {
    pub feeds: Vec<Feed>,
    pub categories: Vec<Category>,
    pub counters: FeedCounters,
    pub user_id: UserID,
}

#[derive(Debug, Clone)]
pub enum MarkAllScope {
    User(UserID),
    Category(CategoryID),
    Feed(FeedID),
    Entries(Vec<EntryID>),
}

#[derive(Debug)]
pub enum ApiCommand {
    Bootstrap,
    LoadEntries {
        filter: Box<EntryFilter>,
        generation: u64,
    },
    SetRead {
        id: EntryID,
        read: bool,
    },
    SetStarred {
        id: EntryID,
        starred: bool,
    },
    SaveEntry {
        id: EntryID,
    },
    FetchContent {
        id: EntryID,
    },
    DiscoverFeeds {
        url: Url,
        category_id: CategoryID,
    },
    CreateFeed {
        url: Url,
        category_id: CategoryID,
    },
    MarkAllRead {
        scope: MarkAllScope,
    },
    Refresh,
}

#[derive(Debug)]
pub enum ApiEvent {
    Bootstrap(Box<BootstrapData>),
    Entries {
        generation: u64,
        entries: Vec<Entry>,
        total: i64,
    },
    ReadUpdated {
        id: EntryID,
        read: bool,
    },
    StarredUpdated {
        id: EntryID,
        starred: bool,
    },
    Saved,
    FetchedContent {
        id: EntryID,
        content: String,
        reading_time: i64,
    },
    FeedsDiscovered {
        subscriptions: Vec<Subscription>,
        category_id: CategoryID,
        source_url: Url,
    },
    FeedCreated {
        feed_id: FeedID,
    },
    MarkedAllRead,
    Error {
        context: String,
        message: String,
    },
    Notice(String),
}

pub struct ApiClient {
    api: MinifluxApi,
    http: Client,
}

impl ApiClient {
    pub fn new(config: &Config) -> Result<Self> {
        let api = match &config.auth {
            Auth::Token(token) => {
                MinifluxApi::new_from_token(&config.url, token.clone())
                    .context("could not use the Miniflux API token")?
            }
            Auth::Password { username, password } => MinifluxApi::new(
                &config.url,
                username.clone(),
                password.clone(),
            ),
        };

        let http = Client::builder()
            .user_agent(concat!("fluxgrove/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(45))
            .build()
            .context("could not create the HTTP client")?;

        Ok(Self { api, http })
    }

    async fn bootstrap(&self) -> Result<BootstrapData> {
        let (feeds, categories, counters, user) = tokio::try_join!(
            self.api.get_feeds(&self.http),
            self.api.get_categories_with_counts(&self.http),
            self.api.get_feed_counters(&self.http),
            self.api.get_current_user(&self.http),
        )?;
        Ok(BootstrapData {
            feeds,
            categories,
            counters,
            user_id: user.id,
        })
    }

    async fn run(
        &self,
        command: ApiCommand,
        events: &mpsc::UnboundedSender<ApiEvent>,
    ) {
        match command {
            ApiCommand::Bootstrap => match self.bootstrap().await {
                Ok(data) => send(events, ApiEvent::Bootstrap(Box::new(data))),
                Err(error) => {
                    send_error(events, "Could not load Miniflux data", error)
                }
            },
            ApiCommand::LoadEntries { filter, generation } => {
                match self.api.get_entry_batch(&filter, &self.http).await {
                    Ok(batch) => send(
                        events,
                        ApiEvent::Entries {
                            generation,
                            entries: batch.entries,
                            total: batch.total,
                        },
                    ),
                    Err(error) => {
                        send_error(events, "Could not load entries", error)
                    }
                }
            }
            ApiCommand::SetRead { id, read } => {
                let status = if read {
                    EntryStatus::Read
                } else {
                    EntryStatus::Unread
                };
                match self
                    .api
                    .update_entries_status(vec![id], status, &self.http)
                    .await
                {
                    Ok(()) => send(events, ApiEvent::ReadUpdated { id, read }),
                    Err(error) => {
                        send_error(events, "Could not update the entry", error);
                    }
                }
            }
            ApiCommand::SetStarred { id, starred } => {
                // The bookmark toggle endpoint works on older Miniflux
                // releases, unlike the bulk starred update.
                match self.api.toggle_bookmark(id, &self.http).await {
                    Ok(()) => {
                        send(events, ApiEvent::StarredUpdated { id, starred })
                    }
                    Err(error) => {
                        send_error(
                            events,
                            "Could not update the bookmark",
                            error,
                        );
                    }
                }
            }
            ApiCommand::SaveEntry { id } => {
                match self.api.save_entry(id, &self.http).await {
                    Ok(()) => send(events, ApiEvent::Saved),
                    Err(error) => {
                        send_error(events, "Could not save the entry", error);
                    }
                }
            }
            ApiCommand::DiscoverFeeds { url, category_id } => {
                match self
                    .api
                    .discover_subscription(url.clone(), &self.http)
                    .await
                {
                    Ok(subscriptions) => send(
                        events,
                        ApiEvent::FeedsDiscovered {
                            subscriptions,
                            category_id,
                            source_url: url,
                        },
                    ),
                    Err(error) => {
                        send_error(events, "Could not discover feeds", error);
                    }
                }
            }
            ApiCommand::CreateFeed { url, category_id } => {
                match self.api.create_feed(&url, category_id, &self.http).await
                {
                    Ok(feed_id) => {
                        send(events, ApiEvent::FeedCreated { feed_id })
                    }
                    Err(error) => {
                        send_error(
                            events,
                            "Could not subscribe to the feed",
                            error,
                        );
                    }
                }
            }
            ApiCommand::FetchContent { id } => {
                match self.api.fetch_entry_content(id, false, &self.http).await
                {
                    Ok(fetched) => send(
                        events,
                        ApiEvent::FetchedContent {
                            id,
                            content: fetched.content,
                            reading_time: fetched.reading_time,
                        },
                    ),
                    Err(error) => {
                        send_error(
                            events,
                            "Could not fetch the original content",
                            error,
                        );
                    }
                }
            }
            ApiCommand::MarkAllRead { scope } => {
                let result = match scope {
                    MarkAllScope::User(id) => {
                        self.api.mark_user_as_read(id, &self.http).await
                    }
                    MarkAllScope::Category(id) => {
                        self.api.mark_category_as_read(id, &self.http).await
                    }
                    MarkAllScope::Feed(id) => {
                        self.api.mark_feed_as_read(id, &self.http).await
                    }
                    MarkAllScope::Entries(ids) => {
                        self.api
                            .update_entries_status(
                                ids,
                                EntryStatus::Read,
                                &self.http,
                            )
                            .await
                    }
                };
                match result {
                    Ok(()) => send(events, ApiEvent::MarkedAllRead),
                    Err(error) => send_error(
                        events,
                        "Could not mark entries as read",
                        error,
                    ),
                }
            }
            ApiCommand::Refresh => {
                if let Err(error) = self.api.refresh_all_feeds(&self.http).await
                {
                    send_error(
                        events,
                        "Could not ask Miniflux to refresh feeds",
                        error,
                    );
                } else {
                    send(
                        events,
                        ApiEvent::Notice(
                            "Feeds are refreshing in the background…".into(),
                        ),
                    );
                }
                tokio::time::sleep(Duration::from_millis(900)).await;
                match self.bootstrap().await {
                    Ok(data) => {
                        send(events, ApiEvent::Bootstrap(Box::new(data)))
                    }
                    Err(error) => send_error(
                        events,
                        "Could not reload Miniflux data",
                        error,
                    ),
                }
            }
        }
    }
}

/// Spawn the API worker and return the command sender / event receiver pair.
pub fn spawn(
    client: ApiClient,
) -> (
    mpsc::UnboundedSender<ApiCommand>,
    mpsc::UnboundedReceiver<ApiEvent>,
) {
    let (command_tx, mut command_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        while let Some(command) = command_rx.recv().await {
            client.run(command, &event_tx).await;
        }
    });

    (command_tx, event_rx)
}

/// Build the API filter for a sidebar selection and search query.
pub fn entry_filter(
    selection: &crate::app::Selection,
    search: &str,
    limit: i64,
    hide_read: bool,
) -> EntryFilter {
    let mut filter = EntryFilter::default()
        .limit(limit)
        .order(OrderBy::PublishedAt)
        .direction(OrderDirection::Desc);

    filter = match selection {
        crate::app::Selection::All => filter,
        crate::app::Selection::Unread => filter.status(EntryStatus::Unread),
        crate::app::Selection::Starred => filter.starred(true),
        crate::app::Selection::Category(id) => filter.category_id(*id),
        crate::app::Selection::Feed(id) => filter.feed_id(*id),
    };

    if hide_read {
        filter = filter.status(EntryStatus::Unread);
    }

    if !search.is_empty() {
        filter = filter.search(search.to_string());
    }

    filter
}

fn send(events: &mpsc::UnboundedSender<ApiEvent>, event: ApiEvent) {
    let _ = events.send(event);
}

fn send_error(
    events: &mpsc::UnboundedSender<ApiEvent>,
    context: &str,
    error: impl std::fmt::Display,
) {
    send(
        events,
        ApiEvent::Error {
            context: context.to_string(),
            message: error.to_string(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Selection;

    #[test]
    fn unread_filter_uses_status_and_sorting() {
        let filter = entry_filter(&Selection::Unread, "", 50, false);
        assert_eq!(filter.status, Some(EntryStatus::Unread));
        assert_eq!(filter.limit, Some(50));
        assert_eq!(filter.order, Some(OrderBy::PublishedAt));
        assert_eq!(filter.direction, Some(OrderDirection::Desc));
        assert!(filter.feed_id.is_none());
        assert!(filter.search.is_none());
    }

    #[test]
    fn feed_filter_and_search_are_combined() {
        let filter =
            entry_filter(&Selection::Feed(FeedID(42)), "rust", 100, false);
        assert_eq!(filter.feed_id, Some(FeedID(42)));
        assert_eq!(filter.search.as_deref(), Some("rust"));
        assert_eq!(filter.limit, Some(100));
    }

    #[test]
    fn hide_read_forces_unread_status() {
        let filter = entry_filter(&Selection::All, "", 25, true);
        assert_eq!(filter.status, Some(EntryStatus::Unread));

        let filter = entry_filter(&Selection::Starred, "", 25, true);
        assert_eq!(filter.status, Some(EntryStatus::Unread));
        assert_eq!(filter.starred, Some(true));
    }

    #[test]
    fn starred_filter_has_no_status() {
        let filter = entry_filter(&Selection::Starred, "", 10, false);
        assert_eq!(filter.starred, Some(true));
        assert!(filter.status.is_none());
    }
}
