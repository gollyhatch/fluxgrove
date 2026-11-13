// Copyright (C) 2026 the fluxgrove authors
// SPDX-License-Identifier: GPL-3.0-only

mod api;
mod app;
mod config;
mod ui;
mod util;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::EventStream;
use futures_util::StreamExt;
use tokio::{
    sync::mpsc::UnboundedReceiver,
    time::{Duration, MissedTickBehavior},
};

use crate::{
    app::App,
    config::{Config, DEFAULT_THEME},
};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = config::Cli::parse();

    if cli.list_themes {
        list_themes();
        return Ok(());
    }

    let config = Config::load(&cli)?;
    let client = api::ApiClient::new(&config)?;

    let requested_theme = config.theme.clone();
    let (theme_id, theme, warning) = match opaline::builtins::load_by_name(
        &requested_theme,
    ) {
        Some(theme) => (requested_theme, theme, None),
        None => (
            DEFAULT_THEME.to_string(),
            opaline::builtins::load_by_name(DEFAULT_THEME)
                .expect("the default builtin theme must exist"),
            Some(format!(
                "Theme “{requested_theme}” was not found; using {DEFAULT_THEME}"
            )),
        ),
    };

    let (api_tx, api_rx) = api::spawn(client);
    let mut app = App::new(config, theme, theme_id, api_tx);
    if let Some(warning) = warning {
        app.set_startup_warning(warning);
    }

    let mut terminal = ratatui::try_init()?;
    let result = run(&mut terminal, &mut app, api_rx).await;
    ratatui::restore();
    result
}

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    mut api_rx: UnboundedReceiver<api::ApiEvent>,
) -> Result<()> {
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(120));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(event)) => app.handle_event(event),
                    Some(Err(error)) => return Err(error).context("could not read terminal events"),
                    None => break,
                }
            }
            Some(event) = api_rx.recv() => app.handle_api(event),
            _ = tick.tick() => app.on_tick(),
        }

        if app.should_quit {
            break;
        }

        if app.dirty {
            terminal
                .draw(|frame| ui::draw(frame, app))
                .context("could not draw the terminal")?;
            app.dirty = false;
        }
    }

    Ok(())
}

fn list_themes() {
    use std::io::Write;

    let mut themes: Vec<&(&str, &str)> =
        opaline::builtins::builtin_names().iter().collect();
    themes.sort_by_key(|(_, name)| name.to_lowercase());

    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    let _ = writeln!(out, "Builtin themes:\n");
    for (id, name) in themes {
        let default = if id == &DEFAULT_THEME {
            " (default)"
        } else {
            ""
        };
        let _ = writeln!(out, "  {id:<28} {name}{default}");
    }
    let _ = writeln!(
        out,
        "\nSet a theme with `theme = \"id\"` in the config or `--theme id`."
    );
}
