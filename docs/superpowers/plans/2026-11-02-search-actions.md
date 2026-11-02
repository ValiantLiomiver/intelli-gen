# Search Actions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Search and run Intelli-gen's supported commands from an Actions tab in Search Everywhere.

**Architecture:** Keep a small action catalog in `SearchEverywhere`, filter it against the shared query, render keyboard- and mouse-selectable results, and dispatch existing `AppEvent`s. Route `⌘⇧A` and Navigate → Find Action… to the tab.

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
- Catalog exactly: Go to File, Recent Files, Go to Class, Go to Java Symbol, Search Everywhere, Find Action, Go to Line, File Structure, Find/Replace in File, Find in Project, Go to definition/type definition/implementation, Find usages, Rename Symbol, Organize imports, Reformat code, Save, Save All, Close File, Build, Test, Clean, Run Selected Configuration, Debug Selected Configuration, and Terminal.

- [x] Add the Actions tab, localized action-result list, shared-query substring matching, arrow/Enter selection and click dispatch in `search_everywhere.rs`.
- [x] Add the `⌘⇧A` keyboard route in `actions.rs` and a Navigate → Find Action… entry in `layout.rs`.
- [x] Add matching action names, tab title, hint, empty state and shortcut translations to both catalogs.
- [x] Document the shortcut in README.
- [x] Run `cargo fmt --check`, `cargo check`, `cargo build --release`, and `git diff --check`.
- [x] Commit the implementation on `main` and push to `origin/main`.
