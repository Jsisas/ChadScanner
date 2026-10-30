# ChadOverlay

Escape from Tarkov item info as a large floating tooltip. Shift + Left-click an item in the stash; the card
shows the price, and whether to keep the item for quests, hideout or Kappa.

- Recognition: [ChadVision](../ChadVision) (icon + hover-tooltip name).
- Data: json.tarkov.dev (`items`, `tasks`, `hideout`, `traders`, `barters`, `crafts`), cached in
  `%APPDATA%\ChadOverlay\cache`, refreshed every 10 min.
- Progress: local `progress.json` (TarkovTracker backup format, importable at tarkovtracker.org) or the
  TarkovTracker API with a per-mode token.
- EFT logs: tasks started/failed/finished, flea sales and the session game mode are picked up live, and
  completions made while the overlay was closed are applied at the next start. Hideout upgrades are not in
  EFT's logs: set them in Settings (local) or on tarkovtracker.org.

## Build and deploy (WSL / Linux)

```
tools/deploy-wsl.sh            # → C:\ChadOverlay
tools/deploy-wsl.sh --out DIR
```

Needs rustup, `lld` (`lld-link`), curl and unzip; installs the Windows target and `cargo-xwin` itself.

## Running

- Run `C:\ChadOverlay\chadoverlay.exe`. It starts in the tray; the tray menu has Settings, Pause scanning
  and Quit.
- **EFT must run in borderless (windowed fullscreen) mode.** Exclusive fullscreen is not supported: the
  tooltip is a normal topmost window and is not drawn over an exclusive-fullscreen game.
- Files: `%APPDATA%\ChadOverlay\` — `config.json`, `progress.json`, `pending.json` (TarkovTracker writes
  waiting for a retry), `logstate.json` (EFT log read position), `log.txt`.

## Tests

`cargo test` runs the portable modules (data parsing, needs, progress store, log parsing, card) on Linux.
