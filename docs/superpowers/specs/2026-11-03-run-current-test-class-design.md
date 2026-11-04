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
  when the active Java file is beneath a module's conventional `src/test/java`
  root.
- Derive its fully qualified name from the relative path and reuse
  `BuildAction::RunTest` with the `test` task.
- Disable the action while another build is running; the existing build handler
  validates the project, uses its selected JDK and wrapper/executable, and
  displays output in the Build panel.
- Localize the action and its disabled hint in `en.ini` and `it.ini`.
- Do not add a dependency, persist a temporary configuration, support custom
  test source sets, or change the debugger.

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

A follow-up can use these existing JDT LS commands to recognize imported test
roots, resolve the Java type name, and pair files by their relative path. Before
changing the current conventional-root behavior, verify the serialized
`java.project.getSettings` result against the packaged 1.62.0 process. No
custom-root behavior is claimed by this evidence.
