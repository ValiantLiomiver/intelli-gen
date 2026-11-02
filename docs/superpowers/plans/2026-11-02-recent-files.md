# Recent Files Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show the current workspace's most recently opened files in the Files tab when its shared query is empty, and open that tab with `⌘E`.

**Architecture:** Keep a 20-path MRU in `AppState`, update it on file activation/load, and pass it into `QuickOpen::render_results`. The Files renderer uses recents only for an empty query and retains the current indexed fuzzy search for nonempty queries.

**Tech Stack:** Rust 2024, eframe/egui, existing app events and file-index worker.

**Spec:** `docs/superpowers/specs/2026-11-02-recent-files-design.md`

## Global Constraints

- Do not add dependencies or a third Search Everywhere tab.
- Keep file reads and indexing off the UI frame.
- Keep paths as `PathBuf`; cap the session history at 20 entries.
- Translate all new visible UI text in English and Italian.

## Review Focus

- Empty query with no recent paths shows a localized empty state and Enter opens nothing.
- Recent paths from another workspace do not appear in the current workspace.
- Duplicate opens move a path to the top without duplicating it.
- Nonempty queries continue to use the existing asynchronous index.
- `⌘E` is consumed by the IDE and opens the Files tab.

---

### Task 1: Recent file navigation

**Files:**
- Modify: `src/app/mod.rs`
- Modify: `src/app/actions.rs`
- Modify: `src/app/quick_open.rs`
- Modify: `src/app/search_everywhere.rs`
- Modify: `src/editor/tabs.rs`
- Modify: `resources/i18n/en.ini`
- Modify: `resources/i18n/it.ini`
- Modify: `README.md`
- Test: `src/app/actions.rs`, `src/app/quick_open.rs`

**Interfaces:**
- `AppState::remember_recent_file(&mut self, path: PathBuf)` moves a path to MRU position 0, removes duplicates, and truncates to 20.
- `QuickOpen::render_results(&mut self, ui: &mut egui::Ui, query: &mut String, recent_files: &[PathBuf]) -> Option<PathBuf>` renders in-project recents for an empty query and the current index results otherwise.

- [x] Add tests for MRU order, duplicate promotion, 20-entry bound, workspace filtering, typed-query behavior, and empty-list selection.
- [x] Add a shortcut-routing test confirming `⌘E` opens Files and plain `E` does not.
- [x] Run focused tests before implementation and confirm they fail for the missing MRU/shortcut behavior.
- [x] Implement the app-level MRU and record successful file loads, existing-file opens, navigation, and editor-tab activation.
- [x] Keep MRU selection stable during index completion, replace renamed paths, and update history for every tab activation route.
- [x] Render the recent list and localized heading/empty state only when the shared query is empty; preserve existing indexed search for typed queries.
- [x] Verify typed queries still select asynchronous index results.
- [x] Route `⌘E` to `SearchEverywhereOpen(Files)`, add the localized File menu item, and list `⌘E` in README.
- [x] Run `cargo fmt --check`, focused recent-file/shortcut/search tests (10 passed), `cargo check`, and `git diff --check`; the full suite remains unrun.
