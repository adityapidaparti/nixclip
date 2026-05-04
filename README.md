# NixClip

Clipboard history manager for GNOME Wayland. Captures clipboard changes through the available Wayland backend, then lets you search and restore them.

Three components:
- **nixclipd** — background daemon that watches the clipboard
- **nixclip** — CLI for scripting and quick access
- **nixclip-ui** — GTK4 popup for browsing history

## Install

### NixOS, `configuration.nix` Only

You do not need to convert your machine to flakes. Add this to
`/etc/nixos/configuration.nix`:

```nix
# configuration.nix
{ pkgs, ... }:

let
  # Pin this to a commit before using it long-term.
  nixclip-src = builtins.fetchTarball {
    url = "https://github.com/adityapidaparti/nixclip/archive/main.tar.gz";
    # Recommended pinned form:
    # url = "https://github.com/adityapidaparti/nixclip/archive/<commit-sha>.tar.gz";
    # sha256 = "...";
  };
  nixclip-pkg = pkgs.callPackage nixclip-src {};
in
{
  imports = [ (nixclip-src + "/nix/module.nix") ];

  services.nixclip = {
    enable = true;
    package = nixclip-pkg;

    settings = {
      general = {
        max_entries = 1000;
        retention = "3months";
      };
      keybind = {
        open_formatted = "Super+V";
        open_plain = "Super+Shift+V";
      };
    };
  };
}
```

Then rebuild:

```bash
sudo nixos-rebuild switch
```

This fetches the NixClip source, builds it, imports the NixOS module, and
installs all three binaries: `nixclipd`, `nixclip`, and `nixclip-ui`.

The module also:

- starts `nixclipd` as a systemd user service with the graphical session
- writes `/etc/xdg/nixclip/config.toml` from `services.nixclip.settings`
- adds GNOME dconf defaults for `Super+V` and `Super+Shift+V`
- frees `Super+V` from GNOME's default message-tray binding while keeping `Super+M`
- disables the daemon's portal shortcut retry loop because GNOME dconf owns the shortcut path

To pin to a specific version (recommended), replace `main` in the URL with a commit SHA and add the `sha256`. You can get the hash by running:

```bash
nix-prefetch-url --unpack https://github.com/adityapidaparti/nixclip/archive/<commit-sha>.tar.gz
```

After rebuilding, log out and back in if the user service or GNOME shortcuts do
not appear immediately. You can also check the service directly:

```bash
systemctl --user status nixclipd
journalctl --user -u nixclipd -f
nixclip doctor
```

If the daemon is running but `nixclip doctor` reports missing Wayland/session
environment, import the graphical session variables and restart the daemon:

```bash
systemctl --user import-environment WAYLAND_DISPLAY DISPLAY XDG_SESSION_TYPE
systemctl --user restart nixclipd
```

### NixOS With Flakes

Flakes are optional. If your NixOS config already uses flakes, add NixClip as an
input and import the module:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    nixclip.url = "github:adityapidaparti/nixclip";
  };

  outputs = { nixpkgs, nixclip, ... }: {
    nixosConfigurations.your-hostname = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        nixclip.nixosModules.default
        ({ pkgs, ... }: {
          nixpkgs.overlays = [ nixclip.overlays.default ];

          services.nixclip = {
            enable = true;
            package = pkgs.nixclip;
            settings.keybind = {
              open_formatted = "Super+V";
              open_plain = "Super+Shift+V";
            };
          };
        })
      ];
    };
  };
}
```

### Home Manager

Import the Home Manager module and configure `programs.nixclip`:

```nix
{ pkgs, ... }:

let
  nixclip-src = builtins.fetchTarball {
    url = "https://github.com/adityapidaparti/nixclip/archive/main.tar.gz";
  };
  nixclip-pkg = pkgs.callPackage nixclip-src {};
