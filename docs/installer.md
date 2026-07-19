# Cyrene Installer

`cyrene-installer` is the only component that changes installed Cyrene state. It is a Rust TUI application with scriptable subcommands and embeds the matching `cyrene-core` executable plus the Codex and pi Adapter packages.

## Build

Windows:

```powershell
.\scripts\build-release.ps1
```

Linux or macOS:

```sh
./scripts/build-release.sh
```

The build order is intentional: Core is compiled first, then its binary is embedded into the Installer through `CYRENE_CORE_ARTIFACT`. A normal development `cargo check` builds an Installer without a Core payload; that binary can run `status`, but refuses installation.

## Commands

Running without a subcommand opens the TUI. Automation can use:

```text
cyrene-installer install --adapter codex,pi
cyrene-installer update
cyrene-installer repair --adapter all
cyrene-installer status --json
cyrene-installer uninstall --adapter all
cyrene-installer uninstall --adapter all --purge-data --yes
```

The default root is `~/.Cyrene`. `--root` accepts an absolute alternate root for testing or managed installations. `CYRENE_HOME` lets an Adapter read the same alternate root.

## Installation layout

```text
~/.Cyrene/
  core/versions/<version>/
  core/current
  adapter/codex/versions/<version>/
  adapter/codex/current
  adapter/pi/versions/<version>/
  adapter/pi/current
  installer/versions/<version>/
  installer/current
  config/adapters/{codex,pi}.json
  data/
  state/install.json
  .agents/plugins/marketplace.json
```

`current` is an NTFS Junction on Windows and a symbolic link on Unix. Adapter configs point to the stable Core path. Payloads are staged, hashed with SHA-256 and promoted before `current` changes.

The administrator Token is stored in Windows Credential Manager, macOS Keychain, or Linux Secret Service. Adapter Tokens remain in owner-only config files because the Adapter configuration contract requires them. A normal uninstall keeps both memory data and its administrator credential; `--purge-data` removes both.

## Host registration

- pi is installed as a local-path package pointing at `~/.Cyrene/adapter/pi/current`.
- Codex uses the non-default local marketplace rooted at `~/.Cyrene`, then installs `cyrene-memory@cyrene-local`. Codex owns its installed cache, so update and repair reinstall the plugin after rotating the cachebuster version.

Codex users must start a new conversation after installation and trust the Cyrene `UserPromptSubmit` hook through `/hooks`.
