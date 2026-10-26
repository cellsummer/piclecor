# Piclecor

macOS photo picker built on GPUI Kit. View photos on a card or folder, select, copy to local or SSH destinations, or back up a whole folder.

## Run

```sh
cargo run --release
```

The binary sets its own Dock icon, so `cargo run` shows it too. As a standalone app (drag the result to `/Applications`):

```sh
scripts/bundle.sh --install   # builds Piclecor.app and copies it to /Applications
scripts/make-icon.sh     # only after editing assets/icon.svg
```

## Use

- **Open folder…** or **Card: NAME** (mounted volumes with `DCIM`) loads photos recursively.
- The toolbar sort button cycles Newest first, Oldest first, Name (file modified date).
- Click toggles selection. Shift-click selects a range. Cmd-A selects all.
- Double-click opens preview: ←/→ browse, Space (or Enter, S) toggles selection, Esc closes. The top bar shows the selection state.
- The toolbar theme button cycles System, Light, Dark.
- **Copy N** sends selected files flat into the destination.
- **Back up folder** sends every new file in the open folder, keeping its tree.

Destinations are local paths (`/Volumes/Disk/Photos`, `~/Pictures/In`) or rsync remotes (`user@host:/photos`). Type paths as plain text, without escapes or quotes: `nas:/media/My Photos/Photos`. The app escapes remote paths itself.

The app adds a destination only when its folder exists. Each destination shows whether its folder is found, checked on launch and after each transfer.

## Rules

- Transfers use `rsync -a --ignore-existing` without `--delete`. The destination never loses or overwrites a file.
- SSH runs in batch mode. Set up key authentication first (`ssh-copy-id user@host`).
- The destination folder must exist. The app never creates it, so an unmounted disk cannot turn into a stray folder under `/Volumes`.
- Flat copy skips a file when the destination already has one with the same name.

## Files

- Config: `~/Library/Application Support/piclecor/config.json`
- Thumbnail cache: `~/Library/Caches/piclecor`
- Thumbnails come from `sips`, so HEIC and camera RAW work.
