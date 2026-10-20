# tosk — build spec

A personal TUI file manager for one machine: Hyprland, kitty, Helix, Catppuccin Mocha. It is not meant for publishing, so it can hardcode assumptions about that setup instead of supporting everything.

`tosk` is named after Ratatosk, the squirrel that runs up and down Yggdrasil carrying messages. The interactive prototype (`tosk.html`) is the reference for look and behaviour. Where this spec and the prototype disagree, **this spec wins**. The prototype fakes the filesystem, the shell and every external command; the commands it prints in its message line show what the real build should run.

---

## 0. Rules for the coding agent

1. Build **one milestone at a time** (section 9). Stop at the end of each milestone, summarise what changed, and wait for review.
2. Command ids, default keys and user-facing strings in this spec are decisions, not suggestions. Don't rename them or add keys beyond what's listed without asking.
3. **No hjkl, no vim conventions.** Movement is `i j k l`. Don't add `g`/`G`, `dd`, `yy`, counts, or a leader key.
4. **Never touch real user files in tests.** Every test that touches the filesystem runs in a `tempfile::TempDir`. Manual testing happens in a scratch folder until milestone 4 is done.
5. Permanent deletion (`D`) and overwriting renames must stay behind a confirmation. Trash is the default everywhere.
6. Run `cargo fmt`, `cargo clippy -- -D warnings` and `cargo test` before finishing a milestone.
7. Ask before adding a dependency that isn't listed in section 2.
8. Check crate versions with `cargo add` rather than trusting any version number written here or in the prototype.

---

## 1. Goals and non-goals

**Goals**

- Fast keyboard-driven browsing with a layout that stays put: sidebar, file list, preview.
- Deep integration with the user's own tools: open files in the already-running Helix window via kitty remote control, run scripts in a built-in terminal panel, copy to the Wayland clipboard.
- A command layer: every action is a named command, and every key is just a pointer to a command name. Rebinding never fights built-in behaviour.
- A `?` menu that shows the shortcuts relevant to what's under the cursor first.

**Non-goals for 1.0**

- Other terminals than kitty, other OSes than Linux, X11.
- Mouse support beyond what comes free.
- Plugins or scripting languages. Custom shell commands in config are enough (section 7).
- Tabs, ripgrep search, undo (see section 11).

---

## 2. Stack

| Concern | Choice | Notes |
|---|---|---|
| Language | Rust, edition 2024 | |
| UI | `ratatui` + `crossterm` | Enable the kitty keyboard protocol at startup (section 6.1). |
| Embedded terminal | `tui-term` + `portable-pty` | `tui-term` renders a `vt100` screen; `portable-pty` spawns `$SHELL`. |
| Images | `ratatui-image` + `image` | Kitty graphics protocol only. No fallback protocols needed. `image` decodes files for it. |
| Trash | `trash` | freedesktop trash, restorable from any file manager. |
| Fuzzy matching | `nucleo` | Same matcher Helix uses. |
| Directory walking | `ignore` | Respects `.gitignore`, like the Helix picker. |
| Syntax highlighting | `syntect` | Text preview only. Catppuccin Mocha `.tmTheme`. |
| Config | `serde` + `toml` | |
| Errors | `anyhow` | |
| Dates | `jiff` | Local-time dates for the list and properties. |
| Paths | `dirs` | XDG dirs for sidebar places and config location. |
| Remote (milestone 9) | `openssh` + `openssh-sftp-client` | Reuses `~/.ssh/config`, agent and ControlMaster. |
| Tests | `tempfile` | |

External programs called at runtime: `kitty` (remote control), `hx`, `wl-copy`, `xdg-open`, `git`. Optional previewers: `pdftoppm`, `ffmpegthumbnailer`, `ffprobe`, `bsdtar`. If an optional one is missing, the preview shows "no previewer" instead of failing.

Concurrency: plain `std::thread` workers with `std::sync::mpsc` channels back to the UI loop. No async runtime unless milestone 9 needs one.

---

