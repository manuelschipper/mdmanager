use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::GlobalConfig;
use crate::context::{Audit, ClaudeScan, ContextRuntime, ContextSource, scan_claude_descendants};

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_INTERVAL: Duration = Duration::from_millis(80);

#[derive(Debug)]
pub(crate) enum ContextScanStatus {
    NotApplicable,
    NotStarted,
    Scanning,
    Complete { unreadable: Vec<PathBuf> },
    Failed,
}

pub(crate) struct ContextUi {
    pub(crate) paths: crate::config::Paths,
    pub(crate) audit: Audit,
    pub(crate) runtime_index: usize,
    pub(crate) source_index: usize,
    pub(crate) scan_status: ContextScanStatus,
    scan_receiver: Option<Receiver<ClaudeScan>>,
    scan_started_at: Option<Instant>,
}

impl ContextUi {
    pub(crate) fn new_at(
        paths: crate::config::Paths,
        global: Option<&GlobalConfig>,
        directory: PathBuf,
    ) -> Result<Self, String> {
        let runtime_index = 0;
        let audit = Audit::resolve(
            ContextRuntime::ALL[runtime_index],
            &directory,
            &paths,
            global,
        );
        Ok(Self {
            paths,
            audit,
            runtime_index,
            source_index: 0,
            scan_status: ContextScanStatus::NotStarted,
            scan_receiver: None,
            scan_started_at: None,
        })
    }

    pub(crate) fn selected(&self) -> Option<&ContextSource> {
        self.audit.sources.get(self.source_index)
    }

    pub(crate) fn select_runtime(&mut self, index: usize, global: Option<&GlobalConfig>) {
        self.runtime_index = index.min(ContextRuntime::ALL.len().saturating_sub(1));
        self.source_index = 0;
        self.reload(global);
        self.start_scan();
    }

    pub(crate) fn reload(&mut self, global: Option<&GlobalConfig>) {
        let selected = self.selected().map(|source| source.path.clone());
        self.scan_receiver = None;
        self.scan_started_at = None;
        self.audit = Audit::resolve(
            ContextRuntime::ALL[self.runtime_index],
            &self.audit.directory,
            &self.paths,
            global,
        );
        self.source_index = selected
            .and_then(|path| {
                self.audit
                    .sources
                    .iter()
                    .position(|source| source.path == path)
            })
            .unwrap_or_else(|| {
                self.source_index
                    .min(self.audit.sources.len().saturating_sub(1))
            });
        self.scan_status = if self.audit.runtime == ContextRuntime::Claude {
            ContextScanStatus::NotStarted
        } else {
            ContextScanStatus::NotApplicable
        };
    }

    pub(crate) fn reload_and_scan(&mut self, global: Option<&GlobalConfig>) {
        self.reload(global);
        self.start_scan();
    }

    pub(crate) fn start_scan(&mut self) {
        if self.audit.runtime != ContextRuntime::Claude
            || !matches!(self.scan_status, ContextScanStatus::NotStarted)
        {
            return;
        }
        let directory = self.audit.directory.clone();
        let paths = self.paths.clone();
        let seen = self
            .audit
            .sources
            .iter()
            .map(|source| source.path.clone())
            .collect::<HashSet<_>>();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(scan_claude_descendants(&directory, &paths, seen));
        });
        self.scan_receiver = Some(receiver);
        self.scan_started_at = Some(Instant::now());
        self.scan_status = ContextScanStatus::Scanning;
    }

    pub(crate) fn poll_scan(&mut self, global: Option<&GlobalConfig>) -> bool {
        let Some(receiver) = &self.scan_receiver else {
            return false;
        };
        match receiver.try_recv() {
            Ok(scan) => {
                let unreadable = scan.unreadable.clone();
                self.audit.add_claude_scan(scan, global);
                self.scan_receiver = None;
                self.scan_started_at = None;
                self.scan_status = ContextScanStatus::Complete { unreadable };
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.scan_receiver = None;
                self.scan_started_at = None;
                self.scan_status = ContextScanStatus::Failed;
                true
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn is_scanning(&self) -> bool {
        matches!(self.scan_status, ContextScanStatus::Scanning)
    }

    pub(crate) fn scan_spinner(&self) -> Option<&'static str> {
        self.scan_started_at
            .map(|started| spinner_frame(started.elapsed()))
    }
}

fn spinner_frame(elapsed: Duration) -> &'static str {
    let frame = elapsed.as_millis() / SPINNER_INTERVAL.as_millis();
    SPINNER_FRAMES[(frame % SPINNER_FRAMES.len() as u128) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_advances_and_wraps() {
        assert_eq!(spinner_frame(Duration::ZERO), "⠋");
        assert_eq!(spinner_frame(SPINNER_INTERVAL), "⠙");
        assert_eq!(
            spinner_frame(SPINNER_INTERVAL * SPINNER_FRAMES.len() as u32),
            "⠋"
        );
    }
}
