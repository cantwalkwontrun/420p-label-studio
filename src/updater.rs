//! Restart helper, recoverable file replacement and first-window acknowledgement.
use std::{fs, io::Read, os::windows::process::CommandExt, path::{Path, PathBuf}, process::{Command, Stdio}, time::{Duration, Instant}};
use crate::update_package::{self as package, FileRecord, Prepared, APP_EXE};

const MAX_JSON: u64 = 2 * 1024 * 1024;
const SCHEMA: u64 = 1;

#[derive(Clone, Copy, Debug)]
struct ProcessIdentity { pid: u32, created: u64 }
struct Plan { prepared: Prepared, source_version: String, source_hash: String, parent: ProcessIdentity, nonce: String, helper_hash: String, recovery: bool }
#[derive(Clone, Debug)]
struct Previous { file: FileRecord, old: Option<(u64, String)> }
struct Journal { source_version: String, records: Vec<Previous> }

pub struct InstanceGuard { _handle: native::Instance }
pub fn download_guard(install: &Path) -> Result<InstanceGuard, String> { Ok(InstanceGuard { _handle: native::named_guard(install, "download", true)? }) }
pub struct WindowRegistration(std::sync::Mutex<Option<native::Instance>>);
impl WindowRegistration {
    fn open(install: &Path) -> Result<Self, String> {
        Ok(Self(std::sync::Mutex::new(Some(native::instance(install, false)?))))
    }
    // The admission lock serialises this handoff against new editor windows.
    // Reusing the lifetime guard also detects tray windows and older clients.
    fn reserve_restart(&self, install: &Path, _admission: &native::Admission) -> Result<(), String> {
        let mut slot = self.0.lock().map_err(|_| "Cannot check the current app window.")?;
        drop(slot.take());
        match native::instance(install, true) {
            Ok(only) => { *slot = Some(only); Ok(()) }
            Err(error) => { *slot = Some(native::instance(install, false)?); Err(error) }
        }
    }
}
pub enum Startup { Run { acknowledge: Option<String>, message: Option<String>, instance: WindowRegistration }, Exit }

pub(crate) fn version(value: &str) -> Result<[u32; 4], String> {
    let parts: Vec<_> = value.split('.').collect();
    if !(3..=4).contains(&parts.len()) { return Err("Unsupported app update version.".into()); }
    let mut result = [0; 4];
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() || part.len() > 5 || (part.len() > 1 && part.starts_with('0')) || !part.bytes().all(|b| b.is_ascii_digit()) { return Err("Invalid app update version.".into()); }
        result[index] = part.parse::<u16>().map_err(|_| "Unsupported app update version.")? as u32;
    }
    Ok(result)
}