in
{
  imports = [ "${nixclip-src}/nix/hm-module.nix" ];

  programs.nixclip = {
    enable = true;
    package = nixclip-pkg;
  };
}
```

On GNOME, the Home Manager module also installs the same custom dconf keyboard
shortcuts as the NixOS module.

## Development

For a full workspace build outside Nix, you need the GTK4/libadwaita/GLib
development packages visible to `pkg-config`. If you do not already have those
system libraries installed, use `nix develop` or your distro's `-dev`/`-devel`
packages before running `cargo build`.

Useful Nix checks:

```bash
nix build .#nixclip
nix build .#checks.$(nix eval --impure --raw --expr builtins.currentSystem).nixclip-nixos-module
nix build .#checks.$(nix eval --impure --raw --expr builtins.currentSystem).nixclip-e2e-smoke
```

The smoke check starts a headless Sway compositor with Wayland data-control,
runs `nixclipd`, writes text through `wl-copy`, waits until `nixclip search`
finds the stored entry, verifies `show` and `paste --plain`, installs
compositor bindings for `Super+V` and `Super+Shift+V`, sends those keypresses
with `wtype`, and verifies that the GTK UI command launched. This exercises
real Wayland key delivery in the test compositor. It still cannot prove that a
running GNOME session loaded the dconf defaults, so real desktop testing should
still include pressing the shortcut once after rebuild.

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

Full details of a single entry: type, timestamps, source app when available, preview text.

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

Diagnostic checks: daemon connectivity, session/runtime environment, Wayland protocol support, GNOME version, portal availability, and config/storage path health. Run this first if something isn't working.

## Keybindings

### Global Shortcuts

The NixOS module installs GNOME custom shortcuts via dconf defaults. Because
GNOME stores custom shortcuts as a single `custom-keybindings` array, an
existing per-user shortcut list can still override those defaults. The Home
Manager module writes the GNOME shortcuts into the user dconf profile directly.
Both modules disable the daemon's portal-based shortcut listener so GNOME does
not keep retrying the portal when declarative GNOME bindings are in use. On
NixOS, an existing per-user shortcut list can still shadow the system dconf
defaults, in which case you may need to manage the binding manually. Outside
those modules, or on non-GNOME sessions, you may still need to configure
shortcuts manually or rely on the GlobalShortcuts portal.

If `Super+V` still opens GNOME's message tray or does nothing after rebuild,
inspect **Settings -> Keyboard -> View and Customize Shortcuts -> Custom
Shortcuts**. Remove a conflicting per-user binding or bind these commands
manually:

```text
Super+V         nixclip-ui
Super+Shift+V   nixclip-ui --plain
```

| Key | Action |
|-----|--------|
| `Super+V` | Open popup (paste with formatting) |
| `Super+Shift+V` | Open popup (paste as plain text) |

### In the Popup

| Key | Action |
|-----|--------|
| `Return` | Paste selected entry (formatting depends on how popup was opened) |
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
open_formatted = "Super+V"
open_plain = "Super+Shift+V"
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
# NixOS
services.nixclip.settings = {
  general = {
    max_entries = 2000;
    retention = "6months";
  };
  keybind = {
    open_formatted = "Super+V";
    open_plain = "Super+Shift+V";
  };
  ignore.apps = [ "org.keepassxc.KeePassXC" ];
};
```

```nix
# Home Manager
programs.nixclip.settings = {
  general = {
    max_entries = 2000;
    retention = "6months";
  };
  keybind = {
    open_formatted = "Super+V";
    open_plain = "Super+Shift+V";
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

# If the daemon cannot see the Wayland session
systemctl --user import-environment WAYLAND_DISPLAY DISPLAY XDG_SESSION_TYPE
systemctl --user restart nixclipd
```

## Requirements

- NixOS or any Linux with Nix
- GNOME on Wayland (Wayland `ext-data-control` or `wlr-data-control` protocol)
- GTK4 + libadwaita

## License

MIT