## 3. Architecture

```
src/
  main.rs          setup, terminal init/restore, main loop
  app.rs           App state: dir, cursor per dir, selection, focus, modal, settings
  command.rs       Command enum, id <-> string mapping, dispatch
  keymap.rs        KeyName parsing ("C-t", "S-Tab", "Space"), default map, config overrides
  input.rs         routes key events by mode: modal > filter > terminal > normal
  fs/
    mod.rs         Vfs trait: list, stat, read_head, rename, trash, delete
    local.rs       local backend
    sftp.rs        remote backend (milestone 9)
  entry.rs         Entry { name, kind, size, mtime, mode, link_target, ... }
  sort.rs          dirs first, natural order, case-insensitive, leading dot ignored
  preview.rs       preview worker + per-kind renderers
  du.rs            background recursive size counting
  git.rs           repo detection and `git status --porcelain=v2 --branch`
  kitty.rs         kitty @ calls, Helix window lookup
  clipboard.rs     wl-copy wrappers
  term_panel.rs    pty + vt100 parser + key encoding
  ui/
    mod.rs         top-level layout
    sidebar.rs  list.rs  preview.rs  status.rs  messages.rs
    help.rs  picker.rs  confirm.rs  rename.rs  props.rs
  theme.rs         Catppuccin Mocha palette
  config.rs        config.toml + keymap loading
```

**Main loop.** Poll for crossterm events with a short timeout (~50 ms), drain worker channels (preview, du, git, pty output), redraw if anything changed.

**Input modes.** These are checked in priority order:

1. A modal is open (help, picker, confirm, rename, properties): the modal gets every key.
2. Filter is being typed: text input.
3. Focus is in the terminal panel: every key goes to the pty, **except** keys bound to `term.toggle`.
4. Normal mode: look up the keymap, dispatch the command.

**Focus.** `Focus::Sidebar | Focus::Files | Focus::Terminal`. `Tab`/`S-Tab` cycle through a list that today holds only `[Sidebar, Files]`. Keep it a list so more views can join later. The terminal is deliberately outside that cycle (section 5.10).

**Messages.** A two-line message area under the status line shows the latest results: what ran, what failed. Errors are red. Messages never block.

---

## 4. Layout

```
┌─────────────────────────────────────────────────────────────────────────┐
│ places           │ ~/code/tosk                      │ main.rs · 1.5K    │
│ PLACES           │   src/                3 items     │   1 use ratatui…  │
│ ~ Home           │ ● Cargo.toml     356B  Oct 19     │   2 mod commands; │
│ › Documents      │   NOTES.md       840B  Oct 20     │   …               │
│ › Downloads      │                                   │                   │
│ …                │      “Cargo.toml” selected (356 bytes) ┐              │
│ NETWORK          │                                   │                   │
│ @ nas            │                                   │                   │
├─────────────────────────────────────────────────────────────────────────┤
│ terminal · ~/code/tosk · Ctrl-t hides          (only when toggled on)   │
│ ~/code/tosk ❯ cargo build                                               │
├─────────────────────────────────────────────────────────────────────────┤
│ FILES  ~/code/tosk  Cargo.toml        main ↑2 ↓0 ~3 · 3 items · ? help   │
│ $ kitty @ send-text --match var:role=helix …   # sent to your Helix     │
└─────────────────────────────────────────────────────────────────────────┘
```

- **Sidebar**: fixed 18 columns, titled `shortcuts`. Groups are separated by divider lines (section 5.16).
- **File list**: flexible, about 60% of the remaining width when the preview is on. Each row has a selection marker column (`●`), the name (folders get a trailing `/`), and right-aligned metadata.
  - Files: size and date.
  - Folders: `N items`.
  - Symlinks: `→ target`.
- **Preview**: the rest of the width. `P` hides it, and the list then takes the full width.
- **Pane headers**: one row each. The focused pane's header is drawn in the accent colour. Headers read:
  - list: the current path
  - preview: `name · size`
  - sidebar: `shortcuts`
