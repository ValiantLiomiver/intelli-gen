use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use super::{
    discovery,
    tree::{ProjectTree, ProjectTreeChildren},
    Project, ProjectKind,
};

#[derive(Clone, Debug)]
pub struct NewProjectRequest {
    pub parent: PathBuf,
    pub name: String,
    pub package: String,
    pub kind: ProjectKind,
}

fn valid_java_identifier(value: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "abstract", "assert", "boolean", "break", "byte", "case", "catch", "char", "class",
        "const", "continue", "default", "do", "double", "else", "enum", "extends", "final",
        "finally", "float", "for", "goto", "if", "implements", "import", "instanceof", "int",
        "interface", "long", "native", "new", "package", "private", "protected", "public",
        "return", "short", "static", "strictfp", "super", "switch", "synchronized", "this",
        "throw", "throws", "transient", "try", "void", "volatile", "while", "true", "false",
        "null", "_",
    ];
    let mut chars = value.chars();
    let Some(first) = chars.next() else { return false; };
    (first.is_alphabetic() || first == '_' || first == '$')
        && chars.all(|character| character.is_alphanumeric() || character == '_' || character == '$')
        && !KEYWORDS.contains(&value)
}

/// Create a new Maven/Gradle project without invoking build tools or replacing
/// an existing directory. Filesystem work is intended for a worker thread.
pub fn create_java_project(request: &NewProjectRequest) -> Result<PathBuf, String> {
    if request.name.is_empty()
        || !request.name.chars().all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
        || !request.name.chars().next().is_some_and(|character| character.is_ascii_alphanumeric())
    {
        return Err("Project name must start with a letter or number and contain only letters, numbers, dots, hyphens, or underscores".into());
    }
    if !request.package.is_empty() && !request.package.split('.').all(valid_java_identifier) {
        return Err("Package must contain valid Java identifiers separated by dots".into());
    }
    if !matches!(request.kind, ProjectKind::Maven | ProjectKind::Gradle) {
        return Err("Choose Maven or Gradle for the new project".into());
    }
    let parent = request.parent.canonicalize()
        .map_err(|error| format!("Cannot resolve parent folder {}: {error}", request.parent.display()))?;
    if !parent.is_dir() {
        return Err(format!("Parent path is not a directory: {}", parent.display()));
    }
    let root = parent.join(&request.name);
    fs::create_dir(&root)
        .map_err(|error| format!("Cannot create project folder {}: {error}", root.display()))?;

    let mut created_directories = vec![root.clone()];
    let mut created_files = Vec::new();
    let result = (|| {
        let mut source_directory = root.clone();
        for segment in ["src", "main", "java"] {
            source_directory.push(segment);
            fs::create_dir(&source_directory)
                .map_err(|error| format!("Cannot create project directory {}: {error}", source_directory.display()))?;
            created_directories.push(source_directory.clone());
        }
        for segment in request.package.split('.').filter(|segment| !segment.is_empty()) {
            source_directory.push(segment);
            fs::create_dir(&source_directory)
                .map_err(|error| format!("Cannot create package directory {}: {error}", source_directory.display()))?;
            created_directories.push(source_directory.clone());
        }
        let config_directory = root.join(".idea-rust");
        fs::create_dir(&config_directory)
            .map_err(|error| format!("Cannot create project configuration directory {}: {error}", config_directory.display()))?;
        created_directories.push(config_directory.clone());

        let main_source = source_directory.join("Main.java");
        let mut source = String::new();
        if !request.package.is_empty() {
            source.push_str(&format!("package {};\n\n", request.package));
        }
        source.push_str("public class Main {\n    public static void main(String[] args) {\n        System.out.println(\"Hello, Java!\");\n    }\n}\n");
        let main_class = if request.package.is_empty() {
            "Main".to_owned()
        } else {
            format!("{}.Main", request.package)
        };

        let mut files = vec![
            (main_source.clone(), source),
            (config_directory.join("run-configurations.toml"), format!(
                "[[configurations]]\nid = \"starter\"\nname = \"Application\"\nmain_class = \"{main_class}\"\nworking_directory = \".\"\n"
            )),
        ];
        match request.kind {
            ProjectKind::Maven => files.push((root.join("pom.xml"), format!(
                "<project xmlns=\"http://maven.apache.org/POM/4.0.0\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:schemaLocation=\"http://maven.apache.org/POM/4.0.0 https://maven.apache.org/xsd/maven-4.0.0.xsd\">\n  <modelVersion>4.0.0</modelVersion>\n  <groupId>{}</groupId>\n  <artifactId>{}</artifactId>\n  <version>1.0-SNAPSHOT</version>\n  <properties>\n    <maven.compiler.source>8</maven.compiler.source>\n    <maven.compiler.target>8</maven.compiler.target>\n    <project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>\n  </properties>\n</project>\n",
                if request.package.is_empty() { "com.example" } else { &request.package },
                request.name,
            ))),
            ProjectKind::Gradle => {
                files.push((root.join("settings.gradle"), format!("rootProject.name = '{}'\n", request.name)));
                files.push((root.join("build.gradle"), format!(
                    "plugins {{\n    id 'java'\n}}\n\ngroup = '{}'\nversion = '1.0-SNAPSHOT'\n\nsourceCompatibility = JavaVersion.VERSION_1_8\ntargetCompatibility = JavaVersion.VERSION_1_8\n",
                    if request.package.is_empty() { "com.example" } else { &request.package },
                )));
            }
            ProjectKind::Unknown => unreachable!(),
        }
        for (path, contents) in files {
            let mut file = OpenOptions::new().write(true).create_new(true).open(&path)
                .map_err(|error| format!("Cannot create project file {}: {error}", path.display()))?;
            created_files.push(path.clone());
            if let Err(error) = file.write_all(contents.as_bytes()).and_then(|()| file.sync_all()) {
                return Err(format!("Cannot write project file {}: {error}", path.display()));
            }
        }
        Ok(main_source)
    })();
    if result.is_err() {
        for path in created_files.iter().rev() { let _ = fs::remove_file(path); }
        for path in created_directories.iter().rev() { let _ = fs::remove_dir(path); }
    }
    result
}

