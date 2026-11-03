# Problem Navigation design

## Intent and evidence

Let developers move from one reported Java or build problem to the next from
the keyboard. IntelliJ IDEA's macOS keymap assigns `F2` / `⇧F2` to next and
previous highlighted errors ([keymap](https://www.jetbrains.com/help/idea/reference-keymap-mac-default.html)). Intelli-gen already combines Java language-server and build diagnostics in the Problems tool window and opens each at its source location.

## Approaches

- **Navigate the existing Problems list (selected):** reuse the same bounded,
  merged diagnostics and existing open-at-location events. This avoids a
  second diagnostic index and keeps Build and Java behavior consistent.
- Add editor-only error state: this would omit build diagnostics already
  visible in the Problems window and duplicate location tracking.
- Add a new diagnostic service: unnecessary because both sources are already
  available in `AppState`.

## Scope

- `F2` selects the next path-backed Java or build diagnostic; `⇧F2` selects
  the previous one. Navigation follows file path, line, and column and wraps at
  the ends.
- Open the selected source location through the existing app events.
- If no path-backed problems exist, report that in the status area.
- Add Navigate-menu and Search Everywhere action entries, with Italian and
  English labels.
- Keep the Problems window's existing severity-first display order unchanged.

## Limits

- Project-level build diagnostics without a source path cannot be navigated to.
- If several diagnostics share a location but have different messages, each is
  reachable; exact duplicate rows are treated as one location.