- **Cursor row**: accent background in the focused pane, a dim `surface0` background in an unfocused pane.
- **Selection summary**: a small box in the bottom-right corner of the file list, only while something is selected (section 5.13).
- **Terminal panel**: full width under the panes, about 7 rows (header plus output). When it's open, the panes shrink to make room.
- **Status line** (one row), left to right:
  - mode chip: `FILES`, `SHORTCUTS`, `TERMINAL` or `FILTER`
  - current path
  - name of the item under the cursor
  - pushed right: git segment (section 5.14), `N items`, `hidden shown` when hidden files are on, `? help`
- **Message area**: two rows.

**Paths** are shown with `~` for home and `ssh://host/path` for remote locations.

**Sorting**: folders first (symlinks to folders count as folders), then natural order (`e5` sorts after `e04`), case-insensitive, ignoring a leading `.`.

**Sizes**: SI units like Nautilus. The list uses compact `356B`, `4.1M`, `1.2G`; summaries and properties use `356 bytes`, `1.2 kB`, `4.1 MB`.

---

## 5. Behaviour

### 5.1 Movement (`i j k l`)

- `i`/`k` (and `↑`/`↓`) move the cursor. `Home`/`End` jump to the first and last item.
- `j` (and `←`) goes to the parent folder with the cursor on the folder you came from.
- `l` (and `→`) is **movement only**:
  - on a folder: enter it
  - on a symlink to a folder: enter the target
  - on a symlink to a file: go to the target's folder with the cursor on the target
  - on a plain file: **do nothing**, with no message
- The cursor position is remembered per folder for the session.
- In the sidebar, `i`/`k` move and `l`/`Enter` jump to the place and move focus back to the file list. `j` does nothing there.

### 5.2 Opening

- `Enter`/`o` open with the default app: `xdg-open <path>`, detached. On a folder they behave like `l`.
- `e` edits in Helix (section 6.2):
  - on a text file: open it in the running Helix
  - on a folder: new kitty tab running `hx .` in that folder
  - on a non-text file: error message `e edits text files; <name> is <kind>`

### 5.3 Selection

- `Space` toggles selection on the item under the cursor **and does not move the cursor**.
- `Esc` clears the selection and any filter.
- Selection order is remembered, because bulk rename numbers files in that order.
- Commands that act on files use the selection when there is one, otherwise the item under the cursor.

### 5.4 Clipboard

- `c` copies the files themselves. Write one `file://` URI per line, percent-encoded, to `wl-copy -t text/uri-list`. GUI apps then paste them as files.
- `C` copies the paths as plain text, space-separated, with `~` expanded.

### 5.5 Trash and delete

- `d` opens a confirm dialog titled `Move to trash` that lists up to 5 paths plus `…and N more`. Footer: `y trash · n or Esc cancel`. Only `y`/`Y` confirms; `n`, `N` or `Esc` cancel.
- `D` works the same way, but with a red border, the title `Delete permanently` and the line `This skips the trash and can't be undone.`. On `y`, it removes the files with `std::fs::remove_file` or `remove_dir_all`.
- After either action, the message reads `N moved to trash, restorable` or `N deleted for good`.

### 5.6 Rename (`r`)

**Single item**: an input prefilled with the current name. `Enter` applies and `Esc` cancels.

**Several selected items**: bulk rename. One template input, with a live old → new preview table (up to 8 rows) under it.

Template tokens:

| Token | Meaning |
|---|---|
| `[01]` | counter starting at 1, zero-padded to the width of the digits written (`[001]` → 001, 002, …) |
| `[0-9]` | counter starting at 0, padded to width 1. In general `[a-b]` starts at `a`. |
| `{name}` | original name without extension |
| `{ext}` | original extension without the dot |

