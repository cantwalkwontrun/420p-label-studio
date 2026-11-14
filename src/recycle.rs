//! Recycle one verified original, never fall back to a direct delete.
//! Windows shell references: IFileOperation::SetOperationFlags and
//! IFileOperationProgressSink::PreDeleteItem on learn.microsoft.com.
use std::{fs, path::{Path, PathBuf}, os::windows::{ffi::OsStrExt, fs::MetadataExt}, sync::{Arc, atomic::{AtomicBool, Ordering}}};
use windows::{core::{implement, Error, HRESULT, PCWSTR, Ref}, Win32::{Foundation::E_ABORT,
    Storage::FileSystem::GetDriveTypeW, System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED},
    UI::Shell::{FileOperation, IFileOperation, IFileOperationProgressSink, IFileOperationProgressSink_Impl, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
        FOFX_RECYCLEONDELETE, FOFX_ADDUNDORECORD, FOFX_EARLYFAILURE, FOF_NOERRORUI, FOF_NOCONFIRMATION, FOF_SILENT, FOF_NO_CONNECTED_ELEMENTS, FOF_NORECURSEREPARSE, TSF_DELETE_RECYCLE_IF_POSSIBLE}}};

fn validate(path: &Path, expected: Option<&[u8]>) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if meta.file_attributes() & 0x400 != 0 || meta.file_type().is_symlink() || path.file_name().is_none() { return Err("Links and drive roots are kept.".into()); }
    let canonical = fs::canonicalize(path).map_err(|e| e.to_string())?;
    if canonical != path { return Err("The original's location changed; it was kept.".into()); }
    match expected {
        Some(bytes) if meta.is_file() && meta.len() == bytes.len() as u64 && fs::read(path).map_err(|e| e.to_string())? == bytes => Ok(()),
        None if meta.is_dir() && fs::read_dir(path).map_err(|e| e.to_string())?.next().is_none() => Ok(()),
        _ => Err("The original changed or the folder is not empty; it was kept.".into()),
    }
}
fn shell_path(item: &IShellItem) -> windows::core::Result<PathBuf> {
    // SAFETY: Shell owns the returned allocation; it is freed after copying.
    unsafe {
        let raw = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let value = raw.to_string(); CoTaskMemFree(Some(raw.0.cast()));
        Ok(PathBuf::from(value?))
    }
}
#[implement(IFileOperationProgressSink)]
struct RecycleGuard { path: PathBuf, expected: Option<Vec<u8>>, confirmed: Arc<AtomicBool> }
#[allow(non_snake_case)]
impl IFileOperationProgressSink_Impl for RecycleGuard_Impl {
    fn StartOperations(&self) -> windows::core::Result<()> { Ok(()) }
    fn FinishOperations(&self, result: HRESULT) -> windows::core::Result<()> { result.ok() }
    fn PreDeleteItem(&self, flags: u32, item: Ref<IShellItem>) -> windows::core::Result<()> {
        if flags & TSF_DELETE_RECYCLE_IF_POSSIBLE.0 as u32 == 0 { return Err(Error::from_hresult(E_ABORT)); }
        let item = item.as_ref().ok_or_else(|| Error::from_hresult(E_ABORT))?;
        let actual = shell_path(item).ok().and_then(|p| fs::canonicalize(p).ok());
        if actual.as_deref() != Some(self.path.as_path()) || validate(&self.path, self.expected.as_deref()).is_err() { return Err(Error::from_hresult(E_ABORT)); }
        Ok(())
    }
    fn PostDeleteItem(&self, _: u32, _: Ref<IShellItem>, result: HRESULT, recycled: Ref<IShellItem>) -> windows::core::Result<()> {
        result.ok()?;
        if recycled.as_ref().is_none() { return Err(Error::from_hresult(E_ABORT)); }
        self.confirmed.store(true, Ordering::Release); Ok(())
    }
    fn PreRenameItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> { Err(Error::from_hresult(E_ABORT)) }
    fn PostRenameItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR, _: HRESULT, _: Ref<IShellItem>) -> windows::core::Result<()> { Ok(()) }
    fn PreMoveItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> { Err(Error::from_hresult(E_ABORT)) }
    fn PostMoveItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR, _: HRESULT, _: Ref<IShellItem>) -> windows::core::Result<()> { Ok(()) }
    fn PreCopyItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> { Err(Error::from_hresult(E_ABORT)) }
    fn PostCopyItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR, _: HRESULT, _: Ref<IShellItem>) -> windows::core::Result<()> { Ok(()) }
    fn PreNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> { Err(Error::from_hresult(E_ABORT)) }
    fn PostNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR, _: &PCWSTR, _: u32, _: HRESULT, _: Ref<IShellItem>) -> windows::core::Result<()> { Ok(()) }
    fn UpdateProgress(&self, _: u32, _: u32) -> windows::core::Result<()> { Ok(()) }
    fn ResetTimer(&self) -> windows::core::Result<()> { Ok(()) }
    fn PauseTimer(&self) -> windows::core::Result<()> { Ok(()) }
    fn ResumeTimer(&self) -> windows::core::Result<()> { Ok(()) }
}
struct ComScope;
impl Drop for ComScope { fn drop(&mut self) { unsafe { CoUninitialize(); } } }

