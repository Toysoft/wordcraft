use super::RecoveryInfo;
use crate::Session;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use wordcraft_doc::Document;

const INTERVAL: Duration = Duration::from_secs(10);
/// A snapshot takes at most this share of the worker's time: after one that took `t`, the next
/// waits at least `t * DUTY`. A two-second snapshot of a large document every ten seconds kept a
/// fifth of a core busy while typing.
const DUTY: u32 = 20;
/// The longest wait between snapshots of a changed document, however slow they are.
const MAX_INTERVAL: Duration = Duration::from_secs(300);
const MAX_BYTES: u64 = 256 << 20;
const MAX_RECORDS: usize = 128;
const MAX_FILES: usize = 16;
const DISK_BUDGET: u64 = 2 << 30;
/// Times a record lock held for a moment (by a process being started) is tried again.
const LOCK_RETRIES: u32 = 10;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    version: u32,
    pub info: RecoveryInfo,
    generation: u64,
    pub document: Document,
}
struct Entry {
    info: RecoveryInfo,
    path: PathBuf,
}
struct Store {
    root: PathBuf,
    entries: BTreeMap<String, Entry>,
    // Advisory locks prevent another running instance from restoring/discarding a live copy.
    locks: BTreeMap<String, File>,
}
fn private_dir(path: &Path) -> io::Result<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(path)
}
/// Make an existing recovery folder private too: `private_dir`'s mode applies only to folders it
/// creates, and copies of unsaved documents must not be readable by other users.
fn tighten(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = fs::metadata(path)?;
        if !meta.is_dir() {
            return Err(io::Error::other("the recovery folder is not a folder"));
        }
        if meta.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(path, fs::Permissions::from_mode(meta.permissions().mode() & 0o700))?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}
