# Recent Files design

## Intent

Make returning to recently used source files quick without changing the
approved Search Everywhere tabs. The current Files tab already has one shared
query; an empty query can show recent files, while typing continues to search
the full indexed project.

## Evidence

- REA Evidence `ev_08312915f357078edd36dc58816e1e9eb9c71b0f0c1ac983356206113a5483f6`
  inventories IntelliJ IDEA 2026.3's
  `platform-recentFiles-plugin` artifact
  (`intellij.platform.recentFiles.jar`, SHA-256
  `63707ffdd1952b4e4aa1ee4dff1b8c96e6aefa5260e106d622afe6f27aff0f1`). The
  package inventory includes recent-file event/model classes. Static artifact
  evidence does not establish the exact popup behavior.
- JetBrains' [predefined macOS keymap](https://www.jetbrains.com/help/idea/reference-keymap-mac-default.html)
  assigns Recent Files to `⌘E`.

## Design

- `⌘E` opens the existing Search Everywhere dialog on Files, with the same
  empty query used by `⌘P`.
- With an empty query, Files shows up to 20 most recently opened files that
  belong to the current workspace. Typing switches to the existing async file
  index and fuzzy matching.
- Update the MRU when an open file becomes active or finishes loading; keep the
  history for the running app session only.
- Selecting a recent entry reuses the existing `OpenFile` path. File indexing
  and file reads stay off the UI frame.
- Translate the recent-files heading, empty state, and shortcut label in
  `en.ini` and `it.ini`.

## Non-goals

- No separate popup, new search tab, persistent file history, recent
  locations, or new indexing backend.

## Approaches

- **Reuse Search Everywhere's Files tab (selected):** one query surface and
  existing open-file flow; blank-query results are session recents.
- **Add a third Recent Files tab:** clearer separation but duplicates the
  existing Files tab and expands the approved dialog.
- **Add a standalone popup:** matches a separate action surface but adds
  another window and selection lifecycle.

The existing Files tab is the smallest approach that preserves the approved
dialog design while adding the IntelliJ-style recent-file shortcut.
