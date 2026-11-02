# Java auto-completion design

## Intent

Make Java editing feel more immediate by opening the existing JDT LS completion list while the user types, instead of requiring a manual command for most identifiers.

## Approaches

- Request completion after every Java identifier character. This feels immediate but can flood the language server and interrupt editing.
- Request completion after the second consecutive Java identifier character, and immediately after `.`. This is the selected approach: it starts useful member/type suggestions early while avoiding a request for every short prefix.
- Keep completion manual except after `.`. This is simple but leaves common identifier completion undiscoverable.

## Goals

- Trigger completion after two consecutive Java identifier characters and after a typed dot.
- Reuse the current JDT LS request path, completion list, selection, and apply action.
- Render valid completion results in a popup anchored at the editor caret; retain the Java tool-window list for paged large files that cannot provide a stable popup position.
- Show results only while the same Java file, document version, and caret position remain current; discard stale responses.
- Keep explicit **Complete** and Ctrl+Space available.

## Non-goals and constraints

- No new dependencies, server implementation, or custom Java parser.
- No auto-inserting or accepting a suggestion without user action.
- Do not change signature-help behavior on `(` and `,`.
- Keep language-server work asynchronous and off the UI frame.

## Implementation calls

- Detect consecutive identifier characters from the current document prefix, rather than counting editor events; this handles multi-character paste and avoids treating punctuation or whitespace as a prefix.
- Keep the existing immediate dot trigger.
- Associate completion results with their requested path, version, and caret position, and hide them after edits or caret movement.

## Acceptance

- Typing one identifier character does not trigger completion; the second consecutive identifier character does.
- A typed dot still triggers completion immediately.
- Pasted identifiers of two or more characters trigger completion.
- Results from an old document version or caret position cannot appear or be applied to the new position.
- Valid completion results appear beside the caret; selecting a row applies it, and Escape or Dismiss closes the list.
- Manual completion and signature help retain current behavior.