#[derive(Clone, Debug)]
pub struct OpenedProject {
    pub root: PathBuf,
    pub project: Project,
    pub tree: Result<ProjectTree, String>,
    pub open_file: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct ProjectOpenEvent {
    pub generation: u64,
    pub result: Result<OpenedProject, String>,
}

pub struct ProjectOpenLoader {
    sender: Sender<ProjectOpenEvent>,
    receiver: Receiver<ProjectOpenEvent>,
    latest_generation: u64,
    workers: Vec<JoinHandle<()>>,
}

fn open_project_path(path: PathBuf) -> Result<OpenedProject, String> {
    let requested = path
        .canonicalize()
        .map_err(|error| format!("Cannot open {}: {error}", path.display()))?;
    let metadata = fs::metadata(&requested)
        .map_err(|error| format!("Cannot inspect {}: {error}", requested.display()))?;
    let (candidate_root, open_file) = if metadata.is_dir() {
        (requested, None)
    } else if metadata.is_file() {
        let parent = requested.parent()
            .ok_or_else(|| format!("File has no parent directory: {}", requested.display()))?;
        let root = parent.canonicalize()
            .map_err(|error| format!("Cannot open workspace {}: {error}", parent.display()))?;
        (root, Some(requested))
    } else {
        return Err(format!("Not a project directory or regular file: {}", requested.display()));
    };
    let candidate_project = discovery::discover(&candidate_root);
    let root = if open_file.is_some() {
        candidate_project.model.as_ref()
            .map(|model| model.root.clone())
            .unwrap_or(candidate_root)
    } else {
        candidate_root
    };
    let project = discovery::discover(&root);
    let tree = super::tree::ProjectTree::read(&root).map_err(|error| error.to_string());
    Ok(OpenedProject { root, project, tree, open_file })
}

#[derive(Clone, Debug)]
pub struct ProjectTreeEvent {
    pub root: PathBuf,
    pub generation: u64,
    pub result: Result<ProjectTree, String>,
}

#[derive(Clone, Debug)]
pub struct ProjectTreeChildrenEvent {
    pub root: PathBuf,
    pub parent: PathBuf,
    pub request: u64,
    pub result: Result<ProjectTreeChildren, String>,
}

#[derive(Clone, Debug)]
pub struct ProjectTreeRootValidationEvent {
    pub root: PathBuf,
    pub request: u64,
    pub result: Result<(), String>,
}

#[derive(Clone, Debug)]
pub enum ProjectTreeLoadEvent {
    Refreshed(ProjectTreeEvent),
    ChildrenLoaded(ProjectTreeChildrenEvent),
    RootValidated(ProjectTreeRootValidationEvent),
}

pub struct ProjectTreeLoader {
    sender: Sender<ProjectTreeLoadEvent>,
    receiver: Receiver<ProjectTreeLoadEvent>,
    latest_generation: u64,
    workers: Vec<JoinHandle<()>>,
}

impl Default for ProjectTreeLoader {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            sender,
            receiver,
            latest_generation: 0,
            workers: Vec::new(),
        }
    }
}

