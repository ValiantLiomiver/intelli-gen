# Project Guide

Intelli-gen is a native Rust desktop workbench for Java projects, built with `eframe`/`egui`. First release targets macOS and supports Maven and Gradle.

## Architecture

- `src/main.rs` starts the native window. `src/app/` owns UI state, renders panels, and routes `AppEvent`s through the central reducer in `actions.rs`.
- `src/project/` discovers Maven and Gradle projects and modules, and reads the project tree. Preserve the opened workspace root when handling nested modules.
- `src/editor/` owns text documents, tabs, highlighting, and asynchronous file I/O.
- `src/java/` handles JDK discovery, builds, diagnostics, Java language-server integration, and run configurations.
- `src/debugger/` owns the Java Debug Adapter Protocol transport, session, breakpoints, stack, and variable inspection.
- `src/search.rs`, `src/terminal.rs`, and `src/git.rs` implement project search, the PTY terminal, and local Git operations.
- `src/settings.rs` persists user preferences as TOML under the platform config directory.

## Implementation Rules

- Keep blocking file, process, build, language-server, debugger, search, and Git work off the UI frame. Send user intents and worker completions through app events.
- Pass process arguments separately. Never build shell command strings. Prefer Maven or Gradle wrappers when present.
- Use configured or discovered external JDK and Java tooling. Do not silently download toolchains or bundle proprietary IntelliJ code.
- Preserve filesystem paths as `PathBuf`; do not round-trip paths through strings for identity or process invocation.
- Report service and settings errors in the UI. Keep editing and unrelated services available when Git metadata or optional Java tooling is unavailable.
- Keep Java/Maven/Gradle as the supported scope. See `docs/superpowers/specs/2026-10-19-java-ide-design.md` for product scope and constraints; implementation plans live in `docs/superpowers/plans/`.

## Commands

- `cargo run` — launch the desktop app.
- `cargo check` — compile-check the project.
- `cargo fmt --check` — check Rust formatting; use `cargo fmt` to format.
- `cargo test` — run Rust tests.

Use Rust 2024 edition and keep formatting compatible with `rustfmt`.
