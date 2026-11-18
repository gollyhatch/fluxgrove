use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
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
    #[arg(long, value_name = "PATH", env = "FLUXGROVE_CONFIG", global = true)]
    pub config: Option<PathBuf>,

    /// Miniflux instance URL, e.g. https://reader.example.com
    #[arg(long, value_name = "URL", env = "MINIFLUX_URL", global = true)]
    pub url: Option<String>,

    /// Miniflux API token.
    #[arg(
        long,
        value_name = "TOKEN",
        env = "MINIFLUX_TOKEN",
        hide_env_values = true,
        global = true
    )]
    pub token: Option<String>,

    /// Miniflux username (used with --password when no token is set).
    #[arg(long, value_name = "USER", env = "MINIFLUX_USERNAME", global = true)]
    pub username: Option<String>,

    /// Miniflux password (used with --username when no token is set).
    #[arg(
        long,
        value_name = "PASSWORD",
        env = "MINIFLUX_PASSWORD",
        hide_env_values = true,
        global = true
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

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Commands that run without starting the terminal interface.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Print the number of unread entries and exit.
    Unread,
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
    pub images: bool,
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
    images: Option<bool>,
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
        // `--config` also carries FLUXGROVE_CONFIG through clap.
        let explicit_config = cli.config.is_some();
        let path = cli.config.clone().unwrap_or_else(default_config_path);
        let file = read_file(&path, explicit_config)?;

        let url = cli
            .url
            .clone()
            .or(file.url)
            .ok_or_else(|| missing_config_error(&path, "url"))?;

        let cli_token =
            cli.token.clone().filter(|token| !token.trim().is_empty());
        let cli_username = cli
            .username
            .clone()
            .filter(|username| !username.trim().is_empty());
        let cli_password = cli
            .password
            .clone()
            .filter(|password| !password.trim().is_empty());
        // Credentials given on the command line (or through their environment
        // variables) outrank the config file, including its token.
        let cli_credentials = cli_token.is_some()
            || cli_username.is_some()
            || cli_password.is_some();
        let token = if cli_credentials && cli_token.is_none() {
            None
        } else {
            cli_token.or(file.token)
        }
        .filter(|token| !token.trim().is_empty());
        let username = cli_username
            .or(file.username)
            .filter(|username| !username.trim().is_empty());
        let password = cli_password
            .or(file.password)
            .filter(|password| !password.trim().is_empty());

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
            images: file.images.unwrap_or(false),
        })
    }
}

fn read_file(path: &Path, required: bool) -> Result<FileConfig> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound && !required => {
            return Ok(FileConfig::default());
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!("could not read configuration file {}", path.display())
            });
        }
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    struct TempConfig(PathBuf);

    impl TempConfig {
        fn new(body: &str) -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock before the epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "fluxgrove-config-{}-{unique}.toml",
                std::process::id()
            ));
            fs::write(&path, body).expect("write the temporary config");
            Self(path)
        }
    }

    impl Drop for TempConfig {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn cli(config: Option<PathBuf>) -> Cli {
        Cli {
            config,
            url: None,
            token: None,
            username: None,
            password: None,
            theme: None,
            limit: None,
            list_themes: false,
            command: None,
        }
    }

    #[test]
    fn images_default_off_and_can_be_enabled() {
        let file = TempConfig::new(
            "url = \"https://reader.example.com\"\ntoken = \"t\"\n",
        );
        let config = Config::load(&cli(Some(file.0.clone()))).expect("load");
        assert!(!config.images);

        let file = TempConfig::new(
            "url = \"https://reader.example.com\"\ntoken = \"t\"\nimages = true\n",
        );
        let config = Config::load(&cli(Some(file.0.clone()))).expect("load");
        assert!(config.images);
    }

    #[test]
    fn unread_is_a_subcommand() {
        let cli = Cli::try_parse_from(["fluxgrove", "unread"]).expect("parse");

        assert!(matches!(cli.command, Some(Command::Unread)));
    }

    #[test]
    fn without_a_subcommand_the_tui_starts() {
        let cli = Cli::try_parse_from(["fluxgrove"]).expect("parse");

        assert!(cli.command.is_none());
    }

    #[test]
    fn connection_options_work_after_the_subcommand() {
        let cli = Cli::try_parse_from([
            "fluxgrove",
            "unread",
            "--url",
            "https://reader.example.com",
            "--token",
            "secret",
        ])
        .expect("parse");

        assert!(matches!(cli.command, Some(Command::Unread)));
        assert_eq!(cli.url.as_deref(), Some("https://reader.example.com"));
        assert_eq!(cli.token.as_deref(), Some("secret"));
    }

    #[test]
    fn command_line_credentials_override_the_file_token() {
        let file = TempConfig::new(
            "url = \"https://reader.example.com\"\ntoken = \"file-token\"\n",
        );
        let mut cli = cli(Some(file.0.clone()));
        cli.username = Some("alice".to_string());
        cli.password = Some("secret".to_string());

        let config = Config::load(&cli).expect("config should load");

        assert!(matches!(
            config.auth,
            Auth::Password { username, password }
                if username == "alice" && password == "secret"
        ));
    }

    #[test]
    fn file_token_is_used_when_no_credentials_are_given() {
        let file = TempConfig::new(
            "url = \"https://reader.example.com\"\ntoken = \"file-token\"\n",
        );

        let config = Config::load(&cli(Some(file.0.clone())))
            .expect("config should load");

        assert!(
            matches!(config.auth, Auth::Token(token) if token == "file-token")
        );
    }

    #[test]
    fn a_missing_explicit_config_file_is_an_error() {
        let path =
            std::env::temp_dir().join("fluxgrove-definitely-missing.toml");
        let _ = fs::remove_file(&path);

        assert!(Config::load(&cli(Some(path))).is_err());
    }

    #[test]
    fn a_blank_password_is_rejected() {
        let file = TempConfig::new(
            "url = \"https://reader.example.com\"\nusername = \"demo\"\npassword = \"\"\n",
        );

        assert!(Config::load(&cli(Some(file.0.clone()))).is_err());
    }
}