pub fn item(path: &Path, expected: Option<&[u8]>) -> Result<(), String> {
    validate(path, expected)?;
    let drive = match path.components().next() {
        Some(std::path::Component::Prefix(prefix)) => match prefix.kind() { std::path::Prefix::Disk(letter) | std::path::Prefix::VerbatimDisk(letter) => letter, _ => return Err("Recycling is limited to local fixed disks; the original was kept.".into()) },
        _ => return Err("The original's absolute location is required.".into()),
    };
    let root = [drive as u16, b':' as u16, b'\\' as u16, 0];
    // SAFETY: The root and item paths are live, NUL-terminated UTF-16 strings.
    if unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } != 3 { return Err("Recycling is limited to local fixed disks; the original was kept.".into()); }
    let mut stage = "initialising the Windows shell";
    let result = (|| -> windows::core::Result<()> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?; }
        let _scope = ComScope;
        let confirmed = Arc::new(AtomicBool::new(false));
        let sink: IFileOperationProgressSink = RecycleGuard { path: path.to_owned(), expected: expected.map(<[u8]>::to_vec), confirmed: confirmed.clone() }.into();
        let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
        // The shell parser rejects canonical Win32 \\?\ prefixes. The
        // validated input is a local disk path, so remove only that prefix.
        if wide.starts_with(&[92, 92, 63, 92]) { wide.drain(..4); }
        wide.push(0);
        // SAFETY: COM is initialised on this worker. All interfaces and input
        // buffers remain alive until the operation and its callbacks finish.
        unsafe {
            let operation: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)?;
            stage = "opening the verified original";
            let source: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None)?;
            stage = "setting recycle-only flags";
            operation.SetOperationFlags(FOFX_RECYCLEONDELETE | FOFX_ADDUNDORECORD | FOFX_EARLYFAILURE | FOF_NOERRORUI | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NO_CONNECTED_ELEMENTS | FOF_NORECURSEREPARSE)?;
            stage = "queueing the original for recycling";
            operation.DeleteItem(&source, &sink)?;
            stage = "recycling the original";
            operation.PerformOperations()?;
            stage = "checking the Recycle Bin result";
            if operation.GetAnyOperationsAborted()?.as_bool() || !confirmed.load(Ordering::Acquire) || path.exists() { return Err(Error::from_hresult(E_ABORT)); }
        }
        Ok(())
    })();
    result.map_err(|e| format!("Windows did not confirm recycling while {stage}; check the original and Recycle Bin: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changed_originals_and_nonempty_folders_are_never_eligible() {
        let folder = std::env::temp_dir().join(format!("420p-recycle-guard-{}", std::process::id())); fs::create_dir(&folder).unwrap(); let folder = fs::canonicalize(folder).unwrap();
        let file = folder.join("original.dymo"); fs::write(&file, b"newer contents").unwrap();
        assert!(validate(&file, Some(b"old contents")).is_err()); assert!(validate(&folder, None).is_err()); assert_eq!(fs::read(&file).unwrap(), b"newer contents");
        fs::remove_file(file).unwrap(); fs::remove_dir(folder).unwrap();
    }
    #[test]
    fn windows_recycles_only_a_synthetic_fixture_when_explicitly_enabled() {
        if std::env::var("DYMO_TEST_RECYCLE").ok().as_deref() != Some("1") { return; }
        let folder = std::env::temp_dir().join(format!("420p-recycle-fixture-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())); fs::create_dir(&folder).unwrap(); let folder = fs::canonicalize(folder).unwrap();
        let file = folder.join("synthetic-label.dymo"); fs::write(&file, b"Synthetic label conversion test").unwrap();
        item(&file, Some(b"Synthetic label conversion test")).unwrap(); assert!(!file.exists()); item(&folder, None).unwrap(); assert!(!folder.exists());
    }
}
