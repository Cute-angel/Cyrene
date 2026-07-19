# Cyrene Adapters

Cyrene v1 ships separate adapters for Codex and pi-agent:

- `codex/cyrene-memory`: Codex plugin with a task hook, MCP tools and memory skills.
- `pi`: pi-agent package with a TypeScript extension and prompt templates.

Both adapters call the versioned Core HTTP RPC protocol. They never read the Cyrene SQLite database directly and they do not share credentials.

## Configuration

The Installer must create one user-only JSON file per Adapter:

```json
{
  "core_url": "http://127.0.0.1:46371",
  "core_path": "C:\\Users\\user\\.Cyrene\\core\\current\\cyrene-core.exe",
  "data_dir": "C:\\Users\\user\\.Cyrene\\data",
  "actor_id": "01900000-0000-7000-8000-000000000000",
  "token": "cyr_...",
  "protocol_version": "1.0"
}
```

Locations:

- Codex: `~/.Cyrene/config/adapters/codex.json`
- pi-agent: `~/.Cyrene/config/adapters/pi.json`

`CYRENE_HOME` overrides `~/.Cyrene` for managed or test installations.

Linux config directories must be mode `0700` and files mode `0600`. Windows installers must restrict the DACL to the current user. Adapter code reads but never creates or rewrites these files.

## Development checks

```powershell
Set-Location src\adapter\codex\cyrene-memory
npm test

Set-Location ..\..\pi
npm test
npm run typecheck
```

The Codex plugin should also be checked with the Codex `plugin-creator` validator before packaging.
