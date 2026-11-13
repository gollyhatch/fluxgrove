use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use serde::Deserialize;
use url::Url;

/// A vim-flavoured terminal client for Miniflux.
#[derive(Debug, Parser)]
#[command(
    name = "fluxgrove",
    version,
    about = "A vim-flavoured terminal client for Miniflux",
    after_help = "Configuration is read from ~/.config/fluxgrove/config.toml (or --config).\n\
                  Values can also be supplied with MINIFLUX_URL, MINIFLUX_TOKEN,\n\
                  MINIFLUX_USERNAME and MINIFLUX_PASSWORD."
)]
pub struct Cli {
    /// Path to the configuration file.
    #[arg(long, value_name = "PATH", env = "FLUXGROVE_CONFIG")]
    pub config: Option<PathBuf>,

    /// Miniflux instance URL, e.g. https://reader.example.com
    #[arg(long, value_name = "URL", env = "MINIFLUX_URL")]
    pub url: Option<String>,

    /// Miniflux API token.
    #[arg(
        long,
        value_name = "TOKEN",
        env = "MINIFLUX_TOKEN",
        hide_env_values = true
    )]
    pub token: Option<String>,

    /// Miniflux username (used with --password when no token is set).
    #[arg(long, value_name = "USER", env = "MINIFLUX_USERNAME")]
    pub username: Option<String>,

    /// Miniflux password (used with --username when no token is set).
    #[arg(
        long,
        value_name = "PASSWORD",
        env = "MINIFLUX_PASSWORD",
        hide_env_values = true
    )]
    pub password: Option<String>,

    /// Builtin theme id, e.g. catppuccin-mocha or tokyo-night.
    #[arg(long, value_name = "THEME")]
    pub theme: Option<String>,

    /// Maximum number of entries to fetch per view.
    #[arg(long, value_name = "N")]
    pub limit: Option<i64>,

    /// List the available builtin themes and exit.
    #[arg(long)]
    pub list_themes: bool,
}

#[derive(Debug, Clone)]
pub enum Auth {
    Token(String),
    Password { username: String, password: String },
}

#[derive(Debug, Clone)]
pub struct Config {
    pub url: Url,
    pub auth: Auth,
    pub theme: String,
    pub limit: i64,
    pub mark_read: bool,
    pub hide_read_feeds: bool,
    pub hide_read_entries: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    url: Option<String>,
    token: Option<String>,
    username: Option<String>,
    password: Option<String>,
    theme: Option<String>,
    limit: Option<i64>,
    mark_read: Option<bool>,
    hide_read_feeds: Option<bool>,
    hide_read_entries: Option<bool>,
}

pub const DEFAULT_THEME: &str = "catppuccin-mocha";
pub const DEFAULT_LIMIT: i64 = 100;

pub fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("fluxgrove")
        .join("config.toml")
}

impl Config {
    pub fn load(cli: &Cli) -> Result<Self> {
        let path = cli
            .config
            .clone()
            .or_else(|| env::var_os("FLUXGROVE_CONFIG").map(PathBuf::from))
            .unwrap_or_else(default_config_path);

        let file = read_file(&path)?;

        let url = cli
            .url
            .clone()
            .or(file.url)
            .ok_or_else(|| missing_config_error(&path, "url"))?;
        let token = cli
            .token
            .clone()
            .or(file.token)
            .filter(|t| !t.trim().is_empty());
        let username = cli
            .username
            .clone()
            .or(file.username)
            .filter(|u| !u.is_empty());
        let password = cli.password.clone().or(file.password);

        let auth = match (token, username, password) {
            (Some(token), _, _) => Auth::Token(token),
            (None, Some(username), Some(password)) => {
                Auth::Password { username, password }
            }
            (None, Some(_), None) => {
                bail!(
                    "a Miniflux password is required when using username authentication"
                )
            }
            (None, None, _) => {
                return Err(missing_config_error(
                    &path,
                    "token (or username and password)",
                ));
            }
        };

        let mut url = Url::parse(url.trim())
            .with_context(|| format!("invalid Miniflux URL: {url}"))?;
        if !matches!(url.scheme(), "http" | "https") {
            bail!("the Miniflux URL must start with http:// or https://");
        }
        // MinifluxApi joins relative paths onto the base URL, which requires a
        // trailing slash so that a sub-path such as /miniflux/ is preserved.
        if !url.path().ends_with('/') {
            let path = format!("{}/", url.path());
            url.set_path(&path);
        }

        let limit = cli.limit.or(file.limit).unwrap_or(DEFAULT_LIMIT);
        if !(1..=500).contains(&limit) {
            bail!("limit must be between 1 and 500");
        }

        Ok(Self {
            url,
            auth,
            theme: cli
                .theme
                .clone()
                .or(file.theme)
                .unwrap_or_else(|| DEFAULT_THEME.to_string()),
            limit,
            mark_read: file.mark_read.unwrap_or(true),
            hide_read_feeds: file.hide_read_feeds.unwrap_or(false),
            hide_read_entries: file.hide_read_entries.unwrap_or(false),
        })
    }
}

fn read_file(path: &Path) -> Result<FileConfig> {
    if !path.exists() {
        return Ok(FileConfig::default());
    }
    let raw = fs::read_to_string(path).with_context(|| {
        format!("could not read configuration file {}", path.display())
    })?;
    toml::from_str(&raw).with_context(|| {
        format!("could not parse configuration file {}", path.display())
    })
}

fn missing_config_error(path: &Path, field: &str) -> anyhow::Error {
    anyhow!(
        "missing Miniflux {field}\n\n\
         Create {} with:\n\n  \
         url = \"https://reader.example.com\"\n  \
         token = \"your-api-token\"\n  \
         theme = \"catppuccin-mocha\"\n\n\
         Alternatively set MINIFLUX_URL and MINIFLUX_TOKEN, or pass --url and --token.\n\
         Generate an API token in Miniflux under Settings → API Keys.",
        path.display()
    )
}
