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

- Window switcher merged into search (needs Shell extension)
- Shell extension also replaces clipboard polling on GNOME and enables paste
- Image clipboard entries
- Calculator, unit/currency conversion
- Files via plocate with preview
- Result actions on Tab (open folder, copy path, KDE Connect send, open in editor)
- Verbs: `define`, `tz`, `color`, `uuid`, `pw`, `b64`, `json`
- Emoji / Unicode picker with recents
- SSH hosts, Docker containers, systemd units as targets with actions
- Browser bookmarks and history
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

## Non-negotiables

- Portable. Nothing machine-specific in code: editor, terminal and project
  folders are detected on first run, written to the config file, and editable.
  Someone on another distro with other tools gets a working launcher unchanged.

- Keypress to results under 50 ms.
- Never block the UI on network. Network-backed providers are async and prefixed.
- Plugins are one file, hot reloadable, language-agnostic.
