//! The resident's update thread. It checks the feed shortly after start and
//! every few hours, or when asked; it downloads an update and starts its
//! installer only when the user asks. What it is doing goes to a small file
//! that terminals read for their sidebar and settings.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use crate::sys;
use crate::update::{self, Channel, Release, Status, Version};

const FIRST_CHECK: Duration = Duration::from_secs(15);
const INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);
/// Far above any real installer or feed; stops a broken server from filling
/// memory.
const MAX_DOWNLOAD: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Check,
    Download,
    Install,
    /// The settings changed.
    Configure {
        channel: Channel,
        automatic: bool,
    },
}

/// Owns the update thread, which ends when this is dropped.
pub struct Updater {
    requests: Sender<Request>,
}

impl Updater {
    /// `on_change` runs on the update thread after every change of status.
    pub fn spawn(
        current: &str,
        channel: Channel,
        automatic: bool,
        on_change: impl Fn(&Status) + Send + 'static,
    ) -> Self {
        let (requests, inbox) = mpsc::channel();
        let worker = Worker {
            current: current.to_owned(),
            channel,
            automatic,
            status: Status::Idle,
            on_change: Box::new(on_change),
        };
        let _ = std::thread::Builder::new()
            .name("aeon-updates".into())
            .spawn(move || worker.run(inbox));
        Self { requests }
    }

    pub fn request(&self, request: Request) {
        let _ = self.requests.send(request);
    }
}

/// `%LOCALAPPDATA%\aeon\updates`: the status file and the downloaded MSI.
/// Development builds use their own, beside an installed Aeon's.
pub fn dir() -> Option<PathBuf> {
    let name = if cfg!(debug_assertions) {
        "updates-dev"
    } else {
        "updates"
    };
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("aeon").join(name))
}

/// What the resident last reported; `Idle` when none runs or never did.
pub fn read_status() -> Status {
    dir()
        .and_then(|d| std::fs::read_to_string(d.join("status")).ok())
        .map_or(Status::Idle, |text| Status::parse(&text))
}

struct Worker {
    current: String,
    channel: Channel,
    automatic: bool,
    status: Status,
    on_change: Box<dyn Fn(&Status) + Send>,
}

impl Worker {
    fn run(mut self, inbox: Receiver<Request>) {
        self.restore();
        let mut next_check = self.automatic.then(|| Instant::now() + FIRST_CHECK);
        loop {
            let request = match next_check {
                Some(at) => {
                    match inbox.recv_timeout(at.saturating_duration_since(Instant::now())) {
                        Ok(request) => Some(request),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                None => match inbox.recv() {
                    Ok(request) => Some(request),
                    Err(_) => return,
                },
            };
            match request {
                None => {
                    self.check(false);
                    next_check = Some(Instant::now() + INTERVAL);
                }
                Some(Request::Check) => self.check(true),
                Some(Request::Download) => self.download(),
                Some(Request::Install) => self.install(),
                Some(Request::Configure { channel, automatic }) => {
                    if automatic != self.automatic {
                        self.automatic = automatic;
                        next_check = automatic.then(|| Instant::now() + FIRST_CHECK);
                    }
                    if channel != self.channel {
                        self.channel = channel;
                        if matches!(self.status, Status::Available(_) | Status::UpToDate) {
                            self.set(Status::Idle);
                        }
                        self.check(false);
                    }
                }
            }
        }
    }

    /// Keeps an update downloaded by an earlier resident, unless it is
    /// already installed; anything else starts over.
    fn restore(&mut self) {
        match read_status() {
            Status::Ready { release, path }
                if is_newer(&release.version, &self.current) && Path::new(&path).exists() =>
            {
                self.set(Status::Ready { release, path })
            }
            _ => {
                if let Some(dir) = dir() {
                    remove_installers(&dir);
                }
                self.set(Status::Idle);
            }
        }
    }

    /// A check the user asked for shows its progress and its errors; a
    /// scheduled one stays quiet unless it finds an update.
    fn check(&mut self, asked: bool) {
        if matches!(self.status, Status::Downloading { .. }) {
            return;
        }
        if asked {
            self.set(Status::Checking);
        }
        match self.fetch() {
            Ok(Some(release)) => {
                let downloaded =
                    matches!(&self.status, Status::Ready { release: r, .. } if *r == release);
                if !downloaded {
                    self.set(Status::Available(release));
                }
            }
            Ok(None) => self.set(Status::UpToDate),
            Err(e) if asked => self.set(Status::Error(format!("Could not check: {e}"))),
            Err(_) => {}
        }
    }

    fn fetch(&self) -> Result<Option<Release>, String> {
        let stable = feed("stable.txt")?;
        let nightly = match self.channel {
            Channel::Nightly => feed("nightly.txt")?,
            Channel::Stable => None,
        };
        Ok(update::pick(self.channel, &self.current, stable, nightly))
    }

    fn download(&mut self) {
        let Status::Available(release) = &self.status else {
            return;
        };
        let release = release.clone();
        let Some(dir) = dir() else {
            return self.set(Status::Error("No LOCALAPPDATA folder".into()));
        };
        self.set(Status::Downloading {
            release: release.clone(),
            percent: 0,
        });
        let mut body = Vec::new();
        let mut shown = 0;
        let result = sys::http_get(
            &release.msi_url,
            |chunk| {
                body.extend_from_slice(chunk);
                if body.len() > MAX_DOWNLOAD {
                    return Err("the download is too large".into());
                }
                Ok(())
            },
            |done, total| {
                let Some(total) = total.filter(|t| *t > 0) else {
                    return;
                };
                let percent = (done * 100 / total).min(100) as u8;
                if percent != shown {
                    shown = percent;
                    self.set(Status::Downloading {
                        release: release.clone(),
                        percent,
                    });
                }
            },
        );
        let fail = |e: String| Status::Error(format!("Download failed: {e}"));
        let status = match result {
            Ok(200) => match sys::sha256(&body) {
                Ok(hash) if hash == release.sha256 => {
                    let path = dir.join(format!("aeon-{}.msi", release.version));
                    remove_installers(&dir);
                    match std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, &body))
                    {
                        Ok(()) => Status::Ready {
                            release,
                            path: path.to_string_lossy().into_owned(),
                        },
                        Err(e) => fail(e.to_string()),
                    }
                }
                Ok(_) => fail("the file does not match its checksum".into()),
                Err(e) => fail(e),
            },
            Ok(code) => fail(format!("GitHub answered {code}")),
            Err(e) => fail(e),
        };
        self.set(status);
    }

