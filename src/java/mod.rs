pub mod build;
pub mod diagnostics;
pub mod jdk;
pub mod lsp;
pub mod lsp_config;
pub mod run_config;

pub(crate) fn kill_process_group(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid.filter(|pid| *pid > 1) {
        // SAFETY: each caller starts the child in a process group with its own PID.
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}