- Files are numbered **in selection order**.
- Example: `show.s01.e[01].mkv` turns five selected files into `show.s01.e01.mkv` … `show.s01.e05.mkv`.
- **Validation**: a row is invalid if the new name is empty, contains `/`, duplicates another new name, or clashes with an unselected file in the same folder. Invalid rows are marked with `✗` in red, and `Enter` refuses with `fix the names marked ✗ first: empty, clashing or containing /`.
- **Apply in two phases** (each file to a temporary name, then to its final name), so swaps like `a→b, b→a` work.
- Footer: `[01] counter · {name} old name · {ext} extension · Enter apply · Esc cancel`.
- Text inputs everywhere support `Backspace`, `Ctrl-u` (clear) and `Ctrl-w` (delete word).

### 5.7 Filter (`Ctrl-f`)

- Live, case-insensitive substring filter on the current folder. The status line shows a `FILTER` chip and the query.
- `Enter` keeps the filter and returns to normal keys. `Esc` clears it.
- Changing folders clears the filter.

### 5.8 File pickers (`f`, `F`)

- `f` searches from the folder tosk was launched in. `F` searches from the current folder.
- The picker is a centred overlay:
  - header: `Files · <root>` and a match count `12/48`
  - an input line
  - results on the left, preview of the highlighted result on the right
- Lists files only, walked with `ignore` so `.gitignore` is respected, plus the hidden-files setting. Matches are ranked by `nucleo`, and matched characters are highlighted in peach.
- Keys: type to filter; `↑`/`↓` or `Ctrl-p`/`Ctrl-n` move; `Enter` jumps to the file's folder with the cursor on it (turning hidden files on if the target is hidden); `Esc` closes.
- Walk on a worker thread and stream results in. Don't block the UI on big trees.

### 5.9 `?` help menu

A lazygit-style list of commands, **most relevant first**. Sections, in order, each shown only when it applies:

1. `Sidebar`, when the sidebar has focus: `nav.enter`, `cursor.up`, `cursor.down`.
2. `Selection · N items`, when 2 or more are selected: properties, rename, copy file, copy path, trash, delete, select toggle, select clear.
3. `On <name>`, for the item under the cursor. Order depends on kind:
   - symlink: `nav.enter`, `open.editor`, `path.copy`
   - folder: `nav.enter`, `open.editor`, `file.rename`, `path.copy`, `file.trash`
   - `.sh` file: `script.run`, `open.editor`, `open.default`, `file.rename`, `file.copy`, `path.copy`
   - text file: `open.editor`, `open.default`, `file.copy`, `path.copy`, `file.rename`
   - other files: `open.default`, `file.copy`, `path.copy`, `file.rename`
   - then always: `file.props`, `select.toggle`, `file.trash`, `file.delete`
4. `In <folder>`: `picker.here`, `filter.start`, `term.toggle`, `view.hidden`, `view.preview`, `nav.parent`.
5. `Everywhere`: `picker.root`, `focus.next`, `focus.prev`, `help.open`, `app.quit`, movement, `select.clear`.

Rules:

- A command appears only once, in the first section that lists it.
- Each row shows the command's **current** first key from the keymap (not the default) and its title.
- Keys inside the menu:
  - `i`/`k` move
  - `Enter` runs the highlighted command
  - pressing any listed command's own key runs it directly
  - `/` starts typing a filter on titles
  - `Esc` or `?` closes
- Footer: `i/k move · Enter run · or press the key itself · / filter · Esc close`.

### 5.10 Terminal panel (`Ctrl-t`)

- A real shell (`$SHELL`) in a pty, rendered with `tui-term`. It starts in the current folder the first time it's opened, and the shell process persists while the panel is hidden.
- `Ctrl-t`:
  - panel hidden → show it and focus it
  - panel shown but not focused → focus it
  - panel focused → hide it and give focus back to the area that had it before
- While the terminal has focus, **every key goes to the shell**, including `Tab` (completion), `Esc` and `q`. The only exception is keys bound to `term.toggle`.
- `Tab`/`S-Tab` never move focus into the terminal.
- Panel header: `terminal · <path> · Ctrl-t hides` when focused, `· Ctrl-t to focus` when not.
- **Key encoding**: `tui-term` only renders, so `term_panel.rs` must turn crossterm `KeyEvent`s into bytes for the pty: printable characters, Enter `\r`, Backspace `\x7f`, Tab `\t`, Esc `\x1b`, arrows `\x1b[A`…, Ctrl+letter as control bytes. Forward pane resizes to the pty.