    /// Starts the installer, which closes every Aeon, replaces it, and starts
    /// the resident again.
    fn install(&mut self) {
        let Status::Ready { release, path } = &self.status else {
            return;
        };
        // The file sat in a user-writable folder since it was checked.
        let intact = std::fs::read(path)
            .ok()
            .and_then(|bytes| sys::sha256(&bytes).ok())
            .is_some_and(|hash| hash == release.sha256);
        if !intact {
            return self.set(Status::Error(
                "The downloaded update changed; check again".into(),
            ));
        }
        let msiexec = std::env::var_os("SystemRoot")
            .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
            .join(r"System32\msiexec.exe");
        if let Err(e) = Command::new(msiexec).arg("/i").arg(path).spawn() {
            self.set(Status::Error(format!("Could not start the installer: {e}")));
        }
    }

    fn set(&mut self, status: Status) {
        self.status = status;
        if let Some(dir) = dir() {
            let tmp = dir.join("status.tmp");
            let _ = std::fs::create_dir_all(&dir)
                .and_then(|()| std::fs::write(&tmp, self.status.to_text()))
                .and_then(|()| std::fs::rename(&tmp, dir.join("status")));
        }
        (self.on_change)(&self.status);
    }
}

/// One feed file for this machine. A missing file is no release: there is no
/// stable release before the first one ships.
fn feed(name: &str) -> Result<Option<Release>, String> {
    let mut body = Vec::new();
    let code = sys::http_get(
        &format!("{}/{name}", update::FEED),
        |chunk| {
            body.extend_from_slice(chunk);
            if body.len() > 64 * 1024 {
                return Err("the feed is too large".into());
            }
            Ok(())
        },
        |_, _| {},
    )?;
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    };
    match code {
        200 => Ok(update::parse_feed(&String::from_utf8_lossy(&body), arch)),
        404 => Ok(None),
        code => Err(format!("GitHub answered {code}")),
    }
}

fn is_newer(version: &str, current: &str) -> bool {
    match (Version::parse(version), Version::parse(current)) {
        (Some(version), Some(current)) => version > current,
        _ => false,
    }
}

fn remove_installers(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.path().extension().is_some_and(|e| e == "msi") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
