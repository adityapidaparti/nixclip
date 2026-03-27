# NixClip

Clipboard history manager for GNOME Wayland. Captures everything you copy, lets you search and restore it.

Three components:
- **nixclipd** — background daemon that watches the clipboard
- **nixclip** — CLI for scripting and quick access
- **nixclip-ui** — GTK4 popup for browsing history

## Install

Requires a flake-based NixOS system. You will edit two files: your `flake.nix` and your `configuration.nix`.

**1.** Add the NixClip input to your `flake.nix`:

```nix
# flake.nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    nixclip.url = "github:adityapidaparti/nixclip";
    # ... your other inputs
  };

  outputs = inputs@{ self, nixpkgs, ... }: {
    nixosConfigurations.your-hostname = nixpkgs.lib.nixosSystem {
      specialArgs = { inherit inputs; };
      modules = [ ./configuration.nix ];
    };
  };
}
```

> If you already have a `flake.nix`, just add the `nixclip` line to your existing
> `inputs` block. The key part is that `inputs` must be passed to your modules
> via `specialArgs` — if you already do this, no other changes to `flake.nix` are needed.

**2.** Enable NixClip in your `configuration.nix`:

```nix
# configuration.nix
{ inputs, pkgs, ... }:
{
  imports = [ inputs.nixclip.nixosModules.default ];

  services.nixclip = {
    enable = true;
    package = inputs.nixclip.packages.${pkgs.system}.default;
  };
}
```

**3.** Rebuild:

```bash
sudo nixos-rebuild switch
```

This installs all three binaries (`nixclipd`, `nixclip`, `nixclip-ui`) and starts a systemd user service that runs automatically with your graphical session.

## Quick Start

```bash
# The daemon starts automatically via systemd. To check:
systemctl --user status nixclipd

# Copy some things, then:
nixclip list                    # see recent history
nixclip search "api key"        # full-text search
nixclip paste 42                # restore entry #42 to clipboard
nixclip paste 42 --plain        # restore as plain text (strip formatting)

# Open the popup UI:
nixclip-ui
```

## CLI Reference

### `nixclip list`

```
nixclip list [--limit N] [--type TYPE] [--json]
```

Show recent clipboard entries. Default: 10 most recent.

- `--limit N` — number of entries (default: 10)
- `--type TYPE` — filter: `text`, `richtext`, `image`, `url`, `files`
- `--json` — JSON output

### `nixclip search <QUERY>`

```
nixclip search <QUERY> [--limit N] [--type TYPE] [--json]
```

Fuzzy full-text search across all entries.

### `nixclip show <ID>`

```
nixclip show <ID> [--json]
```

Full details of a single entry: type, timestamps, source app, preview text.

### `nixclip paste <ID>`

```
nixclip paste <ID> [--plain]
```

Restore an entry to the system clipboard. `--plain` strips rich formatting.

### `nixclip pin <ID>` / `nixclip unpin <ID>`

Pin an entry to prevent it from being auto-pruned or cleared.

### `nixclip delete <ID> [<ID>...]`

Permanently delete entries.

### `nixclip clear`

```
nixclip clear [--include-pinned]
```

Clear all unpinned entries. `--include-pinned` removes everything.

### `nixclip stats`

```
nixclip stats [--json]
```

Entry counts by type, pinned count, total.

### `nixclip config`

```
nixclip config                          # show current config
nixclip config set general.max_entries 5000   # update a setting
```

Live-reloads the daemon config. Supports dotted keys.

### `nixclip doctor`

```
nixclip doctor
```

Diagnostic checks: daemon connectivity, Wayland protocol support, GNOME version, config validity, database access. Run this first if something isn't working.

## UI Keybindings

| Key | Action |
|-----|--------|
| `Return` | Restore selected entry |
| `Shift+Return` | Restore as plain text |
| `Ctrl+BackSpace` | Delete selected entry |
| `Ctrl+P` | Pin/unpin |
| `Ctrl+Shift+Delete` | Clear all unpinned |
| `Ctrl+,` | Open settings |
| `Ctrl+1` through `Ctrl+5` | Switch filter tabs |
| `Escape` | Close popup |

Type any character to start searching. The popup auto-closes when it loses focus.

## Configuration

Config lives at `~/.config/nixclip/config.toml` (or `/etc/xdg/nixclip/config.toml` for system-wide via the NixOS module).

```toml
[general]
max_entries = 1000          # max stored entries
retention = "3months"       # 7days, 30days, 3months, 6months, 1year, unlimited
max_blob_size_mb = 500      # max single entry size
ephemeral_ttl_hours = 24    # TTL for ephemeral entries

[ui]
theme = "auto"              # auto, light, dark
width = 680                 # popup width in pixels
max_visible_entries = 8     # rows before scrolling
show_source_app = true
show_content_badges = true

[keybind]
restore_original = "Return"
restore_plain = "Shift+Return"
delete = "Ctrl+BackSpace"
pin = "Ctrl+P"
clear_all = "Ctrl+Shift+Delete"

[ignore]
apps = [
  "org.keepassxc.KeePassXC",
  "com.1password.1Password",
  "com.bitwarden.desktop",
]
patterns = [
  "^sk-[a-zA-Z0-9]{48}",   # OpenAI keys
  "^ghp_[a-zA-Z0-9]{36}",  # GitHub tokens
]
respect_sensitive_hints = true
```

All settings can be changed at runtime via `nixclip config set`.

### NixOS/Home Manager settings

Pass the same structure through the `settings` option:

```nix
services.nixclip.settings = {
  general = {
    max_entries = 2000;
    retention = "6months";
  };
  ignore.apps = [ "org.keepassxc.KeePassXC" ];
};
```

## Data Storage

| What | Where |
|------|-------|
| Database | `~/.local/share/nixclip/nixclip.db` |
| Large blobs | `~/.local/share/nixclip/blobs/` |
| Config | `~/.config/nixclip/config.toml` |
| Socket | `$XDG_RUNTIME_DIR/nixclip.sock` |

## Content Types

NixClip classifies entries automatically:

- **Text** — plain text
- **RichText** — HTML, RTF, formatted content
- **Image** — PNG, JPEG, etc. (with thumbnail preview)
- **URL** — detected URLs
- **Files** — copied file paths

## Privacy

- Password manager apps are ignored by default (KeePassXC, 1Password, Bitwarden)
- Regex patterns auto-filter secrets (API keys, tokens)
- Clipboard entries marked "sensitive" by apps are respected
- Screen lock pauses capture automatically
- All data is local (SQLite + filesystem), nothing leaves your machine

## Troubleshooting

```bash
# Check if daemon is running
systemctl --user status nixclipd

# View daemon logs
journalctl --user -u nixclipd -f

# Run diagnostics
nixclip doctor

# Restart daemon
systemctl --user restart nixclipd
```

## Requirements

- NixOS or any Linux with Nix
- GNOME on Wayland (Wayland `ext-data-control` or `wlr-data-control` protocol)
- GTK4 + libadwaita

## License

MIT
