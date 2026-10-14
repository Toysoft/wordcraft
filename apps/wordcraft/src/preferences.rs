//! Publish a complete preferences file, so concurrent windows and readers never see a partial
//! JSON write. Preferences still have last-writer-wins semantics across independent windows.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    for _ in 0..128 {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let temporary = path.with_file_name(format!(".wordcraft-prefs-{}-{id}.tmp", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = match options.open(&temporary) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        let result = file.write_all(bytes).and_then(|()| file.sync_all());
        drop(file);
        let result = result.and_then(|()| fs::rename(&temporary, path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return result;
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "couldn't allocate a preferences temporary file"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_writers_publish_whole_preferences() {
        let dir = std::env::temp_dir().join(format!("wordcraft-prefs-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ui.json");
        write(&path, br#"{"window":0}"#).unwrap();
        std::thread::scope(|scope| {
            for index in 0..4 {
                let path = &path;
                scope.spawn(move || {
                    for _ in 0..20 {
                        write(path, format!("{{\"window\":{index}}}").as_bytes()).unwrap();
                        let value: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
                        assert!(value["window"].as_u64().unwrap() < 4);
                    }
                });
            }
        });
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "no temporary files left behind");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_replacement_removes_its_temporary_file() {
        let dir = std::env::temp_dir().join(format!("wordcraft-prefs-fail-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("destination")).unwrap();
        assert!(write(&dir.join("destination"), b"{}").is_err());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
}
