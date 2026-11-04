# Problem Navigation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans (recommended for this single-task change). Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Navigate cyclically through source-backed Java and build problems with F2 and Shift+F2.

**Architecture:** Extract the existing bounded merged-problem collection from `problems.rs`, retain the current severity-first view ordering, and sort a temporary navigation view by source location. Dispatch the existing `OpenFileAtPosition` / `OpenFileAtLine` events.

**Tech Stack:** Rust, eframe/egui, existing `AppEvent` reducer and localization catalogs.

**Spec:** `docs/superpowers/specs/2026-11-03-problem-navigation-design.md`

## Global Constraints

- Keep file and service work off the UI frame.
- Reuse existing diagnostic sources and navigation events.
- Preserve all visible strings in English and Italian.

## Review Focus

- No active editor: next/previous starts at the first/last source problem.
- Current cursor between diagnostics: navigate by path, line, and LSP UTF-16 column.
- At either end: wrap to the opposite end.
- Project-level diagnostics without a path: omit from keyboard navigation.
- Duplicate locations: distinct messages remain reachable; exact duplicate rows do not trap navigation.

---

### Task 1: Navigate Problems

**Files:**
- Modify: `src/app/events.rs`
- Modify: `src/app/actions.rs`
- Modify: `src/app/problems.rs`
- Modify: `src/app/layout.rs`
- Modify: `src/app/search_everywhere.rs`
- Modify: `resources/i18n/en.ini`
- Modify: `resources/i18n/it.ini`
- Modify: `README.md`

**Interfaces:**
- Add `AppEvent::NavigateProblem { next: bool }`.
- Add `problems::navigate(state: &mut AppState, next: bool)`.

- [x] Extract the existing capped Java/build problem collection for reuse by rendering and navigation.
- [x] Order a navigation copy by path, line, and column; wrap relative to the active editor's LSP position and open via existing file-position events.
- [x] Wire `F2`, `Shift+F2`, Navigate menu, and Search Everywhere actions; localize labels and the no-results status.
- [x] Document shortcuts and verify with `cargo fmt --check`, `git diff --check`, and `cargo check --quiet`.
- [x] Commit and push on `main`.
