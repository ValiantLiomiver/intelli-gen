# Relative Go to Line Implementation Plan

> **For agentic workers:** Implement this focused plan natively in the current session.

**Goal:** Accept IntelliJ-style `+N` and `-N` input in the existing Go to Line dialog.

**Architecture:** Extend `parse_position` to receive the current 1-based line and resolve signed offsets there. Keep the dialog and its absolute coordinate path unchanged.

**Tech Stack:** Rust 2024, egui, existing INI localization catalogs.

**Spec:** `docs/superpowers/specs/2026-11-04-relative-go-to-line-design.md`

## Global Constraints

- Preserve the existing `line` and `line:column` formats.
- Clamp relative results to the open document's line range.
- Do not add dependencies or run tests unless requested.

## Review Focus

- Large signed values must not overflow; malformed signed input must leave the dialog open.
- Relative input must use the caret line captured when the dialog opens.

---

### Task 1: Relative line parsing and discoverability

**Files:**
- Modify: `src/app/go_to_line.rs`
- Modify: `resources/i18n/en.ini`
- Modify: `resources/i18n/it.ini`

**Interfaces:**
- Extend the private `parse_position(input, current_line)` helper; the returned line remains 1-based.

- [x] Add parsing for signed decimal line offsets, resolving negative movement without integer underflow and clamping below line 1.
- [x] Pass the captured caret line into the parser and clamp the resulting line to the active document when navigating.
- [x] Add a localized hover hint describing `+N` and `-N`.
- [x] Run `cargo fmt --check`, `cargo check --quiet`, and `git diff --check`.

## Execution record

- `cargo fmt --check`: passed.
- `cargo check --quiet`: passed with existing deprecation and dead-code warnings.
- `git diff --check`: passed.
- Tests were not run, per workspace instructions.
- Fresh read-only review found no actionable issues.
