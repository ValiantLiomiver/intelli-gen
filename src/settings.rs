use std::collections::HashSet;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::java::jdk::{JdkInstallation, ProjectJdkSelection};

const MAX_RECENT_PROJECTS: usize = 12;

/// Preferences only: window geometry belongs to egui, and project state to Project.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct UserSettings {
    pub recent_projects: Vec<PathBuf>,
    pub dark_mode: bool,
    pub show_project_panel: bool,
    pub show_tool_window: bool,
    pub jdks: Vec<JdkInstallation>,
    pub default_jdk_id: Option<String>,
    /// Dedicated JDT LS runtime; bundled launcher requires Java 25+, custom launcher Java 21+.
    pub jdt_ls_jdk_id: Option<String>,
    /// Explicit installed launcher accepting stdio and `-data DIR`.
    pub jdt_ls_executable: Option<PathBuf>,
    /// Explicit external java-debug plugin JAR. No component is downloaded.
    pub java_debug_bundle: Option<PathBuf>,
    pub project_jdks: Vec<ProjectJdkSettings>,
    /// Used only when a project has no wrapper; values are executable paths, never shell commands.
    pub maven_executable: Option<PathBuf>,
    pub gradle_executable: Option<PathBuf>,
    /// Java indentation width, clamped to 1..=8 when used in the editor.
    pub java_indent_size: usize,
    /// Insert spaces instead of literal tabs for Java indentation.
    pub java_insert_spaces: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectJdkSettings {
    pub root: PathBuf,
    pub selection: ProjectJdkSelection,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            recent_projects: Vec::new(),
            dark_mode: true,
            show_project_panel: true,
            show_tool_window: true,
            jdks: Vec::new(),
            default_jdk_id: None,
            jdt_ls_jdk_id: None,
            jdt_ls_executable: None,
            java_debug_bundle: None,
            project_jdks: Vec::new(),
            maven_executable: None,
            gradle_executable: None,
            java_indent_size: 4,
            java_insert_spaces: true,
        }
    }
}

impl UserSettings {
    pub fn config_path() -> Result<PathBuf, SettingsError> {
        let dirs = ProjectDirs::from("dev", "Intelli-gen", "Intelli-gen")
            .ok_or(SettingsError::ConfigDirectoryUnavailable)?;
        Ok(dirs.config_dir().join("settings.toml"))
    }