### 5.11 Run script (`x`)

- On a `.sh` file (or a symlink to one): open the terminal panel if it's hidden, focus it, and type `bash '<absolute path>'\r` into the shell, so the output stays in the panel's scrollback.
- On anything else: error message `x runs .sh files`.

### 5.12 Properties (`p`)

An overlay titled `Properties`. Close it with `Esc`, `p`, `q` or `Enter`.

**One item:**

| Field | Content |
|---|---|
| Name | |
| Type | Folder / Text (.md) / Image (.png) / … / `Symbolic link → target` |
| Size | `4.1 MB (4,100,000 bytes)`; just `840 bytes` under 1 kB |
| Contents | folders only: `N files, M folders · total size` |
| Location | parent path |
| Host | remote only: `<host> over ssh` |
| Modified | full date |
| Permissions | `-rw-r--r--  user:group` |
| Git | if inside a repo: `<branch> · in <repo path>` |

**Several items:**

| Field | Content |
|---|---|
| Selected | `1 folder, 2 files` |
| Total size | recursive: `4.4 kB (4,356 bytes)` |
| Counted | `N files in M folders` |
| Location | the common parent, or `N places` |

Below the fields, list up to 5 names plus `…and N more`.

Folder sizes are counted on the `du` worker. Until it finishes, those fields read `calculating…` in a dim colour, and the footer adds `· sizes are counted in the background`. The worker doesn't follow symlinks.

### 5.13 Selection summary

Shown in the bottom-right corner of the file list whenever the selection isn't empty. It follows Nautilus's wording exactly:

- One file: `“NOTES.md” selected (840 bytes)`
- One folder: `“src” selected (containing 3 items)`
- Several files only: `2 items selected (1.2 kB)`
- Several folders only: `2 folders selected (containing 8 items)`
- A mix: `1 folder selected (containing 3 items), 2 other items selected (1.2 kB)`

Rules:

- "Containing" counts direct children only.
- Folder sizes are **not** added to the summary. That only happens in properties.
- Use singular and plural correctly (`1 item`, `2 items`).

### 5.14 Git segment

- Whenever the current folder is inside a work tree, run `git status --porcelain=v2 --branch` on a worker (debounced, and again after file operations).
- Show `<branch> ↑<ahead> ↓<behind> ~<changed>` in the status line. The branch is peach, the ahead/behind counts muted, and the changed count yellow. Leave out `~N` when it's 0, and leave out ahead/behind when there's no upstream.
- Show nothing outside a repo.

### 5.15 Other toggles

- `h` shows or hides dotfiles. The status line shows `hidden shown` while they're on.
- `P` shows or hides the preview pane.
- `q` quits. Restore the terminal cleanly, kill the pty child, and exit with code 0.

### 5.16 Sidebar shortcuts

- Three groups with a divider line between them, no group headers:
  1. Home and the XDG folders that exist (Desktop, Documents, Downloads, Music, Pictures, Videos), labelled with their real folder names.
  2. Pinned folders. `b` (`shortcut.pin`) pins the folder under the cursor, or the current folder when the cursor is on a file; on a pinned shortcut in the sidebar it unpins. Pins are saved to `~/.config/tosk/shortcuts`, one absolute path per line, and kept even when the folder is missing (an unmounted drive).
  3. Network hosts from config (milestone 9), in teal.
- Every row has a Nerd Font icon. The file list has icons too, by kind and extension.
- The shortcut that contains the current folder is highlighted in the accent colour. Use the deepest match: `~/Videos/show` highlights Videos, not Home.

### 5.17 Previews (worker thread, cached by path and mtime)

