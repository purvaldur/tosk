mod app;
mod clipboard;
mod command;
mod config;
mod counts;
mod du;
mod entry;
mod format;
mod fs;
mod git;
mod grep;
mod help;
mod icons;
mod input;
mod keymap;
mod kitty;
mod launch;
mod modal;
mod picker;
mod preview;
mod props;
mod rename;
mod shortcuts;
mod sort;
mod term_panel;
mod text_input;
mod theme;
mod tool;
mod ui;

use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui_image::picker::{Picker, ProtocolType};

use crate::app::App;
use crate::config::Config;

const TICK: Duration = Duration::from_millis(50);

fn main() -> Result<()> {
    let dir = std::env::current_dir()?;
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        hook(info);
    }));

    let result = init().and_then(|mut terminal| {
        let config_dir = dirs::config_dir().map(|d| d.join("tosk"));
        let pins = config_dir.as_ref().map(|d| d.join("shortcuts"));
        let config = config_dir
            .map(|d| Config::load(&d.join("config.toml")))
            .unwrap_or_default();
        let mut app = App::with_config(dir, kitty_graphics(), pins, config)?;
        run(&mut terminal, &mut app)
    });
    restore();
    result
}

fn init() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    // kitty keeps a separate keyboard mode stack per screen, so push after
    // entering the alternate screen and pop before leaving it.
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        )
    )?;
    Ok(Terminal::new(CrosstermBackend::new(io::stdout()))?)
}

/// Asks the terminal about graphics support and font size. Images only
/// render with the kitty protocol; anything else gets the info card.
fn kitty_graphics() -> Option<Picker> {
    // Only ask kitty: a terminal that never answers leaves the query's
    // reader thread consuming keystrokes.
    std::env::var_os("KITTY_WINDOW_ID")?;
    Picker::from_query_stdio()
        .ok()
        .filter(|p| p.protocol_type() == ProtocolType::Kitty)
}

fn restore() {
    let _ = execute!(
        io::stdout(),
        PopKeyboardEnhancementFlags,
        LeaveAlternateScreen,
        cursor::Show
    );
    let _ = disable_raw_mode();
}

fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    let mut dirty = true;
    while !app.quit {
        dirty |= app.drain_workers();
        if dirty {
            let size = terminal.size()?;
            let full = Rect::new(0, 0, size.width, size.height);
            let areas = ui::Areas::new(full, app);
            app.sync_terminal_size(areas.terminal_size());
            app.sync_preview(areas.preview_size(), ui::picker_preview_size(full));
            terminal.draw(|frame| ui::draw(frame, app))?;
            dirty = false;
        }
        if event::poll(TICK)? {
            // Handle everything that's queued before redrawing once.
            loop {
                match event::read()? {
                    Event::Key(key) => input::handle_key(app, key),
                    Event::Resize(..) => {}
                    _ => continue,
                }
                dirty = true;
                if app.quit || !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
    }
    Ok(())
}
