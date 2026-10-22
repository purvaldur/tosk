# tosk

A keyboard-driven terminal file manager for kitty, Helix and Hyprland,
themed in Catppuccin Mocha. Named after Ratatosk, the squirrel who runs up
and down Yggdrasil carrying messages.

tosk browses folders with `i j k l` and previews files beside the list:
syntax-highlighted text, images through the kitty graphics protocol, PDFs,
videos, audio tags and archive listings. It opens files in the Helix you
already have running, runs scripts in a terminal panel built into the
window, shows git status VS Code-style, and browses servers over ssh.

## Requirements

- **Linux on Wayland.** The clipboard uses `wl-copy`, and opening files
  uses `xdg-open`.
- **[kitty](https://sw.kovidgoyal.net/kitty/)**, for its keyboard protocol,
  image previews and remote control. tosk runs in other terminals too,
  without images, without telling `Shift-Enter` from `Enter`, and without
  opening files in Helix.
- **A [Nerd Font](https://www.nerdfonts.com/)** for the icons. Without one,
  set `icons = false` in the config.
- **[Helix](https://helix-editor.com/)** as `hx` or `helix`, for `e`.
- **Rust 1.85 or newer** to build it.

**Optional previewers.** A missing one just shows "no previewer" for that
kind of file.

| Program | Previews | Arch package |
|---|---|---|
| `pdftoppm`, `pdfinfo` | PDF first page, page count | `poppler` |
| `ffmpegthumbnailer` | video thumbnails, album covers | `ffmpegthumbnailer` |
| `ffprobe` | duration, tags, codecs | `ffmpeg` |
| `bsdtar` | archive contents | `libarchive` |

`git` adds the git segment and file marks. `ssh` (OpenSSH) is needed to
browse servers.

## Install

```sh
git clone https://github.com/purvaldur/tosk
cd tosk
cargo install --path .
```

This puts `tosk` in `~/.cargo/bin`.

## Setup

### kitty

To let `e` open files in a running Helix, kitty needs remote control on a
socket. Add this to `kitty.conf` and restart kitty:

```conf
allow_remote_control socket-only
listen_on unix:/tmp/kitty
```

tosk finds "the editor" by a kitty user variable, `role=helix`. Helix tabs
that tosk opens get it automatically. For a Helix you start yourself, this
shell function sets the variable for the window while Helix runs:

```sh
# bash/zsh: mark the window as the editor while Helix runs.
# Arch names the binary `helix`; elsewhere it's `hx`.
hx() {
  printf '\e]1337;SetUserVar=role=%s\a' "$(printf helix | base64)"
  if command -v helix >/dev/null; then command helix "$@"; else command hx "$@"; fi
  printf '\e]1337;SetUserVar=role\a'
}
```

Without a running Helix, `e` opens one in a new kitty tab.

### Hyprland

```conf
$files = kitty --class tosk -e tosk
bind = SUPER, E, exec, $files
```

## Keys

Press `?` at any time to see the keys that apply to what's under the cursor.
In that menu, `/` filters, and pressing a listed key runs its command.

**Moving around**

| Key | Does |
|---|---|
| `i` / `k` (or `↑` / `↓`) | up / down |
| `j` (or `←`) | parent folder |
| `l` (or `→`) | enter a folder or follow a link |
| `Home` / `End` | first / last item |
| `Tab` / `Shift-Tab` | switch between the sidebar and the list |
| `Ctrl-l` | go to a path, `~`, or `ssh://host/path` |
| `Ctrl-f` | filter this folder as you type |
| `f` | find files under this folder, fuzzy |
| `F` | search file contents, like ripgrep |
| `q` | quit |

**Files**

| Key | Does |
|---|---|
| `Enter`, `o` | open with the default app (folders are entered) |
| `e` | edit in Helix: files in the running Helix, a folder as `hx .` |
| `Space` | select or unselect; the cursor stays put |
| `Esc` | clear the selection and filter |
| `r` | rename, or bulk rename with several selected |
| `n` / `N` | new file / new folder |
| `c` | copy the files, to paste in GUI file managers |
| `C` | copy the paths as text |
| `d` | move to the trash |
| `D` | delete for good, after asking |
| `x` | run a `.sh` file in the terminal panel |
| `p` | properties, with folder sizes counted in the background |
| `b` | pin the folder to the sidebar (or unpin, in the sidebar) |

**View**

| Key | Does |
|---|---|
| `Ctrl-s` | terminal panel: open and focus; again to hide |
| `P` | show or hide the preview |
| `h` | show or hide hidden files |
| `s` | sort by name, type, size or modified date |
| `S` | reverse the sort |
| `L` | show or hide the message log |

**In `f` and `F`:** `Enter` jumps to the file. `Shift-Enter` opens it in
Helix, at the matching line for `F`. In `F`, `%p` starts a fuzzy filter on
the path: `fn main %p src ui`.

**Selections:** a selection only ever holds what's visible in the current
folder. It clears when you change folders and drops anything a filter hides,
so a trash or delete never touches files you can't see.

**Bulk rename** (`r` with several selected) takes a template:

- `[01]` counts up from 01 in the order you selected the files;
- `[5-9]` starts the count at 5;
- `{name}` is the old name without its extension, and `{ext}` the extension.

So `show.s01.e[01].{ext}` numbers episodes in the order you picked them.
Names that clash are marked, and nothing is renamed until all of them are
fine.

## Git

Inside a repository, the file list's header shows the branch, commits
ahead/behind and the number of changes, e.g. `main ↑2 ↓0 ~3`.

**File marks:**
- Names are coloured: untracked and added in green, modified in yellow,
  deleted and conflicted in red.
- A letter on the right says which: `U`, `A`, `M`, `D` or `!`.
- A folder with changes inside shows `●` in the colour of the most
  important one.
- Git-ignored files are dimmed.

## Servers over ssh

tosk talks SFTP through your own `ssh`, so `~/.ssh/config`, keys, the agent
and `known_hosts` all apply. Logins must work without a password prompt (a
key or ssh-agent). The first time, run `ssh <host>` once in a terminal to
accept the server's key.

**Getting to a server:**
- Every `Host` in `~/.ssh/config` appears in the sidebar under the network
  group.
- `Ctrl-l` takes `nas:/srv/media` or `ssh://user@nas:2222/srv`.
- `b` pins a remote folder to the sidebar.

Listings load in the background, so a slow link never freezes the window.
Most keys work the same on a server, with these differences:

| Key | On a server |
|---|---|
| `e` | runs Helix on the server, in a new kitty tab |
| `o` | downloads to `/tmp/tosk/<host>/`, then opens the copy |
| `x` | runs the script with `ssh -t host bash …` in the panel |
| `d` | uses `gio trash` on the server, and refuses if it has none |
| `D` | runs `rm -r` on the server |
| `C` | copies `host:/path`, ready for `scp` or `rsync` |

`c`, `f`, `F` and custom commands only work on your own machine.

## Config

tosk reads `~/.config/tosk/config.toml`. Everything is optional. Mistakes
don't stop tosk: they're listed in a dialog at startup, and the rest of the
file still applies.

```toml
[general]
show_hidden = false
preview = true
editor_role = "helix"          # the kitty user variable that marks the editor

[sidebar]
icons = true                   # Nerd Font icons

[[sidebar.hosts]]              # more hosts for the sidebar
name = "thor@nas"              # [user@]host[:port], or a Host from ~/.ssh/config
root = "/srv/media"            # optional; the home folder when left out

# Rebind keys, or unbind them with "none". A new binding is what `?` shows.
[keys]
E = "open.editor"
e = "none"
W = "wallpaper"                # custom commands bind like built-ins

# Shell commands of your own. They're listed first in `?`.
[commands.wallpaper]
title = "Set as wallpaper"
run = "hyprctl hyprpaper wallpaper ,{file}"
where = "background"           # or "terminal": typed into the terminal panel
```

**Custom commands:**
- **Placeholders:** `{file}` (the item under the cursor), `{dir}` (the
  current folder) and `{selection}` (the selection, or the cursor item).
  Paths are shell-quoted. `{{` and `}}` give literal braces.
- **Background commands** run with `sh -c` in the current folder. The log
  shows when they're done, or the last line they printed on stderr if they
  fail.

**Command ids:** `?` then `/` filters the menu by id, which helps when
writing `[keys]`. These are all of them:

`cursor.up`, `cursor.down`, `cursor.top`, `cursor.bottom`, `nav.parent`,
`nav.enter`, `nav.goto`, `open.default`, `open.editor`, `file.copy`,
`path.copy`, `file.rename`, `file.new`, `file.new_folder`, `file.trash`,
`file.delete`, `file.props`, `select.toggle`, `select.clear`, `script.run`,
`picker.files`, `picker.grep`, `filter.start`, `term.toggle`, `focus.next`,
`focus.prev`, `shortcut.pin`, `view.preview`, `view.hidden`, `view.log`,
`view.sort`, `view.sort_reverse`, `help.open`, `app.quit`.

**Key names:**
- Single characters are case-sensitive: `c` and `C` are different keys.
- Named keys: `Enter`, `Esc`, `Tab`, `Space`, `Backspace`, `Home`, `End`,
  `Up`, `Down`, `Left`, `Right`.
- Modifiers: `C-` (Ctrl), `A-` (Alt), and `S-` (Shift, for named keys only).
  For example `C-t` or `S-Tab`.

**Files tosk keeps in `~/.config/tosk/`:**
- `shortcuts`: pinned folders, one path per line.
- `hosts`: pinned servers, one `ssh://` address per line.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The tests never touch your own files, clipboard, kitty or config:
- File operations run in temporary folders.
- kitty and ssh are replaced by stand-in scripts.
- SFTP is tested against the system's `/usr/lib/ssh/sftp-server` when
  it's installed.

## License

MIT; see `LICENSE`.

The syntax theme is [Catppuccin Mocha for bat](https://github.com/catppuccin/bat),
MIT licensed; see `assets/LICENSE`.
