use crossterm::event::{KeyEvent, KeyEventKind};

use crate::app::App;
use crate::command;
use crate::keymap::Key;

/// Routes a key by mode: modal > filter > terminal > normal. Only normal
/// mode exists so far.
pub fn handle_key(app: &mut App, ev: KeyEvent) {
    if ev.kind == KeyEventKind::Release {
        return;
    }
    let Some(key) = Key::from_event(ev) else {
        return;
    };
    if let Some(cmd) = app.keymap.lookup(key) {
        command::dispatch(app, cmd);
    }
}
