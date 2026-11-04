# Run Current Test Class design

## Intent and evidence

Let a Java developer start the test class open in the editor without creating a
run configuration. IntelliJ IDEA offers running the current test class from the
editor or test gutter ([Run tests](https://www.jetbrains.com/help/idea/performing-tests.html)); Intelli-gen already has a module-aware `BuildAction::RunTest`
flow for rerunning failed classes.

## Approaches

- **Reuse the existing filtered-build action (selected):** resolve a
  conventional `src/test/java` path to its Maven/Gradle module and fully
  qualified class name, then dispatch `RunTest`. This preserves the existing
  JDK, process, output, diagnostics, and cancellation behavior.
- Create a temporary Java run configuration: that launches a test as an
  application and bypasses the build-tool test runner, so it does not match the
  current Maven/Gradle test flow.
- Add a Java test-runner integration: it could support arbitrary source sets,
  but requires a new protocol and lifecycle beyond the existing build service.

## Scope and constraints

- Add **Run Current Test Class** to the Run menu and editor gutter context menu
  when the active Java file is beneath a test source root reported by JDT LS.
- Derive its fully qualified name from the relative path and reuse
  `BuildAction::RunTest`, using the Gradle scope as the task for custom test
  source sets. Maven continues to use its test or integration-test phase.
- Pair test and production files by their relative path under JDT LS source
  roots; retain the conventional `src/main/java` and `src/test/java` fallback
  while source-root metadata is unavailable.
- Disable the action while another build is running; the existing build handler
  validates the project, uses its selected JDK and wrapper/executable, and
  displays output in the Build panel.
- Localize the action and its disabled hint in `en.ini` and `it.ini`.
- Do not add a dependency, persist a temporary configuration, or change the
  debugger.

## Risks and limits

- Package name is inferred from the conventional source path. Projects whose
  package declaration disagrees with the path are not handled specially.
- Integration tests and custom Gradle test tasks require explicit task
  selection and are outside this action.

## REA follow-up evidence (2026-11-04)

The packaged JDT LS 1.62.0 core JAR is
`Contents/Resources/jdtls/plugins/org.eclipse.jdt.ls.core_1.62.0.202610131109.jar`.
REA inventory Evidence `ev_41ab92a7ba0dcdd8ef6b750f12c0f69b8021bdeadf18e85b62d21b944886cecb`
records SHA-256 `ec4b7672831d72c16136b344d8d71919c08b024831d811d4d5d226056069a7f8`.
The shipped `plugin.xml` declares `java.project.isTestFile`,
`java.project.getSettings`, and `java.getFullyQualifiedName`.

The [JDT LS command handler](https://github.com/eclipse-jdtls/eclipse.jdt.ls/blob/main/org.eclipse.jdt.ls.core/src/org/eclipse/jdt/ls/core/internal/JDTDelegateCommandHandler.java)
routes `java.project.isTestFile` to `ProjectCommand.isTestFile`. That
[implementation](https://github.com/eclipse-jdtls/eclipse.jdt.ls/blob/main/org.eclipse.jdt.ls.core/src/org/eclipse/jdt/ls/core/internal/commands/ProjectCommand.java)
classifies a file from the imported Java project's test source classpath
entries, including Maven or Gradle scope metadata. This is source-level
evidence for a supported route to recognize custom test roots; it does not
prove the request/response behavior of Intelli-gen's runtime session.

The packaged JDT LS 1.62.0 process was started with its bundled macOS ARM
configuration and queried over LSP using
`workspace/executeCommand(java.project.getSettings, [fileUri,
["org.eclipse.jdt.ls.core.classpathEntries"]])`. It returned source entries
with `kind: 3`, absolute `path`, optional `output`, and `attributes`; a Gradle
integration-test entry returned `test: "true"` and
`gradle_scope: "integrationTest"`. This runtime observation used a temporary
Eclipse Java project with the same classpath-entry shape, so it verifies the
command serialization but not Gradle's import of every custom source-set DSL.
JDT LS `ProjectCommand.isTestFile` bytecode also confirms that test roots are
determined from source entries marked `isTest()` or scope attributes whose
value contains `test`. Intelli-gen uses the serialized `test` and scope
attributes for imported roots, and maps returned canonical paths back beneath
the workspace path used by the editor.

REA inventory Evidence `ev_41ab92a7ba0dcdd8ef6b750f12c0f69b8021bdeadf18e85b62d21b944886cecb`
identifies the exact JDT LS core JAR and SHA-256 above. IntelliJ's Java test
execution artifact remains inventory-only evidence; its test gutter and run
affordances are corroborated by JetBrains' [Performing
tests](https://www.jetbrains.com/help/idea/performing-tests.html).
