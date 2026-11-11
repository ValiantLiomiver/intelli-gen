# Intelli-gen

<p align="center">
  <img src="resources/branding/app-icon.png" width="128" alt="Intelli-gen app icon">
</p>

![Intelli-gen startup splash screen](resources/branding/splash.png)

Intelli-gen is a native macOS workbench for Java projects, built with Rust, `eframe`, and `egui`. It supports Maven and Gradle projects, with Java language and debugging features provided by external Eclipse tooling.

## Run from source

Install Rust with `rustup`, then from this directory run:

```sh
cargo run
```

Pass a project directory to open it on startup, or a file path to open its containing Maven/Gradle workspace and the file:

```sh
cargo run -- /path/to/project-or-file
```

`cargo check` compile-checks the app. Java, Maven, and Gradle are not downloaded by these commands. When available, project Maven and Gradle wrappers are preferred for builds; otherwise configure installed executables under **Settings**. Before running a project wrapper, Intelli-gen asks for permission because it may download the build tool; approval lasts for that project wrapper until the IDE exits.

## Java setup

1. Open **Settings** and discover or add installed JDKs.
2. Choose a JDK 25 or newer as the **JDT Language Server runtime** for the bundled server. A custom launcher accepts Java 21 or newer. This runtime starts the language server and is separate from the project's JDK.
3. Open **Project Structure** and choose the JDK for the project. Intelli-gen preserves build-file compiler targets and toolchains.
4. Open a Maven or Gradle project. Java completion opens beside the caret after two identifier characters or immediately after a dot; F1 documentation appears at the caret. Java symbols use semantic colors from JDT LS when available, with lexical syntax colors as fallback. Supported Java snippets expand, synchronize repeated placeholders as you type, and let Tab move through fields. Navigation, diagnostics, and debugging require the language server and Java Debug Server plugin. Lombok-generated members are supported when the project declares Lombok and its JAR is cached. If a successful build resolves the JAR after project open, Intelli-gen automatically restarts Java intelligence; dependencies resolved outside the IDE still need a manual restart.

Release packaging places pinned Eclipse JDT Language Server and Microsoft Java Debug Server tooling inside the app bundle's resources. For a source build, use **Settings** to select installed tooling when it is not bundled beside the executable. The launcher and debug plugin can each be overridden independently. Intelli-gen does not install Java or other toolchains silently.

To build the macOS release package, run `scripts/package-release.sh`. This explicitly downloads the pinned upstream Java tooling, verifies the JDT LS archive checksum, and requires an already installed Java 21+ JDK. Review `packaging/java-tools.env` for the pinned versions. The resulting bundle's JDT LS runtime requires Java 25+.
The package contains `dist/Intelli-gen.app` for Finder and a `dist/intelli-gen` command-line link. Launch the app with `open dist/Intelli-gen.app`.
Application branding assets are in `resources/branding/`: `app-icon.png` supplies the app window icon, `app-icon.icns` is installed in the macOS app bundle, and `splash.png` is shown while the IDE starts.

The command-line link accepts the same optional project or file path: `dist/intelli-gen /path/to/project-or-file`.

## Shortcuts

| Action | Shortcut |
| --- | --- |
| Open project / file | File → Open Project… / Open File… |
| Show Project sidebar and navigate its tree | `⌘1`, then `↑` / `↓`, `←` / `→`, `Enter` |
| Go to class | `⌘O` |
| Go to file | `⇧⌘O` |
| Search Everywhere | `Double Shift`; also Navigate → Search Everywhere… |
| Find IDE action | `⌘⇧A`; also Navigate → Find Action… |
| Next / previous editor tab | `⌘⇧]` / `⌘⇧[` (`Ctrl+Tab` aliases) |
| Recent files | `⌘E` |
| Find / replace in file | `⌘F` / `⌘R` |
| Next / previous match in file | `⌘G` / `⌘⇧G` |
| Find in project | `⇧⌘F` |
| Go to line or line and column | `⌘L` or click the current line/column in the status bar; enter `line` or `line:column` |
| Next / previous Java or build problem | `F2` / `⇧F2` |
| Open Problems tool window | `⌘6` |
| Navigate back / forward | `⌘[` / `⌘]` (`Ctrl+Alt+←` / `Ctrl+Alt+→` aliases) |
| Last edit location | `⇧⌘⌫` |
| Toggle line bookmark / show bookmarks | `F3` / `⌘F3` |
| Jump to last tool window / hide all tool windows | `F12` / `⌘⇧F12` |
| Toggle Terminal tool window | `⌥F12` |
| Reformat Java file | `⌥⌘L` |
| Organize Java imports | `⌃⌥O` |
| Go to Java symbol | `⌘⌥O` |
| Switch between a Java test and its production source | `⌘⇧T` |
| File structure | `⌘F12` |
| Complete / parameter info / hover | `Ctrl+Space` / `⌘P` / `F1` (`Ctrl+Shift+Space` alias for parameter info) |
| Go to declaration / definition / find usages | `⌘B` / Code menu / `⌥F7` |
| Rename Java symbol | `Shift+F6` |
| Toggle Java line comment | `⌘/` |
| Indent Java code / selected lines; navigate snippet fields | `Tab` / `⇧Tab` |
| Java quick fixes | `⌥↩` |
| Go to type definition | `⇧⌘B` |
| Go to implementation | `⌥⌘B` |
| Toggle line breakpoint | `⌘F8` |
| View breakpoints | `⌘⇧F8` |
| Run current test method/class (otherwise selected configuration) / debug selected configuration | `⌃R` / `⌃D` |
| Debug: continue / step into / over / out | `⌘⌥R` / `F7` / `F8` / `⇧F8` |
| Evaluate debugger expression | `⌥F8` |
| Save / save all | `⌘S` / `⇧⌘S` |

