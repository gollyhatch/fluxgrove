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
use serde::Serialize;
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

/// Every field `PUT /v1/feeds/{id}` accepts.
///
/// The `miniflux_api` crate models only a subset, so the request body is built
/// here to cover the block/keep lists, the HTTP cache and proxy flags too.
#[derive(Debug, Clone, Serialize)]
pub struct FeedUpdate {
    pub title: String,
    pub category_id: CategoryID,
    pub feed_url: String,
    pub site_url: String,
    pub user_agent: String,
    pub username: String,
    pub password: String,
    pub scraper_rules: String,
    pub rewrite_rules: String,
    pub blocklist_rules: String,
    pub keeplist_rules: String,
    pub crawler: bool,
    pub disabled: bool,
    pub ignore_http_cache: bool,
    pub fetch_via_proxy: bool,
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
    ToggleStarred {
        id: EntryID,
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
    UpdateFeed {
        id: FeedID,
        changes: Box<FeedUpdate>,
    },
    DeleteFeed {
        id: FeedID,
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
    FeedUpdated(Box<Feed>),
    FeedDeleted {
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
    base_url: Url,
    auth: Auth,
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

        Ok(Self {
            api,
            http,
            base_url: config.url.clone(),
            auth: config.auth.clone(),
        })
    }

    /// Update a feed with the full set of options Miniflux accepts.
    ///
    /// `MinifluxApi::update_feed` cannot express every field, so this sends the
    /// documented payload with the same authentication the crate uses.
    async fn update_feed(
        &self,
        id: FeedID,
        changes: &FeedUpdate,
    ) -> Result<Feed> {
        let url = self.base_url.join(&format!("v1/feeds/{id}"))?;
        let request = self.http.put(url).json(changes);
        let request = match &self.auth {
            Auth::Token(token) => request.header("X-Auth-Token", token),
            Auth::Password { username, password } => {
                request.basic_auth(username, Some(password))
            }
        };
        let response =
            request.send().await.context("could not reach Miniflux")?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Miniflux error (HTTP {status}): {}", body.trim());
        }
        response
            .json::<Feed>()
            .await
            .context("could not read the updated feed")
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
            ApiCommand::ToggleStarred { id } => {
                // The bookmark toggle endpoint works on older Miniflux
                // releases, unlike the bulk starred update. Read the entry
                // back afterwards so the UI shows the server's actual state:
                // a toggle applied to a stale local copy inverts it for good.
                match self.api.toggle_bookmark(id, &self.http).await {
                    Ok(()) => match self.api.get_entry(id, &self.http).await {
                        Ok(entry) => send(
                            events,
                            ApiEvent::StarredUpdated {
                                id,
                                starred: entry.starred,
                            },
                        ),
                        Err(error) => send_error(
                            events,
                            "Could not confirm the bookmark",
                            error,
                        ),
                    },
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
            ApiCommand::UpdateFeed { id, changes } => {
                match self.update_feed(id, &changes).await {
                    Ok(feed) => {
                        send(events, ApiEvent::FeedUpdated(Box::new(feed)))
                    }
                    Err(error) => {
                        send_error(events, "Could not update the feed", error);
                    }
                }
            }
            ApiCommand::DeleteFeed { id } => {
                match self.api.delete_feed(id, &self.http).await {
                    Ok(()) => {
                        send(events, ApiEvent::FeedDeleted { feed_id: id })
                    }
                    Err(error) => {
                        send_error(
                            events,
                            "Could not unsubscribe from the feed",
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
    use crate::{app::Selection, config::Auth};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn test_config(url: &str) -> Config {
        Config {
            url: Url::parse(url).expect("valid Miniflux URL"),
            auth: Auth::Token("test-token".to_string()),
            theme: "catppuccin-mocha".to_string(),
            limit: 100,
            mark_read: true,
            hide_read_feeds: false,
            hide_read_entries: false,
        }
    }

    /// Serve the two requests a bookmark toggle makes: the PUT toggle, then the
    /// GET that reports the entry's actual state. Each response closes the
    /// connection so reqwest cannot pipeline them over one socket.
    async fn serve_bookmark_flow(
        listener: TcpListener,
        entry: serde_json::Value,
    ) -> Vec<String> {
        let mut requests = Vec::new();
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buffer = vec![0u8; 8192];
            let read = socket.read(&mut buffer).await.expect("read request");
            let request = String::from_utf8_lossy(&buffer[..read]);
            let request_line =
                request.lines().next().unwrap_or_default().to_string();
            let response =
                if request_line.starts_with("PUT /v1/entries/42/bookmark ") {
                    "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n"
                        .to_string()
                } else if request_line.starts_with("GET /v1/entries/42 ") {
                    let body = entry.to_string();
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                } else {
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\
                 Connection: close\r\n\r\n"
                        .to_string()
                };
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write response");
            requests.push(request_line);
        }
        requests
    }

    fn entry_json(starred: bool) -> serde_json::Value {
        serde_json::json!({
            "id": 42,
            "user_id": 1,
            "feed_id": 7,
            "status": "unread",
            "hash": "hash-42",
            "title": "Entry",
            "url": "https://example.com/42",
            "author": "Author",
            "content": "<p>Body</p>",
            "published_at": "2026-01-01T00:00:00Z",
            "created_at": "2026-01-01T00:00:00Z",
            "share_code": "",
            "starred": starred,
            "reading_time": 1,
            "enclosures": [],
            "tags": [],
            "feed": {
                "id": 7,
                "user_id": 1,
                "title": "Feed",
                "site_url": "https://example.com",
                "feed_url": "https://example.com/feed.xml",
                "category": {"id": 3, "user_id": 1, "title": "Category"}
            }
        })
    }

    #[tokio::test]
    async fn toggling_a_bookmark_reports_the_server_state() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("server address");
        let server =
            tokio::spawn(serve_bookmark_flow(listener, entry_json(true)));

        let client =
            ApiClient::new(&test_config(&format!("http://{address}/")))
                .expect("api client");
        let (events, mut received) = mpsc::unbounded_channel();
        client
            .run(ApiCommand::ToggleStarred { id: EntryID(42) }, &events)
            .await;

        let event = received.recv().await.expect("an event");
        assert!(
            matches!(
                event,
                ApiEvent::StarredUpdated { id, starred: true } if id == EntryID(42)
            ),
            "unexpected event: {event:?}"
        );

        let requests = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server finished")
            .expect("server task");
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("PUT /v1/entries/42/bookmark "));
        assert!(requests[1].starts_with("GET /v1/entries/42 "));
    }

    async fn read_http_request(
        socket: &mut tokio::net::TcpStream,
    ) -> (String, String) {
        let mut data = Vec::new();
        let mut chunk = [0u8; 2048];
        let header_end = loop {
            let read = socket.read(&mut chunk).await.expect("read request");
            assert!(read > 0, "connection closed before the headers");
            data.extend_from_slice(&chunk[..read]);
            if let Some(index) =
                data.windows(4).position(|window| window == b"\r\n\r\n")
            {
                break index + 4;
            }
        };
        let headers = String::from_utf8_lossy(&data[..header_end]).to_string();
        let length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);
        while data.len() < header_end + length {
            let read = socket.read(&mut chunk).await.expect("read body");
            assert!(read > 0, "connection closed before the body");
            data.extend_from_slice(&chunk[..read]);
        }
        let body =
            String::from_utf8_lossy(&data[header_end..header_end + length])
                .to_string();
        (headers, body)
    }

    fn feed_json(title: &str) -> serde_json::Value {
        serde_json::json!({
            "id": 42,
            "user_id": 1,
            "title": title,
            "site_url": "https://example.com",
            "feed_url": "https://example.com/feed.xml",
            "category": {"id": 3, "user_id": 1, "title": "Category"}
        })
    }

    async fn serve_feed_update(
        listener: TcpListener,
        response: serde_json::Value,
    ) -> (String, Option<String>, String) {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let (headers, body) = read_http_request(&mut socket).await;
        let request_line =
            headers.lines().next().unwrap_or_default().to_string();
        let token = headers.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("x-auth-token")
                .then(|| value.trim().to_string())
        });
        let body_out = response.to_string();
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{}",
            body_out.len(),
            body_out
        );
        socket
            .write_all(reply.as_bytes())
            .await
            .expect("write response");
        (request_line, token, body)
    }

    #[tokio::test]
    async fn updating_a_feed_sends_every_option() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("server address");
        let server =
            tokio::spawn(serve_feed_update(listener, feed_json("Updated")));

        let client =
            ApiClient::new(&test_config(&format!("http://{address}/")))
                .expect("api client");
        let changes = FeedUpdate {
            title: "Renamed".to_string(),
            category_id: CategoryID(3),
            feed_url: "https://example.com/feed.xml".to_string(),
            site_url: "https://example.com".to_string(),
            user_agent: "fluxgrove".to_string(),
            username: "user".to_string(),
            password: "secret".to_string(),
            scraper_rules: "article".to_string(),
            rewrite_rules: "replace".to_string(),
            blocklist_rules: "block-me".to_string(),
            keeplist_rules: "keep-me".to_string(),
            crawler: true,
            disabled: true,
            ignore_http_cache: true,
            fetch_via_proxy: true,
        };
        let (events, mut received) = mpsc::unbounded_channel();
        client
            .run(
                ApiCommand::UpdateFeed {
                    id: FeedID(42),
                    changes: Box::new(changes),
                },
                &events,
            )
            .await;

        let event = received.recv().await.expect("an event");
        assert!(
            matches!(&event, ApiEvent::FeedUpdated(feed) if feed.title == "Updated"),
            "unexpected event: {event:?}"
        );

        let (request_line, token, body) =
            tokio::time::timeout(Duration::from_secs(5), server)
                .await
                .expect("server finished")
                .expect("server task");
        assert_eq!(request_line, "PUT /v1/feeds/42 HTTP/1.1");
        assert_eq!(token.as_deref(), Some("test-token"));
        let payload: serde_json::Value =
            serde_json::from_str(&body).expect("JSON request body");
        for field in [
            "title",
            "category_id",
            "feed_url",
            "site_url",
            "user_agent",
            "username",
            "password",
            "scraper_rules",
            "rewrite_rules",
            "blocklist_rules",
            "keeplist_rules",
            "crawler",
            "disabled",
            "ignore_http_cache",
            "fetch_via_proxy",
        ] {
            assert!(payload.get(field).is_some(), "missing {field}: {body}");
        }
        assert_eq!(payload["category_id"], 3);
        assert_eq!(payload["blocklist_rules"], "block-me");
        assert_eq!(payload["keeplist_rules"], "keep-me");
        assert_eq!(payload["ignore_http_cache"], true);
        assert_eq!(payload["fetch_via_proxy"], true);
    }

    #[tokio::test]
    async fn deleting_a_feed_reports_its_id() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("server address");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let (headers, _body) = read_http_request(&mut socket).await;
            let request_line =
                headers.lines().next().unwrap_or_default().to_string();
            socket
                .write_all(
                    b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n",
                )
                .await
                .expect("write response");
            request_line
        });

        let client =
            ApiClient::new(&test_config(&format!("http://{address}/")))
                .expect("api client");
        let (events, mut received) = mpsc::unbounded_channel();
        client
            .run(ApiCommand::DeleteFeed { id: FeedID(42) }, &events)
            .await;

        let event = received.recv().await.expect("an event");
        assert!(
            matches!(event, ApiEvent::FeedDeleted { feed_id } if feed_id == FeedID(42)),
            "unexpected event: {event:?}"
        );

        let request_line = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server finished")
            .expect("server task");
        assert_eq!(request_line, "DELETE /v1/feeds/42 HTTP/1.1");
    }

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
