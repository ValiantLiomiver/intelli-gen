# Search Actions design

This follow-up supersedes the earlier Search Everywhere non-goal that excluded
an Actions tab.

## Intent

Make the commands already available in Intelli-gen searchable from the shared Search Everywhere window, following IntelliJ IDEA's Actions search.

## Evidence

- REA shipped-artifact Evidence `ev_f18e4d50164ed9fdac40ba35e081bf6d5` inventories the installed IntelliJ IDEA 2026.3 Search Everywhere artifact and its `Actions` contributor. This is package structure evidence, not proof of full runtime behavior.
- JetBrains' [Search Everywhere documentation](https://www.jetbrains.com/help/idea/searching-everywhere.html) says the Actions scope finds IDE actions by name and shows available shortcuts. The installed application's native UI was unavailable in this session, so runtime behavior was not observed.
- Intelli-gen already routes commands through `AppEvent`; the shared search window currently renders Files and Java Symbols.

## Approaches

- **Actions tab in Search Everywhere (selected):** reuse the existing query and dialog; search a curated catalog of commands already exposed by Intelli-gen and dispatch their existing events. This adds no search service or dependencies.
- **Separate Find Action dialog:** duplicates the query UI and dismissal lifecycle.
- **Search all menu and tool controls dynamically:** requires a new action registry and refactoring menu handlers before it can be reliable.

## Design

- Add an **Actions** tab and open it with `⌘⇧A` and **Navigate → Find Action…**.
- Search the localized and English action names case-insensitively by substring. Show each matching command with its existing shortcut; allow arrow-key selection, Enter, and click.
- Catalog: Go to File, Recent Files, Go to Class, Go to Java Symbol, Search Everywhere, Find Action, Go to Line, File Structure, Find/Replace in File, Find in Project, Go to definition/type definition/implementation, Find usages, Quick fixes, Rename Symbol, Organize imports, Reformat code, Save, Save All, Close File, Build, Test, Clean, Run Selected Configuration, Debug Selected Configuration, and Terminal.
- Include the existing project navigation, code navigation/refactoring, file editing/search, Build/Test/Clean, and Terminal commands listed in the implementation plan. Dispatch through existing `AppEvent`s and close after selection.
- Keep the existing Files and Java Symbols flows and shared-query behavior unchanged. Translate every new visible string in `en.ini` and `it.ini`.

## Non-goals

- No semantic/synonym search, dynamic action registration, plugin actions, shortcut editing, or commands that Intelli-gen does not already support.
- No action enablement prediction; existing reducers remain responsible for ignoring commands that need a missing project/editor.

## Implementation calls

- Use case-insensitive substring matching for this first catalog; do not add another fuzzy matcher or dependency.
- Keep the catalog beside Search Everywhere, where its labels, shortcuts and existing event dispatch are used.
