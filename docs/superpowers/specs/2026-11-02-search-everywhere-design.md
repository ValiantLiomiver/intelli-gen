# Search Everywhere design

## Intent

Bring project file and Java symbol lookup into one familiar search surface,
while keeping the existing fast file index and asynchronous language-server
search.

## Evidence and current behavior

- The installed IntelliJ IDEA 2026.3 `searchEverywhereMl.jar` inventory
  (REA Evidence `ev_f18e4d50164ed9fdac40ba35e081bf6d5`) contains contributors
  named `All`, `Files`, `Classes`, `Symbols`, and `Actions`. This is package
  structure evidence; it does not establish the complete runtime behavior.
- Intelli-gen currently has separate **Go to File** and **Go to Java Symbol**
  windows. File lookup uses the background project index in `QuickOpen`; symbol
  lookup uses the Java language server in `SymbolSearch`.

## Approaches

- **One dialog with Files and Java Symbols tabs (selected):** combine the two
  existing lookup flows, with a shared query. Preserve each result source and
  its existing open/navigate behavior. Reuse current shortcuts to open the
  dialog on the corresponding tab. This adds a small amount of shared UI state
  but keeps both existing search engines.
- **One blended results list:** query file and symbol sources together and rank
  them in one list. This needs new cross-source ranking and selection behavior.
- **A command palette:** search IDE actions only. It needs a separate action
  catalog and does not address the two existing project-lookup windows.

## Scope and constraints

- Use one search dialog with a shared query and two tabs: **Files** and **Java Symbols**.
- `⇧⌘O` opens Files; **Code → Go to Java Symbol…** opens Java Symbols.
- **Navigate → Search Everywhere…** opens the same dialog on Files.
- Selecting a file opens it; selecting a symbol navigates to its source range.
- Keep file indexing and LSP requests off the UI frame; do not add dependencies.
- Do not add an All or Actions tab, blended ranking, or new search backends.
- Preserve English and Italian labels and keyboard operation.

## Builder calls

- Keep existing result engines and their async cancellation/stale-result
  protections.
- The shared query is retained when switching tabs; opening the dialog starts a
  fresh empty query, matching the current separate-window behavior.
- Add headless egui coverage for opening each tab, switching tabs, and
  dispatching file-open and symbol-navigation results.
