# JavaDoc Delimiter Matching

## Intent

Extend the editor's caret delimiter outlines to the JavaDoc token pairs
registered by IntelliJ: inline tag start `{@` with `}`, and JavaDoc `[]` and
`()`. Keep the existing Java code and generic-angle matching behavior.

## Approaches

- Extend the existing delimiter scanner, which already distinguishes code,
  strings, and comments, to pair JavaDoc delimiters only inside `/** ... */`.
  This avoids a second parser pass and needs no new dependency.
- Scan text for `/**` without lexer scopes. This is shorter but misreads the
  same sequence inside strings or comments.
- Leave the known gap. This avoids code, but keeps a documented difference from
  IntelliJ's registered JavaDoc pairs.

Use the first approach because it reuses the existing stateful Java scan and
keeps matching limited to delimiters IntelliJ registers.

## Requirements and limits

- Match `{@` with `}` only inside a JavaDoc block comment.
- Match `[`/`]` and `(`/`)` inside JavaDoc block comments.
- Preserve existing matching for Java code delimiters and lexer-recognized
  generic angles; comparisons and ordinary comments remain unmatched.
- Preserve source character indices for egui cursor geometry.
- Java Unicode escapes are not preprocessed; source mapping remains out of
  scope.

## Verification

Compile and format-check the Rust application and check the diff. Automated
tests are omitted under the workspace instruction not to add or run tests unless
requested; runtime rendering is not available in the current UI session.
