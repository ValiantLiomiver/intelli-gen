# Terminal Tool Window Shortcut

## Intent

Make the integrated terminal quick to reach with IntelliJ's `⌥F12` shortcut,
and expose the same action in View. IntelliJ's current documentation assigns
`Alt+F12` to showing or closing its Terminal tool window.

## Approach

Reuse Intelli-gen's existing `ShowPanel(Terminal)` and `Terminal(Open)` events.
The key binding toggles the tool window; choosing View → Terminal shows it and
starts a session only when a project is open and no session exists. No new
terminal lifecycle or process code is needed.

## Limits

- Hiding the tool window follows current settings behavior and closes the PTY.
- The shortcut is fixed to the macOS Option+F12 keymap used by this workbench.

## Verification

Run formatting, compile check, and diff validation. Do not add or run tests under
the workspace instruction; native UI verification is unavailable in this
session.
