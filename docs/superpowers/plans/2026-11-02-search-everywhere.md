# Search Everywhere Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace separate project-file and Java-symbol lookup windows with one tabbed search dialog.

**Architecture:** Add a small `SearchEverywhere` UI state that owns visibility, selected tab, and the shared query. Keep `QuickOpen`'s file index and `SymbolSearch`'s LSP query/results as separate async engines, rendering each inside the shared dialog. Existing shortcuts and result navigation continue through their current actions.

**Tech Stack:** Rust 2024, eframe/egui, existing project index and Java language server.

**Spec:** `docs/superpowers/specs/2026-11-02-search-everywhere-design.md`

## Global Constraints

- Use one search dialog with two tabs: **Files** and **Java Symbols**.
- `⇧⌘O` opens Files; **Code → Go to Java Symbol…** opens Java Symbols.
- Double Shift and **Navigate → Search Everywhere…** open the shared dialog on Files.
- Keep file indexing and LSP requests off the UI frame.
- Do not add dependencies, an Actions tab, blended ranking, or new search backends.
- Preserve English and Italian labels and keyboard operation.

## Review Focus

- No project is open: Files displays a useful empty state; it does not start an index.
- Switch tabs: the shared query stays visible and each engine refreshes for that query.
- A stale file-index result or symbol response for an older query is ignored.
- Empty results: Enter does not select a nonexistent item.
- English and Italian labels: both tab names and the dialog title are translated.

---

### Task 1: Unified file and symbol search dialog

**Files:**
- Create: `src/app/search_everywhere.rs`
- Modify: `src/app/mod.rs`
- Modify: `src/app/events.rs`
- Modify: `src/app/actions.rs`
- Modify: `src/app/layout.rs`
- Modify: `src/app/quick_open.rs`
- Modify: `src/app/symbol_search.rs`
- Modify: `resources/i18n/en.ini`
- Modify: `resources/i18n/it.ini`
- Test: `src/app/search_everywhere.rs`
- Test: `src/app/quick_open.rs`
- Test: `src/app/symbol_search.rs`

**Interfaces:**
- `SearchTab`: `Files` or `JavaSymbols`.
- `SearchEverywhere::open(tab: SearchTab)`: open on the requested tab and clear the shared query.
- `SearchEverywhere::render(ctx: &egui::Context, files: &mut QuickOpen, symbols: &mut SymbolSearch, language: &LanguageTool) -> (Option<PathBuf>, Vec<AppEvent>, Option<Location>)`: render one dialog and return file-open and symbol-navigation outcomes for the existing app event layer.
- `QuickOpen::render_results(&mut self, ui: &mut egui::Ui, query: &mut String) -> Option<PathBuf>`: render file results using the supplied shared query.
- `SymbolSearch::render_results(&mut self, ui: &mut egui::Ui, query: &str, language: &LanguageTool) -> (Vec<AppEvent>, Option<Location>)`: render Java symbol results using the supplied shared query.

- [x] **Step 1: Add headless tests for the unified dialog.** Verify each entry point opens the requested tab, switching tabs retains and forwards the query to each engine, no-project and empty-result states render without a selection, and the title and tabs are localized in English and Italian. In the engines, verify an older file-index completion and symbol query remain ignored after the refactor.
- [x] **Step 2: Run the focused test and confirm it fails because the unified dialog does not exist.**
- [x] **Step 3: Add `SearchEverywhere` in `src/app/search_everywhere.rs`.** Own the shared query, requested tab, visibility, and query focus. Render one `egui::Window` with Files and Java Symbols tab buttons, a shared text input, Escape handling, and the selected engine's results.
- [x] **Step 4: Refactor `QuickOpen` and `SymbolSearch` to render their result lists inside the shared dialog.** Remove their separate windows and per-window query/focus state; keep file-index worker generation checks, symbol-query matching, result selection, and all existing error handling.
- [x] **Step 5: Route existing File and Code menu entries and shortcuts through `SearchEverywhereOpen(SearchTab)`.** Route **Go to File…** to Files and **Go to Java Symbol…** to Java Symbols. Preserve `OpenFile` and `OpenFileAtPosition` dispatch.
- [x] **Step 6: Add the dialog title and tab labels to both language catalogs.** `QuickOpen::render_results` shows a clear Files empty state when no project is open and does not start indexing.
- [x] **Step 7: Run `cargo fmt --check`, `cargo test`, `cargo check`, and `git diff --check`.** Confirm stale-result and empty-result behavior, all tests pass, and no new dependency was added.
- [x] **Step 8: Commit and push the finished change directly on `main`.**
