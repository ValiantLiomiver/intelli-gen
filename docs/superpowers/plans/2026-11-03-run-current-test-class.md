# Run Current Test Class Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run the active conventional Java test class through the existing Maven/Gradle test pipeline.

**Architecture:** Resolve the active file path against the project model's deepest module root, derive the class name under `src/test/java`, and dispatch the existing filtered `BuildAction::RunTest` from the Run menu or gutter context menu; do not create a run configuration or another process service.

**Tech Stack:** Rust 2024, eframe/egui, existing Maven/Gradle `BuildService`, `en.ini` and `it.ini` resources.

**Spec:** `docs/superpowers/specs/2026-11-03-run-current-test-class-design.md`

## Global Constraints

- Keep Maven and Gradle as the supported build systems.
- Keep build and process work off the UI frame.
- Pass process arguments separately; preserve project paths as `PathBuf`.
- Reuse the existing project JDK and wrapper/executable selection.

## Review Focus

- Nested Maven/Gradle modules: select the deepest containing module.
- Non-test Java files and custom source roots: disable the action.
- Package path containing invalid UTF-8 or invalid Java identifiers: disable the action.
- An active build: disable the action; preserve the existing single-build guard.
- Localization: ensure the new action and hint have matching English/Italian keys.

---

### Task 1: Run the active Java test class

**Files:**
- Modify: `src/app/actions.rs`
- Modify: `src/app/layout.rs`
- Modify: `resources/i18n/en.ini`
- Modify: `resources/i18n/it.ini`
- Modify: `README.md`

**Interfaces:**
- Add `actions::current_test_class_action(path: &Path, model: &ProjectModel) -> Option<BuildAction>`.
- The returned event uses `RunTest { class_name, module_root, test_task: "test", integration: false }`.

- [x] Resolve the deepest module containing `path`, require `src/test/java`, derive and validate the fully qualified class name.
- [x] Add **Run Current Test Class** to Run and the editor gutter context menu; enable it only for the supported active test file while no build is running, then dispatch the existing action.
- [x] Add English/Italian labels and disabled hints, and document the menu action in README.
- [x] Verify with `cargo fmt --check`, `git diff --check`, and `cargo check --quiet`.
- [x] Commit the feature on `main`.
