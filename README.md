# Intelli-gen

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

`cargo check` compile-checks the app. Java, Maven, and Gradle are not downloaded by these commands. When available, project Maven and Gradle wrappers are preferred for builds; otherwise configure installed executables under **Settings**.

## Java setup

1. Open **Settings** and discover or add installed JDKs.
2. Choose a JDK 25 or newer as the **JDT Language Server runtime** for the bundled server. A custom launcher accepts Java 21 or newer. This runtime starts the language server and is separate from the project's JDK.
3. Open **Project Structure** and choose the JDK for the project. Intelli-gen preserves build-file compiler targets and toolchains.
4. Open a Maven or Gradle project. Java completion opens after two identifier characters or immediately after a dot, expands supported Java snippets, synchronizes repeated placeholders as you type, and lets Tab move through fields; navigation, diagnostics, and debugging require the language server and Java Debug Server plugin. Lombok-generated members are supported when the project declares Lombok and its JAR is cached. If a successful build resolves the JAR after project open, Intelli-gen automatically restarts Java intelligence; dependencies resolved outside the IDE still need a manual restart.

Release packaging places pinned Eclipse JDT Language Server and Microsoft Java Debug Server tooling inside the app bundle's resources. For a source build, use **Settings** to select installed tooling when it is not bundled beside the executable. The launcher and debug plugin can each be overridden independently. Intelli-gen does not install Java or other toolchains silently.

To build the macOS release package, run `scripts/package-release.sh`. This explicitly downloads the pinned upstream Java tooling, verifies the JDT LS archive checksum, and requires an already installed Java 21+ JDK. Review `packaging/java-tools.env` for the pinned versions. The resulting bundle's JDT LS runtime requires Java 25+.
The package contains `dist/Intelli-gen.app` for Finder and a `dist/intelli-gen` command-line link. Launch the app with `open dist/Intelli-gen.app`.
The command-line link accepts the same optional project or file path: `dist/intelli-gen /path/to/project-or-file`.

## Shortcuts

| Action | Shortcut |
| --- | --- |
| Open project / file | `⇧⌘O` / `⌘O` |
| Next / previous editor tab | `Ctrl+Tab` / `Ctrl+Shift+Tab` |
| Go to file | `⌘P` |
| Find / replace in file | `⌘F` / `⌘R` |
| Find in project | `⇧⌘F` |
| Go to line | `⌘L` |
| Navigate to previous / next code location | `Ctrl+Alt+←` / `Ctrl+Alt+→` |
| Reformat Java file | `⌥⌘L` |
| Organize Java imports | `Ctrl+Alt+O` |
| Go to Java symbol | `⌘⌥O` |
| File structure | `⌘F12` |
| Complete / parameter info / hover | `Ctrl+Space` / `Ctrl+Shift+Space` / `F1` |
| Go to definition / find references | `F12` / `Shift+F12` |
| Rename Java symbol | `Shift+F6` |
| Toggle Java line comment | `⌘/` |
| Indent Java code / selected lines; navigate snippet fields | `Tab` / `⇧Tab` |
| Java quick fixes | `⌥↩` |
| Go to type definition | `⇧⌘B` |
| Go to implementation | `⌥⌘B` |
| Save / save all | `⌘S` / `⇧⌘S` |

## Project scope

The Run Console accepts line input for interactive Java processes. The Build panel summarizes Maven Surefire and Gradle test totals and lists failures from fresh Surefire, Failsafe, and Gradle XML reports; select a failure to open its conventional Java test source and stack-trace line when available, or use **Run class** to rerun that class. The **Problems** tool window combines available Java language-server diagnostics with the latest build diagnostics; selecting a result opens its source location.

The first release targets macOS and Java projects using Maven or Gradle. Use **File → New Java Project…** to create a minimal Maven or Gradle starter locally; it creates no wrapper and downloads no tools. The starter includes a run configuration for `Main`; select an installed JDK before launching. **Run** builds the project first and asks before saving open project files with unsaved changes; a failed build does not launch the program. Start, stop, or manage configurations from the **Run** menu. Use a project-tree directory's context menu to create a Java class; an optional package name creates matching subdirectories beneath the selected directory. Set Java indentation to 1–8 spaces or tabs in **Settings**; the editor and Java formatter use the same preference. Parent, root, and nested `.editorconfig` files override these choices by file pattern up to the nearest `root=true`. Files up to 16 MiB are editable; files above 256 KiB use a bounded page editor that keeps the complete document available for save and find/replace. **Find in Project** also supports preview-first replacement: review and select matching files before applying edits to editor buffers, then save those files to keep the changes. Run configurations can select a Maven/Gradle module or detect one from the main class; the JPMS module name remains a separate launch option. The app also includes an integrated terminal, local Git operations, and a Java debugger. Java language services depend on installed or packaged JDT LS and Java Debug Server tooling; editing and builds remain available if optional Java tooling is unavailable.

## IntelliJ reverse-engineering evidence

The local reference inspected with REA 6.7.0 is IntelliJ IDEA Ultimate 2026.3,
build `IU-263.7307.44`. Run REA with Node 26 when reproducing these observations.
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

The extracted `META-INF/plugin.xml` registers `Gradle.ExecuteTask`,
`Gradle.ToggleOfflineAction`, and `Gradle.RefreshDependencies`, including toolbar
or menu references. This proves the shipped action registrations; it does not
establish their runtime behavior. Intelli-gen Settings now provides **Gradle offline mode** for CLI builds, tests,
builds before Run, and task reports. It passes `--offline` so Gradle uses cached
dependencies; missing cached dependencies cause Gradle to fail. It also configures the JDT LS Gradle importer through
`java.import.gradle.offline.enabled`. Use **Apply / restart Java intelligence**
after changing the preference to update an existing language session. After loading tasks, **Refresh dependencies** beside a `:dependencies` task runs
that report with `--refresh-dependencies`, using the selected JDK and the normal
Build output/cancellation flow. The action requires online mode and may contact
repositories or download dependencies. No JetBrains implementation code is included in Intelli-gen.

## Java tooling runtime verification

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

The optional `live_worker_resolves_server_project_name` test passed using the
production `JavaLanguageService` worker and `ClasspathResolver`. It started the
packaged server, received runtime classpaths, resolved the server project name
`smoke` rather than the deliberately incorrect model name `directory-fallback`,
and joined the language worker after shutdown.

For this test, use the same `smoke` fixture with an Eclipse `.project` declaring
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