| Kind | Preview |
|---|---|
| Folder | its listing, no cursor |
| Text | first N lines, line numbers, `syntect` highlighting |
| Image | `ratatui-image`, kitty protocol |
| PDF | first page via `pdftoppm -png -f 1 -l 1 -scale-to 800`, shown as an image |
| Video | thumbnail via `ffmpegthumbnailer`, plus duration |
| Audio | tags and duration via `ffprobe` |
| Archive | `bsdtar -tf`, first N entries |
| Other | type, size, modified date |

A file counts as text if it isn't detected as binary: check for NUL bytes in the first 8 KB.

---

## 6. System integration

### 6.1 Keyboard protocol

At startup, push the crossterm `KeyboardEnhancementFlags` `DISAMBIGUATE_ESCAPE_CODES | REPORT_ALTERNATE_KEYS`, and pop them on exit, panic included. This lets `Ctrl-i` be told apart from `Tab`, `Ctrl-m` from `Enter`, and `Ctrl+Shift+x` from `Ctrl+x`.

Use a panic hook so the terminal is always restored.

### 6.2 kitty and Helix

Required in `kitty.conf`:

```
allow_remote_control socket-only
listen_on unix:/tmp/kitty
```

- kitty sets `$KITTY_LISTEN_ON` in every window it spawns. Pass it as `--to "$KITTY_LISTEN_ON"` instead of hardcoding the socket.
- The "editor" Helix window is found by a user variable, not by title: `kitty @ ls` returns JSON, and you look for a window whose `user_vars.role == "helix"`.
- **Open in Helix** (`e` on a text file):
  1. If a window with `role=helix` exists, run `kitty @ send-text --match var:role=helix "\x1b:open <path>\r"`, then `kitty @ focus-window --match var:role=helix`. Quote the path for Helix's command line if it contains spaces.
  2. Otherwise, run `kitty @ launch --type=tab --var role=helix --cwd <dir> hx <path>`.
- **Edit a folder**: `kitty @ launch --type=tab --var role=helix --cwd <dir> hx .`

### 6.3 Wayland and desktop

- Clipboard: `wl-copy` (section 5.4).
- Default apps: `xdg-open`, spawned detached with stdio set to null.
- Suggested Hyprland launcher, for the README:

```
$files = kitty --class tosk -e tosk
bind = SUPER, E, exec, $files
```

---

## 7. Commands and default keys

Each command id is the string used in the keymap config. Keys marked † were added by the prototype for convenience; keep or drop them at review.

| Command id | Title | Default keys |
|---|---|---|
| `cursor.up` | Move up | `i`, `↑`† |
| `cursor.down` | Move down | `k`, `↓`† |
| `nav.parent` | Parent folder | `j`, `←`† |
| `nav.enter` | Enter folder / follow link | `l`, `→`† |
| `cursor.top` | First item | `Home`† |
| `cursor.bottom` | Last item | `End`† |
| `open.default` | Open with default app | `Enter`, `o` |
| `open.editor` | Edit in Helix | `e` |
| `file.copy` | Copy file to clipboard | `c` |
| `path.copy` | Copy path to clipboard | `C` |
| `file.rename` | Rename (bulk when several are selected) | `r` |
| `file.trash` | Move to trash | `d` |
| `file.delete` | Delete permanently | `D` |
| `select.toggle` | Select / unselect | `Space` |
| `select.clear` | Clear selection and filter | `Esc`† |
| `script.run` | Run script in terminal panel | `x` |
| `picker.root` | File picker from launch folder | `f` |
| `picker.here` | File picker from this folder | `F` |
| `filter.start` | Filter this folder | `C-f` |
| `term.toggle` | Terminal: open and focus, again to hide | `C-t` |
| `focus.next` | Next area | `Tab` |
| `focus.prev` | Previous area | `S-Tab` |
| `file.props` | Properties | `p` |
| `view.preview` | Toggle preview | `P` |
| `view.hidden` | Toggle hidden files | `h` |
| `help.open` | Shortcuts for this context | `?` |
| `shortcut.pin` | Pin or unpin shortcut | `b` |
| `app.quit` | Quit | `q` |

