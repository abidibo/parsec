# Parsec — design notes

A personal GNOME launcher. Opinionated about my workflow rather than a ulauncher clone.

Target: GNOME 46+, Wayland.

## Decisions (2026-10-09)

- **Stack**: Rust + GTK4 + libadwaita.
- **Process model**: resident daemon owns the index and the window. The window is
  toggled over D-Bus (`org.abidibo.Parsec` / `Toggle`). Cold start happens once at login.
- **Hotkey on Wayland**: v1 uses a GNOME custom keybinding running `parsec toggle`
  (a thin client that D-Bus-activates the daemon). v2 adds a GNOME Shell extension,
  which also unlocks the window switcher.
- **Extensibility**: two layers with one contract.
  - Core providers: Rust `Provider` trait impls compiled into the binary.
  - External plugins: any executable speaking a line-based JSON protocol over
    stdin/stdout, wrapped by a `BridgeProvider`. A plugin can be rewritten in Rust
    later without UI changes.

## v1 scope

1. **Apps + frecency**: scan `.desktop` files, fuzzy match, rank by frequency and recency
   of what I actually pick.
2. **Dev verbs**
   - project jumper: index `~/Dev/**` git repos, open in VS Code or terminal
   - `gh <repo>` / `pr`: GitHub repo and open PRs via `gh` CLI
   - `$ <cmd>`: shell runner; Enter opens it in a terminal that stays open,
     Tab switches to run-in-background or copy. No live preview while typing:
     that would execute half-typed commands. A preview needs an explicit trigger
     (backlog).
3. **Clipboard history + snippets**: `cb` verb, searchable history, pinned
   snippets never expire, Enter copies back (Wayland has no safe paste
   injection without a Shell extension). Watcher: `wl-paste --watch` where the
   compositor has data-control, polling fallback elsewhere (GNOME 46/47).

## Backlog (v2+)

- Shell extension: window thumbnails. The hotkey stays a GNOME custom
  shortcut on purpose: one way to manage it is enough.
- Image clipboard entries
- Calculator, unit/currency conversion
- Result actions on Tab (open folder, copy path, KDE Connect send, open in editor)
- Verbs: `define`, `tz`, `color`, `uuid`, `pw`, `b64`, `json`
- Emoji / Unicode picker with recents
- Opt-in natural-language actions via a local model or Claude, behind a prefix

## Status

- 2026-10-09: apps + frecency, projects, shell, github repos and PRs done.
  Tab cycles row actions. `parsec query` for provider debugging. Clipboard next.
- 2026-10-09: config reworked for portability: detection + commented file +
  hot reload. Preferences window planned after clipboard.
- 2026-10-09: clipboard history + snippets done. Verbs configurable. Callback
  actions let providers mutate their own state (pin, delete). v1 complete.
- 2026-10-09: dark panel UI, live user stylesheet, settings window
  (libadwaita, writes the config file), system provider (settings, quit).
  Clipboard fallback moved to xclip: wl-paste one-shot steals focus.
- 2026-10-09: KeePass provider. New core pieces it needed: `Prompt` actions
  (the search box becomes an input field, provider gets the text), `CopySecret`
  (kept out of history, auto-cleared), a secrets registry the clipboard watcher
  consults. Auto-lock, no disk writes, no logging of entry data.

- 2026-10-09: custom shortcuts (ulauncher-style keywords: URL or script,
  default search, run without args) with an editor in settings. Providers can
  own several keywords and offer fallback suggestions when nothing matched.

- 2026-10-09: plugins. JSON-lines protocol over stdin/stdout, one process per
  plugin, 3 s timeout with restart, manifest in plugin.toml, install from zip,
  folder or git (settings page and `parsec plugin`), enable/disable in config,
  host rescans on directory change. Sample: examples/plugins/calc (Python).
  This closes the "external plugins" half of the extensibility decision.

- 2026-10-09: files (Tracker + plocate, merged, fuzzy on name, recency
  boost), SSH hosts, Docker containers, systemd services; light theme and
  GNOME accent sync (47+) with a theme CSS provider between base and user.

- 2026-10-10: GNOME Shell extension, optional. `data/extension` is a JS
  extension exporting `org.abidibo.Parsec.Shell` (ListWindows,
  ActivateWindow, CloseWindow, Paste, GetClipboard, ClipboardChanged);
  `gnome_shell.rs` is the client, a per-process singleton with an
  `is_active()` every caller checks. New: windows provider (mixed into plain
  queries, `win` verb), `Paste` and `PasteSecret` action kinds (clipboard entries, KeePass, plugins),
  clipboard watcher driven by the extension when present. The extension is
  embedded in the binary and written out by `parsec extension install` or the
  settings row; a logout loads it. Without it nothing changes.

- 2026-10-10: drill-down. `ActionKind::Browse` carries a title and a lazy
  loader returning items; the engine returns `Outcome::Browse`, the window
  keeps a stack of levels (chip = level title, entry = filter, Backspace or
  Esc pop). Items inside can browse again, so trees come for free. First
  users: projects (branches, commits, files) and the plugin protocol
  (`browse` action → `browse` message → `results`). Backlog entries that
  are now one lazy loader each: docker details, KeePass groups, folder
  navigation from file results.