impl ProjectTreeLoader {
    pub fn start(&mut self, root: PathBuf, generation: u64) -> Result<(), String> {
        if generation <= self.latest_generation {
            return Err("Project-tree generation must increase for each request".into());
        }
        let sender = self.sender.clone();
        let worker_root = root.clone();
        let worker = thread::Builder::new()
            .name("project-tree-refresh".into())
            .spawn(move || {
                let result = ProjectTree::read(&worker_root).map_err(|error| error.to_string());
                let _ = sender.send(ProjectTreeLoadEvent::Refreshed(ProjectTreeEvent {
                    root: worker_root,
                    generation,
                    result,
                }));
            })
            .map_err(|error| format!("Cannot start project-tree worker: {error}"))?;
        self.latest_generation = generation;
        self.workers.push(worker);
        Ok(())
    }

    pub fn start_children(
        &mut self,
        root: PathBuf,
        parent: PathBuf,
        request: u64,
    ) -> Result<(), String> {
        let sender = self.sender.clone();
        let worker_root = root.clone();
        let worker_parent = parent.clone();
        let worker = thread::Builder::new()
            .name("project-tree-children".into())
            .spawn(move || {
                let result = ProjectTree::read_children(&worker_root, &worker_parent, request);
                let event = ProjectTreeChildrenEvent {
                    root: worker_root,
                    parent: worker_parent,
                    request,
                    result,
                };
                let _ = sender.send(ProjectTreeLoadEvent::ChildrenLoaded(event));
            })
            .map_err(|error| format!("Cannot start project-tree child worker: {error}"))?;
        self.workers.push(worker);
        Ok(())
    }

    pub fn start_root_validation(&mut self, root: PathBuf, request: u64) -> Result<(), String> {
        let sender = self.sender.clone();
        let worker_root = root.clone();
        let worker = thread::Builder::new()
            .name("project-tree-root-check".into())
            .spawn(move || {
                let result = ProjectTree::validate_root(&worker_root);
                let event = ProjectTreeRootValidationEvent {
                    root: worker_root,
                    request,
                    result,
                };
                let _ = sender.send(ProjectTreeLoadEvent::RootValidated(event));
            })
            .map_err(|error| format!("Cannot start project-tree validation worker: {error}"))?;
        self.workers.push(worker);
        Ok(())
    }

    pub fn poll(&mut self) -> Vec<ProjectTreeLoadEvent> {
        let events = self
            .receiver
            .try_iter()
            .filter(|event| match event {
                ProjectTreeLoadEvent::Refreshed(event) => {
                    event.generation == self.latest_generation
                }
                ProjectTreeLoadEvent::ChildrenLoaded(_)
                | ProjectTreeLoadEvent::RootValidated(_) => true,
            })
            .collect();
        let mut active = Vec::new();
        for worker in self.workers.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                active.push(worker);
            }
        }
        self.workers = active;
        events
    }

    pub fn has_workers(&self) -> bool {
        !self.workers.is_empty()
    }
}

impl Default for ProjectOpenLoader {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            sender,
            receiver,
            latest_generation: 0,
            workers: Vec::new(),
        }
    }
}

impl ProjectOpenLoader {
    pub fn start(&mut self, path: PathBuf, generation: u64) -> Result<(), String> {
        if generation <= self.latest_generation {
            return Err("Project-open generation must increase for each request".into());
        }
        let sender = self.sender.clone();
        let worker = thread::Builder::new()
            .name("project-open".into())
            .spawn(move || {
                let result = open_project_path(path);
                let _ = sender.send(ProjectOpenEvent { generation, result });
            })
            .map_err(|error| format!("Cannot start project-open worker: {error}"))?;
        self.latest_generation = generation;
        self.workers.push(worker);
        Ok(())
    }

    pub fn start_new(&mut self, request: NewProjectRequest, generation: u64) -> Result<(), String> {
        if generation <= self.latest_generation {
            return Err("Project-open generation must increase for each request".into());
        }
        let sender = self.sender.clone();
        let worker = thread::Builder::new()
            .name("java-project-create".into())
            .spawn(move || {
                let result = create_java_project(&request).and_then(open_project_path);
                let _ = sender.send(ProjectOpenEvent { generation, result });
            })
            .map_err(|error| format!("Cannot start project creation worker: {error}"))?;
        self.latest_generation = generation;
        self.workers.push(worker);
        Ok(())
    }

    pub fn poll(&mut self) -> Vec<ProjectOpenEvent> {
        let events = self
            .receiver
            .try_iter()
            .filter(|event| event.generation == self.latest_generation)
            .collect();
        let mut active = Vec::new();
        for worker in self.workers.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                active.push(worker);
            }
        }
        self.workers = active;
        events
    }

    pub fn has_workers(&self) -> bool {
        !self.workers.is_empty()
    }
}
