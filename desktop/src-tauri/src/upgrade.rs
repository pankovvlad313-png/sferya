//! Upgrade boundary: never migrate an existing database before a verified snapshot.
use std::{fs::{self, File, OpenOptions}, io::{Read, Write}, path::{Path, PathBuf}};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

type Result<T> = std::result::Result<T, String>;
fn error(e: impl std::fmt::Display) -> String { e.to_string() }
const MARKER: &str = ".sfery-upgrade.json";
const LOCK: &str = ".sfery-instance.lock";

pub fn open_store(dir: &Path, version: &str) -> Result<(Upgrade, crate::storage::Store)> {
    let upgrade = Upgrade::prepare(dir, version)?;
    let store = crate::storage::Store::new(dir)?;
    upgrade.finish()?;
    Ok((upgrade, store))
}

pub fn validate_database(conn: &Connection) -> Result<()> {
    let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0)).map_err(error)?;
    if check != "ok" { return Err("База повреждена. Обновление остановлено; восстановите резервную копию.".into()); }
    let has_settings: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='app_settings')", [], |r| r.get(0)).map_err(error)?;
    if has_settings {
        let version: Option<String> = conn.query_row("SELECT value FROM app_settings WHERE key='schema_version'", [], |r| r.get(0)).optional().map_err(error)?;
        if let Some(version) = version {
            let version: u32 = version.parse().map_err(|_| "Неизвестная версия базы. Обновление остановлено.")?;
            if version > 9 { return Err("Эта база создана более новой Sfery. Используйте последнюю версию приложения.".into()); }
        }
    }
    Ok(())
}

