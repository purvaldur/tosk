//! Dialogs that take every key while they're open.

use std::path::PathBuf;

use crate::text_input::TextInput;

pub enum Modal {
    /// Trash, or with `delete` remove for good, after `y`.
    Confirm { delete: bool, paths: Vec<PathBuf> },
    /// One name means a plain rename; several mean a bulk rename, numbered
    /// in this order.
    Rename { olds: Vec<String>, input: TextInput },
}
