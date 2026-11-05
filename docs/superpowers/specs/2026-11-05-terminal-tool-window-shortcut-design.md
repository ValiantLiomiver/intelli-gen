# Terminal Tool Window Shortcut

## Intent

Make the integrated terminal quick to reach with IntelliJ's `⌥F12` shortcut,
and expose the same action in View. IntelliJ's current documentation assigns
`Alt+F12` to showing or closing its Terminal tool window.

## Approach

Reuse Intelli-gen's existing `Terminal(Open)` event. It selects and shows the
tool window, then starts a
session only when a project is open and no session exists. The shortcut, View
menu, and Find Action therefore share one open path and preserve an existing
terminal session. No new terminal lifecycle or process code is needed.

## Limits

- Hiding the tool window keeps the PTY and its output alive; **Close session**
  and project/app shutdown still terminate it.
- The shortcut is fixed to the macOS Option+F12 keymap used by this workbench.

JetBrains documents that Terminal tabs and sessions are retained when a project
or IntelliJ closes: <https://www.jetbrains.com/help/idea/terminal-emulator.html>.
Intelli-gen currently ends its session on project/app shutdown; this change
matches the hide/show behavior without adding cross-project session persistence.

## Verification

Run formatting, compile check, and diff validation. Do not add or run tests under
the workspace instruction; native UI verification is unavailable in this
session.