pub struct Upgrade {
    // Held for the entire application lifetime. The OS releases it after a crash.
    _lock: File,
    dir: PathBuf,
    version: String,
    backup: Option<PathBuf>,
    completed: bool,
}
impl Upgrade {
    pub fn prepare(dir: &Path, version: &str) -> Result<Self> {
        fs::create_dir_all(dir).map_err(error)?;
        let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(dir.join(LOCK)).map_err(error)?;
        lock.try_lock().map_err(|_| "Другая Sfery уже использует эти данные. Закройте её и повторите запуск.")?;
        let db = dir.join("planner.sqlite");
        if db.exists() {
            let source = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(error)?;
            validate_database(&source)?;
        }
        let marker: Option<Value> = fs::read(dir.join(MARKER)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let completed = marker.as_ref().is_some_and(|m| m["version"] == version && m["status"] == "complete");
        let backup = if db.exists() && !completed { Some(snapshot(dir, version)?) } else { None };
        Ok(Self { _lock: lock, dir: dir.into(), version: version.into(), backup, completed })
    }
    pub fn finish(&self) -> Result<()> {
        if self.completed { return Ok(()); }
        let marker = json!({"version":self.version,"status":"complete","backup":self.backup,"completed_at":chrono::Utc::now().to_rfc3339()});
        write_file(&self.dir.join(".sfery-upgrade.tmp"), &serde_json::to_vec_pretty(&marker).map_err(error)?)?;
        fs::rename(self.dir.join(".sfery-upgrade.tmp"), self.dir.join(MARKER)).map_err(error)
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = File::create(path).map_err(error)?;
    f.write_all(bytes).and_then(|_|f.sync_all()).map_err(error)
}
fn hash(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(error)?;
    let mut sha = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop { let n = file.read(&mut buffer).map_err(error)?; if n == 0 { break; } sha.update(&buffer[..n]); }
    Ok(format!("{:x}", sha.finalize()))
}
fn copy_tree(source: &Path, destination: &Path, root: bool) -> Result<()> {
    fs::create_dir_all(destination).map_err(error)?;
    for entry in fs::read_dir(source).map_err(error)? {
        let entry = entry.map_err(error)?;
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        if root && (matches!(name_text.as_ref(), "backups" | "planner.sqlite" | "planner.sqlite-wal" | "planner.sqlite-shm" | "planner.sqlite-journal" | LOCK | MARKER | ".sfery-upgrade.tmp")) { continue; }
        let ty = entry.file_type().map_err(error)?;
        if ty.is_symlink() { return Err(format!("Резервная копия остановлена: ссылка {}", entry.path().display())); }
        let dest = destination.join(name);
        if ty.is_dir() { copy_tree(&entry.path(), &dest, false)?; }
        else if ty.is_file() {
            let before = hash(&entry.path())?;
            fs::copy(entry.path(), &dest).map_err(error)?;
            OpenOptions::new().write(true).open(&dest).and_then(|f|f.sync_all()).map_err(error)?;
            if hash(&dest)? != before || hash(&entry.path())? != before { return Err("Файлы менялись во время резервного копирования. Закройте старую Sfery и повторите запуск.".into()); }
        }
    }
    Ok(())
}
fn inventory(dir: &Path, base: &Path, files: &mut Vec<Value>) -> Result<()> {
    for entry in fs::read_dir(dir).map_err(error)? {
        let entry = entry.map_err(error)?;
        if entry.file_type().map_err(error)?.is_dir() { inventory(&entry.path(), base, files)?; }
        else { files.push(json!({"path":entry.path().strip_prefix(base).map_err(error)?,"size":entry.metadata().map_err(error)?.len(),"sha256":hash(&entry.path())?})); }
    }
    Ok(())
}
fn snapshot(dir: &Path, version: &str) -> Result<PathBuf> {
    if !version.chars().all(|c|c.is_ascii_alphanumeric() || c == '.' || c == '-') { return Err("Некорректная версия обновления".into()); }
    let parent = dir.join("backups");
    fs::create_dir_all(&parent).map_err(error)?;
    let name = format!("pre-update-{version}-{}", uuid::Uuid::new_v4());
    let partial = parent.join(format!("{name}.partial"));
    let destination = parent.join(name);
    fs::create_dir(&partial).map_err(error)?;
    // A read transaction pins the snapshot; the backup API includes committed WAL pages.
    let source = Connection::open_with_flags(dir.join("planner.sqlite"), OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(error)?;
    source.busy_timeout(std::time::Duration::from_secs(5)).map_err(error)?;
    let revision: i64 = source.query_row("PRAGMA data_version", [], |r|r.get(0)).map_err(error)?;
    source.execute_batch("BEGIN;").map_err(error)?;
    validate_database(&source)?;
    source.backup(rusqlite::MAIN_DB, partial.join("planner.sqlite"), None).map_err(error)?;
    copy_tree(dir, &partial, true)?;
    let saved = Connection::open(partial.join("planner.sqlite")).map_err(error)?;
    saved.execute_batch("PRAGMA journal_mode=DELETE;").map_err(error)?;
    validate_database(&saved)?;
    drop(saved);
    source.execute_batch("ROLLBACK;").map_err(error)?;
    let after: i64 = source.query_row("PRAGMA data_version", [], |r|r.get(0)).map_err(error)?;
    if revision != after { return Err("База менялась во время резервного копирования. Закройте старую Sfery и повторите запуск.".into()); }
    let mut files = Vec::new();
    inventory(&partial, &partial, &mut files)?;
    files.sort_by_key(|f| f["path"].as_str().unwrap_or("").to_owned());
    write_file(&partial.join("manifest.json"), &serde_json::to_vec_pretty(&json!({"status":"verified","target_version":version,"created_at":chrono::Utc::now().to_rfc3339(),"files":files})).map_err(error)?)?;
    fs::rename(&partial, &destination).map_err(error)?;
    Ok(destination)
}

#[cfg(windows)]
pub fn check_original_process() -> Result<()> {
    use windows_sys::Win32::{Foundation::{CloseHandle, INVALID_HANDLE_VALUE}, System::Diagnostics::ToolHelp::*};
    unsafe {
        let handle = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if handle == INVALID_HANDLE_VALUE { return Err("Не удалось проверить, закрыта ли старая Sfery.".into()); }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut has = Process32FirstW(handle, &mut entry);
        while has != 0 {
            let end = entry.szExeFile.iter().position(|&c|c == 0).unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
            if entry.th32ProcessID != std::process::id() && name.eq_ignore_ascii_case("local-planner.exe") { found = true; break; }
            has = Process32NextW(handle, &mut entry);
        }
        CloseHandle(handle);
        if found { return Err("Закройте обычную Sfery перед первым запуском обновления. Данные ещё не изменялись.".into()); }
    }
    Ok(())
}
#[cfg(not(windows))]
pub fn check_original_process() -> Result<()> { Ok(()) }

pub fn show_error(message: &str) {
    eprintln!("{message}");
    #[cfg(windows)] unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK, MB_ICONERROR};
        let title: Vec<u16> = "Sfery — запуск остановлен\0".encode_utf16().collect();
        let text: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
        MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf);
    impl Temp { fn new() -> Self { let path = std::env::temp_dir().join(format!("sfery-upgrade-test-{}",uuid::Uuid::new_v4())); fs::create_dir_all(&path).unwrap(); Self(path) } }
    impl Drop for Temp { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
    fn database(dir: &Path) -> Connection { let db = Connection::open(dir.join("planner.sqlite")).unwrap(); db.execute_batch(include_str!("schema.sql")).unwrap(); db.execute("INSERT INTO app_settings VALUES('schema_version','9')",[]).unwrap(); db.execute("INSERT INTO tasks(id,title) VALUES(42,'Сохранить задачу')",[]).unwrap(); db }
    #[test] fn wal_attachments_credentials_and_settings_are_backed_up_before_changes() {
        let temp = Temp::new(); let db = database(&temp.0);
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; INSERT INTO app_settings VALUES('theme','dark');").unwrap();
        fs::create_dir_all(temp.0.join("attachments/nested")).unwrap(); fs::write(temp.0.join("attachments/nested/file"),b"attachment").unwrap(); fs::write(temp.0.join("cloud-device.bin"),b"encrypted").unwrap();
        let upgrade = Upgrade::prepare(&temp.0,"1.45.0").unwrap(); let backup = upgrade.backup.as_ref().unwrap();
        let saved = Connection::open(backup.join("planner.sqlite")).unwrap();
        assert_eq!(saved.query_row("SELECT value FROM app_settings WHERE key='theme'",[],|r|r.get::<_,String>(0)).unwrap(),"dark");
        assert_eq!(fs::read(backup.join("attachments/nested/file")).unwrap(),b"attachment");
        assert_eq!(fs::read(backup.join("cloud-device.bin")).unwrap(),b"encrypted");
        assert!(!backup.join("planner.sqlite-wal").exists()); assert!(!temp.0.join(MARKER).exists());
        upgrade.finish().unwrap(); drop(upgrade);
        assert!(Upgrade::prepare(&temp.0,"1.45.0").unwrap().completed);
    }
    #[test] fn second_instance_cannot_open_same_profile() { let temp=Temp::new(); let first=Upgrade::prepare(&temp.0,"1.45.0").unwrap(); assert!(Upgrade::prepare(&temp.0,"1.45.0").is_err()); drop(first); assert!(Upgrade::prepare(&temp.0,"1.45.0").is_ok()); }
    #[test] fn newer_schema_is_rejected_without_rewriting_database() { let temp=Temp::new(); let db=database(&temp.0); db.execute("UPDATE app_settings SET value='10' WHERE key='schema_version'",[]).unwrap(); drop(db); let before=hash(&temp.0.join("planner.sqlite")).unwrap(); assert!(Upgrade::prepare(&temp.0,"1.45.0").is_err()); assert!(crate::storage::Store::new(&temp.0).is_err()); assert_eq!(before,hash(&temp.0.join("planner.sqlite")).unwrap()); assert!(!temp.0.join(MARKER).exists()); }
    #[test] fn migration_failure_rolls_back_added_columns() { let temp=Temp::new(); let db=Connection::open(temp.0.join("planner.sqlite")).unwrap(); db.execute_batch("CREATE TABLE tasks(id INTEGER PRIMARY KEY,completed INTEGER,updated_at TEXT); CREATE VIEW allocated_ids AS SELECT 1;").unwrap(); drop(db); assert!(crate::storage::Store::new(&temp.0).is_err()); let db=Connection::open(temp.0.join("planner.sqlite")).unwrap(); assert!(!db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('tasks') WHERE name='completed_at')",[],|r|r.get::<_,bool>(0)).unwrap()); }
    #[test] fn failed_backup_never_marks_upgrade_complete() { let temp=Temp::new(); drop(database(&temp.0)); fs::write(temp.0.join("backups"),b"not a directory").unwrap(); let before=hash(&temp.0.join("planner.sqlite")).unwrap(); assert!(Upgrade::prepare(&temp.0,"1.45.0").is_err()); assert_eq!(before,hash(&temp.0.join("planner.sqlite")).unwrap()); assert!(!temp.0.join(MARKER).exists()); }
    #[test] fn native_fixture_retains_snapshot_and_all_settings_after_upgrade() {
        let temp=Temp::new(); fs::write(temp.0.join("planner.sqlite"),include_bytes!("../fixtures/original-probe.sqlite")).unwrap();
        let seed=Connection::open(temp.0.join("planner.sqlite")).unwrap();
        seed.execute_batch("INSERT INTO app_settings VALUES('schema_version','9'),('theme','dark'),('custom-preserved-setting','original'),('id_high_water_tasks','9007199254740991'); INSERT INTO allocated_ids VALUES('tasks',99999);").unwrap();
        drop(seed);
        let upgrade=Upgrade::prepare(&temp.0,"1.45.0").unwrap(); let backup=upgrade.backup.clone().unwrap();
        let migrated=crate::storage::Store::new(&temp.0).unwrap();
        let old=Connection::open(backup.join("planner.sqlite")).unwrap();
        let tables=old.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap().query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        for table in tables {
            let rows=|db:&Connection| { let mut q=db.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap(); let count=q.column_count(); let mut rows=q.query_map([],|r|Ok((0..count).map(|i|format!("{:?}",r.get_ref(i).unwrap())).collect::<Vec<_>>())).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap(); rows.sort(); rows };
            let before=rows(&old);let after=rows(&migrated.conn);
            if matches!(table.as_str(),"allocated_ids"|"app_settings") {
                // Reconstruction may fill missing ID reservations and derived high-water settings.
                for row in before { assert!(after.contains(&row),"Existing row in {table} changed"); }
            } else { assert_eq!(before,after,"Table {table} changed"); }
        }
        upgrade.finish().unwrap();
    }
}
