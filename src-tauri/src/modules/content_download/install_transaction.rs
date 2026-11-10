//! Durable same-volume multi-target replacement. A failed rollback retains its journal and backups.
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use crate::error::KernelError;

pub struct RootLock(std::fs::File);
impl Drop for RootLock { fn drop(&mut self) { let _ = self.0.unlock(); } }
/// Kernel-backed file lock is released automatically on crash, unlike a lock directory.
pub fn lock(root: &Path) -> Result<RootLock, KernelError> {
    std::fs::create_dir_all(root)?;
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(root.join("install.lock"))?;
    file.try_lock().map_err(|_| KernelError::Conflict("Another process is installing into this instance".into()))?;
    Ok(RootLock(file))
}

#[derive(Serialize, Deserialize)]
struct Entry { target: PathBuf, backup: PathBuf, staged: PathBuf, had_previous: bool, started: bool }
#[derive(Serialize, Deserialize)]
struct Journal { committed: bool, entries: Vec<Entry> }
fn save(root: &Path, journal: &Journal) -> Result<(), KernelError> {
    use std::io::Write;
    let sequence = std::fs::read_dir(root)?.filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("journal-")).count();
    let path = root.join(format!("journal-{sequence:08}.json"));
    let mut file = std::fs::File::create(root.join("journal.next"))?;
    file.write_all(&serde_json::to_vec(journal)?)?;
    file.sync_all()?;
    // Append-only generations avoid a remove/replace gap on Windows.
    std::fs::rename(root.join("journal.next"), path)?;
    Ok(())
}
fn copy(src: &Path, dst: &Path) -> Result<(), KernelError> {
    let metadata = std::fs::symlink_metadata(src)?;
    if metadata.file_type().is_symlink() { return Err(KernelError::InvalidArgument("Linked import sources are forbidden".into())); }
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?; let from = entry.path(); let to = dst.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_symlink() { return Err(KernelError::InvalidArgument("Linked import sources are forbidden".into())); }
        if kind.is_dir() { copy(&from, &to)?; } else { std::fs::copy(from, to)?; }
    }
    Ok(())
}
fn rollback(journal: &Journal) -> Result<(), KernelError> {
    for entry in journal.entries.iter().rev().filter(|e| e.started) {
        if entry.backup.exists() {
            if entry.target.exists() { std::fs::remove_dir_all(&entry.target)?; }
            std::fs::rename(&entry.backup, &entry.target)?;
        } else if !entry.had_previous && entry.target.exists() { std::fs::remove_dir_all(&entry.target)?; }
    }
    Ok(())
}
/// Must run under the same root lock as commit. No backup is removed before the commit marker.
pub fn recover(root: &Path) -> Result<(), KernelError> {
    if !root.exists() { return Ok(()); }
    for item in std::fs::read_dir(root)? {
        let dir = item?.path(); if !dir.is_dir() { continue; }
        let mut journals: Vec<_> = std::fs::read_dir(&dir)?.filter_map(Result::ok).map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("journal-") && n.to_string_lossy().ends_with(".json"))).collect();
        journals.sort();
        let Some(source) = journals.pop() else { continue; };
        let journal: Journal = serde_json::from_slice(&std::fs::read(source)?)?;
        if journal.entries.iter().any(|entry| entry.backup.parent() != Some(dir.as_path()) || entry.staged.parent() != Some(dir.as_path()) || !entry.target.is_absolute() || entry.target.components().any(|c| matches!(c, std::path::Component::ParentDir))) {
            return Err(KernelError::Conflict("Invalid recovery journal; manual recovery required".into()));
        }
        if !journal.committed { rollback(&journal)?; }
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}
/// Metadata work belongs inside this callback: failure restores all previous target bytes.
pub fn commit(targets: &[(PathBuf, PathBuf)], root: &Path, metadata: impl FnOnce() -> Result<(), KernelError>) -> Result<Vec<PathBuf>, KernelError> {
    std::fs::create_dir_all(root)?;
    let dir = root.join(uuid::Uuid::new_v4().to_string()); std::fs::create_dir(&dir)?;
    let mut journal = Journal { committed: false, entries: Vec::new() };
    let preparation = (|| {
    let mut seen = std::collections::HashSet::new();
    for (index, (source, target)) in targets.iter().enumerate() {
        if !seen.insert(target.to_string_lossy().to_lowercase()) { return Err(KernelError::InvalidArgument("Duplicate transaction target".into())); }
        if target.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) { return Err(KernelError::InvalidArgument("Linked import targets are forbidden".into())); }
        let staged = dir.join(format!("new-{index}")); copy(source, &staged)?;
        journal.entries.push(Entry { target: target.clone(), backup: dir.join(format!("old-{index}")), staged, had_previous: target.exists(), started: false });
    }
    save(&dir, &journal)?;
    Ok::<_, KernelError>(())
    })();
    if let Err(error) = preparation { let _ = std::fs::remove_dir_all(&dir); return Err(error); }
    let result = (|| {
        for index in 0..journal.entries.len() {
            journal.entries[index].started = true; save(&dir, &journal)?;
            let e = &journal.entries[index];
            std::fs::create_dir_all(e.target.parent().ok_or_else(|| KernelError::InvalidArgument("Invalid import target".into()))?)?;
            if e.had_previous { std::fs::rename(&e.target, &e.backup)?; }
            std::fs::rename(&e.staged, &e.target)?;
        }
        metadata()?;
        journal.committed = true; save(&dir, &journal)?;
        Ok::<_, KernelError>(())
    })();
    if let Err(error) = result {
        if let Err(restore) = rollback(&journal) { return Err(KernelError::Conflict(format!("Import failed ({error}); recovery required ({restore}); backups retained"))); }
        std::fs::remove_dir_all(&dir)?;
        return Err(error);
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(targets.iter().map(|(_, target)| target.clone()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!("copper-transaction-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap(); root
    }
    fn targets(root: &Path, count: usize) -> Vec<(PathBuf, PathBuf)> {
        (0..count).map(|i| {
            let source = root.join(format!("src-{i}")); let target = root.join(format!("dst-{i}"));
            std::fs::create_dir(&source).unwrap(); std::fs::write(source.join("bytes"), [i as u8, 1, 2]).unwrap();
            std::fs::create_dir(&target).unwrap(); std::fs::write(target.join("bytes"), [255, i as u8, 0]).unwrap();
            (source, target)
        }).collect()
    }
    #[test]
    fn metadata_failure_restores_all_three_targets_byte_for_byte() {
        let root = fixture(); let targets = targets(&root, 3);
        let result = commit(&targets, &root.join("transactions"), || Err(KernelError::Conflict("metadata injected failure".into())));
        assert!(result.is_err());
        for (i, (_, target)) in targets.iter().enumerate() { assert_eq!(std::fs::read(target.join("bytes")).unwrap(), [255, i as u8, 0]); }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn second_and_third_target_commit_failure_restore_previous_targets() {
        for failing in [1usize, 2] {
            let root = fixture(); let mut targets = targets(&root, 3);
            let blocked = root.join("blocked"); std::fs::write(&blocked, b"file not directory").unwrap();
            targets[failing].1 = blocked.join("target");
            assert!(commit(&targets, &root.join("transactions"), || Ok(())).is_err());
            for (i, (_, target)) in targets.iter().enumerate().take(failing) { assert_eq!(std::fs::read(target.join("bytes")).unwrap(), [255, i as u8, 0]); }
            assert_eq!(std::fs::read(blocked).unwrap(), b"file not directory");
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn recovery_failure_keeps_backup_and_journal() {
        let root = fixture(); let tx = root.join("pending"); std::fs::create_dir(&tx).unwrap();
        let backup = tx.join("old"); std::fs::create_dir(&backup).unwrap(); std::fs::write(backup.join("bytes"), b"original").unwrap();
        let blocked = root.join("blocked"); std::fs::write(&blocked, b"block").unwrap();
        let journal = Journal { committed: false, entries: vec![Entry { target: blocked.join("target"), backup: backup.clone(), staged: tx.join("new"), had_previous: true, started: true }] };
        save(&tx, &journal).unwrap();
        assert!(recover(&root).is_err());
        assert_eq!(std::fs::read(backup.join("bytes")).unwrap(), b"original");
        assert!(tx.join("journal-00000000.json").exists());
        std::fs::remove_file(blocked).unwrap(); std::fs::create_dir(root.join("blocked")).unwrap();
        recover(&root).unwrap();
        assert_eq!(std::fs::read(root.join("blocked/target/bytes")).unwrap(), b"original");
        std::fs::remove_dir_all(root).unwrap();
    }
}
