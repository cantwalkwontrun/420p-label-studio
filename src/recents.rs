//! Recent label paths are stored locally and never uploaded.
use std::{io::Read, path::{Path, PathBuf}};
fn location() -> Option<PathBuf> {
    #[cfg(test)] { return std::env::var_os("DYMO_TEST_RECENTS_DIR").map(|p| PathBuf::from(p).join("recent-labels.json")); }
    #[cfg(not(test))] { Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("420P Label Studio").join("recent-labels.json")) }
}
pub fn load() -> Vec<PathBuf> {
    let Some(path) = location() else { return Vec::new() }; let Ok(file) = std::fs::File::open(path) else { return Vec::new() };
    let mut data = Vec::new(); if file.take(65537).read_to_end(&mut data).is_err() || data.len() > 65536 { return Vec::new(); }
    serde_json::from_slice::<Vec<String>>(&data).unwrap_or_default().into_iter().filter(|p| !p.is_empty() && p.len() <= 32768).take(10).map(PathBuf::from).collect()
}
pub fn record(path: &Path) {
    if !path.is_absolute() || !path.is_file() { return; }
    let Some(store) = location() else { return }; let Some(parent) = store.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() { return; }
    let mut entries = load(); let key = path.to_string_lossy().to_lowercase();
    entries.retain(|p| p.to_string_lossy().to_lowercase() != key); entries.insert(0, path.to_owned()); entries.truncate(10);
    if let Ok(bytes) = serde_json::to_vec(&entries) { let _ = crate::project_io::write_project(&store, &bytes, store.is_file()); }
}
pub fn clear() { if let Some(path) = location() { let _ = std::fs::remove_file(path); } }
