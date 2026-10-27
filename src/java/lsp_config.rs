use std::path::{Path, PathBuf};
use std::ffi::OsString;

use serde_json::{Value, json};

use super::jdk::{JdkCatalog, JdkInstallation};
use super::lsp::LspError;

#[cfg(test)]
mod maven_import_tests {
    use serde_json::json;

    use super::{default_java_import_settings, java_settings_for_maven_refresh};

    #[test]
    fn default_settings_disable_maven_import_and_keep_gradle_enabled() {
        let imports = default_java_import_settings(json!("/jdk/home"));
        assert_eq!(imports["maven"]["enabled"], false);
        assert_eq!(imports["gradle"]["enabled"], true);
        assert_eq!(imports["gradle"]["java"]["home"], "/jdk/home");
    }

    #[test]
    fn refresh_settings_enable_maven_without_changing_gradle() {
        let initial = json!({
            "java": {
                "configuration": {"updateBuildConfiguration": "automatic"},
                "import": {"maven": {"enabled": false}, "gradle": {"enabled": true}}
            }
        });

        let refreshed = java_settings_for_maven_refresh(&initial);
        assert_eq!(refreshed["java"]["import"]["maven"]["enabled"], true);
        assert_eq!(refreshed["java"]["import"]["gradle"]["enabled"], true);
        assert_eq!(refreshed["java"]["configuration"]["updateBuildConfiguration"], "automatic");
        assert_eq!(initial["java"]["import"]["maven"]["enabled"], false);
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedJavaTools {
    pub arguments: Vec<OsString>,
    pub display_path: PathBuf,
    pub shared_configuration: Option<PathBuf>,
    pub package_version: Option<OsString>,
}

fn bundled_directory(executable_directory: &Path, name: &str) -> PathBuf {
    let adjacent = executable_directory.join(name);
    if adjacent.is_dir() {
        return adjacent;
    }
    if executable_directory.file_name().is_some_and(|part| part == "MacOS") {
        if let Some(contents) = executable_directory.parent() {
            let resources = contents.join("Resources").join(name);
            if resources.is_dir() {
                return resources;
            }
        }
    }
    adjacent
}

fn executable_directory(executable: &Path) -> Result<PathBuf, LspError> {
    let resolved = executable.canonicalize().unwrap_or_else(|_| executable.to_path_buf());
    resolved.parent().map(Path::to_path_buf)
        .ok_or_else(|| LspError("Cannot locate the application distribution directory".into()))
}

/// Resolve each optional override independently, falling back to the tools
/// staged beside the application executable. The executable is explicit so
/// packaged layouts can be inspected without depending on the current process.
pub fn resolve_jdtls(settings: &crate::settings::UserSettings, executable: &Path) -> Result<ResolvedJavaTools, LspError> {
    let distribution = executable_directory(executable)?;
    if let Some(jdtls) = &settings.jdt_ls_executable {
        if !jdtls.is_file() {
            return Err(LspError(format!("The configured JDT LS launcher does not exist: {}", jdtls.display())));
        }
        return Ok(ResolvedJavaTools { arguments: Vec::new(), display_path: jdtls.clone(), shared_configuration: None, package_version: None });
    }
    let jdtls_dir = bundled_directory(&distribution, "jdtls");
    let plugins = jdtls_dir.join("plugins");
    let launcher = std::fs::read_dir(&plugins).map_err(|_| LspError(
        "JDT LS is not bundled; configure its launcher in Settings to enable Java intelligence".into()))?
        .filter_map(Result::ok).map(|entry| entry.path())
        .find(|path| path.file_name().and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("org.eclipse.equinox.launcher_") && name.ends_with(".jar")))
        .ok_or_else(|| LspError("Bundled JDT LS Equinox launcher JAR is missing".into()))?;
    let configuration = jdtls_dir.join("config_mac");
    if !configuration.is_dir() { return Err(LspError("Bundled JDT LS macOS configuration is missing".into())); }
    let core_bundle = std::fs::read_dir(&plugins).map_err(|error| LspError(format!("Cannot inspect JDT LS plugins: {error}")))?
        .filter_map(Result::ok).map(|entry| entry.path())
        .find(|path| path.file_name().and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("org.eclipse.jdt.ls.core_") && name.ends_with(".jar")))
        .ok_or_else(|| LspError("Bundled JDT LS core plugin JAR is missing".into()))?;
    let package_version = core_bundle.file_name().ok_or_else(|| LspError("Bundled JDT LS package version is unavailable".into()))?.to_owned();
    let mut shared_area = OsString::from("-Dosgi.sharedConfiguration.area=");
    shared_area.push(configuration.as_os_str());
    let mut arguments: Vec<OsString> = ["-Declipse.application=org.eclipse.jdt.ls.core.id1", "-Dosgi.bundles.defaultStartLevel=4",
        "-Declipse.product=org.eclipse.jdt.ls.core.product", "-Dlog.protocol=true", "-Dlog.level=ALL",
        "-Dosgi.sharedConfiguration.area.readOnly=true", "-Dosgi.configuration.cascaded=true",
        "-Xmx1G", "--add-modules=ALL-SYSTEM", "--add-opens", "java.base/java.util=ALL-UNNAMED",
        "--add-opens", "java.base/java.lang=ALL-UNNAMED"]
        .into_iter().map(OsString::from).collect();
    arguments.push(shared_area);
    arguments.extend([OsString::from("-jar"), launcher.as_os_str().to_owned(), OsString::from("-configuration"), configuration.as_os_str().to_owned()]);
    Ok(ResolvedJavaTools { arguments, display_path: launcher,
        shared_configuration: Some(configuration), package_version: Some(package_version) })
}