fn private_file(path: &Path, exclusive: bool) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.read(true).write(true);
    if exclusive {
        o.create_new(true);
    } else {
        o.create(true).truncate(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}
fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}
fn id_ok(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
fn new_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn generation(path: &Path) -> Option<u64> {
    path.file_name()?.to_str()?.strip_prefix("snapshot-")?.strip_suffix(".json")?.parse().ok()
}
fn read_snapshot(path: &Path) -> Result<Snapshot, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return Err("not a regular recovery file, or it exceeds 256 MiB".into());
    }
    let file = File::open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("not a regular recovery file".into());
    }
    let s: Snapshot = serde_json::from_reader(BufReader::new(file.take(MAX_BYTES + 1))).map_err(|e| e.to_string())?;
    if s.version != 1 || !id_ok(&s.info.id) {
        return Err("unsupported recovery file".into());
    }
    Ok(s)
}
impl Store {
    fn new(root: PathBuf) -> Result<Self, String> {
        private_dir(&root).map_err(|e| e.to_string())?;
        tighten(&root).map_err(|e| e.to_string())?;
        Ok(Self { root, entries: BTreeMap::new(), locks: BTreeMap::new() })
    }
    fn directories(&self) -> Result<Vec<PathBuf>, String> {
        let mut dirs = Vec::new();
        for e in fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let e = e.map_err(|e| e.to_string())?;
            if e.file_type().map_err(|e| e.to_string())?.is_dir() && e.file_name().to_str().is_some_and(id_ok) {
                if dirs.len() >= MAX_RECORDS {
                    return Err("recovery folder has too many records".into());
                }
                dirs.push(e.path());
            }
        }
        Ok(dirs)
    }
    fn files(dir: &Path) -> Result<Vec<PathBuf>, String> {
        let mut paths = Vec::new();
        for e in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let e = e.map_err(|e| e.to_string())?;
            if e.file_type().map_err(|e| e.to_string())?.is_file() && generation(&e.path()).is_some() {
                if paths.len() >= MAX_FILES {
                    return Err("recovery record has too many generations".into());
                }
                paths.push(e.path());
            }
        }
        paths.sort_by_key(|p| std::cmp::Reverse(generation(p)));
        Ok(paths)
    }
    /// Locks a record's `owner.lock`, retrying [`LOCK_RETRIES`] times 10 ms apart.
    fn lock(&self, id: &str) -> Result<File, String> {
        if !id_ok(id) {
            return Err("invalid recovery id".into());
        }
        let file = private_file(&self.root.join(id).join("owner.lock"), false).map_err(|e| e.to_string())?;
        // A process another part of the app (or a test) is starting holds the lock for a moment
        // (a forked child shares it until it execs): wait that out, but not a live owner.
        for _ in 0..LOCK_RETRIES {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => return Err(format!("recovery copy is in use or cannot be locked: {e}")),
            }
        }
        file.try_lock().map_err(|e| format!("recovery copy is in use or cannot be locked: {e}"))?;
        Ok(file)
    }
    fn acquire(&mut self, id: &str) -> Result<(), String> {
        if !self.locks.contains_key(id) {
            let lock = self.lock(id)?;
            self.locks.insert(id.to_string(), lock);
        }
        Ok(())
    }
    fn scan(&mut self) -> Result<(), String> {
        let mut entries = BTreeMap::new();
        let mut unreadable = false;
        for dir in self.directories()? {
            let Some(id) = dir.file_name().and_then(|s| s.to_str()) else { continue };
            // One record that can't be read (say, too many generations) is skipped and reported;
            // it must not hide the others.
            match self.scan_record(&dir, id, &mut entries) {
                Ok(complete) => unreadable |= !complete,
                Err(e) => {
                    unreadable = true;
                    log::warn!("recovery: skipping record {}: {e}", dir.display());
                }
            }
        }
        self.entries = entries;
        if unreadable { Err("Some recovery files could not be read. Earlier valid copies are listed when available.".into()) } else { Ok(()) }
    }
    /// List record `id`'s newest valid generation in `entries`. `Ok(false)` when a newer one
    /// couldn't be read.
    fn scan_record(&mut self, dir: &Path, id: &str, entries: &mut BTreeMap<String, Entry>) -> Result<bool, String> {
        let guard = if self.locks.contains_key(id) {
            None
        } else {
            match self.lock(id) {
                Ok(f) => Some(f),
                Err(e) => {
                    log::debug!("recovery: {e}");
                    return Ok(true);
                }
            }
        };
        let mut complete = true;
        // A crashed writer can leave an incomplete temporary file. Only clean records
        // we have locked, so another instance's write is never interrupted.
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_type().map_err(|e| e.to_string())?.is_file()
                && entry.file_name().to_str().is_some_and(|n| n.strip_prefix("pending-").and_then(|n| n.strip_suffix(".tmp")).is_some_and(id_ok))
            {
                fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
            }
        }
        for path in Self::files(dir)? {
            match read_snapshot(&path) {
                Ok(s) if s.info.id == id && Some(s.generation) == generation(&path) => {
                    entries.insert(id.to_string(), Entry { info: s.info, path });
                    break;
                }
                Ok(_) => {
                    complete = false;
                    log::warn!("recovery: mismatched record {}", path.display());
                }
                Err(e) => {
                    complete = false;
                    log::warn!("recovery: unreadable {}: {e}", path.display());
                }
            }
        }
        drop(guard);
        // A crash before the first commit leaves no document to recover. Reclaim
        // that slot only under a fresh lock and the discard path's entry checks.
        if Self::files(dir)?.is_empty()
            && let Err(e) = self.discard(id)
        {
            log::warn!("recovery: cannot remove empty record: {e}");
        }
        Ok(complete)
    }
    fn inventory(&self) -> Vec<RecoveryInfo> {
        let mut v: Vec<_> = self.entries.values().map(|e| e.info.clone()).collect();
        v.sort_by(|a, b| b.saved_at.cmp(&a.saved_at).then(a.id.cmp(&b.id)));
        v
    }
    fn disk_used(&self) -> Result<u64, String> {
        let mut used = 0u64;
        for dir in self.directories()? {
            for p in Self::files(&dir)? {
                used = used.saturating_add(fs::metadata(p).map_err(|e| e.to_string())?.len());
            }
        }
        Ok(used)
    }
    fn save(&mut self, s: Snapshot) -> Result<(), String> {
        let id = s.info.id.clone();
        let result = self.write_snapshot(s);
        // Include failures before temp-file creation (quota checks, locking, etc.).
        // A record containing any committed generation is never removed here.
        if result.is_err()
            && Self::files(&self.root.join(&id)).is_ok_and(|files| files.is_empty())
            && let Err(e) = self.discard(&id)
        {
            log::warn!("recovery: cannot remove failed empty record: {e}");
        }
        result
    }
    fn write_snapshot(&mut self, mut s: Snapshot) -> Result<(), String> {
        let dir = self.root.join(&s.info.id);
        if !dir.exists() {
            if self.directories()?.len() >= MAX_RECORDS {
                return Err("recovery folder is full; discard an older copy".into());
            }
            private_dir(&dir).map_err(|e| e.to_string())?;
        }
        tighten(&dir).map_err(|e| e.to_string())?;
        self.acquire(&s.info.id)?;
        let files = Self::files(&dir)?;
        if files.len() >= MAX_FILES {
            return Err("older recovery generations could not be pruned; keeping the last valid copy".into());
        }
        s.generation = files.first().and_then(|p| generation(p)).unwrap_or(0).checked_add(1).ok_or("recovery generation overflow")?;
        let previous = self.entries.get(&s.info.id).map(|e| e.path.clone());
        let limit = MAX_BYTES.min(DISK_BUDGET.saturating_sub(self.disk_used()?));
        let tmp = dir.join(format!("pending-{}.tmp", new_id()?));
        let final_path = dir.join(format!("snapshot-{:020}.json", s.generation));
        let result = (|| {
            let file = private_file(&tmp, true).map_err(|e| e.to_string())?;
            let mut writer = Limited { inner: BufWriter::new(file), remaining: limit };
            serde_json::to_writer(&mut writer, &s).map_err(|e| e.to_string())?;
            writer.flush().map_err(|e| e.to_string())?;
            writer.inner.get_ref().sync_all().map_err(|e| e.to_string())?;
            drop(writer);
            let check = read_snapshot(&tmp)?;
            if check.document != s.document {
                return Err("recovery format did not preserve the full document".into());
            }
            // A new generation is never written over the last valid file.
            fs::rename(&tmp, &final_path).map_err(|e| e.to_string())?;
            sync_dir(&dir).map_err(|e| e.to_string())?;
            self.entries.insert(s.info.id.clone(), Entry { info: s.info, path: final_path.clone() });
            for old in Self::files(&dir)?.into_iter().filter(|p| p != &final_path && Some(p) != previous.as_ref()) {
                if let Err(e) = fs::remove_file(old) {
                    log::warn!("recovery: cannot prune old generation: {e}");
                }
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
    fn load(&mut self, id: &str) -> Result<Snapshot, String> {
        if !self.entries.contains_key(id) {
            return Err("no such recovery copy".into());
        }
        self.acquire(id)?;
        read_snapshot(&self.entries.get(id).ok_or("no such recovery copy")?.path)
    }
    fn discard(&mut self, id: &str) -> Result<(), String> {
        if !id_ok(id) {
            return Err("invalid recovery id".into());
        }
        let dir = self.root.join(id);
        if !dir.exists() {
            self.entries.remove(id);
            return Ok(());
        }
        self.acquire(id)?;
        for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let known = name == "owner.lock"
                || generation(&entry.path()).is_some()
                || name.to_str().is_some_and(|n| n.strip_prefix("pending-").and_then(|n| n.strip_suffix(".tmp")).is_some_and(id_ok));
            if !known || !entry.file_type().map_err(|e| e.to_string())?.is_file() {
                return Err("recovery folder contains an unexpected entry".into());
            }
        }
        for p in Self::files(&dir)? {
            fs::remove_file(p).map_err(|e| e.to_string())?;
        }
        for e in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let e = e.map_err(|e| e.to_string())?;
            let name = e.file_name();
            if name.to_str().is_some_and(|n| n.strip_prefix("pending-").and_then(|n| n.strip_suffix(".tmp")).is_some_and(id_ok)) {
                fs::remove_file(e.path()).map_err(|e| e.to_string())?;
            }
        }
        sync_dir(&dir).map_err(|e| e.to_string())?;
        self.entries.remove(id);
        self.locks.remove(id);
        fs::remove_file(dir.join("owner.lock")).map_err(|e| e.to_string())?;
        fs::remove_dir(&dir).map_err(|e| e.to_string())?;
        sync_dir(&self.root).map_err(|e| e.to_string())?;
        Ok(())
    }
}
struct Limited<W> {
    inner: W,
    remaining: u64,
}
impl<W: Write> Write for Limited<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(io::Error::other("recovery snapshot or folder size limit exceeded"));
        }
        let n = self.inner.write(bytes)?;
        self.remaining -= n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