    pub fn load() -> Result<Self, SettingsError> {
        let path = Self::config_path()?;
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(SettingsError::Io { path, source }),
        };
        let mut settings: Self =
            toml::from_str(&text).map_err(|source| SettingsError::Parse { path, source })?;
        settings.normalize_recent_projects();
        Ok(settings)
    }

    pub fn save(&self) -> Result<(), SettingsError> {
        let path = Self::config_path()?;
        let text = toml::to_string_pretty(self).map_err(SettingsError::Serialize)?;
        let parent = path
            .parent()
            .ok_or(SettingsError::ConfigDirectoryUnavailable)?;
        fs::create_dir_all(parent).map_err(|source| SettingsError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        // Write a sibling first so a failed write cannot truncate existing preferences.
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temporary = parent.join(format!(".settings-{}-{nonce}.tmp", std::process::id()));
        let result = (|| -> io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            fs::rename(&temporary, &path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(|source| SettingsError::Io { path, source })
    }

    pub fn remember_project(&mut self, path: PathBuf) {
        self.recent_projects.insert(0, path);
        self.normalize_recent_projects();
    }

    pub fn project_jdk_id(&self, root: &std::path::Path) -> Option<&str> {
        self.project_jdks.iter().find(|entry| entry.root == root)
            .map(|entry| entry.selection.default_jdk_id.as_str())
            .filter(|id| !id.is_empty())
    }

    pub fn set_project_jdk(&mut self, root: PathBuf, id: Option<String>) {
        self.project_jdks.retain(|entry| entry.root != root);
        if let Some(default_jdk_id) = id {
            self.project_jdks.push(ProjectJdkSettings {
                root, selection: ProjectJdkSelection { default_jdk_id },
            });
        }
    }

    /// An explicit missing/invalid override is an error, never a silent fallback.
    pub fn selected_jdk(&self, root: &std::path::Path, override_id: Option<&str>) -> Result<&JdkInstallation, String> {
        let id = override_id.or_else(|| self.project_jdk_id(root))
            .or(self.default_jdk_id.as_deref())
            .ok_or_else(|| "Choose a JDK in Settings or Project Structure".to_owned())?;
        let jdk = self.jdks.iter().find(|jdk| jdk.id == id)
            .ok_or_else(|| format!("Selected JDK {id} is no longer registered; select a replacement"))?;
        if !jdk.valid {
            return Err(format!("Selected JDK {} is marked unavailable; validate or replace it", jdk.home.display()));
        }
        Ok(jdk)
    }

    /// Resolve one launch's override without changing the catalog or project defaults.
    pub fn run_jdk(&self, root: &std::path::Path, configuration: &crate::java::run_config::RunConfiguration) -> Result<&JdkInstallation, String> {
        self.selected_jdk(root, configuration.jdk_id.as_deref())
    }

    pub fn jdt_ls_jdk(&self) -> Result<&JdkInstallation, String> {
        let minimum = self.jdt_ls_minimum_major();
        let id = self.jdt_ls_jdk_id.as_deref()
            .ok_or_else(|| format!("Choose a Java {minimum} or newer runtime for JDT LS in Settings"))?;
        let jdk = self.jdks.iter().find(|jdk| jdk.id == id)
            .ok_or_else(|| "The JDT LS runtime is no longer registered".to_owned())?;
        if jdk.valid && jdk.version.major >= minimum {
            return Ok(jdk);
        }
        self.jdks.iter()
            .filter(|candidate| candidate.valid && candidate.version.major >= minimum)
            .max_by_key(|candidate| candidate.version.major)
            .ok_or_else(|| {
                if !jdk.valid {
                    format!("The selected JDT LS runtime is unavailable, and no valid Java {minimum}+ runtime is installed")
                } else {
                    format!("{} JDT LS requires Java {minimum} or newer; selected runtime is Java {} and no compatible runtime is installed", if self.jdt_ls_executable.is_some() { "Custom" } else { "Bundled" }, jdk.version.major)
                }
            })
    }

    pub fn jdt_ls_minimum_major(&self) -> u32 {
        if self.jdt_ls_executable.is_some() { 21 } else { 25 }
    }

    fn normalize_recent_projects(&mut self) {
        let mut seen = HashSet::new();
        self.recent_projects = std::mem::take(&mut self.recent_projects)
            .into_iter()
            .filter(|path| !path.as_os_str().is_empty())
            // Resolve aliases when available, but retain unavailable projects so
            // removable disks and temporarily inaccessible paths remain in recents.
            .map(|path| path.canonicalize().unwrap_or(path))
            .filter(|path| seen.insert(path.clone()))
            .take(MAX_RECENT_PROJECTS)
            .collect();
    }
}

/// Runs preference writes away from the UI and serializes snapshots so an older
/// write can never overwrite a newer one.
#[derive(Default)]
pub struct SettingsSaver {
    generation: u64,
    latest: Arc<AtomicU64>,
    write_lock: Arc<Mutex<()>>,
    tx_rx: Option<(mpsc::Sender<(u64, Result<(), String>)>, mpsc::Receiver<(u64, Result<(), String>)>)>,
    workers: Vec<JoinHandle<()>>,
}

impl SettingsSaver {
    pub fn start(&mut self, settings: UserSettings) -> Result<u64, String> {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.latest.store(generation, Ordering::SeqCst);
        let tx = self.tx_rx.get_or_insert_with(mpsc::channel).0.clone();
        let latest = Arc::clone(&self.latest);
        let write_lock = Arc::clone(&self.write_lock);
        let worker_tx = tx.clone();
        let worker = thread::Builder::new()
            .name("settings-save".into())
            .spawn(move || {
                let result = match write_lock.lock() {
                    Ok(_guard) if latest.load(Ordering::SeqCst) == generation => settings.save().map_err(|error| error.to_string()),
                    Ok(_) => return,
                    Err(_) => Err("Settings save lock is unavailable".to_owned()),
                };
                let _ = worker_tx.send((generation, result));
            })
            .map_err(|error| error.to_string())?;
        self.workers.push(worker);
        Ok(generation)
    }

    pub fn poll(&mut self) -> Vec<(u64, Result<(), String>)> {
        let mut results = Vec::new();
        if let Some((_, receiver)) = &self.tx_rx {
            while let Ok(result) = receiver.try_recv() { results.push(result); }
        }
        let mut active = Vec::new();
        for worker in self.workers.drain(..) {
            if worker.is_finished() { let _ = worker.join(); } else { active.push(worker); }
        }
        self.workers = active;
        results
    }

    pub fn has_workers(&self) -> bool { !self.workers.is_empty() }
}

#[derive(Debug)]
pub enum SettingsError {
    ConfigDirectoryUnavailable,
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    Serialize(toml::ser::Error),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigDirectoryUnavailable => {
                write!(f, "Application config directory is unavailable")
            }
            Self::Io { path, source } => write!(f, "Settings at {}: {source}", path.display()),
            Self::Parse { path, source } => {
                write!(f, "Cannot parse settings at {}: {source}", path.display())
            }
            Self::Serialize(source) => write!(f, "Cannot encode settings: {source}"),
        }
    }
}

impl std::error::Error for SettingsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::Serialize(source) => Some(source),
            Self::ConfigDirectoryUnavailable => None,
        }
    }
}