pub fn resolve_debug_bundle(settings: &crate::settings::UserSettings, executable: &Path) -> Result<PathBuf, LspError> {
    if let Some(path) = &settings.java_debug_bundle { return Ok(path.clone()); }
    let distribution = executable_directory(executable)?;
    let dir = bundled_directory(&distribution, "java-debug");
    let mut candidates = std::fs::read_dir(&dir).map_err(|_| LspError(
        "Java Debug Server is not bundled; configure its plugin JAR in Settings".into()))?
        .filter_map(Result::ok).map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| path.file_name().and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("com.microsoft.java.debug.plugin-") && name.ends_with(".jar")));
    let first = candidates.next().ok_or_else(|| LspError(
        "Java Debug Server is not bundled; configure its plugin JAR in Settings".into()))?;
    if candidates.next().is_some() {
        return Err(LspError("Multiple bundled Java Debug Server plugin JARs found; configure one in Settings".into()));
    }
    Ok(first)
}

/// `executable` is either an explicit custom launcher or the bundled Equinox
/// JAR. Bundled configuration is copied into user data on the language worker.
/// The JDK catalog is a snapshot; launch validation happens on that worker.
#[derive(Clone, Debug)]
pub struct JdtlsConfig {
    pub executable: PathBuf,
    pub launcher_arguments: Vec<OsString>,
    /// Project-resolved Lombok agent, when Lombok is declared and already cached locally.
    pub lombok_agent: Option<PathBuf>,
    pub shared_configuration: Option<PathBuf>,
    pub package_version: Option<OsString>,
    pub runtime_jdk_id: String,
    pub workspace_data: PathBuf,
    pub jdks: Vec<JdkInstallation>,
    pub project_jdk_id: Option<String>,
    pub debug_bundle: Option<PathBuf>,
    pub debug_bundle_error: Option<String>,
}

pub(crate) fn default_java_import_settings(gradle_java_home: Value) -> Value {
    json!({
        "maven": {"enabled": false},
        "gradle": {"enabled": true, "java": {"home": gradle_java_home}}
    })
}

pub(crate) fn java_settings_for_maven_refresh(settings: &Value) -> Value {
    let mut refreshed = settings.clone();
    refreshed["java"]["import"]["maven"]["enabled"] = json!(true);
    refreshed["java"]["configuration"]["updateBuildConfiguration"] = json!("automatic");
    refreshed
}