- 2026-10-10: Alt+1…9 runs the nth result (badges appear while Alt is
  held). `sys` verb: GSettings toggles read live, nmcli/bluetoothctl/wpctl
  toggles probed off-thread, session commands via gnome-session-quit (it
  confirms), screenshot via the Shell's D-Bus, settings panels as a
  drill-down. `ps` verb: own processes from /proc, lifetime CPU share, RSS,
  SIGTERM on Enter. The hotkey question is closed; see 2026-10-10 above.

- 2026-10-10: shell history under `$`. Parsers for zsh (extended and
  plain, backslash continuations), bash (`#ts` lines) and fish (YAML-ish
  blocks); files detected from $HISTFILE and defaults, merged with Parsec's
  own run log (written on every run, 0600). No file monitor: the index
  stats its sources on each query and reparses only when an mtime moved;
  the daemon pre-warms it at startup. Frecency ids are hashed.

- 2026-10-13: ports under `ps :`. Built into the processes provider rather
  than a plugin: same /proc scan, same actions. Listening sockets come from
  /proc/net/tcp{,6} and are matched by inode against /proc/[pid]/fd links;
  unmatched ones are named by the Docker container publishing the port
  (`docker ps`, only on `ps :` queries, cached 5 s, shared helpers in
  infra.rs) or else by the socket's uid. Mapping root's sockets to a PID
  needs root; we don't ask for it. Adds ~20 ms to the off-thread scan,
  still cached for 2 s.

- 2026-10-16: browser history and bookmarks under `h`; bookmarks also in
  plain queries at half score, so they rank below apps. One browser at a
  time (`auto` follows the https handler) from a table of installs: two
  families, Chromium (`History` + `Bookmarks` JSON) and Firefox
  (`places.sqlite`). Read via rusqlite (bundled) from a copy, with its
  journal or WAL, in `$XDG_RUNTIME_DIR`, deleted after. Reads run in the
  background and queries serve the last copy, so a 36 MB History never
  costs a keystroke; re-read on mtime change, at most once a minute.
  Opens with the browser's own desktop entry. Frecency ids are hashed.