enum Job {
    Save(Box<Snapshot>),
    Scan,
    Load(String, (u64, u64)),
    Discard(String),
}
enum Outcome {
    Done,
    Discarded(String),
    Loaded(Box<Snapshot>, (u64, u64)),
}
struct Event {
    documents: Vec<RecoveryInfo>,
    outcome: Result<Outcome, String>,
    /// How long a snapshot took to write (also when it failed), to pace the next ([`DUTY`]).
    saved_in: Option<Duration>,
}
pub(crate) struct Recovery {
    /// `None` once dropped, which ends the worker ([`Drop`]).
    tx: Option<mpsc::SyncSender<Job>>,
    worker: Option<std::thread::JoinHandle<()>>,
    rx: mpsc::Receiver<Event>,
    pub documents: Vec<RecoveryInfo>,
    pub ready: bool,
    pub busy: bool,
    pub error: Option<String>,
    document: Option<u64>,
    id: Option<String>,
    revision: Option<u64>,
    due: Instant,
    alive: bool,
    /// Copies of documents that were saved and then replaced before a tick discarded them:
    /// removed when the worker is free, so they aren't offered as unsaved work at the next start.
    stale: Vec<String>,
}
impl Recovery {
    pub fn start(root: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        let (tx, jobs) = mpsc::sync_channel(1);
        let (events, rx) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("wordcraft-recovery".into())
            .spawn(move || {
                let mut store = match Store::new(root) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = events.send(Event { documents: Vec::new(), outcome: Err(e), saved_in: None });
                        wake();
                        return;
                    }
                };
                let initial = store.scan().map(|_| Outcome::Done);
                if events.send(Event { documents: store.inventory(), outcome: initial, saved_in: None }).is_err() {
                    return;
                }
                wake();
                while let Ok(job) = jobs.recv() {
                    let mut saved_in = None;
                    let result = match job {
                        Job::Save(s) => {
                            let started = Instant::now();
                            let result = store.save(*s).map(|_| Outcome::Done);
                            saved_in = Some(started.elapsed());
                            result
                        }
                        Job::Scan => store.scan().map(|_| Outcome::Done),
                        Job::Load(id, expected) => store.load(&id).map(|s| Outcome::Loaded(Box::new(s), expected)),
                        Job::Discard(id) => store.discard(&id).map(|_| Outcome::Discarded(id)),
                    };
                    if events.send(Event { documents: store.inventory(), outcome: result, saved_in }).is_err() {
                        break;
                    }
                    wake();
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            tx: Some(tx),
            worker: Some(worker),
            rx,
            documents: Vec::new(),
            ready: false,
            busy: true,
            error: None,
            document: None,
            id: None,
            revision: None,
            due: Instant::now() + INTERVAL,
            alive: true,
            stale: Vec::new(),
        })
    }
    fn queue(&mut self, job: Job) -> Result<(), String> {
        if !self.alive {
            return Err("recovery worker is unavailable".into());
        }
        if self.busy {
            return Err("recovery is busy; try again when it finishes".into());
        }
        self.tx.as_ref().ok_or("recovery worker is unavailable")?.try_send(job).map_err(|e| e.to_string())?;
        self.busy = true;
        self.error = None;
        Ok(())
    }
    pub fn poll(&mut self) -> Option<(Snapshot, (u64, u64))> {
        match self.rx.try_recv() {
            Ok(e) => {
                self.busy = false;
                self.ready = true;
                self.documents = e.documents;
                let loaded = match e.outcome {
                    Ok(Outcome::Done) => None,
                    Ok(Outcome::Discarded(id)) => {
                        if self.id.as_deref() == Some(&id) {
                            self.id = None;
                        }
                        None
                    }
                    Ok(Outcome::Loaded(s, expected)) => Some((*s, expected)),
                    Err(e) => {
                        self.error = Some(e);
                        self.revision = None;
                        self.due = Instant::now() + INTERVAL;
                        None
                    }
                };
                // A slow snapshot spaces out the next, so recovery stays a small share of a core.
                if let Some(took) = e.saved_in {
                    self.due = self.due.max(Instant::now() + took.saturating_mul(DUTY).min(MAX_INTERVAL));
                }
                loaded
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.busy = false;
                self.alive = false;
                if self.error.is_none() {
                    self.error = Some("recovery worker is unavailable".into());
                }
                None
            }
        }
    }
    pub fn adopt(&mut self, id: String, document: u64, revision: u64) {
        self.id = Some(id);
        self.document = Some(document);
        self.revision = Some(revision);
        self.due = Instant::now() + INTERVAL;
    }
    pub fn tick(&mut self, s: &Session, now: Instant) -> Result<(), String> {
        if self.document != Some(s.document_id()) {
            self.document = Some(s.document_id());
            self.id = None;
            self.revision = None;
        }
        if self.busy || !self.ready || !self.alive {
            return Ok(());
        }
        if let Some(id) = self.stale.pop() {
            return self.queue(Job::Discard(id));
        }
        if !s.dirty || s.password.is_some() {
            if let Some(id) = self.id.clone() {
                self.queue(Job::Discard(id))?;
            }
            return Ok(());
        }
        if self.revision == Some(s.rev()) || now < self.due {
            return Ok(());
        }
        let id = match &self.id {
            Some(id) => id.clone(),
            None => new_id()?,
        };
        let name = s.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled draft".into());
        let info = RecoveryInfo { id: id.clone(), name, saved_at: crate::cmd::now_iso() };
        self.queue(Job::Save(Box::new(Snapshot { version: 1, info, generation: 0, document: s.doc.clone() })))?;
        self.id = Some(id);
        self.revision = Some(s.rev());
        self.due = now + INTERVAL;
        Ok(())
    }
    pub fn document_is(&self, document: u64) -> bool {
        self.document == Some(document)
    }
    /// The document `document` is being replaced while saved: its copy, if a tick hasn't
    /// discarded it yet, is out of date.
    pub fn replaced_clean(&mut self, document: u64) {
        if self.document == Some(document)
            && let Some(id) = self.id.take()
        {
            self.stale.push(id);
            self.revision = None;
        }
    }
    pub fn has_stale(&self) -> bool {
        !self.stale.is_empty()
    }
    pub fn wait(&self, document: u64, revision: u64) -> Option<Duration> {
        if self.alive && self.ready && !self.busy && !self.stale.is_empty() {
            return Some(Duration::from_millis(100));
        }
        if !self.alive || !self.ready || self.busy || (self.document == Some(document) && self.revision == Some(revision)) {
            return None;
        }
        Some(self.due.saturating_duration_since(Instant::now()).max(Duration::from_millis(100)))
    }
    pub fn restore(&mut self, id: &str, expected: (u64, u64)) -> Result<(), String> {
        if !self.documents.iter().any(|d| d.id == id) {
            return Err("no such recovery copy".into());
        }
        self.queue(Job::Load(id.into(), expected))
    }
    pub fn discard(&mut self, id: &str, current: (u64, u64)) -> Result<(), String> {
        if !self.documents.iter().any(|d| d.id == id) {
            return Err("no such recovery copy".into());
        }
        self.queue(Job::Discard(id.into()))?;
        if self.id.as_deref() == Some(id) {
            self.revision = Some(current.1);
        }
        Ok(())
    }
    pub fn refresh(&mut self) -> Result<(), String> {
        self.queue(Job::Scan)
    }
    /// Remove the current document's copy now, waiting for the worker.
    pub fn purge(&mut self) -> Result<(), String> {
        let Some(id) = self.id.clone() else { return Ok(()) };
        self.discard_now(id)?;
        self.id = None;
        self.revision = None;
        Ok(())
    }
    /// Remove the copies of replaced saved documents ([`Self::replaced_clean`]) now, before the
    /// app exits. A copy that can't be removed stays (it is never lost).
    pub fn purge_stale(&mut self) {
        while let Some(id) = self.stale.pop() {
            if let Err(e) = self.discard_now(id) {
                log::warn!("recovery: cannot discard an out-of-date copy: {e}");
            }
        }
    }
    /// Discard the copy `id`, waiting for the job under way and then for the discard.
    fn discard_now(&mut self, id: String) -> Result<(), String> {
        if self.busy {
            let e = self.rx.recv().map_err(|e| e.to_string())?;
            self.documents = e.documents;
            self.busy = false;
            self.ready = true;
            if matches!(e.outcome, Ok(Outcome::Loaded(..))) {
                return Err("finish recovery before protecting this document".into());
            }
        }
        self.queue(Job::Discard(id))?;
        let e = self.rx.recv().map_err(|e| e.to_string())?;
        self.busy = false;
        self.documents = e.documents;
        e.outcome.map(|_| ())
    }
}