With the Project sidebar focused, use `↑` / `↓` to move through visible entries,
`←` / `→` to collapse or expand folders, `Enter` to open files, or type a name
prefix to search project files and reveal the best match in the tree. Backspace
edits the query; Escape clears it.

Project-tree file names stay on one line and truncate when the sidebar is narrow;
hover to see the full path. Maven modules use a package icon and bold label.
In Java files with JDT LS folding support, use the gutter chevron to collapse or
expand code regions. Moving the caret into a collapsed region expands it.

## Project scope

The Run Console accepts line input for interactive Java processes. The Build panel summarizes Maven Surefire and Gradle test totals and lists failures from fresh Surefire, Failsafe, and Gradle XML reports; select a failure to open its conventional Java test source and stack-trace line when available, or use **Run class**/**Run method** to rerun it. Method reruns use Maven Surefire/Failsafe selectors or Gradle test filters and are shown for plain Java method names; Maven method filtering depends on the configured test provider. The **Problems** tool window combines available Java language-server diagnostics with the latest build diagnostics; selecting a result opens its source location.

The first release targets macOS and Java projects using Maven or Gradle. Use **File → New Java Project…** to create a minimal Maven or Gradle starter locally; it creates no wrapper and downloads no tools. The starter includes a run configuration for `Main`; select an installed JDK before launching. **Run** builds the project first and asks before saving open project files with unsaved changes; a failed build does not launch the program. Start, stop, or manage configurations from the **Run** menu. Use a project-tree directory's context menu to create a Java class; an optional package name creates matching subdirectories beneath the selected directory. Set Java indentation to 1–8 spaces or tabs in **Settings**; the editor and Java formatter use the same preference, closing braces align to their matching block, and Enter after `(` or `[` indents continuation lines while splitting empty pairs onto separate lines. A matching `.editorconfig` `continuation_indent_size` overrides the continuation width; otherwise `indent_size` applies. Enter between `{}` creates an indented inner line. In Java files, matching parentheses, braces, square brackets, and generic angle brackets are outlined at the caret; JavaDoc inline-tag and bracket pairs are recognized in documentation comments. Parent, root, and nested `.editorconfig` files override these choices by file pattern up to the nearest `root=true`. Files up to 16 MiB are editable; files above 256 KiB use a bounded page editor that keeps the complete document available for save and find/replace. **Find in Project** also supports preview-first replacement: review and select matching files before applying edits to editor buffers, then save those files to keep the changes. Run configurations can select a Maven/Gradle module or detect one from the main class; the JPMS module name remains a separate launch option. The app also includes an integrated terminal, local Git operations, and a Java debugger. Java language services depend on installed or packaged JDT LS and Java Debug Server tooling; editing and builds remain available if optional Java tooling is unavailable.

With a Java test open under a test source root imported by JDT LS, use **Run → Run Current Test Class** or choose **Run Current Test Class** from the editor gutter menu. Gradle source-set scope selects the test task. Place the caret on a directly annotated JUnit/TestNG test method or a JUnit 3 `test…` method to run it through the project's Maven or Gradle test task; the same method action is available from its declaration line in the gutter.

Use **Navigate → Show Bookmarks** (`⌘F3`) to revisit or remove project line bookmarks; **F3** toggles a bookmark at the caret. Bookmarks are saved with Intelli-gen settings.
Bookmarks below an inserted or removed line move with the edit; replacing a selection across multiple lines may require repositioning affected bookmarks.

**Navigate → Test / production source** (`⌘⇧T`) switches between matching files in JDT LS imported test and production source roots when the counterpart exists. The conventional `src/main/java` and `src/test/java` layout remains the fallback until Java intelligence returns project metadata.

## IntelliJ reverse-engineering evidence

The local reference is IntelliJ IDEA Ultimate 2026.3, currently installed as
build `IU-263.7307.112` (2026.3 RC). Run REA with Node 26 when reproducing these
observations. In this session, REA's artifact inventory
Evidence `ev_197a48a21bd285a492c45770de0061946531c998fabe998cb972ae20483b9711`
identified the current `.app` bundle with SHA-256
`8f081ef0a900148af899b8cd1e5a3a9960b1b6e6a5a829894ff4381027d0baf5`; the
completed inventory substep is Evidence
`ev_c50e47dfca8462105506064cad1c8225b59d23e7a38e664582da97305281ddc6`.
Its `product-info.json` declares 86 modules and 164 bundled plugins. This is
package-structure evidence only; no runtime behavior is inferred from it.

REA extracted IntelliJ's `intellij.platform.ide.impl.jar` (Evidence
`ev_e8f021237bb481a73e72c6835709b13d7a6e0e0a9b401fbfc2dd260f80fd19a`,
SHA-256 `20d266471eb1767256487ee1413edea8e39b678af315317d5049266bdd0e07f8`).
The archive contains `GotoLineAction`, `EditorGotoLineNumberDialog`, and
`GotoLineNumberDialog`. Bytecode inspection shows the dialog labels the input
with the line/column syntax, initializes it from the caret as `line:column`,
maps valid coordinates to an editor logical position, centers scrolling on the
caret, and leaves the dialog open for invalid input. This is shipped-bytecode
evidence, not a separate runtime observation. Intelli-gen's Go to Line dialog
uses a persistent syntax label, starts with the current caret position selected,
and scrolls the requested caret into view.

REA also inventoried the installed `intellij.platform.projectView.jar`
(Evidence `ev_bbbbad1a5f12027ca29087b9d874e62ee05ac0d9a5155933b753e4f85572531c`,
SHA-256 `3228a8dbe1db28ff8dce684d38ceaa9e66109cde0f8348b30ea3d2d353d5937e`).
This confirms the Project View module is shipped. JetBrains documents arrow-key
navigation for its tree views in the [Project tool window guide](https://www.jetbrains.com/help/idea/project-tool-window.html);
the package inventory itself does not establish that runtime behavior.

REA inventoried IntelliJ's `intellij.java.execution.jar` (Evidence
`ev_c97068a5ea5c5d058757347c42db60745655b13349168fc2c8ffe52ec8555393`,
SHA-256 `333c8562d13b4243703d8a972a679c7f11046402b3279a636572c4bde6f38560`).
This confirms the Java execution module is shipped. JetBrains documents running
Java tests from editor gutters in [Performing tests](https://www.jetbrains.com/help/idea/performing-tests.html);
the package inventory itself does not establish that runtime behavior.

On 2026-11-09, REA successfully analyzed the IntelliJ launcher with Ghidra
12.1.4 (Evidence
`ev_1c72833e4e7877bfdf75f14de3b68893e8818feb9cf7420c01a6b75fb005eb2a`,
SHA-256 `2f9ff57814cc03916bcecca2b05122534793214d4243f7e565e4735c8e1b0785`).
The overview reports 2,578 identified procedures and 3,541 defined strings;
these inventories may be incomplete and do not establish Java IDE behavior.
This succeeded after an earlier environment blocked Ghidra's child process.

REA inventoried `intellij.java.frontback.impl.jar` from this IntelliJ build
(Evidence `ev_0e742228001339686e30c3744db137582b30a1e4b386bdb2b6b9e03e99d23ae5`,
SHA-256 `4ed9b0d1abce05031a1521e5ba594f5be706adeac49b483d867f2f0e8d6e72ce`).
Bytecode inspection of `JavaBraceMatcher` shows registered pairs for `()`, `{}`,
`[]`, generic angle brackets, and JavaDoc inline-tag/bracket tokens; the angle
matcher has a specialized implementation. This establishes shipped matcher
registration, not all runtime highlighting rules. Intelli-gen uses the existing
Java lexer scopes for generic angles and a comment-aware scan for JavaDoc tokens.

REA inspected IntelliJ's `intellij.platform.lang.impl.jar` (Evidence
`ev_5353d4cc54ff2e377ee24aa577368f8157e17bfbde6386794d295940a334c1f3`,
SHA-256 `50fe3a0bcd177d65c881aec2eef76e8e0710cdd3a963d9b6f8fd923e374d2164`; exact
delegate entries `ev_02db0e34b32ad28e4f6a71dcda3dd49ba58a9ec525035637a068ea3ee86dadc8`).
Bytecode shows `EnterBetweenBracesDelegate` recognizes `()` and `{}`, its
`EnterBetweenBracesAndBracketsDelegate` adds `[]`, and the handler delegates line
indentation to `CodeStyleManager`. This is shipped-bytecode evidence, not a
separate runtime observation.

The focused Java logpoint and Gradle module observations below were first
recorded on build `IU-263.7307.44`. The current installed logpoint JAR has the
same SHA-256, so that artifact evidence remains applicable; the Gradle evidence
remains tied to build `.44`. Run REA with Node 26 when reproducing these
observations.
In this session, the default Node 20 invocation reported an integrity mismatch;
Node 26 successfully inspected the same logpoint JAR with its previously recorded
digest. Those failed invocations do not establish that the installed JAR changed.

```sh
node --version
rea inspect-artifact "/Applications/IntelliJ IDEA.app/Contents/plugins/java/lib/modules/intellij.debugger.logpoints.java.jar"
rea inspect-artifact "/Applications/IntelliJ IDEA.app/Contents/plugins/gradle-plugin/lib/gradle-plugin.jar"
rea extract-artifact "/Applications/IntelliJ IDEA.app/Contents/plugins/gradle-plugin/lib/gradle-plugin.jar"
```

The Java logpoint inspection is evidence
`ev_09ad380733826f3594a075bfd8d4c9758161201d866158b24808ec238e038e21`,
with SHA-256 `9743442b241193d070c7916dc19867bf1ba4125deff5bb53facde4a591d462b0`.
The Gradle inspection is evidence
`ev_20efa51dedcc6166cb3e0be40bd89d1cbad8bc639ccb5627c8d4ac78920a81bf`,
with SHA-256 `b9c8e1b2186d302bca9b0b7fe7b7070c2b18a7521d39e4a2f1d48968ef7ecce8`.
Gradle extraction evidence
`ev_4a4a5832b6e2241e69b3c07c4bf7de1892c285fb86ddb7f03b65f45220ec4aef`
records verified containment and no integrity contradictions.

REA inventory of IntelliJ's `intellij.java.bookmarks.jar` is evidence
`ev_3b2f91dfc3a8a903954ccfb3cdd1b4e881efb70bdd0064ed07938d1913cc14ed`
(SHA-256 `b8b1131204022062a9ef5ae1a1212c9cc221b848902838c6b95f0937f590ff1f`).
This confirms the shipped bookmark module only; the bookmark shortcuts and
workflow are corroborated by the official [macOS keymap](https://www.jetbrains.com/help/idea/reference-keymap-mac-default.html)
and [bookmark guide](https://www.jetbrains.com/guide/java/tutorials/presenting/bookmarks/).

The extracted `META-INF/plugin.xml` registers `Gradle.ExecuteTask`,
`Gradle.ToggleOfflineAction`, and `Gradle.RefreshDependencies`, including toolbar
or menu references. This proves the shipped action registrations; it does not
establish their runtime behavior. Intelli-gen Settings now provides **Gradle offline mode** for CLI builds, tests,
builds before Run, and task reports. It passes `--offline` so Gradle uses cached
dependencies; missing cached dependencies cause Gradle to fail. It also configures the JDT LS Gradle importer through
`java.import.gradle.offline.enabled`. Use **Apply / restart Java intelligence**
after changing the preference to update an existing language session. A configured
Gradle executable in a standard distribution (`bin/gradle` or `bin/gradle.bat`,
with its launcher JAR under `lib`) supplies the importer's Gradle home. Without
one, automatic Gradle import is disabled and the IDE shows a warning. The language
server does not use the project wrapper; user-initiated builds still prefer it.
Restart Java intelligence after changing that executable. After loading tasks, **Refresh dependencies** beside a `:dependencies` task runs
that report with `--refresh-dependencies`, using the selected JDK and the normal
Build output/cancellation flow. The action requires online mode and may contact
repositories or download dependencies. No JetBrains implementation code is included in Intelli-gen.

For the no-installation regression smoke test, point `INTELLI_GEN_DAP_PROJECT` at a
disposable Gradle project, `GRADLE_USER_HOME` at an empty disposable directory,
and set `INTELLI_GEN_JAVA_HOME` and `INTELLI_GEN_JAVA_RESOURCES` as described below.
Run `cargo test live_gradle_import_without_local_gradle_does_not_install_distribution -- --ignored`;
the test starts JDT LS and checks that no wrapper distribution is installed.

## Java tooling runtime verification

On 2026-11-02, REA observed the packaged native window through macOS
Accessibility after opening the app with Launch Services (Evidence
`ev_1133777feb23fd52749d57ab74c83c22e0362a933d1e7e3c59a0c53a7db829ac`,
executable SHA-256 `d8bbac0cc84205d33a16526b77d625d23c3059b78c95f7c7b22c8ca574c188c3`).
The untruncated tree showed the opened Maven project and Italian menus/panels.
This exposed untranslated run-selector and project-opening text, now covered by
regression tests and the two language catalogs. Screen capture was disabled
because the host lacked macOS Screen Recording permission; this observation
does not establish visual layout or the full edit/build/debug widget workflow.

A local protocol smoke test on 2026-10-30 started the packaged JDT LS 1.62.0
with the existing IntelliJ JBR 25.0.4.1 runtime, a temporary workspace, and a
writable copy of the packaged macOS configuration. It loaded the packaged Java
Debug Server 0.53.2 plugin, completed LSP `initialize`, executed
`vscode.java.startDebugSession`, connected to its loopback port, completed DAP
`initialize`, and shut down JDT LS through LSP `shutdown` / `exit`.

The same test inside the restricted agent sandbox initialized LSP but returned
`-1` from `startDebugSession`; running it with permission to open local sockets
succeeded.

An extended test compiled a temporary Java program with debug information and
registered it as an Eclipse Java project named `smoke`. JDT LS returned its runtime
classpath. DAP launched the program with that classpath and `projectName`, verified
a source breakpoint at line 4, and stopped there. Stack and scope requests exposed
the local `value = 40`; evaluating `value + 2` returned `42`. Step over stopped at
line 5 with `value = 42`. Continue, program termination, disconnect, and LSP
shutdown all completed successfully. An earlier fixture without `projectName`
could inspect locals but could not evaluate expressions; Intelli-gen's launch
request already supplies that field.

These observations verify the installed tools through direct protocol requests.
They do not verify Intelli-gen's Rust worker integration, Maven/Gradle project
import, or native UI interactions. The tests did not download tooling or change
the IDE's configured JDK.

The optional Rust test `live_java_debug_transport` also passed against that live
adapter. It exercises the production `DapConnection` / `DapClient` transport,
including concurrent launch/configuration requests, breakpoint verification,
stack/scopes/locals, expression evaluation, step over, continue, termination,
and disconnect. The app's session reducer and native widgets remain unverified.

To run this test, start JDT LS with the debug plugin and import an Eclipse Java
project named `smoke`. Compile the following `Main.java` into its `bin/` directory
with `javac -g -d bin Main.java`; preserve these source line numbers:

```java
public class Main {
  public static void main(String[] args) {
    int value = 40;
    value += 2;
    System.out.println(value);
  }
}
```

Execute `vscode.java.startDebugSession` through LSP, then set
`INTELLI_GEN_DAP_PORT` to the returned loopback port,
`INTELLI_GEN_DAP_PROJECT` to the absolute project directory, and
`INTELLI_GEN_DAP_JAVA` to the absolute Java executable. With JDT LS still running,
execute `cargo test live_java_debug_transport -- --ignored`. Local socket
permissions are required. The test launches and terminates this fixture program.

When runtime classpaths omit `projectName`, Intelli-gen asks the installed Java
Debug Server for `vscode.java.resolveMainClass` and matches both the main class
and its source path. This preserves the server's logical project name when it
differs from the source directory name, which is important for expression
evaluation in remapped Gradle modules. Existing name inference remains available
when the server returns no matching main class.

The optional `live_worker_resolves_server_project_name` test passed on
2026-11-02 using the packaged app built from commit `4f6256c`. It exercised the
production `JavaLanguageService`, `ClasspathResolver`, and `DebugService`: Maven
import, project-name resolution, breakpoint and expression evaluation, step over,
step into/out, conditional logpoint output, cancellation, and worker cleanup.
The server resolved `smoke` rather than the deliberately incorrect model name
`directory-fallback`.

It also passed on 2026-11-09 against the current Rust worker and packaged Java
tools with an Eclipse fixture and JDK 25, including all three debug sessions.
The worker test was repeated successfully against commit `09627ce`.
On 2026-11-10 it passed against commit `0f9caa4` with a fresh Eclipse fixture,
GraalVM 23 as the project JDK, and JBR 25 for the language server and debug
launch. This repeats the worker-level breakpoint, evaluation, stepping,
logpoint, cancellation, and cleanup checks; it is not a native UI test.
On 2026-11-11 the same live worker test passed with JDT LS 1.61.0 and Java Debug
Server 0.53.2 from local VS Code extensions, using Java 25 and a fresh Eclipse
fixture. It additionally asserted that the live language worker requests an egui
repaint during initialization. External network and writes to the shared Maven and Gradle caches
were denied; loopback was permitted for the debug adapter.
On 2026-11-09, a native accessibility smoke check launched the then-current debug
build and opened a disposable Gradle project from the recent-project list. The
project status, tree, and window title confirmed the result. A separate native
smoke check opened a disposable Java file, edited it through accessibility,
saved it from the File menu, and verified the resulting bytes on disk. A native
debug smoke check created a run configuration, stopped at a source breakpoint,
displayed the main stack frame, evaluated `1 + 2` as `3`, continued, and
observed `43` in the console. A second native session stepped into `add` at
line 8, out to line 4, and over to line 5 before continuing to output `42`.
A further session selected GraalVM 23 in the run configuration while the
project default was Corretto 8; evaluation inside the stopped debuggee returned
`"23"` for `java.specification.version`.
The native Run flow also built the Gradle 6.7 fixture with the project JDK 8,
then launched it with the configuration's JDK 23 and displayed `42`.
With an edited file, Cancel kept the buffer dirty and disk unchanged; Save and
Run saved the file, built it, and displayed the updated output `77`.
A separate native Maven smoke check built a disposable project offline through
its wrapper, confirmed `Main.class` on disk, then saved an intentional compiler
error and navigated from the Build diagnostic to line 3. JDT LS reported the
file as outside an imported Java project until Maven refresh was requested;
after refresh it reported the undefined method as a semantic error. The
Maven sidebar also loaded its profiles, lifecycle, and plugin sections.
A native unsaved-file check confirmed that Cancel retains the dirty buffer,
Save and Close writes the disposable file, and Discard closes its tab without
changing disk contents. The Go to Line dialog also accepted `3:3` and moved the
caret there. These modal buttons expose non-empty bounds to macOS accessibility.
A native save-conflict check confirmed that an external change present before
Save remains on disk, the editor retains its unsaved buffer, and the error is
reported without leaving the status as “Saving”.
On 2026-11-10, a native MCP accessibility check on `0f9caa4` edited and saved a
disposable Java file, built its Maven reactor from the workspace root, and
followed an intentionally failing test report to line 5 of `MainTest.java`.
The resulting class printed the edited value `73`. The same check exposed an
unnamed project-tree disclosure control; after the fix, its localized expand
and collapse buttons both worked through macOS accessibility.
On 2026-11-11, a native MCP accessibility check on `426bd52` used a disposable
Maven project with a local marker-only wrapper. Cancelling the localized wrapper
dialog left the marker absent; allowing it ran the wrapper, and a second task ran
without another prompt in the same session. The fixture performed no download.
A second disposable Maven wrapper check confirmed that editing its POM did not
run the wrapper before consent, but did start a reload after consent. Its wrapper
deliberately failed; the Maven error stayed visible and a later POM edit did not
start repeated retries. Java HTTP(S) proxy settings pointed to a closed local port.
On 2026-11-11, a separate native check used the installed Maven 3.9.9 offline:
after approval, editing the POM added a profile to the Maven panel automatically.
A wrapper that rewrote a watched file triggered one follow-up, then stopped with
a stale warning. A second check confirmed that a POM edit was processed and its
new profile was ready while the IDE window was minimized. These runs denied
network access and writes to the shared Maven cache and Intelli-gen settings/cache.
An idle-process spot check on the same macOS host compared a visible disposable
Maven workspace before and after reducing idle polling: the IDE process used
about 4.3 CPU seconds over 16 wall seconds before, and 1.6 over 18 after. This
is a short local observation, not a benchmark; child processes were not included.
The project tree still reflected a new file, and a POM change was noticed while
the window was minimized.

For this test, use the `Main.java` with the `add` helper shown in the Maven
fixture below, compiled into `bin/` with debug information. Use an Eclipse
`smoke` fixture with a `.project` declaring
`org.eclipse.jdt.core.javanature` and a `.classpath` with the source root, JRE
container, and `bin` output directory. Set `INTELLI_GEN_DAP_PROJECT` to the fixture,
`INTELLI_GEN_JAVA_HOME` to a Java 25+ JDK home, and
`INTELLI_GEN_JAVA_RESOURCES` to the packaged app's `Contents/Resources` directory.
Run `cargo test live_worker_resolves_server_project_name -- --ignored`. This test
starts its own JDT LS process and uses a sibling `rust-worker-workspace` directory
for temporary language-server data; run it with a disposable fixture directory.

The same live worker test now also starts the production `DebugService` with an
initial source breakpoint. It verifies the `Stopped` state, stack location,
expression result, step over, final console output `42`, normal termination,
worker reaping, and cleanup without requiring a language-host restart. The Java
adapter in this run omitted an exit code; the session correctly retained `None`
rather than inventing a successful code. This covers the debug session worker;
the app event reducer and native UI still require end-to-end verification.

The live worker test additionally reuses the same language host for a second
debug session. It builds the initial requests through the production breakpoint
model: a conditional logpoint at line 4 emits `smoke=40`, and an ordinary
breakpoint stops at line 5. The assertion checks that the logpoint did not
suspend execution. Cancelling this stopped session joins its worker and leaves
the host reusable without a restart request.

The standard test `italian_gutter_menu_emits_enable_and_disable_intents` drives
real egui frames with pointer events. It opens the editor gutter popup, locates
the rendered Italian action label, clicks it, and checks the resulting enable or
disable intent for breakpoints and logpoints. This verifies the widget flow
without an OS window; it does not verify native window rendering.

The optional `live_gradle_worker_builds_and_cleans_remapped_module` test passed
with installed Gradle 6.7.1 and Corretto JDK 8, offline and with an isolated Gradle
user home. It invokes the production build worker for Build, Test, and Clean, checks
successful completion events and selected JDK reporting, confirms compilation of
`custom/app/src/main/java/Main.java`, and checks that Clean removes its `.class`.
It passed again on 2026-11-09 against the current Rust worker.
The same offline worker test also passed against commit `09627ce`.
It passed again against `0f9caa4` on 2026-11-10 using a fresh disposable
fixture, Gradle 6.7.1, Corretto 8, and an isolated Gradle user home.
The fixture's settings include `:app` with
`project(':app').projectDir = file('custom/app')`; its module applies the Java
plugin and uses only local JUnit/Hamcrest JARs for its test dependencies.

To repeat it, set `INTELLI_GEN_GRADLE_PROJECT` to a disposable fixture root,
`INTELLI_GEN_GRADLE` to the installed executable, and
`INTELLI_GEN_BUILD_JAVA_HOME` to a compatible JDK home. Set `GRADLE_USER_HOME` to a
temporary directory and run
`cargo test live_gradle_worker_builds_and_cleans_remapped_module -- --ignored`.
It executes Clean, so use only a disposable fixture. The test expects two JUnit tests, one deliberately failing. It checks exit code
`1`, totals of two tests / one passed / one failed, and the failed method’s source
path and line parsed from the XML report. The native Build panel is covered
separately by the Maven build/diagnostic smoke check above.

For the extended Gradle fixture, place locally installed `junit-4.13.jar` and
`hamcrest-core-1.3.jar` in `custom/app/lib/` and declare
`testImplementation files('lib/junit-4.13.jar', 'lib/hamcrest-core-1.3.jar')` in the
module's `dependencies` block. The tested local copies came from the installed
Gradle distribution. Add `custom/app/src/test/java/MainTest.java`:

```java
import org.junit.Test;
import static org.junit.Assert.*;
public class MainTest {
  @Test public void passing() { assertEquals(42, 42); }
  @Test public void failing() { assertEquals(41, 42); }
}
```

The optional `live_maven_worker_prefers_wrapper_and_reports_test_failures` test
passed again on 2026-11-02 with Maven 3.9.9, JDK 21, and a reactor containing
`custom/app` (it was previously verified with Corretto JDK 8). It exercises the
production build worker for Build, Test, and Clean, confirms JDK reporting,
checks one passing and one failing JUnit test plus its source location, and
verifies that Clean removes the compiled class. The Maven fixture provides an
executable `mvnw` that invokes the installed Maven with `--offline`; the
configured Maven path is deliberately nonexistent to verify wrapper precedence.
It passed again on 2026-11-09 with Corretto JDK 8 and a disposable reactor.
It also passed against `0f9caa4` on 2026-11-10 with a fresh reactor and the
same Maven 3.9.9 and Corretto 8 setup.

Set `INTELLI_GEN_MAVEN_PROJECT` to this disposable reactor and
`INTELLI_GEN_BUILD_JAVA_HOME` to its JDK, then run
`cargo test live_maven_worker -- --ignored`. Use the same `Main.java` and JUnit
`MainTest.java` fixture described above. Its parent POM declares `custom/app`,
Java source/target 8, and cached plugin versions: compiler 3.11.0, resources 3.3.1,
jar 3.3.0, Surefire 2.22.1, and clean 3.2.0. The module uses local system-scoped
`junit:junit:4.13` and `org.hamcrest:hamcrest-core:1.3` JARs. These coordinates are
needed for Surefire to select JUnit 4. All plugin dependencies and the JUnit 4
provider must already exist in the local Maven cache; the test downloads nothing.

The optional `real_maven_profile_refresh_preserves_default_cache` test passed on
2026-11-09 with Maven 3.9.9 and JDK 23. It uses the production Maven view worker
to load a default reactor, activate an `extra` profile that adds a module, and
confirm that the default view remains in the cache. To repeat it, use a fresh
disposable reactor with an offline `mvnw` and that profile, set
`INTELLI_GEN_PROFILE_MAVEN_PROJECT` and `INTELLI_GEN_BUILD_JAVA_HOME`, then run
`cargo test real_maven_profile_refresh_preserves_default_cache -- --ignored`.
The POM watcher and cache fingerprint include literal module paths declared in
Maven profiles. After a successful Maven refresh, the watcher also monitors
resolved module paths that use Maven properties. Property-based paths cannot be
fingerprinted statically, so new cache writes are disabled; an existing cached
view may still be shown with a fingerprint error.
Profile module discovery stays inside the reactor root; an external module also
disables the cache fingerprint.
Only a Maven result accepted for the current project and refresh can schedule a
cache write. Accepted writes run off the UI thread and are ordered per project
within one IDE process. The cache also records the configured JDK home and
catalog version and the selected Maven executable or wrapper path; a mismatch or
missing identity triggers a refresh while keeping the cached view as a fallback.
When a project has a Maven wrapper, automatic refresh is deferred until the
wrapper is approved for the current IDE session, because it can download Maven.
After approval, subsequent project changes can refresh the view automatically. Changes
to the project's wrapper scripts and `.mvn` configuration also change the cache
fingerprint. The IDE warns when monitored inputs cannot be fully checked and
pauses automatic retries after a Maven error until a manual reload. Separate IDE processes do not
coordinate cache writes. The identity does not detect an installed Maven
executable, downloaded wrapper distribution, or JDK replaced at the same path
without a corresponding catalog-version update. User-level Maven settings and
environment variables are not included in the cache fingerprint.

The optional `real_maven_resolves_property_module_and_records_its_pom` test
passed with Maven 3.9.9 and JDK 23 on a disposable reactor whose `extra` profile
uses `${extra.module}`. Set `INTELLI_GEN_PROPERTY_MAVEN_PROJECT` to such a reactor
with an offline `mvnw` and `INTELLI_GEN_BUILD_JAVA_HOME` to the JDK, then run
`cargo test real_maven_resolves_property_module_and_records_its_pom -- --ignored`.

The live language/debug worker test also passed with a minimal Maven project
containing only a POM and conventional Java sources, without an Eclipse
`.project` or `.classpath`. In this case it discovers the Maven model, issues the
production `RefreshMaven` command, waits for `Maven import request completed`,
then polls for up to 60 seconds for the imported `target/classes` classpath
(command acknowledgement can precede import completion). It checks the server
project name and runs the same two debug
sessions (breakpoint/evaluation/stepping plus conditional logpoint/cancellation).

To reproduce this variant, use the same environment and ignored test command,
but provide a POM with group/artifact `smoke:smoke`, version `1`, and Java
source/target/release `25`, without dependencies. Put this `Main.java` in
`src/main/java/` and compile it with debug information into `target/classes`
before the test. The helper method at line 8 is used by the step-into assertion;
line 4 is the initial breakpoint and line 5 is the following statement.

```java
public class Main {
  public static void main(String[] args) {
    int value = 40;
    value = add(value, 2);
    System.out.println(value);
  }
  static int add(int left, int right) {
    return left + right;
  }
}
```

The fixture intentionally uses a different physical directory name from its
Maven artifact name. This verifies project import and the Java debug worker
together; native project opening and widget routing remain outside this test.

The same live worker test passed for a Gradle 8.12.1 project with no Eclipse
metadata or wrapper, using its local distribution and an isolated Gradle cache
in offline mode. The project JDK was GraalVM Java 23; the language server and
debug launch used JBR Java 25. It checked the server project name `smoke-project`,
the imported `bin/main` classpath and compiled `Main.class`, then completed both
debug sessions. Evaluating `System.getProperty("java.specification.version")`
inside the debuggee confirmed Java 25. These are worker-level checks, not native
UI verification. The same run now also covers step into the `add` method on
line 8, step out to its call on line 4, and step over to line 5.

To reproduce the Gradle variant, use a disposable directory named `project`,
`settings.gradle` containing `rootProject.name = 'smoke'`, and `build.gradle`
containing `plugins { id 'java' }`. Put this `Main.java` under `src/main/java/`:

```java
public class Main {
  public static void main(String[] args) {
    int value = 40;
    value = add(value, 2);
    System.out.println(value);
  }
  static int add(int left, int right) {
    return left + right;
  }
}
```

Run the installed Gradle's `--offline --no-daemon classes` with Java 23 first.
Set `INTELLI_GEN_DAP_PROJECT`, `INTELLI_GEN_JAVA_HOME` and
`INTELLI_GEN_JAVA_RESOURCES` as above, plus `INTELLI_GEN_PROJECT_JAVA_HOME` to
Java 23 and `INTELLI_GEN_GRADLE_HOME` to the Gradle distribution root. Keep
`GRADLE_USER_HOME` on the same disposable cache for the build and test. Run
`cargo test live_worker_resolves_server_project_name -- --ignored`; it starts
JDT LS and uses the same breakpoint, stepping, evaluation and logpoint checks.