fn read_metadata(path: &Path) -> Result<Vec<u8>, String> {
    package::plain_file(path)?;
    let mut bytes = Vec::new();
    fs::File::open(path).map_err(|e| e.to_string())?.take(MAX_JSON + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_JSON { return Err("Update recovery metadata is too large.".into()); }
    Ok(bytes)
}

fn read_json(path: &Path) -> Result<serde_json::Value, String> {
    serde_json::from_slice(&read_metadata(path)?).map_err(|e| format!("Invalid update recovery metadata: {e}"))
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_JSON { return Err("Update recovery metadata is too large.".into()); }
    if path.exists() { package::plain_file(path)?; }
    crate::project_io::write_project(path, &bytes, path.exists())
}

fn string(value: &serde_json::Value, key: &str) -> Result<String, String> { value[key].as_str().filter(|s| s.len() <= 4096).map(str::to_owned).ok_or_else(|| format!("Invalid update metadata field: {key}")) }
fn record_value(file: &FileRecord) -> serde_json::Value { serde_json::json!({"path":file.path,"bytes":file.bytes,"sha256":file.sha256}) }
fn record(value: &serde_json::Value) -> Result<FileRecord, String> {
    let file = FileRecord { path: string(value, "path")?, bytes: value["bytes"].as_u64().ok_or("Invalid update file size.")?, sha256: string(value, "sha256")? };
    package::validate_records(std::slice::from_ref(&file))?;
    Ok(file)
}
fn nonce() -> String { format!("{:032x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() ^ ((std::process::id() as u128) << 64)) }
fn valid_nonce(value: &str) -> bool { value.len() == 32 && value.bytes().all(|b| b.is_ascii_hexdigit()) }

impl Plan {
    fn value(&self) -> serde_json::Value {
        serde_json::json!({"schema":SCHEMA,"version":self.prepared.version,"sourceVersion":self.source_version,"sourceSha256":self.source_hash,
            "parentPid":self.parent.pid,"parentCreated":self.parent.created,"nonce":self.nonce,"helperSha256":self.helper_hash,"recovery":self.recovery,
            "files":self.prepared.files.iter().map(record_value).collect::<Vec<_>>()})
    }
    fn read(root: &Path) -> Result<Self, String> {
        let value = read_json(&root.join("plan.json"))?;
        if value["schema"].as_u64() != Some(SCHEMA) { return Err("Unsupported update protocol.".into()); }
        let target_version = string(&value, "version")?;
        let source_version = string(&value, "sourceVersion")?;
        version(&target_version)?; version(&source_version)?;
        let files = value["files"].as_array().filter(|a| a.len() <= package::MAX_FILES).ok_or("Invalid update file list.")?.iter().map(record).collect::<Result<Vec<_>, _>>()?;
        package::validate_records(&files)?;
        let plan = Self { prepared: Prepared { version: target_version, install: root.parent().ok_or("Invalid update workspace.")?.to_owned(), files },
            source_version, source_hash: string(&value, "sourceSha256")?, parent: ProcessIdentity {
                pid: u32::try_from(value["parentPid"].as_u64().filter(|n| *n != 0).ok_or("Invalid parent process.")?).map_err(|_| "Invalid parent process.")?,
                created: value["parentCreated"].as_u64().filter(|n| *n != 0).ok_or("Invalid parent process creation time.")? },
            nonce: string(&value, "nonce")?, helper_hash: string(&value, "helperSha256")?, recovery: value["recovery"].as_bool().ok_or("Invalid update mode.")? };
        if !valid_nonce(&plan.nonce) || !package::valid_hash(&plan.source_hash) || !package::valid_hash(&plan.helper_hash) { return Err("Invalid update integrity metadata.".into()); }
        Ok(plan)
    }
}

impl Journal {
    fn value(&self) -> serde_json::Value {
        serde_json::json!({"schema":SCHEMA,"sourceVersion":self.source_version,"records":self.records.iter().map(|r| {
            let old = r.old.as_ref().map(|(bytes, hash)| serde_json::json!({"bytes":bytes,"sha256":hash}));
            serde_json::json!({"file":record_value(&r.file),"old":old})
        }).collect::<Vec<_>>()})
    }
    fn read(backup: &Path) -> Result<Self, String> {
        package::plain_directory(backup)?;
        let value = read_json(&backup.join("journal.json"))?;
        if value["schema"].as_u64() != Some(SCHEMA) { return Err("Unsupported update journal.".into()); }
        let source_version = string(&value, "sourceVersion")?; version(&source_version)?;
        let mut records = Vec::new();
        for row in value["records"].as_array().filter(|a| a.len() <= package::MAX_FILES).ok_or("Invalid update journal.")? {
            let file = record(&row["file"])?;
            let old = if row["old"].is_null() { None } else {
                let hash = string(&row["old"], "sha256")?;
                let bytes = row["old"]["bytes"].as_u64().filter(|n| *n <= package::MAX_EXPANDED).ok_or("Invalid rollback file size.")?;
                if !package::valid_hash(&hash) { return Err("Invalid rollback file digest.".into()); }
                Some((bytes, hash))
            };
            records.push(Previous { file, old });
        }
        package::validate_records(&records.iter().map(|r| r.file.clone()).collect::<Vec<_>>())?;
        Ok(Self { source_version, records })
    }
}

fn remove_file(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(_) => { package::plain_file(path)?; fs::remove_file(path).map_err(|e| e.to_string()) }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

fn write_result(root: &Path, message: &str) { let _ = write_json(&root.join("result.json"), &serde_json::json!({"schema":SCHEMA,"message":message})); }

fn check_lock(root: &Path) -> Result<bool, String> {
    if !root.join("install.lock.json").try_exists().map_err(|e| e.to_string())? { return Ok(false); }
    let _guard = metadata_guard(root)?;
    check_lock_guarded(root)
}

// Windows releases the file lock when the holder exits, including after a
// process crash. All lock-record readers and writers serialise through it.
fn metadata_guard(root: &Path) -> Result<fs::File, String> {
    use std::os::windows::fs::OpenOptionsExt;
    let path = root.join("install-metadata.guard");
    let file = fs::OpenOptions::new().read(true).write(true).create(true).truncate(false)
        .custom_flags(0x00200000) // FILE_FLAG_OPEN_REPARSE_POINT
        .open(&path).map_err(|e| format!("Cannot reserve update metadata: {e}"))?;
    package::plain_file(&path)?;
    file.lock().map_err(|e| format!("Cannot lock update metadata: {e}"))?;
    Ok(file)
}

fn check_lock_guarded(root: &Path) -> Result<bool, String> {
    let lock = root.join("install.lock.json");
    if !lock.try_exists().map_err(|e| e.to_string())? { return Ok(false); }
    let bytes = read_metadata(&lock)?;
    let identity = serde_json::from_slice::<serde_json::Value>(&bytes).ok().and_then(|v| {
        let pid = u32::try_from(v["pid"].as_u64()?).ok().filter(|p| *p != 0)?;
        let created = v["created"].as_u64().filter(|c| *c != 0)?;
        Some(ProcessIdentity { pid, created })
    });
    if let Some(identity) = identity {
        if native::alive(identity)? { return Ok(true); }
    }
    // Earlier releases could leave an empty lock before writing the owner.
    // Keep the plan and journal for normal startup recovery in every case.
    remove_file(&lock)?;
    Ok(false)
}

fn claim_install(root: &Path) -> Result<(), String> {
    let _guard = metadata_guard(root)?;
    if check_lock_guarded(root)? { return Err("Another restart helper is already running.".into()); }
    let own = native::current()?;
    write_json(&root.join("install.lock.json"), &serde_json::json!({"pid":own.pid,"created":own.created}))
}

fn clear_empty_intent(root: &Path) -> Result<(), String> {
    let pending = root.join("pending-backup");
    if pending.exists() && !pending.join("journal.json").exists() {
        package::plain_directory(&pending)?;
        if fs::read_dir(&pending).map_err(|e| e.to_string())?.next().is_some() {
            return Err("An interrupted update has incomplete recovery metadata. Its files have been retained for manual recovery.".into());
        }
        // The journal is persisted before any original file is moved. An empty
        // directory here therefore represents an interruption before changes.
        package::remove_owned(root, "pending-backup")?;
    }
    Ok(())
}

pub fn start(prepared: &Prepared, window: &WindowRegistration) -> Result<(), String> {
    if version(&prepared.version)? <= version(env!("CARGO_PKG_VERSION"))? { return Err("Only a newer app version can be installed.".into()); }
    let install = package::install_directory()?;
    let _download_guard = download_guard(&install)?;
    if prepared.install != install || std::env::current_exe().map_err(|e| e.to_string())?.file_name().and_then(|s| s.to_str()) != Some(APP_EXE) { return Err("The downloaded update belongs to a different app folder.".into()); }
    let root = package::workspace(&install)?;
    if check_lock(&root)? || root.join("pending-backup").exists() { return Err("Another update or recovery is already in progress.".into()); }
    package::recover_stage(&root)?;
    package::verify_files(&root.join("staging/payload"), &prepared.files)?;
    verify_executable(&root.join("staging/payload").join(APP_EXE), &prepared.version)?;
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    let plan = Plan { prepared: prepared.clone(), source_version: env!("CARGO_PKG_VERSION").into(), source_hash: package::hash_file(&current)?,
        parent: native::current()?, nonce: nonce(), helper_hash: package::hash_file(&current)?, recovery: false };
    let admission = native::admission(&install)?;
    window.reserve_restart(&install, &admission)?;
    launch_helper(&root, plan)
}

fn launch_helper(root: &Path, plan: Plan) -> Result<(), String> {
    for name in ["helper.ready", "installed.ready"] { remove_file(&root.join(name))?; }
    let helper = root.join("updater.exe");
    if helper.exists() { package::plain_file(&helper)?; }
    fs::copy(std::env::current_exe().map_err(|e| e.to_string())?, &helper).map_err(|e| format!("Cannot prepare the restart helper: {e}"))?;
    if package::hash_file(&helper)? != plan.helper_hash { return Err("Restart helper verification failed.".into()); }
    write_json(&root.join("plan.json"), &plan.value())?;
    let mut child = Command::new(helper).creation_flags(0x08000000).arg("--apply-update").arg(root.join("plan.json"))
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| format!("Cannot start the update helper: {e}"))?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(12) {
        if fs::read_to_string(root.join("helper.ready")).ok().as_deref() == Some(plan.nonce.as_str()) { return Ok(()); }
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            return Err(failed_attempt(root, &plan).unwrap_or_else(|| "The restart helper could not prepare the update. The current app has been kept open.".into()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill(); let _ = child.wait();
    Err("The restart helper did not respond. The current app has been kept open.".into())
}

pub fn startup() -> Result<Startup, String> {
    startup_with_recovery(launch_helper, native::ask_recovery)
}

fn failed_attempt(root: &Path, plan: &Plan) -> Option<String> {
    let result = read_json(&root.join("result.json")).ok()?;
    let message = result["message"].as_str()?;
    let matches = result["failedAttempt"].as_str() == Some(plan.nonce.as_str());
    // Older helpers wrote only text. Their recorded recovery failure is
    // relevant while the legacy recovery plan and journal are still pending.
    let legacy = result["failedAttempt"].is_null() && message.starts_with("The update could not finish:") && (plan.recovery || {
        let modified = |name| fs::metadata(root.join(name)).and_then(|m| m.modified()).ok();
        message.contains("Rollback needs attention.") &&
            modified("result.json").zip(modified("plan.json")).is_some_and(|(result, plan)| result >= plan)
    });
    (matches || legacy).then(|| message.chars().take(2000).collect())
}

fn record_failure(root: &Path, plan: &Plan, error: &str) {
    let _ = write_json(&root.join("result.json"), &serde_json::json!({
        "schema": SCHEMA, "failedAttempt": plan.nonce, "message": format!("The update could not finish: {error}")
    }));
}

fn startup_with_recovery(launch_recovery: impl FnOnce(&Path, Plan) -> Result<(), String>,
        ask_recovery: impl Fn(&Path, &str) -> Result<bool, String>) -> Result<Startup, String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().and_then(|s| s.to_str()) == Some("--apply-update") {
        if args.len() != 2 { return Err("Invalid restart helper arguments.".into()); }
        let path = PathBuf::from(&args[1]).canonicalize().map_err(|e| e.to_string())?;
        let root = path.parent().ok_or("Invalid update plan.")?;
        if path.file_name().and_then(|s| s.to_str()) != Some("plan.json") || root.file_name().and_then(|s| s.to_str()) != Some(".420p-updater")
            || package::workspace(root.parent().ok_or("Invalid installation folder.")?)? != root
            || std::env::current_exe().map_err(|e| e.to_string())?.canonicalize().map_err(|e| e.to_string())? != root.join("updater.exe") { return Err("The update helper was launched from an unexpected location.".into()); }
        let result = apply(root);
        if let Err(error) = result {
            if let Ok(plan) = Plan::read(root) {
                if failed_attempt(root, &plan).is_none() { record_failure(root, &plan, &error); }
            }
        }
        return Ok(Startup::Exit);
    }
    let install = package::install_directory()?;
    let root_path = install.join(".420p-updater");
    let acknowledge = if args.first().and_then(|s| s.to_str()) == Some("--update-ready") {
        if args.len() != 2 { return Err("Invalid update restart arguments.".into()); }
        let value = args[1].to_str().filter(|s| valid_nonce(s)).ok_or("Invalid update acknowledgement.")?.to_owned();
        Some(value)
    } else { None };
    let run = |acknowledge, message| Ok(Startup::Run { acknowledge, message, instance: WindowRegistration::open(&install)? });
    let mut retry_nonce = None;
    loop {
        let admission = native::admission(&install)?;
        if !root_path.exists() {
            if acknowledge.is_some() { return Err("The update restart metadata is missing.".into()); }
            return run(None, None);
        }
        let root = match package::workspace(&install) {
            Ok(root) => root,
            Err(error) if acknowledge.is_none()
                && !root_path.join("pending-backup").try_exists().unwrap_or(true)
                && !root_path.join("install.lock.json").try_exists().unwrap_or(true) => {
                    // An unrelated or incomplete updater folder can disable
                    // updates without preventing the ordinary editor from opening.
                    return run(None, Some(format!("App updates are unavailable: {error}")));
                }
            Err(error) => return Err(error),
        };
        if let Some(ref token) = acknowledge {
            let plan = Plan::read(&root)?;
            if plan.nonce != *token || version(&plan.prepared.version)? != version(env!("CARGO_PKG_VERSION"))? || !check_lock(&root)? { return Err("The update restart could not be verified.".into()); }
            package::verify_files(&install, &plan.prepared.files)?;
            return run(Some(token.clone()), Some(format!("Installed version {}. Finishing the restart check…", env!("CARGO_PKG_VERSION"))));
        }
        if check_lock(&root)? { return Err("An app update is in progress. Wait for it to finish before opening another window.".into()); }
        if root.join("pending-backup").exists() {
            // Serialise recovery decisions with launches from a second shortcut
            // and with ordinary update requests until the helper is ready.
            let _guard = download_guard(&install)?;
            if check_lock(&root)? { return Err("An app update is in progress. Wait for it to finish.".into()); }
            clear_empty_intent(&root)?;
            if !root.join("pending-backup").exists() {
                return run(None, None);
            }
            let previous = Plan::read(&root)?;
            if fs::read_to_string(root.join("installed.ready")).ok().as_deref() == Some(previous.nonce.as_str())
                && version(&previous.prepared.version)? == version(env!("CARGO_PKG_VERSION"))? {
                package::verify_files(&install, &previous.prepared.files)?;
                let message = match finalise(&root) {
                    Ok(()) => format!("Updated to version {}. The previous version is kept locally for recovery.", env!("CARGO_PKG_VERSION")),
                    Err(_) => "The update is installed. Rotation of the local rollback is still pending; both recovery sets have been retained.".into(),
                };
                return run(None, Some(message));
            }
            // A previous helper stopped mid-transaction. Recovery runs outside the
            // installed executable, after this process has released its file locks.
            let pending = root.join("pending-backup");
            Journal::read(&pending)?;
            let mut plan = previous;
            if retry_nonce.as_ref() != Some(&plan.nonce) {
                let problem = failed_attempt(&root, &plan).or_else(|| plan.recovery.then(|| "The previous recovery attempt was interrupted.".into()));
                if let Some(problem) = problem {
                    drop(_guard);
                    drop(admission);
                    if !ask_recovery(&pending, &problem)? { return Ok(Startup::Exit); }
                    retry_nonce = Some(plan.nonce);
                    continue;
                }
            }
            let _only_launcher = native::instance(&install, true)?;
            plan.parent = native::current()?; plan.nonce = nonce(); plan.recovery = true;
            plan.helper_hash = package::hash_file(&std::env::current_exe().map_err(|e| e.to_string())?)?;
            launch_recovery(&root, plan)?;
            return Ok(Startup::Exit);
        }
        let message = if root.join("result.json").exists() { read_json(&root.join("result.json")).ok().and_then(|value| value["message"].as_str().map(|s| s.chars().take(1500).collect())) } else { None };
        return run(None, message);
    }
}

pub fn acknowledge(token: &str) -> Result<(), String> {
    let install = package::install_directory()?;
    let root = package::workspace(&install)?;
    let plan = Plan::read(&root)?;
    if plan.nonce != token || !valid_nonce(token) { return Err("The update restart token changed.".into()); }
    fs::write(root.join("installed.ready"), token).map_err(|e| e.to_string())
}

fn apply(root: &Path) -> Result<(), String> {
    let plan = Plan::read(root)?;
    let own_image = std::env::current_exe().map_err(|e| e.to_string())?;
    if package::hash_file(&own_image)? != plan.helper_hash { return Err("The restart helper changed after verification.".into()); }
    let parent = native::watch(plan.parent)?;
    let lock_path = root.join("install.lock.json");
    claim_install(root)?;
    let mut parent_closed = false;
    let operation: Result<bool, String> = (|| {
        if !plan.recovery {
            if version(&plan.prepared.version)? <= version(&plan.source_version)? { return Err("An update cannot downgrade or reinstall this version.".into()); }
            if package::hash_file(&plan.prepared.install.join(APP_EXE))? != plan.source_hash { return Err("The installed executable changed before restart.".into()); }
            verify_executable(&plan.prepared.install.join(APP_EXE), &plan.source_version)?;
            package::verify_files(&root.join("staging/payload"), &plan.prepared.files)?;
            verify_executable(&root.join("staging/payload").join(APP_EXE), &plan.prepared.version)?;
        }
        fs::write(root.join("helper.ready"), &plan.nonce).map_err(|e| e.to_string())?;
        parent.wait(Duration::from_secs(120))?;
        parent_closed = true;
        // Every normal app window holds this named object's lifetime. Creating
        // it exclusively after the parent exits detects other open windows in
        // this Windows session, before any application file can be moved.
        let _exclusive = native::instance(&plan.prepared.install, true)?;
        if plan.recovery {
            restore_pending(root, &plan.prepared.install)?;
            write_result(root, "An interrupted update was recovered. The previous version has been restored.");
            return Ok(false);
        }
        let installed = install_files(root, &plan.prepared).and_then(|_| verify_backup(&root.join("pending-backup")));
        if let Err(error) = installed {
            if root.join("pending-backup/journal.json").exists() { restore_pending(root, &plan.prepared.install)?; }
            else { clear_empty_intent(root)?; }
            write_result(root, &format!("The update was not installed; the previous version was restored. {error}"));
            return Ok(false);
        }
        let mut child = match Command::new(plan.prepared.install.join(APP_EXE)).arg("--update-ready").arg(&plan.nonce).current_dir(&plan.prepared.install)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            Ok(child) => child,
            Err(error) => {
                restore_pending(root, &plan.prepared.install)?;
                write_result(root, &format!("The updated app could not start. The previous version was restored. {error}"));
                return Ok(false);
            }
        };
        let start = Instant::now();
        let mut accepted = false;
        while start.elapsed() < Duration::from_secs(60) {
            match child.try_wait() { Ok(None) => {}, Ok(Some(_)) | Err(_) => break }
            if fs::read_to_string(root.join("installed.ready")).ok().as_deref() == Some(plan.nonce.as_str()) { accepted = true; break; }
            std::thread::sleep(Duration::from_millis(100));
        }
        if !accepted {
            let _ = child.kill(); let _ = child.wait();
            restore_pending(root, &plan.prepared.install)?;
            write_result(root, "The updated app did not complete its restart check. The previous version was restored.");
            return Ok(false);
        }
        // The new app checked every installed file before showing its window.
        // Once acknowledged, cleanup failure must never terminate that window
        // or revert work the user may already have started in it.
        let message = match finalise(root) {
            Ok(()) => format!("Updated to version {}. The previous version is kept locally for recovery.", plan.prepared.version),
            Err(_) => "The update is installed. Rotation of the local rollback is still pending; both recovery sets have been retained.".into(),
        };
        write_result(root, &message);
        let _ = package::remove_owned(root, "staging");
        Ok(true)
    })();
    if let Err(error) = &operation { record_failure(root, &plan, error); }
    let _ = metadata_guard(root).and_then(|_guard| remove_file(&lock_path));
    match operation {
        Ok(true) => Ok(()),
        Ok(false) => {
            Command::new(plan.prepared.install.join(APP_EXE)).current_dir(&plan.prepared.install).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
                .map_err(|e| format!("The previous version was restored but could not be reopened: {e}"))?;
            Ok(())
        }
        Err(error) => {
            if parent_closed && !root.join("pending-backup").exists() && package::hash_file(&plan.prepared.install.join(APP_EXE)).ok().as_deref() == Some(plan.source_hash.as_str()) {
                write_result(root, &format!("The update was not installed. {error}"));
                let _ = Command::new(plan.prepared.install.join(APP_EXE)).current_dir(&plan.prepared.install).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
            }
            Err(error)
        }
    }
}

fn install_files(root: &Path, prepared: &Prepared) -> Result<(), String> {
    let payload = root.join("staging/payload");
    package::verify_files(&payload, &prepared.files)?;
    if root.join("pending-backup").exists() { return Err("An earlier update still needs recovery.".into()); }
    let mut journal = Journal { source_version: {
        let plan = Plan::read(root)?; plan.source_version
    }, records: Vec::new() };
    for file in &prepared.files {
        let target = package::checked_target(&prepared.install, &file.path, true)?;
        let old = if target.exists() { Some((fs::metadata(&target).map_err(|e| e.to_string())?.len(), package::hash_file(&target)?)) } else { None };
        journal.records.push(Previous { file: file.clone(), old });
    }
    // Write the complete journal before the first managed file changes.
    let next_journal = root.join("next-journal.json");
    write_json(&next_journal, &journal.value())?;
    fs::create_dir(root.join("pending-backup")).map_err(|e| e.to_string())?;
    fs::rename(next_journal, root.join("pending-backup/journal.json")).map_err(|e| e.to_string())?;
    for record in &journal.records {
        let target = package::checked_target(&prepared.install, &record.file.path, true)?;
        let source = package::checked_target(&payload, &record.file.path, false)?;
        if let Some((bytes, hash)) = &record.old {
            if fs::metadata(&target).map_err(|e| e.to_string())?.len() != *bytes || package::hash_file(&target)? != *hash { return Err("An installed file changed during the update.".into()); }
            let backup = package::checked_target(&root.join("pending-backup"), &record.file.path, true)?;
            if backup.exists() { return Err("An update backup file already exists.".into()); }
            // Keep the launch path present throughout both installation and
            // rollback. A partial backup is harmless while the original lives.
            use std::os::windows::fs::OpenOptionsExt;
            // Deny writers while verifying the recovery copy. Windows
            // requires closing this read handle before replacing the path.
            let mut original = fs::OpenOptions::new().read(true).share_mode(0x1 | 0x4)
                .open(&target).map_err(|e| e.to_string())?;
            let mut copy = fs::File::create_new(&backup).map_err(|e| e.to_string())?;
            let copied = std::io::copy(&mut original, &mut copy).map_err(|e| e.to_string())?;
            copy.sync_all().map_err(|e| e.to_string())?;
            drop(copy);
            if copied != *bytes || package::hash_file(&backup)? != *hash {
                return Err("Could not verify the original app file before replacing it.".into());
            }
            drop(original);
            native::move_file(&source, &target, true)?;
        } else {
            native::move_file(&source, &target, false)?;
        }
    }
    package::verify_files(&prepared.install, &prepared.files)
}

fn verify_backup(backup: &Path) -> Result<(), String> {
    let journal = Journal::read(backup)?;
    for record in journal.records {
        if let Some((bytes, hash)) = record.old {
            let file = package::checked_target(backup, &record.file.path, false)?;
            if fs::metadata(&file).map_err(|e| e.to_string())?.len() != bytes || package::hash_file(&file)? != hash { return Err("The local rollback could not be verified.".into()); }
        }
    }
    Ok(())
}

fn finalise(root: &Path) -> Result<(), String> {
    verify_backup(&root.join("pending-backup"))?;
    package::remove_owned(root, "rollback")?;
    fs::rename(root.join("pending-backup"), root.join("rollback")).map_err(|e| e.to_string())
}

fn restore_pending(root: &Path, install: &Path) -> Result<(), String> {
    let backup = root.join("pending-backup");
    let journal = Journal::read(&backup)?;
    // Look for conflicts before consuming any backup. Restore the launch
    // executable last, so a failed DLL restore can still show recovery choices.
    for apply in [false, true] {
        let reversed = journal.records.iter().rev();
        for record in reversed.clone().filter(|r| !r.file.path.eq_ignore_ascii_case(APP_EXE))
                .chain(reversed.filter(|r| r.file.path.eq_ignore_ascii_case(APP_EXE))) {
            restore_record(install, &backup, record, apply).map_err(|error|
                format!("Rollback needs attention. Recovery files remain in {}. {}: {error}", backup.display(), record.file.path))?;
        }
    }
    package::remove_owned(root, "pending-backup")?;
    let _ = package::remove_owned(root, "staging");
    Ok(())
}

fn restore_record(install: &Path, backup: &Path, record: &Previous, apply: bool) -> Result<(), String> {
    let target = package::checked_target(install, &record.file.path, true)?;
    let saved = package::checked_target(backup, &record.file.path, true)?;
    if let Some((bytes, hash)) = &record.old {
        if target.exists() && fs::metadata(&target).map_err(|e| e.to_string())?.len() == *bytes && package::hash_file(&target)? == *hash { return Ok(()); }
    }
    if saved.exists() {
        package::plain_file(&saved)?;
        let (bytes, hash) = record.old.as_ref().ok_or("Unexpected recovery file.")?;
        if fs::metadata(&saved).map_err(|e| e.to_string())?.len() != *bytes || package::hash_file(&saved)? != *hash { return Err("An original recovery file was changed.".into()); }
        let replace = target.try_exists().map_err(|e| e.to_string())?;
        if replace && package::hash_file(&target)? != record.file.sha256 { return Err("The installed file was changed. Move your edited copy out of the installation folder before retrying.".into()); }
        if apply { native::move_file(&saved, &target, replace)?; }
    } else if record.old.is_some() {
        return Err("An original app file needs manual recovery.".into());
    } else if target.exists() {
        if package::hash_file(&target)? != record.file.sha256 { return Err("The newly installed file was changed. Move your edited copy out of the installation folder before retrying.".into()); }
        if apply { fs::remove_file(target).map_err(|e| e.to_string())?; }
    }
    Ok(())
}

pub fn verify_executable(path: &Path, expected: &str) -> Result<(), String> {
    package::plain_file(path)?;
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut header = [0u8; 64];
    file.read_exact(&mut header).map_err(|_| "The update executable is incomplete.")?;
    if &header[..2] != b"MZ" { return Err("The update is not a Windows executable.".into()); }
    use std::io::{Seek, SeekFrom};
    let offset = u32::from_le_bytes(header[60..64].try_into().unwrap());
    file.seek(SeekFrom::Start(offset as u64)).map_err(|e| e.to_string())?;
    let mut pe = [0u8; 6]; file.read_exact(&mut pe).map_err(|_| "The update has an invalid PE header.")?;
    if &pe[..4] != b"PE\0\0" || u16::from_le_bytes([pe[4], pe[5]]) != 0x8664 { return Err("The update must be a Windows x64 application.".into()); }
    if native::file_version(path)? != version(expected)? { return Err("The update executable's version differs from the GitHub release.".into()); }
    Ok(())
}

pub fn show_error(message: &str) { native::message(message); }

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;
    fn fixture() -> (PathBuf, PathBuf, Prepared) {
        let install = std::env::temp_dir().join(format!("420p-update-install-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&install).unwrap();
        let install = install.canonicalize().unwrap();
        let root = package::workspace(&install).unwrap();
        fs::create_dir_all(root.join("staging/payload/sdk")).unwrap(); fs::create_dir(install.join("sdk")).unwrap();
        fs::write(install.join(APP_EXE), b"previous application").unwrap(); fs::write(install.join("sdk/component.dll"), b"previous component").unwrap();
        fs::write(install.join("label.420p"), b"synthetic user work").unwrap();
        fs::write(root.join("staging/payload").join(APP_EXE), b"next application").unwrap(); fs::write(root.join("staging/payload/sdk/component.dll"), b"next component").unwrap();
        fs::write(root.join("staging/payload/sdk/additional.dll"), b"new component").unwrap();
        let files = [APP_EXE, "sdk/component.dll", "sdk/additional.dll"].into_iter().map(|name| {
            let path = root.join("staging/payload").join(name); FileRecord { path: name.into(), bytes: fs::metadata(&path).unwrap().len(), sha256: package::hash_file(&path).unwrap() }
        }).collect();
        let prepared = Prepared { version: "9.0.0".into(), install: install.clone(), files };
        let plan = Plan { prepared: prepared.clone(), source_version: "1.0.0".into(), source_hash: package::hash_file(&install.join(APP_EXE)).unwrap(), parent: native::current().unwrap(), nonce: "a".repeat(32), helper_hash: "b".repeat(64), recovery: false };
        write_json(&root.join("plan.json"), &plan.value()).unwrap();
        (install, root, prepared)
    }
    fn cleanup(install: PathBuf) {
        assert_eq!(install.parent(), Some(std::env::temp_dir().canonicalize().unwrap().as_path()));
        assert!(install.file_name().unwrap().to_str().unwrap().starts_with("420p-update-install-")); fs::remove_dir_all(install).unwrap();
    }
    fn original(install: &Path) {
        assert_eq!(fs::read(install.join(APP_EXE)).unwrap(), b"previous application");
        assert_eq!(fs::read(install.join("sdk/component.dll")).unwrap(), b"previous component");
        assert_eq!(fs::read(install.join("label.420p")).unwrap(), b"synthetic user work");
    }
    #[test] fn managed_install_can_be_restored_without_touching_user_projects() {
        let (install, root, prepared) = fixture();
        install_files(&root, &prepared).unwrap(); package::verify_files(&install, &prepared.files).unwrap(); verify_backup(&root.join("pending-backup")).unwrap();
        assert_eq!(fs::read(install.join("label.420p")).unwrap(), b"synthetic user work");
        restore_pending(&root, &install).unwrap(); original(&install);
        assert!(!install.join("sdk/additional.dll").exists()); assert!(!root.join("pending-backup").exists()); cleanup(install);
    }
    #[test] fn real_windows_file_lock_after_partial_install_restores_previous_files() {
        let (install, root, prepared) = fixture();
        fs::create_dir(root.join("rollback")).unwrap(); fs::write(root.join("rollback/retained.txt"), b"known good").unwrap();
        let lock = fs::OpenOptions::new().read(true).share_mode(1).open(install.join("sdk/component.dll")).unwrap();
        assert!(install_files(&root, &prepared).is_err());
        // The executable was replaced, but the locked component was retained.
        assert_eq!(fs::read(install.join(APP_EXE)).unwrap(), b"next application");
        restore_pending(&root, &install).unwrap(); original(&install);
        assert_eq!(fs::read(root.join("rollback/retained.txt")).unwrap(), b"known good"); drop(lock); cleanup(install);
    }
    #[test] fn missing_or_changed_staged_files_never_modify_the_current_installation() {
        let (install, root, prepared) = fixture();
        fs::write(root.join("staging/payload/sdk/component.dll"), b"tampered").unwrap();
        assert!(install_files(&root, &prepared).is_err()); original(&install); assert!(!root.join("pending-backup").exists()); cleanup(install);
    }
    #[test] fn journal_recovers_a_crash_between_original_move_and_replacement_move() {
        let (install, root, prepared) = fixture();
        let records = prepared.files.iter().map(|file| {
            let path = install.join(&file.path);
            let old = path.exists().then(|| (fs::metadata(&path).unwrap().len(), package::hash_file(&path).unwrap()));
            Previous { file: file.clone(), old }
        }).collect();
        fs::create_dir(root.join("pending-backup")).unwrap();
        write_json(&root.join("pending-backup/journal.json"), &Journal { source_version: "1.0.0".into(), records }.value()).unwrap();
        fs::rename(install.join(APP_EXE), root.join("pending-backup").join(APP_EXE)).unwrap();
        restore_pending(&root, &install).unwrap(); original(&install); cleanup(install);
    }
    #[test] fn normal_launcher_recovers_interrupted_replacement_and_legacy_empty_lock() {
        // The child runs from the same path as a desktop shortcut. Substitute
        // only the final helper launch, so the test can perform rollback once
        // the child releases its mapped executable. No GUI is opened.
        const PROBE: &str = "LABEL_STUDIO_TEST_LAUNCH_PHASE";
        if let Ok(phase) = std::env::var(PROBE) {
            assert_eq!(std::env::current_exe().unwrap().file_name().unwrap(), APP_EXE);
            let mut called = false;
            let asked = std::cell::Cell::new(false);
            let outcome = startup_with_recovery(|_, plan| {
                assert!(plan.recovery); called = true; Ok(())
            }, |folder, message| {
                assert!(!asked.replace(true));
                assert_eq!(folder.file_name().unwrap(), "pending-backup");
                assert!(message.contains("sdk/component.dll") || message.contains("interrupted"));
                Ok(phase == "retry")
            }).unwrap();
            assert_eq!(asked.get(), matches!(phase.as_str(), "defer" | "retry"));
            assert_eq!(called, matches!(phase.as_str(), "recovery" | "retry"));
            assert_eq!(matches!(outcome, Startup::Exit), called || asked.get());
            if phase == "unowned" {
                assert!(matches!(outcome, Startup::Run { message: Some(text), .. } if text.contains("updates are unavailable")));
            }
            return;
        }
        for locked_file in [APP_EXE, "sdk/component.dll"] {
            let (install, root, mut prepared) = fixture();
            let launcher = install.join(APP_EXE);
            fs::copy(std::env::current_exe().unwrap(), &launcher).unwrap();
            let staged = root.join("staging/payload").join(APP_EXE);
            fs::copy(&launcher, &staged).unwrap();
            // A PE permits trailing data. Both fixture versions launch through
            // Windows, while their distinct digests exercise actual rollback.
            use std::io::Write;
            fs::OpenOptions::new().append(true).open(&staged).unwrap().write_all(b"next test build").unwrap();
            let record = prepared.files.iter_mut().find(|r| r.path == APP_EXE).unwrap();
            record.bytes = fs::metadata(&staged).unwrap().len(); record.sha256 = package::hash_file(&staged).unwrap();
            let old = package::hash_file(&launcher).unwrap(); let next = record.sha256.clone();
            let mut plan = Plan::read(&root).unwrap(); plan.prepared = prepared.clone(); plan.source_hash = old.clone();
            write_json(&root.join("plan.json"), &plan.value()).unwrap();
            let probe = |phase| {
                let child = Command::new(&launcher).creation_flags(0x08000000)
                    .args(["--exact", "updater::tests::normal_launcher_recovers_interrupted_replacement_and_legacy_empty_lock"])
                    .env(PROBE, phase).output().unwrap();
                assert!(child.status.success(), "{}", String::from_utf8_lossy(&child.stdout));
            };
            if locked_file == APP_EXE {
                let marker = root.join("owner.txt");
                let contents = fs::read(&marker).unwrap();
                fs::remove_file(&marker).unwrap();
                probe("unowned");
                assert!(!marker.exists());
                fs::write(&marker, contents).unwrap();
            }
            fs::write(root.join("install.lock.json"), b"").unwrap();
            probe("run"); assert!(!root.join("install.lock.json").exists());
            let lock = fs::OpenOptions::new().read(true).share_mode(1).open(install.join(locked_file)).unwrap();
            assert!(install_files(&root, &prepared).is_err()); drop(lock);
            assert_eq!(package::hash_file(&launcher).unwrap(), if locked_file == APP_EXE { &old } else { &next }.as_str());
            probe("recovery");
            let component = install.join("sdk/component.dll");
            let prior_component = fs::read(&component).unwrap();
            fs::write(&component, b"Independent SDK edit").unwrap();
            let conflict = restore_pending(&root, &install).unwrap_err();
            assert!(conflict.contains("sdk/component.dll"));
            record_failure(&root, &plan, &conflict);
            probe("defer"); probe("defer"); probe("retry");
            assert_eq!(fs::read(&component).unwrap(), b"Independent SDK edit");
            fs::write(&component, prior_component).unwrap();
            assert_eq!(Plan::read(&root).unwrap().nonce, plan.nonce);
            plan.nonce = nonce();
            write_json(&root.join("plan.json"), &plan.value()).unwrap();
            probe("recovery"); // A result from another transaction is ignored.
            plan.recovery = true;
            write_json(&root.join("plan.json"), &plan.value()).unwrap();
            probe("defer"); // A recovery helper interrupted before recording its result.
            write_result(&root, "The update could not finish: Conflicting sdk/component.dll");
            probe("defer"); // Pre-upgrade recovery results contained only a message.
            restore_pending(&root, &install).unwrap();
            assert_eq!(package::hash_file(&launcher).unwrap(), old);
            probe("run");
            cleanup(install);
        }
    }
    #[test] fn external_changes_are_preserved_when_rollback_cannot_safely_replace_them() {
        let (install, root, prepared) = fixture(); install_files(&root, &prepared).unwrap();
        fs::write(install.join(APP_EXE), b"externally modified application").unwrap();
        assert!(restore_pending(&root, &install).unwrap_err().contains("Recovery files remain"));
        assert_eq!(fs::read(install.join(APP_EXE)).unwrap(), b"externally modified application");
        assert_eq!(fs::read(root.join("pending-backup").join(APP_EXE)).unwrap(), b"previous application");
        assert_eq!(fs::read(install.join("label.420p")).unwrap(), b"synthetic user work"); cleanup(install);
    }
    #[test] fn rollback_rotation_keeps_the_new_recovery_point_and_removes_only_the_old_set() {
        let (install, root, prepared) = fixture();
        fs::create_dir(root.join("rollback")).unwrap(); fs::write(root.join("rollback/previous-set.txt"), b"old recovery set").unwrap();
        install_files(&root, &prepared).unwrap(); finalise(&root).unwrap();
        assert!(!root.join("pending-backup").exists()); assert!(!root.join("rollback/previous-set.txt").exists());
        assert_eq!(fs::read(root.join("rollback").join(APP_EXE)).unwrap(), b"previous application");
        package::verify_files(&install, &prepared.files).unwrap(); assert_eq!(fs::read(install.join("label.420p")).unwrap(), b"synthetic user work"); cleanup(install);
    }
    #[test] fn damaged_recovery_files_prevent_rotation_of_the_known_good_set() {
        let (install, root, prepared) = fixture(); fs::create_dir(root.join("rollback")).unwrap(); fs::write(root.join("rollback/keep.txt"), b"keep").unwrap();
        install_files(&root, &prepared).unwrap(); fs::write(root.join("pending-backup").join(APP_EXE), b"changed backup").unwrap();
        assert!(finalise(&root).is_err()); assert_eq!(fs::read(root.join("rollback/keep.txt")).unwrap(), b"keep"); cleanup(install);
    }
    #[test] fn process_identity_rejects_pid_reuse_and_waits_for_the_exact_process() {
        let current = native::current().unwrap(); assert!(native::alive(current).unwrap());
        assert!(!native::alive(ProcessIdentity { created: current.created + 1, ..current }).unwrap());
        assert!(native::watch(ProcessIdentity { created: current.created + 1, ..current }).is_err());
        assert!(native::watch(current).unwrap().wait(Duration::ZERO).is_err());
    }
    #[test] fn other_app_windows_block_installation_until_their_guard_is_released() {
        let (install, _, _) = fixture();
        let first = WindowRegistration::open(&install).unwrap();
        let second = native::instance(&install, false).unwrap(); // also used by the previous release
        let gate = native::admission(&install).unwrap();
        assert!(first.reserve_restart(&install, &gate).unwrap_err().contains("tray"));
        drop(second);
        assert!(native::instance(&install, true).is_err()); // the original window kept its registration
        first.reserve_restart(&install, &gate).unwrap();
        assert!(native::instance(&install, true).is_err());
        drop(first); drop(gate);
        assert!(native::instance(&install, true).is_ok()); cleanup(install);
    }
    #[test] fn concurrent_downloads_are_serialised_separately_from_editor_windows() {
        let (install, _, _) = fixture(); let editor = native::instance(&install, false).unwrap();
        let download = download_guard(&install).unwrap(); assert!(download_guard(&install).is_err());
        drop(download); let retry = download_guard(&install).unwrap(); drop(retry); drop(editor); cleanup(install);
    }
    #[test] fn live_updater_lock_blocks_a_second_installer_and_stale_identity_is_cleared() {
        let (install, root, _) = fixture(); let current = native::current().unwrap();
        write_json(&root.join("install.lock.json"), &serde_json::json!({"pid":current.pid,"created":current.created})).unwrap(); assert!(check_lock(&root).unwrap());
        write_json(&root.join("install.lock.json"), &serde_json::json!({"pid":current.pid,"created":current.created+1})).unwrap(); assert!(!check_lock(&root).unwrap());
        assert!(!root.join("install.lock.json").exists()); cleanup(install);
    }
    #[test] fn stale_malformed_lock_is_replaced_by_one_complete_owner() {
        let (install, root, _) = fixture();
        fs::write(root.join("install.lock.json"), b"{").unwrap();
        let barrier = std::sync::Barrier::new(2);
        let successes = std::thread::scope(|scope| {
            let attempt = || { barrier.wait(); claim_install(&root).is_ok() };
            let worker = scope.spawn(attempt);
            usize::from(attempt()) + usize::from(worker.join().unwrap())
        });
        assert_eq!(successes, 1); assert!(check_lock(&root).unwrap());
        cleanup(install);
    }
    #[test] fn invalid_version_protocol_and_pe_metadata_are_rejected() {
        for value in ["1.2", "1.2.3-beta", "1.02.3", "65536.0.0", "../1.2.3", "1.2.3.4.5"] { assert!(version(value).is_err(), "{value}"); }
        assert_eq!(version("1.2.3").unwrap(), version("1.2.3.0").unwrap());
        let (install, root, _) = fixture(); assert!(verify_executable(&install.join(APP_EXE), "1.2.3").is_err());
        let mut value = read_json(&root.join("plan.json")).unwrap(); value["files"][0]["path"] = serde_json::json!("../outside.exe"); write_json(&root.join("plan.json"), &value).unwrap();
        assert!(Plan::read(&root).is_err()); cleanup(install);
    }
    #[test] fn an_interruption_before_the_journal_does_not_block_the_unchanged_app() {
        let (install, root, _) = fixture(); fs::create_dir(root.join("pending-backup")).unwrap();
        clear_empty_intent(&root).unwrap(); original(&install); assert!(!root.join("pending-backup").exists()); cleanup(install);
    }
    #[test] fn missing_recovery_metadata_never_causes_unknown_backups_to_be_deleted() {
        let (install, root, _) = fixture(); fs::create_dir(root.join("pending-backup")).unwrap(); fs::write(root.join("pending-backup/original.dll"), b"keep").unwrap();
        assert!(clear_empty_intent(&root).is_err()); assert_eq!(fs::read(root.join("pending-backup/original.dll")).unwrap(), b"keep"); original(&install); cleanup(install);
    }
}