/// Let the worker finish the job under way and release its record locks before the session is
/// gone: a snapshot being written at exit completes rather than being cut off, and a session
/// started next (the next launch, or a test) can lock and list that copy.
impl Drop for Recovery {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            log::warn!("recovery: the worker stopped abnormally");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wordcraft_doc::StoryRef;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("wordcraft-recovery-test-{}", new_id().unwrap()));
            private_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    /// A store reopened on `root` whose scan reports an unreadable record. The record's lock can be
    /// briefly held by a process another test is starting (a forked child holds the descriptor
    /// until it execs), and a record that can't be locked is skipped, not reported: so try again
    /// for up to a second rather than read a skipped record as a clean scan.
    fn reopen_reporting_unreadable(root: &std::path::Path) -> Store {
        for _ in 0..50 {
            let mut store = Store::new(root.to_path_buf()).unwrap();
            if store.scan().is_err() {
                return store;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("the scan never reported the unreadable record");
    }
    /// `store.load(id)`, tried again for up to a second while the record can't be locked: a process
    /// another test is starting holds the lock for a moment (see [`reopen_reporting_unreadable`]).
    fn load_retrying(store: &mut Store, id: &str) -> Snapshot {
        let mut last = String::new();
        for _ in 0..50 {
            match store.load(id) {
                Ok(snapshot) => return snapshot,
                Err(e) => last = e,
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("the record never loaded: {last}");
    }
    fn snapshot(id: &str, text: &str) -> Snapshot {
        Snapshot {
            version: 1,
            info: RecoveryInfo { id: id.into(), name: "Synthetic draft".into(), saved_at: crate::cmd::now_iso() },
            generation: 0,
            document: Document::from_text(text),
        }
    }
    fn settle(s: &mut Session) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            s.recovery_tick();
            if s.recovery_ready() && !s.recovery_busy() {
                break;
            }
            assert!(Instant::now() < end, "recovery did not finish");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn session(root: &Path) -> Session {
        let mut s = Session::new(Document::new());
        s.enable_recovery(root.to_owned(), Arc::new(|| {})).unwrap();
        settle(&mut s);
        s
    }
    fn save_now(s: &mut Session) {
        s.recovery.as_mut().unwrap().due = Instant::now();
        s.recovery_tick();
        settle(s);
    }

    #[test]
    fn recovery_commands_only_address_known_profile_records() {
        use crate::file_access::{AutomationRoots, confine};
        let profile = Temp::new();
        let files = Temp::new();
        let original = files.0.join("original.txt");
        fs::write(&original, b"External synthetic original").unwrap();
        let roots = AutomationRoots::new(Some(&files.0), Some(&files.0)).unwrap().unwrap();
        let mut s = session(&profile.0);
        s.run("document.setText", &json!({"text": "Recoverable synthetic draft"})).unwrap();
        save_now(&mut s);
        let id = s.recovery_documents()[0].id.clone();
        confine(Some(&roots), || {
            // A command cannot use a file path or choose a different storage folder.
            for bad in [original.to_string_lossy().into_owned(), "../original.txt".into()] {
                for action in ["restore", "discard"] {
                    let err = s.run("file.recover", &json!({action: bad, "root": files.0})).unwrap_err();
                    assert!(err.to_string().contains("no such recovery copy"), "{err}");
                }
            }
            assert_eq!(s.recovery_documents().len(), 1);
            s.run("file.recover", &json!({"restore": id})).unwrap();
            settle(&mut s);
            assert!(s.path.is_none());
            assert!(s.dirty);
            assert_eq!(s.doc.plain_text(StoryRef::Body), "Recoverable synthetic draft");
            s.run("file.recover", &json!({"discard": id})).unwrap();
            settle(&mut s);
            assert!(s.recovery_documents().is_empty());
        });
        assert_eq!(fs::read(&original).unwrap(), b"External synthetic original");
        assert_eq!(fs::read_dir(&files.0).unwrap().count(), 1);
    }

    #[test]
    fn failed_write_keeps_last_valid_generation_and_discovery_falls_back() {
        let t = Temp::new();
        let id = new_id().unwrap();
        let mut store = Store::new(t.0.clone()).unwrap();
        store.save(snapshot(&id, "First valid copy")).unwrap();
        let first = store.entries[&id].path.clone();
        let bytes = fs::read(&first).unwrap();
        // Simulate a final rename failure without relying on permissions (root can bypass them).
        let obstruction = t.0.join(&id).join("snapshot-00000000000000000002.json");
        fs::create_dir(&obstruction).unwrap();
        assert!(store.save(snapshot(&id, "Failed copy")).is_err());
        assert_eq!(fs::read(&first).unwrap(), bytes);
        fs::remove_dir(obstruction).unwrap();
        store.save(snapshot(&id, "Second valid copy")).unwrap();
        let second = store.entries[&id].path.clone();
        fs::write(second, b"{truncated").unwrap();
        drop(store);
        let mut reopened = reopen_reporting_unreadable(&t.0);
        assert_eq!(load_retrying(&mut reopened, &id).document.plain_text(StoryRef::Body), "First valid copy");
        // Never reuse the corrupt generation's name or prune the valid fallback.
        reopened.save(snapshot(&id, "Third valid copy")).unwrap();
        assert_eq!(fs::read(first).unwrap(), bytes);
        assert_eq!(Store::files(&t.0.join(&id)).unwrap().len(), 2);
    }

    #[test]
    fn size_limit_and_lossy_values_fail_without_committing() {
        let mut out = Limited { inner: Vec::new(), remaining: 3 };
        assert!(out.write_all(b"four").is_err());
        assert!(out.inner.is_empty());
        let t = Temp::new();
        let id = new_id().unwrap();
        let mut store = Store::new(t.0.clone()).unwrap();
        store.save(snapshot(&id, "valid")).unwrap();
        let first = store.entries[&id].path.clone();
        let bytes = fs::read(&first).unwrap();
        let mut invalid = snapshot(&id, "invalid");
        invalid.document.last_section.page_w = f32::NAN;
        assert!(store.save(invalid).is_err());
        assert_eq!(fs::read(first).unwrap(), bytes);
        assert_eq!(Store::files(&t.0.join(id)).unwrap().len(), 1);
    }

    #[test]
    fn failed_pruning_cannot_push_a_record_beyond_the_discovery_limit() {
        let t = Temp::new();
        let id = new_id().unwrap();
        let mut store = Store::new(t.0.clone()).unwrap();
        store.save(snapshot(&id, "Last valid copy")).unwrap();
        let first = store.entries[&id].path.clone();
        let bytes = fs::read(&first).unwrap();
        // A failed cleanup must not allow the next write to exceed the reader's bound.
        for n in 2..=MAX_FILES {
            fs::write(t.0.join(&id).join(format!("snapshot-{n:020}.json")), b"{truncated").unwrap();
        }
        assert!(store.save(snapshot(&id, "New copy")).is_err());
        assert_eq!(fs::read(first).unwrap(), bytes);
        drop(store);
        let mut reopened = reopen_reporting_unreadable(&t.0);
        assert_eq!(load_retrying(&mut reopened, &id).document.plain_text(StoryRef::Body), "Last valid copy");
    }

    #[test]
    fn failed_first_write_does_not_consume_a_record_slot() {
        let t = Temp::new();
        let mut store = Store::new(t.0.clone()).unwrap();
        let mut invalid = snapshot(&new_id().unwrap(), "Synthetic draft");
        invalid.document.last_section.page_w = f32::NAN;
        assert!(store.save(invalid).is_err());
        assert!(store.directories().unwrap().is_empty());
    }

    #[test]
    fn discovery_reclaims_a_crashed_first_write_but_keeps_valid_records() {
        let t = Temp::new();
        let id = new_id().unwrap();
        let empty = new_id().unwrap();
        let mut store = Store::new(t.0.clone()).unwrap();
        store.save(snapshot(&id, "Valid synthetic draft")).unwrap();
        drop(store);
        let dir = t.0.join(&empty);
        private_dir(&dir).unwrap();
        fs::write(dir.join(format!("pending-{}.tmp", new_id().unwrap())), b"{unfinished").unwrap();
        let mut reopened = Store::new(t.0.clone()).unwrap();
        reopened.scan().unwrap();
        assert!(!dir.exists());
        assert_eq!(load_retrying(&mut reopened, &id).document.plain_text(StoryRef::Body), "Valid synthetic draft");
    }

    #[test]
    fn live_records_are_locked_and_discard_releases_storage() {
        let t = Temp::new();
        let id = new_id().unwrap();
        let mut owner = Store::new(t.0.clone()).unwrap();
        owner.save(snapshot(&id, "live")).unwrap();
        let mut other = Store::new(t.0.clone()).unwrap();
        other.scan().unwrap();
        assert!(other.inventory().is_empty());
        assert!(other.discard(&id).is_err());
        assert!(other.discard("../outside").is_err());
        drop(owner);
        other.scan().unwrap();
        assert_eq!(other.inventory().len(), 1);
        other.discard(&id).unwrap();
        assert!(!t.0.join(id).exists(), "discard must release the record slot");
    }

    #[test]
    fn new_draft_restarts_restores_unsaved_and_explicitly_discards() {
        let t = Temp::new();
        let mut s = session(&t.0);
        s.run("text.insert", &json!({"text": "Unsaved synthetic draft"})).unwrap();
        save_now(&mut s);
        assert!(s.recovery_wait().is_none(), "unchanged documents must not schedule idle work");
        let expected = s.doc.clone();
        drop(s);
        let mut s = session(&t.0);
        let id = s.recovery_documents()[0].id.clone();
        s.run("file.recover", &json!({"restore": id})).unwrap();
        settle(&mut s);
        assert_eq!(s.doc, expected);
        assert!(s.dirty && s.path.is_none());
        s.run("file.recover", &json!({"discard": id})).unwrap();
        settle(&mut s);
        save_now(&mut s);
        assert!(s.recovery_documents().is_empty(), "discard isn't undone by the next timer");
        assert_eq!(s.doc, expected, "discard deletes the copy, not the editing document");
    }

    #[test]
    fn existing_file_is_unchanged_and_successful_save_removes_copy() {
        let t = Temp::new();
        let file = t.0.join("original.docx");
        let root = t.0.join("profile");
        crate::io::save_path(&file, &Document::from_text("Original business note")).unwrap();
        let original = fs::read(&file).unwrap();
        let mut s = session(&root);
        s.run("file.open", &json!({"path": file})).unwrap();
        s.run("text.insert", &json!({"text": "Unsaved edit. "})).unwrap();
        save_now(&mut s);
        let expected = s.doc.clone();
        drop(s);
        let mut s = session(&root);
        let id = s.recovery_documents()[0].id.clone();
        s.run("file.recover", &json!({"restore": id})).unwrap();
        settle(&mut s);
        assert_eq!(s.doc, expected);
        assert!(s.path.is_none() && s.dirty);
        assert_eq!(fs::read(&file).unwrap(), original);
        let saved = t.0.join("recovered.json");
        s.run("file.save", &json!({"path": saved})).unwrap();
        settle(&mut s);
        assert!(s.recovery_documents().is_empty());
        assert_eq!(fs::read(file).unwrap(), original);
    }

    #[test]
    fn password_barrier_clears_in_flight_plaintext_and_blocks_new_copies() {
        let t = Temp::new();
        let mut s = session(&t.0);
        s.run("text.insert", &json!({"text": "Confidential synthetic draft"})).unwrap();
        s.recovery.as_mut().unwrap().due = Instant::now();
        s.recovery_tick();
        assert!(s.recovery_busy());
        s.run("file.encrypt", &json!({"password": "synthetic-password"})).unwrap();
        assert!(s.recovery_documents().is_empty());
        s.run("text.insert", &json!({"text": "More confidential edits"})).unwrap();
        save_now(&mut s);
        assert!(fs::read_dir(&t.0).unwrap().next().is_none());
        // Removing protection makes future recovery possible again.
        s.run("file.encrypt", &json!({"password": null})).unwrap();
        save_now(&mut s);
        assert_eq!(s.recovery_documents().len(), 1);
    }

    #[test]
    fn failed_password_cleanup_rejects_protection_and_preserves_snapshot() {
        let t = Temp::new();
        let mut s = session(&t.0);
        s.run("text.insert", &json!({"text": "Synthetic text"})).unwrap();
        save_now(&mut s);
        let id = s.recovery_documents()[0].id.clone();
        let dir = t.0.join(id);
        // An unknown file prevents removal: don't report that plaintext was purged.
        let first = Store::files(&dir).unwrap()[0].clone();
        let bytes = fs::read(&first).unwrap();
        fs::write(dir.join("unexpected"), b"unexpected").unwrap();
        assert!(s.run("file.encrypt", &json!({"password": "synthetic-password"})).is_err());
        assert!(s.password.is_none());
        assert_eq!(fs::read(first).unwrap(), bytes);
    }

    #[test]
    fn delayed_restore_cannot_replace_new_edits() {
        let t = Temp::new();
        let mut s = session(&t.0);
        s.run("text.insert", &json!({"text": "Saved draft"})).unwrap();
        save_now(&mut s);
        drop(s);
        let mut s = session(&t.0);
        let id = s.recovery_documents()[0].id.clone();
        s.run("file.recover", &json!({"restore": id})).unwrap();
        s.run("text.insert", &json!({"text": "New work"})).unwrap();
        settle(&mut s);
        assert_eq!(s.doc.plain_text(StoryRef::Body), "New work");
        assert!(s.recovery_error().unwrap().contains("document changed"));
    }

    #[test]
    fn opening_protected_file_and_password_save_never_leave_plaintext_copies() {
        let t = Temp::new();
        let original = t.0.join("protected.docx");
        let root = t.0.join("profile");
        crate::io::save_path_with(
            &original,
            &Document::from_text("Synthetic confidential note"),
            Some("synthetic-password"),
            &crate::io::ExportOptions::default(),
        )
        .unwrap();
        let bytes = fs::read(&original).unwrap();
        let mut s = session(&root);
        s.run("file.open", &json!({"path": original, "password": "synthetic-password"})).unwrap();
        s.run("text.insert", &json!({"text": "Confidential edit"})).unwrap();
        save_now(&mut s);
        assert!(fs::read_dir(&root).unwrap().next().is_none());
        assert_eq!(fs::read(&original).unwrap(), bytes);
        s.run("file.new", &json!({})).unwrap();
        s.run("text.insert", &json!({"text": "New confidential draft"})).unwrap();
        save_now(&mut s);
        assert_eq!(s.recovery_documents().len(), 1);
        let saved = t.0.join("saved.docx");
        s.run("file.save", &json!({"path": saved, "password": "synthetic-password"})).unwrap();
        settle(&mut s);
        assert!(fs::read_dir(root).unwrap().next().is_none());
        assert_eq!(fs::read(original).unwrap(), bytes);
    }

    #[test]
    fn protection_after_document_switch_does_not_discard_another_draft() {
        let t = Temp::new();
        let mut s = session(&t.0);
        s.run("text.insert", &json!({"text": "First synthetic draft"})).unwrap();
        save_now(&mut s);
        s.run("file.new", &json!({})).unwrap();
        // Protection can be commanded before the host's next recovery tick.
        s.run("file.encrypt", &json!({"password": "synthetic-password"})).unwrap();
        settle(&mut s);
        assert_eq!(s.recovery_documents().len(), 1);
    }

    /// `private_dir` sets the mode only on folders it creates: an existing recovery folder (and
    /// record folder) that others could read is made private before a copy is written into it.
    #[cfg(unix)]
    #[test]
    fn an_existing_recovery_folder_is_made_private() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let t = Temp::new();
        let root = t.0.join("recovery");
        let id = new_id().unwrap();
        fs::create_dir_all(root.join(&id)).unwrap();
        for dir in [&root, &root.join(&id)] {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut store = Store::new(root.clone()).unwrap();
        assert_eq!(mode(&root), 0o700);
        store.save(snapshot(&id, "Private synthetic draft")).unwrap();
        assert_eq!(mode(&root.join(&id)), 0o700);
        // Every file in a record, the lock included, is the owner's only.
        let files: Vec<PathBuf> = fs::read_dir(root.join(&id)).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(files.len(), 2, "{files:?}");
        for f in files {
            assert_eq!(mode(&f), 0o600, "{}", f.display());
        }
    }

    /// One record failing a per-record check (here, more generations than discovery reads) is
    /// skipped and reported; it used to end the scan, so no copy at all was offered.
    #[test]
    fn one_unreadable_record_does_not_hide_the_others() {
        let t = Temp::new();
        let (good, bad) = (new_id().unwrap(), new_id().unwrap());
        // The same store scans, holding the good record's lock: no other process can be holding
        // it for a moment (#1849).
        let mut store = Store::new(t.0.clone()).unwrap();
        store.save(snapshot(&good, "Valid synthetic draft")).unwrap();
        private_dir(&t.0.join(&bad)).unwrap();
        for n in 1..=MAX_FILES + 1 {
            fs::write(t.0.join(&bad).join(format!("snapshot-{n:020}.json")), b"{truncated").unwrap();
        }
        store.entries.clear();
        assert!(store.scan().is_err(), "the bad record is reported");
        let listed: Vec<String> = store.inventory().into_iter().map(|i| i.id).collect();
        assert_eq!(listed, vec![good.clone()]);
        assert_eq!(store.load(&good).unwrap().document.plain_text(StoryRef::Body), "Valid synthetic draft");
        assert!(t.0.join(&bad).exists(), "the bad record is left alone");
    }

    /// A document saved and then replaced before a tick saw it saved (Save, then New or Open at
    /// once) left its copy behind, offered as unsaved work at every later start; so did Don't Save.
    #[test]
    fn copies_of_saved_or_abandoned_documents_do_not_outlive_them() {
        let t = Temp::new();
        let root = t.0.join("profile");
        let mut s = session(&root);
        s.run("text.insert", &json!({"text": "Saved synthetic draft"})).unwrap();
        save_now(&mut s);
        assert_eq!(s.recovery_documents().len(), 1);
        s.run("file.save", &json!({"path": t.0.join("saved.docx")})).unwrap();
        s.run("file.new", &json!({})).unwrap();
        assert!(s.recovery_wait().is_some(), "the out-of-date copy schedules its own removal");
        save_now(&mut s);
        settle(&mut s);
        assert!(s.recovery_documents().is_empty());
        assert!(fs::read_dir(&root).unwrap().next().is_none(), "the out-of-date copy is gone from disk");

        // Saved, then the app closes before a tick: the copy goes before exit.
        s.run("text.insert", &json!({"text": "Saved before closing"})).unwrap();
        save_now(&mut s);
        s.run("file.save", &json!({"path": t.0.join("closing.docx")})).unwrap();
        s.discard_recovery();
        assert!(fs::read_dir(&root).unwrap().next().is_none());

        // Don't Save: the abandoned document's copy goes at once, without waiting for a tick.
        s.run("file.new", &json!({})).unwrap();
        s.run("text.insert", &json!({"text": "Abandoned synthetic draft"})).unwrap();
        save_now(&mut s);
        assert_eq!(s.recovery_documents().len(), 1);
        s.discard_recovery();
        assert!(fs::read_dir(&root).unwrap().next().is_none());

        // An unsaved document replaced without a choice (an agent opening a file) keeps its copy.
        s.run("text.insert", &json!({"text": "Only copy of this work"})).unwrap();
        save_now(&mut s);
        s.run("file.new", &json!({})).unwrap();
        save_now(&mut s);
        assert_eq!(s.recovery_documents().len(), 1);
        drop(s);
        let s = session(&root);
        assert_eq!(s.recovery_documents().len(), 1, "offered at the next start");
    }

    /// A slow snapshot spaces out the next, at most [`MAX_INTERVAL`] later, so recovery stays a
    /// small share of a core on a large document; a quick one keeps the ten-second interval.
    #[test]
    fn slow_snapshots_space_out_the_next() {
        let t = Temp::new();
        let mut s = session(&t.0);
        let r = s.recovery.as_mut().unwrap();
        // Stand in for the worker's report.
        let (events, rx) = mpsc::channel();
        let worker_events = std::mem::replace(&mut r.rx, rx);
        for (took, least, most) in [
            (Duration::from_millis(20), INTERVAL - Duration::from_secs(1), INTERVAL),
            (Duration::from_secs(3), Duration::from_secs(59), Duration::from_secs(60)),
            (Duration::from_secs(100), MAX_INTERVAL - Duration::from_secs(1), MAX_INTERVAL),
        ] {
            r.due = Instant::now() + INTERVAL;
            r.busy = true;
            events.send(Event { documents: Vec::new(), outcome: Ok(Outcome::Done), saved_in: Some(took) }).unwrap();
            assert!(r.poll().is_none());
            let wait = r.due.saturating_duration_since(Instant::now());
            assert!(wait > least && wait <= most, "{took:?}: next in {wait:?}");
        }
        drop(worker_events);
    }

    /// Ending a session lets the worker finish the snapshot under way and release its lock, so a
    /// session started next lists the copy. It used to be cut off or still locked, and missed.
    #[test]
    fn ending_a_session_finishes_the_snapshot_under_way() {
        let t = Temp::new();
        let mut s = session(&t.0);
        s.run("text.insert", &json!({"text": "Written as the session ends"})).unwrap();
        s.recovery.as_mut().unwrap().due = Instant::now();
        s.recovery_tick();
        assert!(s.recovery_busy());
        drop(s);
        let records: Vec<PathBuf> = fs::read_dir(&t.0).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(records.len(), 1);
        assert_eq!(Store::files(&records[0]).unwrap().len(), 1, "the snapshot was finished");
        let s = session(&t.0);
        assert_eq!(s.recovery_documents().len(), 1);
    }

    /// Measures what recovery costs on a large synthetic document: copy size, `.json` size, the
    /// worker's time per snapshot and its share of a core while typing. Run on demand:
    /// `cargo test --release -p wordcraft-engine recovery_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "measurement"]
    fn recovery_cost_on_a_large_document() {
        let para = "Synthetic recovery measurement text with several ordinary words in it. ".repeat(8);
        let text: Vec<String> = (0..6000).map(|i| format!("{i} {para}")).collect();
        let mut doc = Document::from_text(&text.join("\n"));
        // Incompressible bytes, as a photograph's are.
        let mut noise = vec![0u8; 27 << 20];
        let mut x = 0x2545_f491_4f6c_dd1du64;
        for b in noise.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = x as u8;
        }
        doc.media.insert("media/image1.png".into(), Arc::new(noise));
        let t0 = Instant::now();
        let saved = crate::io::save_bytes("doc.json", &doc).unwrap();
        println!(".json file: {} MB, written in {:?}", saved.len() >> 20, t0.elapsed());
        let t0 = Instant::now();
        let reopened = crate::io::open_bytes("doc.json", &saved).unwrap();
        println!(".json file: opened in {:?}", t0.elapsed());
        assert_eq!(reopened.media, doc.media);
        drop((saved, reopened));
        let t = Temp::new();
        let mut s = session(&t.0);
        s.set_document(doc);
        s.run("text.insert", &json!({"text": "edit"})).unwrap();
        let t0 = Instant::now();
        s.recovery.as_mut().unwrap().due = Instant::now();
        s.recovery_tick();
        println!("UI thread: recovery_tick that queues a snapshot {:?}", t0.elapsed());
        settle(&mut s);
        println!("worker: first snapshot written in {:?} (error: {:?})", t0.elapsed(), s.recovery_error());
        let size: u64 = Store::files(&t.0.join(&s.recovery_documents()[0].id)).unwrap().iter().map(|p| fs::metadata(p).unwrap().len()).sum();
        println!("recovery copy: {} MB", size >> 20);
        // Two minutes of typing, a character every 100 ms, ticking as the app's frames do.
        let (window, mut busy_since, mut busy, mut snapshots) = (Duration::from_secs(120), None::<Instant>, Duration::ZERO, 0);
        let (start, mut typed) = (Instant::now(), Instant::now());
        while start.elapsed() < window {
            if typed.elapsed() >= Duration::from_millis(100) {
                s.run("text.insert", &json!({"text": "x"})).unwrap();
                typed = Instant::now();
            }
            s.recovery_tick();
            match (busy_since, s.recovery_busy()) {
                (None, true) => {
                    busy_since = Some(Instant::now());
                    snapshots += 1;
                }
                (Some(since), false) => {
                    busy += since.elapsed();
                    busy_since = None;
                }
                _ => {}
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        if let Some(since) = busy_since {
            busy += since.elapsed();
        }
        let share = busy.as_secs_f64() * 100.0 / start.elapsed().as_secs_f64();
        println!("typing for {window:?}: {snapshots} snapshots, worker busy {busy:?} = {share:.1}% of a core");
        settle(&mut s);
        drop(s);
        let t0 = Instant::now();
        let s = session(&t.0);
        println!("start-up scan listing {} copies: {:?}", s.recovery_documents().len(), t0.elapsed());
    }
}