**Key name syntax.**

- Single characters are case-sensitive: `c` and `C` are different keys.
- Named keys: `Enter`, `Esc`, `Tab`, `Space`, `Backspace`, `Home`, `End`, `Up`/`Down`/`Left`/`Right`.
- Modifier prefixes: `C-` for Ctrl, `A-` for Alt, `S-` only for named keys (`S-Tab`).

**Commands that act on files** (`open.*`, `file.*`, `path.copy`, `select.toggle`, `script.run`) do nothing while the sidebar has focus, and show the message `<title> works on the file list; Tab over first`.

---

## 8. Config

Location: `~/.config/tosk/config.toml`. Every key is optional. A missing file means defaults. Everything tosk stores lives in `~/.config/tosk/` (pinned shortcuts are in `shortcuts` next to it).

```toml
[general]
show_hidden = false
preview = true
editor_role = "helix"          # kitty user var used to find the editor window

[sidebar]
icons = true                   # Nerd Font icons

[[sidebar.hosts]]              # milestone 9
name = "nas"                   # a Host from ~/.ssh/config
root = "/"

# Rebind or add keys. A key mapped to "none" is unbound.
[keys]
"C-t" = "term.toggle"
"S-Tab" = "focus.prev"
W = "wallpaper"                # a custom command, defined below

# Custom shell commands become commands too and can be bound like built-ins.
# Placeholders: {file} {dir} {selection} (space-separated, shell-quoted)
[commands.wallpaper]
title = "Set as wallpaper"
run = "hyprctl hyprpaper wallpaper ,{file}"
where = "background"           # background | terminal
```

On startup, validate the config. An unknown command id or a malformed key should show a message, not crash the program.

---

## 9. Milestones

Every milestone ends with a working binary, passing tests and a short summary.

**M1 — Skeleton and listing**
- Terminal init/restore with a panic hook, the keyboard enhancement flags, the main loop.
- Local `Vfs`: list a folder with name, kind, size, mtime and symlink target.
- Sorting rules (section 4) with unit tests, including natural order and the leading-dot rule.
- Three-pane layout with an empty sidebar and preview, status line, message area, the Mocha theme.
- Commands: movement, `nav.parent`, `nav.enter` (folders and symlinks; files do nothing), `view.hidden`, `app.quit`.
- Command enum, keymap with defaults, key-name parsing with tests.
- *Accept*: browse `~` with `ijkl`; `l` on a file does nothing; the cursor is remembered per folder; `q` exits cleanly.

**M2 — Sidebar and preview**
- Places sidebar, focus cycle with `Tab`/`S-Tab`, deepest-match highlight.
- Preview worker: folder, text with highlighting, image via `ratatui-image`, generic info. `view.preview` (`P`).
- *Accept*: `Tab` into the sidebar, pick Downloads, focus returns to the list; images render in kitty; moving fast doesn't stutter.

**M3 — Selection, clipboard, opening**
- `select.toggle` (cursor stays), `select.clear`, the selection summary with exact strings (unit-test every case in section 5.13).
- `open.default`, `file.copy` (uri-list), `path.copy`.
- *Accept*: copying 2 files with `c` and pasting into Nautilus copies the files.

**M4 — File operations**
- Trash and delete with confirm dialogs, single rename, bulk rename with tokens, validation and two-phase apply. Test the bulk renamer thoroughly in tempdirs, including swaps and clashes.
- `filter.start`.
- *Accept*: the five-episode example renames correctly in selection order; a clash refuses to apply.

**M5 — Help menu and pickers**
- `?` menu with sections, live keys, run-by-key and `/` filtering.
- `f`/`F` pickers with `ignore` + `nucleo`, streamed results and a preview.
- *Accept*: on a `.sh` file, `?` lists "Run script" first.