- 2026-10-16: Quit and Force quit on running apps (Tab). Extension 0.2.0
  adds `RunningApps` (desktop id → window pids, from Shell.AppSystem) and
  `QuitApp` (Shell.App.request_quit, the dash's Quit). The apps provider
  caches the running list for 1 s, only to decide which rows get the
  actions; Force quit asks for the pids again at the moment it runs, so a
  stale pid is never killed. SIGKILL is sent by Parsec, not the extension.
  An older extension makes `RunningApps` fail: no actions, nothing else
  changes. Enter on a running app still launches; `win` switches.

- 2026-10-16: Update button (launcher entry, settings card,
  notification button via `app.update`). Runs install.sh in the configured
  terminal pinned with PARSEC_VERSION, not in the background: the script
  may need sudo to build from source, and it kills the daemon that would
  otherwise own its output. Offered only when current_exe is
  ~/.local/bin/parsec, so it never overwrites an install the script
  doesn't manage. Testable with a `PARSEC_VERSION=1.0.0` build.

- 2026-10-16: `qr` verb. Native, not a plugin: it needs a large image in
  the launcher, which the plugin protocol can't carry. New `Icon::Picture`
  (a gdk::Texture) makes a row draw the picture large with title and
  actions underneath; plugins could map an image field to it later.
  qrcode crate without default features (no `image`); pixels drawn by
  hand at an even module size aiming at 480 px, shown at 240, so HiDPI
  is sharp and 1x halves cleanly. Clipboard secrets are never drawn.
  `qr wifi [name]`: the standard WIFI: payload from nmcli (`-e no -s -g`,
  off-thread, ~40 ms); the password is copied only as a secret and never
  logged or written to disk unless you save the image.

- 2026-10-16: slow plugins. A `browse` action with `"slow": true` gets
  120 s and may stream `progress` lines (each replaces the level's items)
  before `results`. Cancel is kill-and-restart, not a protocol message, so
  plugins stay single-threaded: Level owns the load's JoinHandle; leaving
  the level or hiding aborts it, and a guard in the plugin host stops the
  process (not a failure). Items gained `body` (plugin `text`): wrapped
  multi-line text under the title. Queries keep the 3 s limit.

- 2026-10-16: plugin settings. `~/.config/parsec/plugins/<id>.toml`
  overrides `[config]` key by key, read in read_manifest, so it survives
  reinstalls (which replace the plugin folder). The host's rebuild stamp
  includes the settings files' mtimes and a changed config restarts the
  plugin, so `init` carries the new values. Settings › Plugins has an edit
  button that seeds the file from the plugin's `[config]` text, comments
  included.

- 2026-10-16: `ip` verb, native. Interfaces classified from sysfs, never
  by name: `device` = real card (`wireless`/`phy80211` = Wi-Fi), `tun_flags`
  or DEVTYPE=wireguard = VPN, everything else hidden. iproute2 JSON for
  addresses and `route get` toward a public probe address, which also says
  whether a VPN carries all traffic. Public address from api.ipify.org /
  api6.ipify.org, only under the verb, cached 5 min per route, off with
  `[ip] public_lookup`.

- 2026-10-16: `img` verb, native, on gdk-pixbuf (no new dependency): input
  is the clipboard's FileList from Files, or a copied path. Steps parsed in
  typed order (crop ratio/exact, trim by corner-colour bounding box,
  rotate, flip, percent/width/height/fit), optional format. Pixbuf isn't
  Send, so load-process-save happens entirely in one spawn_blocking; only
  paths and bytes cross. Output next to the original with a step suffix,
  never overwriting; single image can go to the clipboard instead (wl-copy
  with the format's mime). EXIF orientation applied on load; JPEG flattens
  alpha onto white. Image data on the clipboard (no file) is read as a
  texture, PNG-encoded off-thread, and goes back to the clipboard by
  default. The copied images are cached until the clipboard's `changed`
  signal or the verb is typed afresh, so typing steps stays instant.

- 2026-10-16: own scopes for launched programs. GIO's launch and our
  spawns left children in Parsec's own scope (systemd-oomd would kill
  them with it, their memory was billed to Parsec, cgroups couldn't say
  which app is which). `core/scope.rs` does what GNOME Shell does: the
  launch context's `launched` signal, or the spawned child's pid, goes to
  systemd's `StartTransientUnit` as `app-parsec-<app id>-<pid>.scope` in
  `app.slice` (freedesktop naming, so portals read the app id). Async on
  the main loop, failures only logged. A process that exits before the
  move leaves an empty scope systemd never collects; one still empty after
  2 s is stopped (empty, so nothing can be killed). D-Bus-activated apps
  report no pid and keep the unit the bus gave them.

- 2026-10-16: pipes. Ctrl+Enter commits the selected row as a stage: a
  chip before the verb chip, the box empties, the next verb gets the
  stage's output. Interactive, not typed: `|` stays the shell's and each
  stage uses the row you chose. What flows is `Item::output`, or derived
  from the first copy/paste/open action (file URIs as paths); rows whose
  Enter is a secret and secrets' text never flow. Only verbs get input:
  providers whose `takes_input` says yes (shell, plugins with `input =
  true`) get it as `Query::input`; the rest get `engine::compose` (`{}`
  replaced, else appended). Plain queries ignore it, so a 1 MB diff never
  reaches plocate. `$` is the one stage with an effect: its output is
  `Output::Later`, run at commit (this is the "explicit trigger" from v1)
  with the input on stdin or in `{}` (inline-quoted up to 32 KiB, else a
  0600 file in `$XDG_RUNTIME_DIR/parsec`), captured up to 1 MiB in 120 s,
  killed when the stage is cancelled (Esc, hide) via a drop guard. Its
  terminal and background runs read the input from that file, the command
  grouped (`{ …\n}`, fish `begin; …; end`) so a pipeline's first command
  gets stdin. Backspace undoes a stage back to the search that found it
  (for a drilled-into row, the search that opened the list).
  `parsec query --input` exercises stages from a shell. New plugin action
  `query` (put text in the search box) for the voice plugin.

- 2026-10-16: Quick Look. `Item::preview`: a file, a ready `Sheet`
  (picture/icon/swatch, text, facts) or a lazy one. The pane is a
  revealer beside the results; the window is always wide enough for it
  (a non-resizable window takes its content's width, so a fixed-width box
  holds the centred panel) and the input region is trimmed to the panel,
  wide only while the pane shows, so it opens symmetrically and clicks
  beside the panel reach the desktop. Files are inspected in
  spawn_blocking: images scaled by gdk-pixbuf to 720 px PNG bytes, else
  GIO's thumbnail path, else 200 lines of text/plain; textures made on
  the main thread. Sheets cached per item id until the next show; a
  render doesn't churn the selection, so the pane stays put while typing.
  Users: files, project files, projects (README, branch, last commit,
  changes, lazily), clipboard (text, CSS colour swatch), plugin
  `preview`, pipe input.

- 2026-10-20: Markdown in Quick Look. `Sheet::markdown`; `ui/markdown.rs`
  turns pulldown-cmark events into Pango markup for the same label (links
  via GtkLabel's `<a>`, opened by its default handler; the launcher hides
  on focus loss). Images and HTML dropped (README badges), relative links
  shown as text, tables flattened to a line per row: Pango has no tables
  and the pane is 360 px. Set for `.md`/`.markdown` READMEs and
  `text/markdown` files; not yet in the plugin protocol.

## Non-negotiables

- Portable. Nothing machine-specific in code: editor, terminal and project
  folders are detected on first run, written to the config file, and editable.
  Someone on another distro with other tools gets a working launcher unchanged.

- Keypress to results under 50 ms.
- Never block the UI on network. Network-backed providers are async and prefixed.
- Plugins are one file, hot reloadable, language-agnostic.