mod native {
    use super::*;
    use std::{ffi::c_void, os::windows::{ffi::OsStrExt, io::{AsRawHandle, FromRawHandle, OwnedHandle}}};
    type Handle = *mut c_void;
    #[repr(C)] #[derive(Default)] struct FileTime { low: u32, high: u32 }
    #[link(name="kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn GetProcessTimes(process: Handle, created: *mut FileTime, exit: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
        fn WaitForSingleObject(handle: Handle, timeout: u32) -> u32;
        fn CloseHandle(handle: Handle) -> i32;
        fn CreateMutexW(attributes: *const c_void, owner: i32, name: *const u16) -> Handle;
        fn ReleaseMutex(mutex: Handle) -> i32;
        fn MoveFileExW(existing: *const u16, target: *const u16, flags: u32) -> i32;
        fn GetLastError() -> u32;
    }
    pub(super) fn move_file(source: &Path, target: &Path, replace: bool) -> Result<(), String> {
        // Same-volume rename with write-through. Existing managed paths always
        // name either their old or new file, including the main executable.
        let source = wide(source.as_os_str()); let target = wide(target.as_os_str());
        let flags = 0x8 | u32::from(replace); // MOVEFILE_WRITE_THROUGH | REPLACE_EXISTING
        if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), flags) } == 0 {
            return Err(format!("Cannot replace an application file: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }
    fn created(handle: Handle) -> Result<u64, String> {
        let (mut a, mut b, mut c, mut d) = (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
        // SAFETY: The process handle is live and all output buffers are valid.
        if unsafe { GetProcessTimes(handle, &mut a, &mut b, &mut c, &mut d) } == 0 { return Err(std::io::Error::last_os_error().to_string()); }
        Ok(((a.high as u64) << 32) | a.low as u64)
    }
    pub(super) fn current() -> Result<ProcessIdentity, String> {
        // SAFETY: GetCurrentProcess returns a process-owned pseudo-handle.
        Ok(ProcessIdentity { pid: std::process::id(), created: created(unsafe { GetCurrentProcess() })? })
    }
    pub(super) struct Watch(Handle);
    impl Drop for Watch { fn drop(&mut self) { unsafe { CloseHandle(self.0); } } }
    pub(super) struct Instance(OwnedHandle);
    pub(super) struct Admission(Instance, std::marker::PhantomData<std::rc::Rc<()>>);
    impl Drop for Admission {
        fn drop(&mut self) { unsafe { ReleaseMutex(self.0.0.as_raw_handle()); } }
    }
    pub(super) fn admission(install: &Path) -> Result<Admission, String> {
        let guard = named_guard(install, "admission", false)?;
        match unsafe { WaitForSingleObject(guard.0.as_raw_handle(), 20_000) } {
            0 | 0x80 => Ok(Admission(guard, std::marker::PhantomData)),
            258 => Err("Another window is preparing a restart. Try again when it has finished.".into()),
            _ => Err(format!("Cannot check app windows: {}", std::io::Error::last_os_error())),
        }
    }
    pub(super) fn instance(install: &Path, exclusive: bool) -> Result<Instance, String> {
        named_guard(install, "windows", exclusive)
    }
    pub(super) fn named_guard(install: &Path, kind: &str, exclusive: bool) -> Result<Instance, String> {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(install.to_string_lossy().to_lowercase().as_bytes());
        let suffix: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        let name = wide(std::ffi::OsStr::new(&format!("Local\\420PLabelStudio-{kind}-{suffix}")));
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        let error = unsafe { GetLastError() };
        if handle.is_null() { return Err(format!("Cannot reserve the app's update guard: {}", std::io::Error::from_raw_os_error(error as i32))); }
        let guard = Instance(unsafe { OwnedHandle::from_raw_handle(handle) });
        if exclusive && error == 183 {
            return Err(if kind == "download" { "Another window is already preparing an update." } else { "Close every other 420P Label Studio window, including tray windows, before installing an update." }.into());
        }
        Ok(guard)
    }
    pub(super) fn watch(identity: ProcessIdentity) -> Result<Watch, String> {
        // SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION. Creation time closes
        // the PID-reuse race; this exact kernel handle is retained while waiting.
        let handle = unsafe { OpenProcess(0x00100000 | 0x1000, 0, identity.pid) };
        if handle.is_null() { return Err(std::io::Error::last_os_error().to_string()); }
        let watch = Watch(handle);
        if created(handle)? != identity.created { return Err("The parent process identity changed.".into()); }
        Ok(watch)
    }
    pub(super) fn alive(identity: ProcessIdentity) -> Result<bool, String> {
        let handle = unsafe { OpenProcess(0x00100000 | 0x1000, 0, identity.pid) };
        if handle.is_null() {
            let error = std::io::Error::last_os_error();
            return if error.raw_os_error() == Some(87) { Ok(false) } else { Err(format!("Cannot inspect the updater process: {error}")) };
        }
        let watch = Watch(handle);
        if created(watch.0)? != identity.created { return Ok(false); }
        match unsafe { WaitForSingleObject(watch.0, 0) } { 0 => Ok(false), 258 => Ok(true), _ => Err("Cannot inspect the updater process.".into()) }
    }
    impl Watch {
        pub(super) fn wait(&self, timeout: Duration) -> Result<(), String> {
            match unsafe { WaitForSingleObject(self.0, timeout.as_millis().min(u32::MAX as u128) as u32) } {
                0 => Ok(()), 258 => Err("The app did not close in time; no update was installed.".into()), _ => Err("Cannot wait for the app to close.".into()),
            }
        }
    }
    fn wide(path: &std::ffi::OsStr) -> Vec<u16> { path.encode_wide().chain(Some(0)).collect() }
    #[link(name="version")]
    unsafe extern "system" {
        fn GetFileVersionInfoSizeW(filename: *const u16, handle: *mut u32) -> u32;
        fn GetFileVersionInfoW(filename: *const u16, handle: u32, length: u32, data: *mut c_void) -> i32;
        fn VerQueryValueW(block: *const c_void, sub: *const u16, value: *mut *mut c_void, length: *mut u32) -> i32;
    }
    pub(super) fn file_version(path: &Path) -> Result<[u32; 4], String> {
        let path = wide(path.as_os_str()); let mut ignored = 0;
        let size = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), &mut ignored) };
        if size == 0 || size > 1024 * 1024 { return Err("The update executable has no valid version information.".into()); }
        let mut data = vec![0u8; size as usize];
        if unsafe { GetFileVersionInfoW(path.as_ptr(), 0, size, data.as_mut_ptr().cast()) } == 0 { return Err("Cannot read the update executable's version.".into()); }
        let query = [b'\\' as u16, 0]; let mut pointer = std::ptr::null_mut(); let mut length = 0;
        if unsafe { VerQueryValueW(data.as_ptr().cast(), query.as_ptr(), &mut pointer, &mut length) } == 0 || pointer.is_null() || length < 52 { return Err("Invalid Windows version metadata.".into()); }
        // VS_FIXEDFILEINFO's signature, structure version, fileVersionMS/LS.
        let fields: [u32; 4] = std::array::from_fn(|index| unsafe { std::ptr::read_unaligned(pointer.cast::<u32>().add(index)) });
        if fields[0] != 0xFEEF04BD { return Err("Invalid Windows version signature.".into()); }
        Ok([fields[2] >> 16, fields[2] & 0xffff, fields[3] >> 16, fields[3] & 0xffff])
    }
    #[link(name="user32")] unsafe extern "system" { fn MessageBoxW(window: Handle, text: *const u16, caption: *const u16, kind: u32) -> i32; }
    fn dialog(message: &str, kind: u32) -> i32 {
        let text = wide(std::ffi::OsStr::new(message)); let title = wide(std::ffi::OsStr::new(crate::branding::APP_NAME));
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), kind) }
    }
    pub(super) fn message(message: &str) { dialog(message, 0x10); }
    pub(super) fn ask_recovery(folder: &Path, problem: &str) -> Result<bool, String> {
        let text = format!("{problem}\n\nRecovery folder: {}\n\nYes — retry recovery after resolving the conflict.\nNo — open the recovery folder in Explorer.\nCancel — close Studio and keep the recovery files.", folder.display());
        // Yes/No/Cancel with Cancel as the default, including Escape.
        match dialog(&text, 0x233) {
            6 => Ok(true),
            7 => {
                package::plain_directory(folder)?;
                let explorer = PathBuf::from(std::env::var_os("SystemRoot").ok_or("Cannot locate Windows Explorer.")?).join("explorer.exe");
                Command::new(explorer).arg(folder).spawn().map_err(|e| format!("Cannot open the recovery folder: {e}"))?;
                Ok(false)
            }
            2 => Ok(false),
            _ => Err(format!("Cannot display the recovery choices. Recovery folder: {}", folder.display())),
        }
    }
}
