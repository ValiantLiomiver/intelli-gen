# Terminal Tool Window Shortcut Implementation Plan

> **For agentic workers:** Implement this focused plan natively in the current session.

**Goal:** Open and close the Terminal tool window with `⌥F12`.

**Architecture:** Route the shortcut and View menu item through existing panel and terminal events. Start a PTY only when opening the tool window with an open project and no current terminal session.

**Tech Stack:** Rust 2024, egui, existing INI catalogs.

**Spec:** `docs/superpowers/specs/2026-11-05-terminal-tool-window-shortcut-design.md`

## Constraints

- Keep terminal process work in the existing terminal worker.
- Localize the View action in English and Italian.
- Do not add or run tests unless requested.

---

### Task 1: Route and document the shortcut

**Files:**
- Modify: `src/app/actions.rs`
- Modify: `src/app/layout.rs`
- Modify: `src/app/search_everywhere.rs`
- Modify: `resources/i18n/en.ini`
- Modify: `resources/i18n/it.ini`
- Modify: `README.md`

- [x] Add the Option+F12 toggle, View menu action, and conditional terminal open.
- [x] Document the shortcut and localize the menu label.
- [x] Run `cargo fmt --check`, `cargo check --quiet`, and `git diff --check`.
- [x] Commit and push on `main`.

## Follow-up

Route the existing Find Action Terminal entry through the same idempotent open
event, show the Terminal panel, and preserve any active session.

Route the View menu and keyboard shortcut through one toggle helper so both
hide an active Terminal panel without closing its PTY.

## Execution record

- `cargo fmt --check` passed.
- `cargo check --quiet` passed with existing warnings.
- `git diff --check` passed.
- No tests were added or run, per workspace instructions.
