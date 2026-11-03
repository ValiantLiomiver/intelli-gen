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

- Add **Run Current Test Class** to the Run menu when the active Java file is
  beneath a module's conventional `src/test/java` root.
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
