use std::collections::HashSet;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JavaVersion {
    pub major: u32,
    pub raw: String,
}

impl JavaVersion {
    fn parse(raw: &str) -> Option<Self> {
        let mut parts = raw.split(['.', '-', '+', '_']);
        let first: u32 = parts.next()?.parse().ok()?;
        let major = if first == 1 { parts.next()?.parse().ok()? } else { first };
        (major > 0).then(|| Self { major, raw: raw.to_owned() })
    }
}

impl fmt::Display for JavaVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.raw.is_empty() { "Unknown version" } else { &self.raw })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JdkInstallation {
    pub id: String,
    pub home: PathBuf,
    pub version: JavaVersion,
    pub vendor: Option<String>,
    pub valid: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProjectJdkSelection {
    pub default_jdk_id: String,
}

impl JdkInstallation {
    /// Retain unavailable registrations so removable installations can return.
    pub fn unavailable(path: &Path) -> Self {
        let home = normalize_home(path);
        Self {
            id: installation_id(&home), home,
            version: JavaVersion::default(), vendor: None, valid: false,
        }
    }

    pub fn refresh(&mut self) {
        match JdkCatalog::add(&self.home) {
            Ok(current) => {
                self.home = current.home;
                self.version = current.version;
                self.vendor = current.vendor;
                self.valid = true;
            }
            Err(_) => self.valid = false,
        }
    }

    pub fn label(&self) -> String {
        format!("{} {}{}", self.vendor.as_deref().unwrap_or("JDK"), self.version,
            if self.valid { "" } else { " (invalid path)" })
    }

    /// Values apply to one child only; never call set_var in the application.
    pub fn child_environment(&self) -> Result<Vec<(OsString, OsString)>, JdkError> {
        let current = JdkCatalog::add(&self.home)?;
        let mut paths = vec![current.home.join("bin")];
        if let Some(parent_path) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&parent_path));
        }
        let path = std::env::join_paths(paths).map_err(|error| JdkError {
            home: current.home.clone(), message: format!("Cannot construct PATH: {error}"),
        })?;
        Ok(vec![("JAVA_HOME".into(), current.home.into_os_string()), ("PATH".into(), path)])
    }

    pub fn configure_command(&self, command: &mut std::process::Command) -> Result<(), JdkError> {
        command.envs(self.child_environment()?);
        Ok(())
    }
}

pub struct JdkCatalog;

#[derive(Clone, Debug)]
pub enum JdkSelectionTarget {
    Catalog,
    Global,
    JdtLs,
    Project(PathBuf),
}

#[derive(Clone, Debug)]
pub struct JdkDiscoveryEvent {
    pub catalog: Vec<JdkInstallation>,
    pub selected_id: Option<String>,
    pub target: JdkSelectionTarget,
    pub error: Option<String>,
}

/// Runs catalog refresh away from the UI frame and returns the catalog via `poll`.
pub struct JdkDiscovery {
    sender: Sender<JdkDiscoveryEvent>,
    receiver: Receiver<JdkDiscoveryEvent>,
    worker: Option<JoinHandle<()>>,
}

impl Default for JdkDiscovery {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self { sender, receiver, worker: None }
    }
}

impl JdkDiscovery {
    pub fn start_catalog(&mut self, existing: Vec<JdkInstallation>) -> Result<(), String> {
        self.start(existing, JdkSelectionTarget::Catalog, None, true, 0)
    }

    pub fn start_validate(&mut self, existing: Vec<JdkInstallation>) -> Result<(), String> {
        self.start(existing, JdkSelectionTarget::Catalog, None, false, 0)
    }

    pub fn start_add(
        &mut self,
        path: PathBuf,
        existing: Vec<JdkInstallation>,
        target: JdkSelectionTarget,
        minimum_jdt_ls_major: u32,
    ) -> Result<(), String> {
        self.start(existing, target, Some(path), false, minimum_jdt_ls_major)
    }

    fn start(
        &mut self,
        mut existing: Vec<JdkInstallation>,
        target: JdkSelectionTarget,
        path: Option<PathBuf>,
        discover: bool,
        minimum_jdt_ls_major: u32,
    ) -> Result<(), String> {
        if self.worker.as_ref().is_some_and(|worker| !worker.is_finished()) {
            return Err("A JDK catalog operation is already running".into());
        }
        self.reap();
        let sender = self.sender.clone();
        self.worker = Some(thread::Builder::new()
            .name("jdk-discovery".into())
            .spawn(move || {
                let mut selected_id = None;
                let mut error = None;
                if let Some(path) = path {
                    let candidate = match JdkCatalog::add(&path) {
                        Ok(jdk)
                            if matches!(&target, JdkSelectionTarget::JdtLs)
                                && jdk.version.major < minimum_jdt_ls_major =>
                        {
                            error = Some(format!(
                                "JDT LS requires Java {minimum_jdt_ls_major} or newer; installation added without selecting it"
                            ));
                            jdk
                        }
                        Ok(jdk) => { selected_id = Some(jdk.id.clone()); jdk }
                        Err(problem) => {
                            error = Some(format!("{problem}. Entry retained; repair its path or remove it."));
                            JdkInstallation::unavailable(&path)
                        }
                    };
                    if let Some(existing) = existing.iter_mut().find(|jdk| jdk.home == candidate.home) {
                        let id = existing.id.clone();
                        *existing = candidate;
                        existing.id = id.clone();
                        selected_id = if error.is_none() { Some(id) } else { None };
                    } else {
                        existing.push(candidate);
                    }
                } else {
                    for jdk in &mut existing { jdk.refresh(); }
                    if discover {
                        for discovered in JdkCatalog::discover() {
                            if !existing.iter().any(|jdk| jdk.home == discovered.home) { existing.push(discovered); }
                        }
                    }
                }
                let _ = sender.send(JdkDiscoveryEvent { catalog: existing, selected_id, target, error });
            })
            .map_err(|error| format!("Cannot start JDK discovery: {error}"))?);
        Ok(())
    }

