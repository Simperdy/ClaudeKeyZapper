use super::{binding::Ownership, HermesError, HermesResult, HermesTarget};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs::{self, File, OpenOptions}, path::PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation { Connect, Disconnect }
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Journal { pub plan: super::binding::BindingPlan, pub completed: Vec<String>, pub phase: Phase, pub operation: Operation }
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase { PreparingBackup, Prepared, Applying, Committed, RecoveryRequired, CleanupPending }
#[derive(Serialize, Deserialize, Default)]
struct Registry { ownership: BTreeMap<String, Ownership>, journals: BTreeMap<String, Journal> }

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum StoreWrite { RawBackup, BackupSaved, BackupTemporary, Journal, Commit, Cleanup, FinishCleanup }
pub struct HermesStore {
    dir: PathBuf,
    #[cfg(test)]
    faults: std::cell::RefCell<Vec<(StoreWrite, usize)>>,
    #[cfg(test)]
    interruptions: std::cell::RefCell<Vec<(StoreWrite, usize)>>,
}
impl HermesStore {
    pub fn new(dir: PathBuf) -> Self { Self { dir, #[cfg(test)] faults: Default::default(), #[cfg(test)] interruptions: Default::default() } }
    #[cfg(test)]
    pub(super) fn fail_on(&self, operation: StoreWrite, occurrence: usize) { self.faults.borrow_mut().push((operation, occurrence)); }
    #[cfg(test)]
    pub(super) fn interrupt_on(&self, operation: StoreWrite, occurrence: usize) { self.interruptions.borrow_mut().push((operation, occurrence)); }
    #[cfg(test)]
    fn check_fault(&self, operation: StoreWrite) -> HermesResult<()> {
        let interrupted = {
            let mut interruptions = self.interruptions.borrow_mut();
            for (kind, remaining) in interruptions.iter_mut() { if *kind == operation { *remaining -= 1; } }
            let interrupted = interruptions.iter().any(|(_, remaining)| *remaining == 0);
            interruptions.retain(|(_, remaining)| *remaining != 0);
            interrupted
        };
        if interrupted { panic!("synthetic store lifecycle interruption"); }
        let mut faults = self.faults.borrow_mut();
        for (kind, remaining) in faults.iter_mut() { if *kind == operation { *remaining -= 1; } }
        let failed = faults.iter().any(|(_, remaining)| *remaining == 0);
        faults.retain(|(_, remaining)| *remaining != 0);
        if failed { Err(HermesError::JournalFailed) } else { Ok(()) }
    }
    fn registry_path(&self) -> PathBuf { self.dir.join("registry.json") }
    fn lock_registry(&self) -> HermesResult<File> {
        crate::paths::create_private_dir(&self.dir).map_err(|_| HermesError::JournalFailed)?;
        let file = OpenOptions::new().create(true).read(true).write(true).open(self.dir.join("registry.lock")).map_err(|_| HermesError::JournalFailed)?;
        file.lock().map_err(|_| HermesError::JournalFailed)?; Ok(file)
    }
    pub fn lock_target(&self, target: &HermesTarget) -> HermesResult<File> {
        let key = Self::resource_key(target)?; let digest = format!("{:x}", Sha256::digest(key.as_bytes()));
        let dir = self.dir.join("operations"); crate::paths::create_private_dir(&dir).map_err(|_| HermesError::JournalFailed)?;
        let file = OpenOptions::new().create(true).read(true).write(true).open(dir.join(format!("{digest}.lock"))).map_err(|_| HermesError::JournalFailed)?;
        file.lock().map_err(|_| HermesError::JournalFailed)?; Ok(file)
    }
    fn read_registry(&self) -> HermesResult<Registry> {
        let path = self.registry_path();
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Registry::default()),
            Err(_) => Err(HermesError::JournalFailed),
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => Err(HermesError::JournalFailed),
            Ok(_) => serde_json::from_slice(&fs::read(path).map_err(|_| HermesError::JournalFailed)?).map_err(|_| HermesError::JournalFailed),
        }
    }
    fn write_registry(&self, registry: &Registry) -> HermesResult<()> {
        let bytes = serde_json::to_vec(registry).map_err(|_| HermesError::JournalFailed)?;
        crate::paths::write_atomic(&self.registry_path(), &bytes, true).map_err(|_| HermesError::JournalFailed)
    }
    pub fn load(&self) -> HermesResult<BTreeMap<String, Ownership>> { let _lock=self.lock_registry()?; Ok(self.read_registry()?.ownership) }
    pub fn owner(&self, key: &str) -> HermesResult<Option<Ownership>> { Ok(self.load()?.remove(key)) }
    pub fn save(&self, ownership: &BTreeMap<String, Ownership>) -> HermesResult<()> {
        let _lock=self.lock_registry()?; let mut registry=self.read_registry()?; registry.ownership=ownership.clone(); self.write_registry(&registry)
    }
    pub fn save_journal(&self, key: &str, journal: &Journal) -> HermesResult<()> {
        #[cfg(test)] self.check_fault(StoreWrite::Journal)?;
        let _lock=self.lock_registry()?; let mut registry=self.read_registry()?; registry.journals.insert(key.into(),journal.clone()); self.write_registry(&registry)
    }
    pub fn commit(&self, key: &str, owner: Option<Ownership>) -> HermesResult<()> {
        #[cfg(test)] self.check_fault(StoreWrite::Commit)?;
        let _lock=self.lock_registry()?; let mut registry=self.read_registry()?;
        if let Some(owner)=owner { registry.ownership.insert(key.into(),owner); } else { registry.ownership.remove(key); }
        // Ownership and the cleanup obligation become visible in one transaction.
        if let Some(journal) = registry.journals.get_mut(key) { journal.phase = Phase::CleanupPending; }
        self.write_registry(&registry)
    }
    pub(super) fn finish_cleanup(&self, key: &str) -> HermesResult<()> {
        #[cfg(test)] self.check_fault(StoreWrite::FinishCleanup)?;
        let _lock = self.lock_registry()?; let mut registry = self.read_registry()?;
        match registry.journals.get(key).map(|journal| journal.phase) {
            Some(Phase::PreparingBackup | Phase::CleanupPending) => {},
            _ => return Err(HermesError::RecoveryRequired),
        }
        registry.journals.remove(key);
        self.write_registry(&registry)
    }
    pub fn journal(&self, key: &str) -> HermesResult<Option<Journal>> { let _lock=self.lock_registry()?; Ok(self.read_registry()?.journals.remove(key)) }
    pub fn has_pending_operation(&self) -> HermesResult<bool> { let _lock=self.lock_registry()?; Ok(!self.read_registry()?.journals.is_empty()) }
    /// Read-only projection of persisted identities. Stored keys are authoritative
    /// registry indexes here; unlike mutation keys, this never canonicalizes paths.
    pub(super) fn persisted_targets(&self) -> HermesResult<Vec<(String, Option<Ownership>, Option<Journal>)>> {
        let _lock = self.lock_registry()?;
        let registry = self.read_registry()?;
        let keys: std::collections::BTreeSet<_> = registry.ownership.keys().chain(registry.journals.keys()).cloned().collect();
        keys.into_iter().map(|key| {
            let (home, executable) = key.split_once('\n').ok_or(HermesError::JournalFailed)?;
            if home.is_empty() || executable.is_empty() || home.contains('\n') || executable.contains('\n')
                || !PathBuf::from(home).is_absolute() || !PathBuf::from(executable).is_absolute() {
                return Err(HermesError::JournalFailed);
            }
            let owner = registry.ownership.get(&key).cloned();
            let journal = registry.journals.get(&key).cloned();
            for target in owner.iter().map(|owner| &owner.selection.target).chain(journal.iter().map(|journal| &journal.plan.selection.target)) {
                if !target.home.is_absolute() || !target.executable.is_absolute() || target.display_name.trim().is_empty() {
                    return Err(HermesError::JournalFailed);
                }
            }
            Ok((key.clone(), owner, journal))
        }).collect()
    }
    /// Called under the resource lock for mutations. Legacy registry identities stay
    /// distinct, but cannot adopt or roll back another executable's shared home.
    pub(super) fn check_resource(&self, target: &HermesTarget) -> HermesResult<()> {
        let key = Self::target_key(target)?;
        let prefix = format!("{}\n", Self::resource_key(target)?);
        let _lock = self.lock_registry()?;
        let registry = self.read_registry()?;
        if registry.journals.keys().any(|other| other != &key && other.starts_with(&prefix)) { return Err(HermesError::RecoveryRequired); }
        if registry.ownership.keys().any(|other| other != &key && other.starts_with(&prefix)) { return Err(HermesError::Conflict); }
        Ok(())
    }
    pub fn save_raw_backup(&self, target: &HermesTarget, raw: &[u8]) -> HermesResult<()> {
        #[cfg(test)] self.check_fault(StoreWrite::RawBackup)?;
        let key=Self::target_key(target)?; let digest=format!("{:x}",Sha256::digest(key.as_bytes())); let dir=self.dir.join("backups");
        crate::paths::create_private_dir(&dir).map_err(|_|HermesError::JournalFailed)?;
        let temporary = dir.join(format!(".{digest}.raw.keyzapper-{}", uuid::Uuid::new_v4()));
        let mut options = OpenOptions::new(); options.write(true).create_new(true);
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        {
            use std::io::Write;
            let mut file = options.open(&temporary).map_err(|_| HermesError::JournalFailed)?;
            file.write_all(raw).and_then(|_| file.sync_all()).map_err(|_| HermesError::JournalFailed)?;
        }
        // Errors/interruptions leave artifacts covered by PreparingBackup. Cleanup
        // is one fallible lifecycle step, not an ignored best-effort unlink here.
        #[cfg(test)] self.check_fault(StoreWrite::BackupTemporary)?;
        fs::rename(&temporary, dir.join(format!("{digest}.raw"))).map_err(|_| HermesError::JournalFailed)?;
        #[cfg(test)] self.check_fault(StoreWrite::BackupSaved)?;
        Ok(())
    }
    pub fn remove_raw_backup(&self, target: &HermesTarget) -> HermesResult<()> {
        #[cfg(test)] self.check_fault(StoreWrite::Cleanup)?;
        let key=Self::target_key(target)?; let digest=format!("{:x}",Sha256::digest(key.as_bytes()));
        let dir = self.dir.join("backups");
        let mut artifacts = vec![dir.join(format!("{digest}.raw"))];
        // Only this target's exact atomic-writer namespace, including older files.
        // Never sweep the directory or another target's in-flight temporary.
        let prefix = format!(".{digest}.raw.keyzapper-");
        match fs::read_dir(&dir) {
            Ok(entries) => for entry in entries {
                let entry = entry.map_err(|_| HermesError::JournalFailed)?;
                if entry.file_name().to_str().and_then(|name| name.strip_prefix(&prefix))
                    .is_some_and(|suffix| uuid::Uuid::parse_str(suffix).is_ok_and(|id| id.hyphenated().to_string() == suffix)) {
                    artifacts.push(entry.path());
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(_) => return Err(HermesError::JournalFailed),
        }
        for path in artifacts {
            match fs::remove_file(&path) {
                Ok(()) => {},
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(_) => return Err(HermesError::JournalFailed),
            }
            match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                _ => return Err(HermesError::JournalFailed),
            }
        }
        Ok(())
    }
    pub(super) fn raw_backup(&self, target: &HermesTarget) -> HermesResult<Vec<u8>> {
        let key = Self::target_key(target)?;
        let digest = format!("{:x}", Sha256::digest(key.as_bytes()));
        let path = self.dir.join("backups").join(format!("{digest}.raw"));
        let meta = fs::symlink_metadata(&path).map_err(|_| HermesError::RecoveryRequired)?;
        if !meta.is_file() || meta.file_type().is_symlink() { return Err(HermesError::RecoveryRequired); }
        fs::read(path).map_err(|_| HermesError::RecoveryRequired)
    }
    pub fn target_key(target: &HermesTarget) -> HermesResult<String> {
        let home=fs::canonicalize(&target.home).map_err(|_|HermesError::WrongTarget)?; let executable=fs::canonicalize(&target.executable).map_err(|_|HermesError::WrongTarget)?;
        Ok(format!("{}\n{}",home.to_string_lossy(),executable.to_string_lossy()))
    }
    /// Mutation exclusion is deliberately broader than installation/ownership identity.
    pub fn resource_key(target: &HermesTarget) -> HermesResult<String> {
        Ok(fs::canonicalize(&target.home).map_err(|_| HermesError::WrongTarget)?.to_string_lossy().into_owned())
    }
}