impl JdtlsConfig {
    pub fn from_settings(settings: &crate::settings::UserSettings, root: &Path) -> Result<Self, LspError> {
        let runtime_jdk_id = settings.jdt_ls_jdk().map_err(LspError)?.id.clone();
        let application = std::env::current_exe().map_err(|error| LspError(format!("Cannot locate Intelli-gen executable: {error}")))?;
        let resolved = resolve_jdtls(settings, &application)?;
        let lombok_agent = find_project_lombok(root);
        let (debug_bundle, debug_bundle_error) = match resolve_debug_bundle(settings, &application) {
            Ok(path) => (Some(path), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let dirs = directories::ProjectDirs::from("dev", "Intelli-gen", "Intelli-gen")
            .ok_or_else(|| LspError("Application data directory is unavailable".into()))?;
        // Stable per-build-root data identity, independent of basename or hash randomization.
        let mut hash = 0xcbf29ce484222325u64;
        for byte in root.as_os_str().as_encoded_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        Ok(Self { executable: resolved.display_path, launcher_arguments: resolved.arguments, lombok_agent,
            shared_configuration: resolved.shared_configuration, package_version: resolved.package_version, runtime_jdk_id,
            workspace_data: dirs.data_local_dir().join("jdtls").join(format!("{hash:016x}")),
            jdks: settings.jdks.clone(),
            debug_bundle,
            debug_bundle_error,
            project_jdk_id: settings.project_jdk_id(root).map(str::to_owned)
                .or_else(|| settings.default_jdk_id.clone()),
        })
    }

    pub fn runtime(&self) -> Result<JdkInstallation, LspError> {
        let registered = self.jdks.iter().find(|jdk| jdk.id == self.runtime_jdk_id)
            .ok_or_else(|| LspError("The dedicated JDT LS runtime is no longer registered".into()))?;
        let current = JdkCatalog::add(&registered.home).map_err(|error| LspError(error.to_string()))?;
        let minimum = if self.package_version.is_some() { 25 } else { 21 };
        if current.version.major < minimum { return Err(LspError(format!("JDT LS requires a dedicated Java {minimum}+ runtime; selected runtime is Java {}", current.version.major))); }
        Ok(current)
    }

    pub fn java_settings(&self) -> Result<Value, LspError> {
        let selected = self.project_jdk_id.as_ref().ok_or_else(|| LspError("Choose a project JDK to configure Java language intelligence".into()))?;
        let project_jdk = self.jdks.iter().find(|jdk| &jdk.id == selected)
            .and_then(|jdk| JdkCatalog::add(&jdk.home).ok())
            .ok_or_else(|| LspError("The project JDK is unavailable; choose a replacement in Project Structure".into()))?;
        // JDT LS reads Maven compiler properties / Gradle toolchains itself. The
        // project JDK is the default runtime; we do not override descriptor targets.
        let mut runtimes = Vec::new();
        let mut majors = std::collections::HashSet::new();
        let mut ordered: Vec<_> = self.jdks.iter().collect();
        ordered.sort_by_key(|jdk| jdk.id != *selected);
        for jdk in ordered {
            let Ok(current) = JdkCatalog::add(&jdk.home) else { continue; };
            if !majors.insert(current.version.major) { continue; }
            let name = match current.version.major { 1..=8 => format!("JavaSE-1.{}", current.version.major), major => format!("JavaSE-{major}") };
            runtimes.push(json!({"name": name, "path": current.home, "default": &jdk.id == selected}));
        }
        Ok(json!({"java": {
            // This preference is shared with Gradle, so retain automatic
            // updates while the Maven importer itself is disabled by default.
            "configuration": {"runtimes": runtimes, "updateBuildConfiguration": "automatic"},
            "import": default_java_import_settings(json!(project_jdk.home)),
            "autobuild": {"enabled": true}
        }}))
    }
}

/// Locate Lombok only when a Maven or Gradle descriptor in this workspace declares it.
/// The IDE never downloads dependencies: only jars already present in the user's caches qualify.
fn find_project_lombok(root: &Path) -> Option<PathBuf> {
    fn inspect(dir: &Path, depth: usize, descriptors: &mut Vec<String>) {
        if depth > 6 { return; }
        let Ok(entries) = std::fs::read_dir(dir) else { return; };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if path.is_dir() {
                if matches!(name.to_str(), Some(".git" | "target" | "build" | ".gradle" | "node_modules")) { continue; }
                inspect(&path, depth + 1, descriptors);
            } else if matches!(name.to_str(), Some("pom.xml" | "build.gradle" | "build.gradle.kts")) {
                if let Ok(text) = std::fs::read_to_string(path) { descriptors.push(text); }
            }
        }
    }
    let mut descriptors = Vec::new();
    inspect(root, 0, &mut descriptors);
    let declared = descriptors.iter().any(|text| {
        text.contains("org.projectlombok")
            || text.contains("projectlombok:lombok")
            || text.contains("lombok(\"")
            || text.contains("lombok:")
    });
    if !declared { return None; }

    let version = descriptors.iter().find_map(|text| {
        let start = text.find("<lombok.version>")? + "<lombok.version>".len();
        let end = text[start..].find("</lombok.version>")? + start;
        Some(text[start..end].trim().to_owned())
    }).or_else(|| descriptors.iter().find_map(|text| {
        let start = text.find("<artifactId>lombok</artifactId>")?;
        let dependency_end = text[start..].find("</dependency>")? + start;
        let block = &text[start..dependency_end];
        let start = block.find("<version>")? + "<version>".len();
        let end = block[start..].find("</version>")? + start;
        Some(block[start..end].trim().to_owned())
    })).or_else(|| descriptors.iter().find_map(|text| {
        let start = text.find("projectlombok:lombok:")? + "projectlombok:lombok:".len();
        let version: String = text[start..].chars().take_while(|ch| ch.is_ascii_digit() || *ch == '.').collect();
        (!version.is_empty()).then_some(version)
    }));
    let version = version.map(|version| {
        if version.starts_with("${") && version.ends_with('}') {
            let property = &version[2..version.len() - 1];
            descriptors.iter().find_map(|text| {
                let tag = format!("<{property}>");
                let start = text.find(&tag)? + tag.len();
                let end = text[start..].find(&format!("</{property}>"))? + start;
                Some(text[start..end].trim().to_owned())
            }).unwrap_or_default()
        } else { version }
    }).filter(|version| !version.is_empty() && !version.contains("${"));
    let home = directories::UserDirs::new().map(|dirs| dirs.home_dir().to_path_buf())?;
    let maven_repository = maven_local_repository(&home);
    let gradle_home = std::env::var_os("GRADLE_USER_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".gradle"));
    let mut candidates = Vec::new();
    if let Some(version) = version {
        {
            candidates.push(maven_repository.join("org/projectlombok/lombok").join(&version).join(format!("lombok-{version}.jar")));
            let gradle = gradle_home.join("caches/modules-2/files-2.1/org.projectlombok/lombok").join(&version);
            if let Ok(entries) = std::fs::read_dir(gradle) {
                for hash in entries.flatten() {
                    if let Ok(jars) = std::fs::read_dir(hash.path()) {
                        candidates.extend(jars.flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|ext| ext == "jar")));
                    }
                }
            }
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn maven_local_repository(home: &Path) -> PathBuf {
    let settings = std::env::var_os("MAVEN_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".m2"))
        .join("settings.xml");
    if let Ok(text) = std::fs::read_to_string(settings)
        && let Some(start) = text.find("<localRepository>")
    {
        let start = start + "<localRepository>".len();
        if let Some(end) = text[start..].find("</localRepository>") {
            let configured = text[start..start + end].trim()
                .replace("${user.home}", &home.to_string_lossy());
            let path = PathBuf::from(configured);
            if path.is_absolute() { return path; }
        }
    }
    home.join(".m2/repository")
}
