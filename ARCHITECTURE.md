# Architecture

Intelli-gen is a native macOS workbench for Java projects. It is written in
Rust 2024 and uses `eframe`/`egui` for its desktop UI. Maven and Gradle provide
project structure and builds; Java language intelligence and debugging are
provided by external Eclipse tooling.

This document describes the architecture implemented in `src/`.

## Runtime and event flow

`src/main.rs` parses the optional startup path, creates the native window, and
instantiates the application. Its `StartupApp` handles startup-specific work,
then delegates frame rendering to `IdeaLikeApp` in `src/app/`.

The app is divided between UI state, rendering, and action handling:

- `app::AppState` owns the open project, editor tabs, active panel, settings,
  navigation history, pending workflows, and service handles/loaders.
- `app/layout.rs` renders menus, panels, sidebars, dialogs, and project/editor
  surfaces.
- `app/events.rs` defines `AppEvent` and the typed action enums for builds,
  Maven, Gradle, search, terminal, Git, debugger, and run configuration actions.
- `app/actions.rs` translates UI intents into state transitions and service
  requests, and applies worker completions to app state.

The common interaction is: UI emits an event; the event handler updates app
state and starts work when needed; a service performs I/O or process work away
from the UI frame; its completion returns as an event; rendering reflects the
new state. Generation counters and request identifiers are used in asynchronous
flows to ignore stale results where the state has moved on. Services should not
hold mutable references to `AppState`.

Long-running or blocking operations—including file I/O, project discovery,
search, builds, language-server requests, debugger communication, and Git
operations—must stay off the UI frame. Keep operation errors visible in app
state and surface them in the relevant UI.

## Source map

### Application: `src/app/`

`mod.rs` defines `AppState`, `IdeaLikeApp`, active panels, navigation state,
and the pending state used by multi-step workflows. `layout.rs` is the main UI
composition layer. `actions.rs` owns event processing and keyboard shortcuts;
`events.rs` is the shared intent/completion boundary.

Focused UI modules implement supporting navigation and views:

- `quick_open.rs` opens files by name.
- `search_everywhere.rs` coordinates the Search Everywhere surface.
- `symbol_search.rs` searches Java workspace symbols.
- `go_to_line.rs` provides line and column navigation.
- `file_structure.rs` shows a file's structural outline.
- `finder.rs` reveals paths in the operating system's file manager.
- `problems.rs` renders diagnostics and build problems.

The app coordinates the services below; it is not the implementation owner of
their domain logic.

### Projects: `src/project/`

`Project` and related models describe the opened workspace and its modules.
`discovery.rs` identifies supported projects, while `kind.rs` represents
Maven/Gradle kind. `workspace.rs` loads the project and tree. `tree.rs` models
the project tree and its interactions; `tree_watch.rs` monitors filesystem
changes.

Maven support is split among `maven.rs` (project metadata and operations),
`maven_view.rs` (sidebar/task view), `maven_cache.rs` (cached project view),
and `maven_watch.rs` (POM changes). Gradle support is in `gradle.rs` and
`gradle_tasks.rs`. `editor_config.rs` resolves project editor preferences such
as `.editorconfig` settings.

The opened workspace root is the identity boundary for project operations.
Nested Maven or Gradle modules are represented within that workspace, rather
than treated as replacement workspace roots. Filesystem paths remain
`PathBuf` values for identity and process invocation.

### Editing: `src/editor/`

`document.rs` owns editable document content and document-level operations.
`tabs.rs` models editor tabs and their view state. `highlighter.rs` provides
syntax highlighting, and `find_replace.rs` implements in-file find/replace
behavior. `mod.rs` contains editor integration, gutter behavior, and
`EditorIo`, which handles asynchronous file reads, writes, and related
completions.

Workspace replacement is preview-first: search produces candidate matches,
the user selects files, and the app applies accepted edits to open buffers and
coordinates saving. Java-specific editor commands, completions, diagnostics,
and edits flow through the Java language integration.

### Java tooling: `src/java/`

- `jdk.rs` discovers and represents JDK installations and project JDK choices.
- `build.rs` runs Maven or Gradle builds and returns build results/diagnostics.
- `diagnostics.rs` represents and summarizes Java and build diagnostics.
- `lsp_config.rs` resolves JDT Language Server and Java Debug Server tooling.
- `lsp.rs` manages the Java language-server process, protocol communication,
  workspace state, and editor-facing Java features.
- `run_config.rs` stores and executes Java run configurations using the
  project's build system and selected JDK.

Java tooling is external and is discovered, bundled in release packaging, or
selected in settings. The app does not silently download Java toolchains.
The JDT LS runtime JDK is distinct from the JDK selected for a project. Builds
and editing remain available when optional language tooling is unavailable.

### Debugging: `src/debugger/`

`dap.rs` models Java Debug Adapter Protocol messages and payloads.
`session.rs` manages the debug client/service and current debug state.
`breakpoints.rs` stores and synchronizes breakpoints. `stack.rs` and
`variables.rs` represent stopped-thread stack frames, scopes, variables, and
evaluation results. The module root validates debug tooling bundles, handles
debugger actions, and renders the debugger UI.

### Supporting services

- `src/search.rs` runs project search and replacement previews asynchronously.
- `src/terminal.rs` manages the integrated PTY session and terminal UI.
- `src/git.rs` performs local repository status, diff, staging, unstaging, and
  commit operations. Missing Git metadata should not disable editing or other
  services.
- `src/settings.rs` models user preferences, project JDK selections, and
  bookmarks, and persists settings as TOML in the platform configuration
  directory. Saving is handled away from the UI frame.
- `src/i18n.rs` loads language catalogs and provides translated UI text.

## Dependency direction

The executable root (`main.rs`) wires together app, editor, project, Java,
debugger, and supporting services. The app layer coordinates these modules and
owns workflow state. Project/editor/Java/debugger modules own their respective
domain operations and return results for the app to apply. UI rendering may
call read-only presentation helpers, but blocking operations belong in service
workers and their completions cross back through app events.

Process execution uses argument lists rather than shell command strings.
Project Maven and Gradle wrappers are preferred when present; otherwise the
configured or discovered executables are used. Filesystem paths should not be
converted to strings for path identity or process arguments.

## Product boundaries

The supported project scope is Java with Maven or Gradle, and the first release
targets macOS. Intelli-gen does not bundle proprietary IntelliJ code and does
not silently install JDKs, Maven, Gradle, or Java language/debug tooling. When
optional Git metadata or Java tooling is missing, report the relevant issue and
keep unrelated editing and project services available.

## Build and validation commands

- `cargo run` launches the desktop app.
- `cargo check` compile-checks the project.
- `cargo fmt --check` checks Rust formatting.
- `cargo test` runs Rust tests.

Release packaging is performed by `scripts/package-release.sh`; pinned external
Java tooling versions are recorded in `packaging/java-tools.env`.
