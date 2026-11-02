# Search Actions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Search and run Intelli-gen's supported commands from an Actions tab in Search Everywhere.

**Architecture:** Keep a small action catalog in `SearchEverywhere`, rank localized and English names with the existing file-search fuzzy scorer, render keyboard- and mouse-selectable results, and dispatch existing `AppEvent`s. Route `⌘⇧A` and Navigate → Find Action… to the tab.

**Tech Stack:** Rust 2024, eframe/egui, existing app events and INI catalogs.

**Spec:** `docs/superpowers/specs/2026-11-02-search-actions-design.md`

## Global Constraints

- Do not add dependencies.
- Keep current Files, Java Symbols and Java Classes entry points working.
- Keep all visible new text localized in `en.ini` and `it.ini`.
- Dispatch only existing `AppEvent`s; do not duplicate command execution logic.

## Review Focus

- Empty and unmatched queries keep the window usable and show localized feedback; verify with `cargo check`.
- Query changes reset keyboard selection; selecting with Enter or click dispatches one event and closes the dialog; verify by code-path review.
- `⌘⇧A` opens Actions while ordinary Shift+A does not; verify by code-path review.
- Localized titles and English action names both remain searchable; verify by code-path review.
- Partial names rank by match quality and preserve the existing catalog order for an empty query; verify by code-path review.
- One character edit or adjacent-letter transposition can match one action, and curated English/Italian aliases match only their mapped action; verify by code-path review.

---

### Task 1: Add searchable supported actions

**Files:**
- Modify: `src/app/search_everywhere.rs`
- Modify: `src/app/actions.rs`
- Modify: `src/app/layout.rs`
- Modify: `resources/i18n/en.ini`
- Modify: `resources/i18n/it.ini`
- Modify: `README.md`

**Interfaces:**
- Add `SearchTab::Actions`.
- Keep the existing render result tuple; the Actions tab returns its selected command as one `AppEvent`.
- Reuse `SearchEverywhereOpen(SearchTab::Actions)` for both menu and keyboard routing.
- Catalog exactly: Go to File, Recent Files, Go to Class, Go to Java Symbol, Search Everywhere, Find Action, Go to Line, File Structure, Find/Replace in File, Find in Project, Go to definition/type definition/implementation, Find usages, Quick fixes, Rename Symbol, Organize imports, Reformat code, Save, Save All, Close File, Build, Test, Clean, Run Selected Configuration, Debug Selected Configuration, and Terminal.

- [x] Add the Actions tab, localized action-result list, shared-query fuzzy matching, arrow/Enter selection and click dispatch in `search_everywhere.rs`.
- [x] Add the `⌘⇧A` keyboard route in `actions.rs` and a Navigate → Find Action… entry in `layout.rs`.
- [x] Add matching action names, tab title, hint, empty state and shortcut translations to both catalogs.
- [x] Document the shortcut in README.
- [x] Run `cargo fmt --check`, `cargo check`, `cargo build --release`, and `git diff --check`.

### Task 2: Rank fuzzy action matches

**Files:**
- Modify: `src/app/quick_open.rs`
- Modify: `src/app/search_everywhere.rs`

**Interfaces:**
- Expose `quick_open::fuzzy_score` to sibling app modules with `pub(super)`.
- Search localized and English action labels, normalize spaces to underscores, and sort nonempty-query results by descending score.

- [x] Reuse the file-search scorer for action labels, keeping English and Italian labels searchable.
- [x] Preserve catalog order for an empty query and reset selection when the query changes.
- [x] Verify with `cargo fmt --check`, `cargo check`, and `git diff --check`.
- [x] Commit the matcher update to `main` and push to `origin/main`.

### Task 3: Add bounded aliases and typo tolerance

**Files:**
- Modify: `src/app/search_everywhere.rs`

**Interfaces:**
- Match localized and English action labels, then curated aliases, with `action_search_score`.
- If subsequence matching fails, allow at most one single-character edit or adjacent transposition across all query words; keep typo-only scores below fuzzy matches.

- [x] Add explicit English and Italian aliases for declaration navigation, references, quick fixes, formatting, build, tests, save all, and terminal.
- [x] Add one-edit matching for query words of at least four characters.
- [x] Treat one adjacent-letter transposition as a single typo.
- [x] Verify with `cargo fmt --check`, `cargo check`, and `git diff --check`.
- [x] Commit the alias update to `main` and push to `origin/main`.
