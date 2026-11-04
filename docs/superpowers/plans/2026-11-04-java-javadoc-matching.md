# JavaDoc Delimiter Matching Implementation Plan

> **For agentic workers:** Implement this focused plan natively in the current session.

**Goal:** Highlight matching IntelliJ-style JavaDoc inline-tag, square-bracket, and parenthesis delimiters at the editor caret.

**Architecture:** Extend the existing stateful delimiter scan to recognize real JavaDoc block comments and match their registered tokens. Keep the Syntect angle-token scan restricted to caret positions adjacent to `<` or `>`.

**Tech Stack:** Rust 2024, Syntect, egui.

**Spec:** `docs/superpowers/specs/2026-11-04-java-javadoc-matching-design.md`

## Global Constraints

- Keep matching and painting in `src/editor/highlighter.rs`.
- Do not add dependencies or preprocess Java Unicode escapes.
- Preserve existing code-delimiter and generic-angle behavior.
- Do not add or run tests unless requested.

## Review Focus

- A `/**` sequence in a string or ordinary comment must not start JavaDoc token matching.
- Nested inline tags and nested `[]`/`()`: only matching token types pair.
- Unmatched JavaDoc delimiters must not match Java code delimiters outside the comment.
- Character indices must remain correct across newlines and Unicode text.

---

### Task 1: Tokenize and match JavaDoc delimiters

**Files:**
- Modify: `src/editor/highlighter.rs`
- Modify: `README.md`

**Interfaces:**
- The matcher returns both source character indices and the opening token
  width so `{@` is outlined as one token.

- [x] Detect JavaDoc comments only from code state and pair `{@`/`}`, `[]`, and
  `()` using a comment-local stack.
- [x] Keep generic-angle matching scoped to lexer-recognized Java tokens and
  outline the complete `{@` opening token.
- [x] Update the README's delimiter list to include JavaDoc inline tags and
  brackets.
- [x] Run `cargo fmt --check`, `cargo check --quiet`, and `git diff --check`;
  do not run tests.
- [x] Commit and push on `main`.

## Execution record

- IntelliJ bytecode inspection confirmed the JavaDoc pairs `{@`/`}`, `[]`, and
  `()` in `JavaBraceMatcher`.
- Syntect scope probe confirmed its Java grammar labels JavaDoc content only as
  a generic block comment; the existing lexical state machine was extended so
  string and ordinary-comment text cannot be mistaken for JavaDoc.
- `cargo fmt --check`, `cargo check --quiet`, and `git diff --check` passed.
- Tests and UI runtime rendering were not run.