**M6 — Terminal panel**
- pty + `tui-term`, key encoding, resize, `Ctrl-t` focus rules, `script.run`.
- *Accept*: `Tab` completes in the shell; `Ctrl-t` from inside hides the panel and focus returns; `x` on a script opens the panel and runs it.

**M7 — kitty and Helix**
- `kitty.rs`: `ls` lookup, send-text, focus, launch fallback. `open.editor` for files and folders.
- *Accept*: with Helix open in another kitty window launched with `--var role=helix`, `e` opens the file there and focuses it.

**M8 — Git and properties**
- Git worker and status segment. Properties overlay with the background `du` and "calculating…".
- The remaining previewers from section 5.17: PDF, video, audio, archive.
- *Accept*: inside a repo that's 2 commits ahead, the status line shows `↑2`; properties on a large folder never block input.

**M9 — Network (ssh)**
- `sftp` backend behind the `Vfs` trait, hosts in the sidebar, `ssh://host/path` display.
- Remote behaviour:
  - `e`: `kitty @ launch --type=tab kitten ssh <host> hx <path>`
  - open default: copy to `/tmp/tosk/` and `xdg-open`
  - `x`: type `ssh <host> bash '<path>'` into the panel
  - `d`: `gio trash` over ssh, or fall back to refusing with a message if `gio` is missing
  - `D`: remote `rm -r` after the confirm
  - previews of remote files: fetch the first 64 KB
- *Accept*: browse the NAS from the sidebar without freezing the UI on a slow link.

**M10 — Config and custom commands**
- `config.toml` loading, keymap overrides, custom `[commands.*]` with placeholders, validation messages.
- *Accept*: rebinding `e` to `E` in config works and the `?` menu shows `E`.

---

## 10. Theme — Catppuccin Mocha

| Role | Token | Hex |
|---|---|---|
| App background | base | `#1e1e2e` |
| Sidebar, status line, overlays | mantle | `#181825` |
| Deepest background | crust | `#11111b` |
| Borders, unfocused cursor row | surface0 | `#313244` |
| Stronger borders, selection summary box | surface1 | `#45475a` |
| Dim text, line numbers | overlay0 | `#6c7086` |
| Secondary text | subtext0 | `#a6adc8` |
| Text | text | `#cdd6f4` |
| Accent: cursor row, focused header, mode chip | lavender | `#b4befe` |
| Folders | blue | `#89b4fa` |
| Selected items, keywords | mauve | `#cba6f7` |
| Symlinks, git changed count | yellow | `#f9e2af` |
| Network hosts | teal | `#94e2d5` |
| Git branch, help section titles, fuzzy matches | peach | `#fab387` |
| Success, terminal mode chip, strings | green | `#a6e3a1` |
| Errors, permanent delete | red | `#f38ba8` |

Text on an accent background uses crust. Keep the palette in `theme.rs` as named constants so a different Catppuccin flavour is a one-file change.

---

## 11. After 1.0

- Tabs: `Ctrl-w` closes one. Opening needs a new key, since `Ctrl-t` is the terminal.
- Ripgrep search across files, like Helix `space /`.
- Undo for rename, move and trash (one step per bulk rename).
- Copy, cut and paste of files within tosk, with a progress display.
- "cd on quit": write the last folder to a file so a shell wrapper can `cd` there.

---

## 12. Open questions

1. **`Ctrl-t` inside the shell**: bash uses it to swap two characters, and fzf binds it to file search. Keep `Ctrl-t` for the terminal or move it (Ctrl plus backtick is a common terminal toggle)?
2. **Terminal and file list folder sync**: the prototype syncs both ways. The real build only starts the shell in the current folder. Following later folder changes would need OSC 7 reporting from the shell, plus a decision on whether tosk may type `cd` into it.
3. **Bulk rename syntax**: is `[01]` / `[a-b]` / `{name}` / `{ext}` enough, or should regex capture groups be supported?
4. **Arrow, Home, End and Esc bindings** (marked † in section 7): keep them?
5. **Remote trash**: `gio trash` on the remote, or just refuse `d` remotely and require `D`?
