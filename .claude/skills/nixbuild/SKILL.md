---
name: nixbuild
description: Build and verify this project on Linux via nixbuild.net remote builder. Use when you need to run `cargo check`, `cargo build`, or `nix build` on a Linux x86_64 target — especially for GTK4/Wayland crates that can't compile on macOS.
---

# nixbuild — Remote Linux Build Verification

## Purpose

This project targets Linux (GTK4/Wayland). macOS cannot compile the `nixclip` or `nixclipd` crates due to missing system libraries. Use nixbuild.net to verify builds on real Linux x86_64 hardware.

## Account

- **Service**: nixbuild.net (25 free build hours/month)
- **Account**: adipidaparti@gmail.com
- **SSH key**: `~/.ssh/nixbuild_ed25519`
- **Auth token**: `~/.config/nixbuild/token`

## SSH Config

Already configured in `~/.ssh/config`:

```
Host eu.nixbuild.net
  PubkeyAcceptedKeyTypes ssh-ed25519
  ServerAliveInterval 60
  IPQoS throughput
  IdentityFile /Users/pidaparti/.ssh/nixbuild_ed25519
```

## Usage

### Quick build check via flake

```bash
# Build the package remotely using the flake
nix build .#nixclip \
  --max-jobs 0 \
  --builders "ssh://eu.nixbuild.net x86_64-linux - 100 1 big-parallel,benchmark" \
  --option builders-use-substitutes true \
  --option extra-experimental-features "nix-command flakes"
```

### Run cargo check/build remotely via nix develop

```bash
# Enter a remote nix develop shell and run cargo check
nix develop .# \
  --max-jobs 0 \
  --builders "ssh://eu.nixbuild.net x86_64-linux - 100 1 big-parallel,benchmark" \
  --option builders-use-substitutes true \
  --command cargo check --workspace
```

### Direct nix-build (no flakes)

```bash
nix-build \
  --max-jobs 0 \
  --builders "ssh://eu.nixbuild.net x86_64-linux - 100 1 big-parallel,benchmark" \
  --option builders-use-substitutes true
```

### Verify SSH connectivity

```bash
ssh eu.nixbuild.net nix-store --serve --write
# Should return cleanly (no output = success)
```

## Important Notes

- **Budget**: 25 hours/month free tier. Each build counts against this.
- **No local nix needed**: macOS doesn't have nix installed. This skill is for CI-like verification or for delegating to the user's NixOS machine.
- **Token expiry**: Auth token expires 2026-04-24. Regenerate at nixbuild.net/settings/auth-tokens if needed.
- **Architecture**: nixbuild.net provides x86_64-linux builders. This matches the project's target platform.
- The `--max-jobs 0` flag tells nix to send ALL builds to the remote builder instead of trying to build locally.
- `builders-use-substitutes true` lets nixbuild.net pull cached deps from cache.nixos.org instead of uploading them.
