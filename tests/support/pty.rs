use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use nix::poll::{PollFd, PollFlags, poll};
use nix::pty::openpty;

pub struct Pty {
    child: Child,
    terminal: File,
    output: String,
    _runtime_path: tempfile::TempDir,
}

impl Pty {
    // Both streams must be terminals to reach the CLI's interactive confirmation.
    pub fn spawn(home: &Path, directory: &Path, arguments: &[&str]) -> Self {
        let pair = openpty(None, None).unwrap();
        // Clones are close-on-exec, keeping PTYs out of parallel CLI children.
        let terminal = File::from(pair.master).try_clone().unwrap();
        let slave = File::from(pair.slave).try_clone().unwrap();
        let runtime_path = super::runtime_path();
        let child = Command::new(env!("CARGO_BIN_EXE_mdmanager"))
            .args(arguments)
            .env("HOME", home)
            .env_remove("CODEX_HOME")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("PI_CODING_AGENT_DIR")
            .env("PATH", runtime_path.path())
            .current_dir(directory)
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave))
            .spawn()
            .unwrap();
        Self {
            child,
            terminal,
            output: String::new(),
            _runtime_path: runtime_path,
        }
    }

    pub fn wait_for_prompt(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.output.contains("[y/N] ") {
            self.read_output(deadline);
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "exited before confirmation: {}",
                self.output
            );
        }
    }

    pub fn answer(&mut self, answer: &[u8]) {
        self.terminal.write_all(answer).unwrap();
    }

    pub fn finish(&mut self) -> (ExitStatus, String) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.read_output(deadline);
            if let Some(status) = self.child.try_wait().unwrap() {
                // Drain output already queued before the slave closed.
                loop {
                    let mut fds = [PollFd::new(self.terminal.as_fd(), PollFlags::POLLIN)];
                    poll(&mut fds, 0u16).unwrap();
                    if !fds[0].revents().unwrap().contains(PollFlags::POLLIN) {
                        break;
                    }
                    if !self.read_output(deadline) {
                        break;
                    }
                }
                return (status, self.output.clone());
            }
        }
    }

    fn read_output(&mut self, deadline: Instant) -> bool {
        assert!(Instant::now() < deadline, "PTY timeout: {}", self.output);
        let mut fds = [PollFd::new(self.terminal.as_fd(), PollFlags::POLLIN)];
        poll(&mut fds, 50u16).unwrap();
        if fds[0].revents().unwrap().contains(PollFlags::POLLIN) {
            let mut bytes = [0; 4096];
            let count = self.terminal.read(&mut bytes).unwrap();
            self.output
                .push_str(&String::from_utf8_lossy(&bytes[..count]));
            return count != 0;
        }
        false
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
