use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

#[derive(Default)]
pub struct FinderReveal {
    channel: Option<(Sender<Result<(), String>>, Receiver<Result<(), String>>)>,
    workers: Vec<JoinHandle<()>>,
}

impl FinderReveal {
    pub fn start(&mut self, path: PathBuf) -> Result<(), String> {
        let sender = self.channel.get_or_insert_with(mpsc::channel).0.clone();
        let worker = thread::Builder::new()
            .name("finder-reveal".into())
            .spawn(move || {
                #[cfg(target_os = "macos")]
                let result = std::process::Command::new("/usr/bin/open")
                    .arg("-R")
                    .arg(&path)
                    .output()
                    .map_err(|error| format!("Finder could not reveal {}: {error}", path.display()))
                    .and_then(|output| {
                        if output.status.success() {
                            Ok(())
                        } else {
                            Err(format!(
                                "Finder could not reveal {}: {}",
                                path.display(),
                                output.status
                            ))
                        }
                    });
                #[cfg(not(target_os = "macos"))]
                let result = Err(format!(
                    "Reveal in Finder is available on macOS: {}",
                    path.display()
                ));
                let _ = sender.send(result);
            })
            .map_err(|error| format!("Cannot start Finder reveal: {error}"))?;
        self.workers.push(worker);
        Ok(())
    }

    pub fn poll(&mut self) -> Vec<Result<(), String>> {
        let mut results = Vec::new();
        if let Some((_, receiver)) = &self.channel {
            while let Ok(result) = receiver.try_recv() {
                results.push(result);
            }
        }
        let mut active = Vec::new();
        for worker in self.workers.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                active.push(worker);
            }
        }
        self.workers = active;
        results
    }

    pub fn has_workers(&self) -> bool {
        !self.workers.is_empty()
    }
}
