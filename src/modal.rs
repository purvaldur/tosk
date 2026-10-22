//! Dialogs that take every key while they're open.

use std::path::PathBuf;

use crate::grep::ContentSearch;
use crate::help::HelpState;
use crate::picker::FilePicker;
use crate::props::Props;
use crate::text_input::TextInput;

pub enum Modal {
    /// Trash, or with `delete` remove for good, after `y`.
    Confirm {
        delete: bool,
        paths: Vec<PathBuf>,
    },
    /// One name means a plain rename; several mean a bulk rename, numbered
    /// in this order.
    Rename {
        olds: Vec<String>,
        input: TextInput,
    },
    /// A new empty file, or with `folder` a new folder, here.
    Create {
        folder: bool,
        input: TextInput,
    },
    /// `Ctrl-l`: a path or `ssh://` address to go to.
    Goto(TextInput),
    Help(HelpState),
    /// Something to read, like problems in the config; Esc closes it.
    Notice {
        title: String,
        lines: Vec<String>,
    },
    Picker(Box<FilePicker>),
    Search(Box<ContentSearch>),
    /// `p`, with folder sizes counting in the background.
    Props(Box<Props>),
}
