# Java Auto-Completion Implementation Plan

> **For agentic workers:** Use the executing-plans workflow to implement this plan task-by-task.

**Goal:** Trigger Java completion after two identifier characters or immediately after a dot, and discard results made stale by edits or caret movement.

**Architecture:** Detect triggers from the current Java document and caret in the editor. Keep JDT LS asynchronous and attach the requested document identity, version, and caret position to its result; only show or apply a result while those values still match.

**Tech Stack:** Rust 2024, eframe/egui, existing JDT LS client.

**Spec:** `docs/superpowers/specs/2026-10-28-java-auto-completion-design.md`

## Global Constraints

- Keep blocking language-server work off the UI frame.
- Use the existing JDT LS completion path and do not add dependencies.
- Preserve explicit Complete/Ctrl+Space and signature help on `(` and `,`.
- Never show or apply results for a different file, document version, or caret position.

## Review Focus

- Pasted multi-character identifiers trigger once from the resulting document prefix.
- A single identifier character does not trigger completion.
- Dot completion remains immediate.
- Caret movement, document edits, switching files, and delayed responses invalidate old results.
- Manual completion and signature-help behavior remain available.

---

### Task 1: Trigger and validate Java completion

**Files:**
- Modify: `src/editor/tabs.rs`
- Modify: `src/java/lsp.rs`

**Interfaces:**
- Use `Document::lsp_position()` and `Document::completion_prefix_range()` for caret/prefix data.
- Keep `LanguageCommand::Request` and `LanguageEvent::Completion` asynchronous.
- Carry the requested position with completion state for UI validity and application checks.

- [x] Detect two consecutive Java identifier characters from text before the caret; trigger on typed or pasted trailing `.`.
- [x] Capture completion position and document version with each completion result.
- [x] Before rendering or applying results, verify active path, version, and current caret.
- [x] Preserve Complete/Ctrl+Space and signature-help triggers.
- [x] Run `cargo check --quiet`, `cargo test`, and `git diff --check`.
- [x] Update the README's Java editing summary.
- [x] Commit with `fix(editor): trigger completion after pasted dot`.

### Task 2: Show completion beside the caret

**Files:**
- Modify: `src/editor/tabs.rs`
- Modify: `src/java/lsp.rs`
- Modify: `README.md`

- [x] Anchor valid completion results to the visible editor caret and preserve stale-result checks.
- [x] Keep the Java tool-window completion list for paged large files.
- [x] Verify with formatting, diff, compile, and release-build checks.

### Task 3: Anchor hover documentation to the caret

- [x] Retain the request version and position with hover results, and show matching results beside the caret.
- [x] Keep the Java tool-window fallback for paged large files.
