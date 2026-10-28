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
4. Open a Maven or Gradle project. Java completion, navigation, diagnostics, and debugging require the language server and Java Debug Server plugin. Lombok-generated members are supported when the project declares Lombok and its JAR is cached. If a successful build resolves the JAR after project open, Intelli-gen automatically restarts Java intelligence; dependencies resolved outside the IDE still need a manual restart.

Release packaging places pinned Eclipse JDT Language Server and Microsoft Java Debug Server tooling inside the app bundle's resources. For a source build, use **Settings** to select installed tooling when it is not bundled beside the executable. The launcher and debug plugin can each be overridden independently. Intelli-gen does not install Java or other toolchains silently.

To build the macOS release package, run `scripts/package-release.sh`. This explicitly downloads the pinned upstream Java tooling, verifies the JDT LS archive checksum, and requires an already installed Java 21+ JDK. Review `packaging/java-tools.env` for the pinned versions. The resulting bundle's JDT LS runtime requires Java 25+.
The package contains `dist/Intelli-gen.app` for Finder and a `dist/intelli-gen` command-line link. Launch the app with `open dist/Intelli-gen.app`.
The command-line link accepts the same optional project or file path: `dist/intelli-gen /path/to/project-or-file`.

## Shortcuts

| Action | Shortcut |
| --- | --- |
| Open project / file | `⇧⌘O` / `⌘O` |
| Go to file | `⌘P` |
| Find / replace in file | `⌘F` / `⌘R` |
| Find in project | `⇧⌘F` |
| Go to line | `⌘L` |
| Reformat Java file | `⌥⌘L` |
| Organize Java imports | `Ctrl+Alt+O` |
| Go to Java symbol | `⌘⌥O` |
| File structure | `⌘F12` |
| Complete / parameter info / hover | `Ctrl+Space` / `Ctrl+Shift+Space` / `F1` |
| Go to definition / find references | `F12` / `Shift+F12` |
| Rename Java symbol | `Shift+F6` |
| Java quick fixes | `⌥↩` |
| Go to type definition | `⇧⌘B` |
| Go to implementation | `⌥⌘B` |
| Save / save all | `⌘S` / `⇧⌘S` |

## Project scope

The Run Console accepts line input for interactive Java processes. The Build panel summarizes Maven Surefire and Gradle test totals and lists failures from fresh Surefire, Failsafe, and Gradle XML reports; select a failure to open its conventional Java test source and stack-trace line when available, or use **Run class** to rerun that class. The **Problems** tool window combines available Java language-server diagnostics with the latest build diagnostics; selecting a result opens its source location.

The first release targets macOS and Java projects using Maven or Gradle. Use **File → New Java Project…** to create a minimal Maven or Gradle starter locally; it creates no wrapper and downloads no tools. The starter includes a run configuration for `Main`; select an installed JDK before launching. Start, stop, or manage configurations from the **Run** menu. Use a project-tree directory's context menu to create a Java class; an optional package name creates matching subdirectories beneath the selected directory. Files up to 16 MiB are editable; files above 256 KiB use a bounded page editor that keeps the complete document available for save and find/replace. Run configurations can select a Maven/Gradle module or detect one from the main class; the JPMS module name remains a separate launch option. The app also includes project search, an integrated terminal, local Git operations, and a Java debugger. Java language services depend on installed or packaged JDT LS and Java Debug Server tooling; editing and builds remain available if optional Java tooling is unavailable.