    pub fn poll(&mut self) -> Option<JdkDiscoveryEvent> {
        let result = self.receiver.try_recv().ok();
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) { self.reap(); }
        result
    }

    pub fn has_worker(&self) -> bool {
        self.worker.is_some()
    }

    fn reap(&mut self) {
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(worker) = self.worker.take() { let _ = worker.join(); }
        }
    }
}

impl JdkCatalog {
    /// Read local release or bundle metadata only. Discovered executables are never run.
    pub fn discover() -> Vec<JdkInstallation> {
        let mut candidates = Vec::new();
        let mut roots = vec![PathBuf::from("/Library/Java/JavaVirtualMachines"),
            PathBuf::from("/System/Library/Java/JavaVirtualMachines")];
        if let Some(home) = directories::BaseDirs::new() {
            roots.push(home.home_dir().join("Library/Java/JavaVirtualMachines"));
            roots.push(home.home_dir().join(".sdkman/candidates/java"));
        }
        if let Some(sdkman) = std::env::var_os("SDKMAN_DIR") {
            roots.push(PathBuf::from(sdkman).join("candidates/java"));
        }
        if let Some(home) = std::env::var_os("JAVA_HOME") { candidates.push(PathBuf::from(home)); }
        for root in roots { candidates.extend(children(&root)); }
        for prefix in ["/opt/homebrew", "/usr/local"] {
            // opt links cover current formulae; Cellar also retains older installs.
            for formula in children(&Path::new(prefix).join("opt")).into_iter()
                .chain(children(&Path::new(prefix).join("Cellar"))) {
                if !formula.file_name().is_some_and(|name| name.to_string_lossy().starts_with("openjdk")) { continue; }
                candidates.push(formula.join("libexec/openjdk.jdk"));
                for version in children(&formula) { candidates.push(version.join("libexec/openjdk.jdk")); }
            }
        }
        let mut seen = HashSet::new();
        let mut installations: Vec<_> = candidates.into_iter().filter_map(|path| Self::add(&path).ok())
            .filter(|jdk| seen.insert(jdk.home.clone())).collect();
        installations.sort_by(|a, b| b.version.major.cmp(&a.version.major).then(a.home.cmp(&b.home)));
        installations
    }

    pub fn add(path: &Path) -> Result<JdkInstallation, JdkError> {
        let home = normalize_home(path);
        let fail = |message: String| JdkError { home: home.clone(), message };
        if !home.join("bin/java").is_file() {
            return Err(fail("Missing bin/java".into()));
        }
        let release_path = home.join("release");
        let (version, vendor) = match fs::read_to_string(&release_path) {
            Ok(release) => {
                let value = |key: &str| release.lines().find_map(|line| {
                    let (name, value) = line.split_once('=')?;
                    (name.trim() == key).then(|| value.trim().trim_matches('"').to_owned())
                });
                let version = value("JAVA_VERSION").and_then(|raw| JavaVersion::parse(&raw))
                    .ok_or_else(|| fail("Missing or invalid JAVA_VERSION in release metadata".into()))?;
                (version, value("IMPLEMENTOR").or_else(|| value("JAVA_VENDOR")))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let info_path = home.parent().unwrap_or(home.as_path()).join("Info.plist");
                let info = plist::Value::from_file(&info_path)
                    .map_err(|error| fail(format!("Cannot read bundle metadata: {error}")))?;
                let java_vm = info.as_dictionary()
                    .and_then(|info| info.get("JavaVM"))
                    .and_then(plist::Value::as_dictionary)
                    .ok_or_else(|| fail("Missing JavaVM metadata in bundle Info.plist".into()))?;
                let version = java_vm.get("JVMVersion")
                    .and_then(plist::Value::as_string)
                    .and_then(JavaVersion::parse)
                    .ok_or_else(|| fail("Missing or invalid JVMVersion in bundle Info.plist".into()))?;
                let vendor = java_vm.get("JVMVendor")
                    .and_then(plist::Value::as_string)
                    .map(str::to_owned);
                (version, vendor)
            }
            Err(error) => return Err(fail(format!("Cannot read release metadata: {error}"))),
        };
        Ok(JdkInstallation { id: installation_id(&home), home, version, vendor, valid: true })
    }
}

fn children(path: &Path) -> Vec<PathBuf> {
    fs::read_dir(path).into_iter().flatten().filter_map(Result::ok).map(|entry| entry.path()).collect()
}

fn normalize_home(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() { path.to_path_buf() }
        else { std::env::current_dir().map(|root| root.join(path)).unwrap_or_else(|_| path.to_path_buf()) };
    let home = if absolute.join("Contents/Home").is_dir() || absolute.extension().is_some_and(|ext| ext == "jdk") {
        absolute.join("Contents/Home")
    } else { absolute };
    home.canonicalize().unwrap_or(home)
}

fn installation_id(home: &Path) -> String {
    // Fixed FNV hash gives a stable opaque identifier, including non-UTF8 paths.
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in home.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("jdk-{hash:016x}")
}

#[derive(Debug)]
pub struct JdkError { pub home: PathBuf, pub message: String }

impl fmt::Display for JdkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JDK at {}: {}", self.home.display(), self.message)
    }
}
impl std::error::Error for JdkError {}
