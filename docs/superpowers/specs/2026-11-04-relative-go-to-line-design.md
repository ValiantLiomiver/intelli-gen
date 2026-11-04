# Relative Go to Line Design

## Intent

Match IntelliJ's Go to Line dialog by accepting signed relative line counts in
addition to the existing absolute `line` and `line:column` forms.

## Approaches

- Extend the existing input parser and use the current caret line; chosen because
  it preserves the one-dialog workflow and needs no new state outside its
  current line.
- Add separate relative-navigation actions; rejected because the dialog already
  owns navigation syntax and a second path would duplicate behavior.

## Behavior

- `+N` and `-N` move N lines from the current line, keeping the current column
  behavior at the first column.
- Clamp navigation to the first line and let the editor clamp targets past EOF
  to the last line.
- Keep absolute coordinates and the existing English/Italian dialog unchanged;
  a localized hover hint documents relative syntax.

## Evidence and scope

REA extraction of IntelliJ `intellij.platform.ide.impl.jar` is recorded in
README Evidence `ev_e8f021237bb481a73e72c6835709b13d7a6e0e0a9b401fbfc2dd260f80fd19a`.
Bytecode for `EditorGotoLineNumberDialog` contains the signed-integer pattern
`[+-]N` and computes the destination from the current logical line.
