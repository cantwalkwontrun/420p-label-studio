//! Local, non-destructive file and folder conversion. No source is overwritten.
use eframe::egui;
use std::{fs, path::{Path, PathBuf}, sync::{Arc, atomic::{AtomicBool, Ordering}}};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Direction { #[default] ToProject, ToDymo }
impl Direction {
    pub fn extension(self) -> &'static str { match self { Self::ToProject => "420p", Self::ToDymo => "dymo" } }
    fn accepts(self, path: &Path) -> bool {
        path.extension().and_then(|e| e.to_str()).is_some_and(|e| match self {
            Self::ToProject => e.eq_ignore_ascii_case("dymo") || e.eq_ignore_ascii_case("label"),
            Self::ToDymo => e.eq_ignore_ascii_case("420p"),
        })
    }
}
#[derive(Clone)]
pub enum Source { Files(Vec<PathBuf>), Folder(PathBuf) }
#[derive(Clone)]
pub enum Destination { BesideOriginals, Folder(PathBuf), NewFolder { parent: PathBuf, name: String } }
#[derive(Clone)]
pub struct Request {
    pub source: Source, pub destination: Destination, pub direction: Direction, pub recursive: bool,
    pub preserve_folders: bool, pub recycle_originals: bool, pub remove_empty_folders: bool,
}
#[cfg(test)]
impl Request {
    pub fn folder(source: PathBuf, output: PathBuf, direction: Direction, recursive: bool) -> Self { Self { source: Source::Folder(source), destination: Destination::Folder(output), direction, recursive, preserve_folders: true, recycle_originals: false, remove_empty_folders: false } }
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum DestinationMode { #[default] BesideOriginals, ExistingFolder, NewFolder }
pub enum Event { Progress { processed: usize, total: usize }, Finished(Summary) }
#[derive(Default)]
pub struct Summary { pub converted: usize, pub skipped: usize, pub processed: usize, pub total: usize, pub recycled: usize, pub folders_removed: usize, pub cancelled: bool, pub details: Vec<String>, pub output_folders: Vec<PathBuf> }
impl Summary {
    pub fn message(&self) -> String {
        if self.total == 0 { return format!("{} {}", if self.cancelled { "Stopped." } else { "No labels converted." }, self.details.first().map(String::as_str).unwrap_or("No matching labels found.")); }
        format!("{}Converted {}; skipped {}; processed {} of {}. Recycled {} original(s) and {} empty folder(s).", if self.cancelled { "Stopped. " } else { "" }, self.converted, self.skipped, self.processed, self.total, self.recycled, self.folders_removed)
    }
    fn detail(&mut self, value: String) { if self.details.len() < 100 { self.details.push(value.chars().take(1000).collect()); } }
}

pub struct Converter {
    pub open: bool,
    direction: Direction,
    source: Option<Source>,
    output: Option<PathBuf>,
    destination_mode: DestinationMode,
    new_folder_name: String,
    recursive: bool,
    preserve_folders: bool,
    recycle_originals: bool,
    remove_empty_folders: bool,
    open_when_finished: bool,
    output_folders: Vec<PathBuf>,
    pub cancel: Option<Arc<AtomicBool>>,
    pub status: String,
    pub details: Vec<String>,
}
impl Default for Converter {
    fn default() -> Self { Self { open: false, direction: Direction::ToProject, source: None, output: None, destination_mode: DestinationMode::BesideOriginals, new_folder_name: "Converted labels".into(), recursive: false, preserve_folders: true, recycle_originals: false, remove_empty_folders: false, open_when_finished: false, output_folders: Vec::new(), cancel: None, status: String::new(), details: Vec::new() } }
}
impl Converter {
    pub fn finish(&mut self, summary: Summary) { self.finish_with(summary, open_output_folder); }
    fn finish_with(&mut self, summary: Summary, mut open: impl FnMut(&Path) -> Result<(), String>) {
        self.status = summary.message(); self.cancel = None;
        let auto_open = self.open_when_finished && summary.converted > 0 && !summary.cancelled;
        self.details = summary.details; self.output_folders = summary.output_folders;
        if auto_open && self.output_folders.len() == 1 {
            if let Err(error) = open(&self.output_folders[0]) { self.details.push(error); }
        } else if auto_open && self.output_folders.len() > 1 { self.status.push_str(" Choose an output folder below."); }
    }
    pub fn show(&mut self, ctx: &egui::Context, busy: bool) -> Option<Request> {
        if !self.open { return None; }
        let mut open = self.open; let mut request = None;
        egui::Window::new("Convert labels").open(&mut open).default_width(490.0).resizable(true).show(ctx, |ui| {
            ui.add_enabled_ui(!busy, |ui| {
                let previous = self.direction;
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(&mut self.direction, Direction::ToProject, "DYMO → 420P");
                    ui.selectable_value(&mut self.direction, Direction::ToDymo, "420P → DYMO");
                });
                if previous != self.direction { self.source = None; self.status.clear(); self.details.clear(); }
                ui.horizontal(|ui| {
                    if ui.button("Choose file(s)…").clicked() {
                        let dialog = match self.direction {
                            Direction::ToProject => rfd::FileDialog::new().add_filter("DYMO labels", &["dymo", "label"]),
                            Direction::ToDymo => rfd::FileDialog::new().add_filter("420P projects", &["420p"]),
                        };
                        if let Some(files) = dialog.pick_files() { self.source = Some(Source::Files(files)); }
                    }
                    if ui.button("Choose source folder…").clicked() {
                        if let Some(folder) = rfd::FileDialog::new().set_title("Choose the source label folder").pick_folder() { self.source = Some(Source::Folder(folder)); }
                    }
                });
                match &self.source {
                    Some(Source::Files(files)) => { ui.label(format!("{} selected file(s)", files.len())); }
                    Some(Source::Folder(folder)) => { ui.label(folder.display().to_string()); }
                    None => { ui.weak("Choose files or a source folder."); }
                }
                ui.add_enabled(matches!(self.source, Some(Source::Folder(_))), egui::Checkbox::new(&mut self.recursive, "Include subfolders"));
                ui.separator();
                ui.label("Save converted labels");
                ui.radio_value(&mut self.destination_mode, DestinationMode::BesideOriginals, "Beside each original (current directory)");
                ui.radio_value(&mut self.destination_mode, DestinationMode::ExistingFolder, "In an existing folder");
                ui.radio_value(&mut self.destination_mode, DestinationMode::NewFolder, "In a new folder");
                if self.destination_mode != DestinationMode::BesideOriginals {
                    ui.horizontal(|ui| {
                        if ui.button("Choose location…").clicked() {
                            if let Some(folder) = rfd::FileDialog::new().set_title("Choose the destination location").pick_folder() { self.output = Some(folder); }
                        }
                        if self.destination_mode == DestinationMode::NewFolder && ui.button("Use current directory").clicked() { self.output = None; }
                    });
                    if let Some(folder) = &self.output { ui.label(folder.display().to_string()); }
                    else if self.destination_mode == DestinationMode::NewFolder { ui.weak("Location: the source folder / current directory."); }
                }
                if self.destination_mode == DestinationMode::NewFolder {
                    ui.horizontal(|ui| { ui.label("Folder name"); ui.text_edit_singleline(&mut self.new_folder_name); });
                }
                ui.add_enabled(self.destination_mode != DestinationMode::BesideOriginals && matches!(self.source, Some(Source::Folder(_))), egui::Checkbox::new(&mut self.preserve_folders, "Preserve subfolder structure"));
                ui.separator();
                ui.checkbox(&mut self.recycle_originals, "Recycle originals after verified conversion");
                ui.add_enabled(self.recycle_originals && matches!(self.source, Some(Source::Folder(_))), egui::Checkbox::new(&mut self.remove_empty_folders, "Remove emptied source folders afterwards"));
                ui.checkbox(&mut self.open_when_finished, "Open output folder when finished");
                ui.weak("Existing destination files are skipped. Failed, changed or unconverted originals are kept. Folder scans skip symbolic links and junctions.");
                if self.direction == Direction::ToDymo { ui.weak("DYMO labels retain the print design. Studio guides and editing settings remain in the .420p originals."); }
                let destination = match self.destination_mode {
                    DestinationMode::BesideOriginals => Some(Destination::BesideOriginals),
                    DestinationMode::ExistingFolder => self.output.clone().map(Destination::Folder),
                    DestinationMode::NewFolder => self.output.clone().or_else(|| self.source.as_ref().and_then(source_location)).map(|parent| Destination::NewFolder { parent, name: self.new_folder_name.clone() }),
                };
                let ready = self.source.is_some() && destination.is_some();
                if ui.add_enabled(ready, egui::Button::new(if self.recycle_originals { "Convert and recycle verified originals" } else { "Convert labels" })).clicked() {
                    request = Some(Request { source: self.source.clone().unwrap(), destination: destination.unwrap(), direction: self.direction, recursive: self.recursive, preserve_folders: self.preserve_folders, recycle_originals: self.recycle_originals, remove_empty_folders: self.remove_empty_folders });
                }
            });
            if busy {
                ui.horizontal(|ui| { ui.spinner(); ui.label(&self.status); });
                if ui.button("Stop after current label").clicked() {
                    if let Some(cancel) = &self.cancel { cancel.store(true, Ordering::Release); self.status = "Stopping after the current label…".into(); }
                }
            } else if !self.status.is_empty() { ui.label(&self.status); }
            if !busy {
                for folder in &self.output_folders {
                    let label = if self.output_folders.len() == 1 { "Open output folder".to_owned() } else { format!("Open {}", folder.display()) };
                    if ui.button(label).on_hover_text(folder.display().to_string()).clicked() {
                        if let Err(error) = open_output_folder(folder) { self.details.push(error); }
                    }
                }
            }
            if !self.details.is_empty() {
                ui.collapsing("Skipped files and errors (up to 100)", |ui| {
                    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| { for detail in &self.details { ui.label(detail); } });
                });
            }
        });
        self.open = open; request
    }
}

fn open_output_folder(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    if !path.is_absolute() || !path.is_dir() { return Err("The output folder is no longer available.".into()); }
    let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if wide.starts_with(&[92,92,63,92,85,78,67,92]) { wide.splice(..8, [92,92]); }
    else if wide.starts_with(&[92,92,63,92]) { wide.drain(..4); }
    // Keep the complete directory as a single shell item, never a command line.
    let shell_path = std::ffi::OsString::from_wide(&wide);
    let file = shell_path.encode_wide().chain(Some(0)).collect::<Vec<_>>();
    let verb = [111_u16,112,101,110,0];
    #[link(name="shell32")]
    unsafe extern "system" { fn ShellExecuteW(window: *mut std::ffi::c_void, verb: *const u16, file: *const u16, parameters: *const u16, directory: *const u16, show: i32) -> isize; }
    // SAFETY: Live UTF-16 buffers; null optional arguments; directory checked above.
    let result = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1) };
    if result <= 32 { Err(format!("Windows could not open the output folder (code {result}).")) } else { Ok(()) }
}

struct Job { source: PathBuf, relative: PathBuf }
struct Plan { output: Option<PathBuf>, new_output: bool, source_root: Option<PathBuf>, jobs: Vec<Job>, warnings: Vec<String> }
impl Plan { fn warn(&mut self, message: String) { if self.warnings.len() < 100 { self.warnings.push(message.chars().take(1000).collect()); } } }

fn source_location(source: &Source) -> Option<PathBuf> {
    match source {
        Source::Folder(folder) => Some(folder.clone()),
        Source::Files(files) => {
            let first = files.first()?.parent()?;
            files.iter().all(|p| p.parent() == Some(first)).then(|| first.to_owned())
        }
    }
}
fn new_folder(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if name.is_empty() || name.trim() != name || name.ends_with('.') || name.encode_utf16().count() > 120 || name.chars().any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        || ["CON","PRN","AUX","NUL","COM1","COM2","COM3","COM4","COM5","COM6","COM7","COM8","COM9","LPT1","LPT2","LPT3","LPT4","LPT5","LPT6","LPT7","LPT8","LPT9"].contains(&stem.as_str()) {
        return Err("Choose a valid new folder name without path separators or reserved Windows names.".into());
    }
    let parent = fs::canonicalize(parent).map_err(|e| format!("Cannot open the new folder location: {e}"))?;
    if !parent.is_dir() { return Err("Choose a folder for the new location.".into()); }
    let target = parent.join(name);
    if fs::symlink_metadata(&target).is_ok() { return Err("That folder name already exists. Choose a different name or use Existing folder.".into()); }
    Ok(target)
}

fn is_link(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_type().is_symlink() || meta.file_attributes() & 0x400 != 0
}
fn add_file(jobs: &mut Vec<Job>, source: PathBuf, relative: PathBuf) -> Result<(), String> {
    if jobs.len() >= 10_000 { return Err("The folder contains more than 10,000 labels. Convert smaller folders separately.".into()); }
    jobs.push(Job { source, relative }); Ok(())
}
fn plan(request: &Request, cancel: &AtomicBool) -> Result<Plan, String> {
    let (output, new_output) = match &request.destination {
        Destination::BesideOriginals => (None, false),
        Destination::Folder(folder) => {
            let folder = fs::canonicalize(folder).map_err(|e| format!("Cannot open the destination folder: {e}"))?;
            if !folder.is_dir() { return Err("Choose a destination folder.".into()); } (Some(folder), false)
        }
        Destination::NewFolder { parent, name } => (Some(new_folder(parent, name)?), true),
    };
    let mut plan = Plan { output, new_output, source_root: None, jobs: Vec::new(), warnings: Vec::new() };
    match &request.source {
        Source::Files(files) => {
            for file in files {
                if request.direction.accepts(file) {
                    let name = file.file_name().ok_or("Invalid label file name.")?;
                    add_file(&mut plan.jobs, file.clone(), PathBuf::from(name))?;
                }
            }
        }
        Source::Folder(folder) => {
            let root_meta = fs::symlink_metadata(folder).map_err(|e| format!("Cannot open the source folder: {e}"))?;
            if is_link(&root_meta) || !root_meta.is_dir() { return Err("Choose a regular source folder rather than a symbolic link or junction.".into()); }
            let root = fs::canonicalize(folder).map_err(|e| e.to_string())?;
            plan.source_root = Some(root.clone());
            let mut pending = vec![(root.clone(), 0_usize)];
            while let Some((folder, depth)) = pending.pop() {
                if cancel.load(Ordering::Acquire) { return Err("Stopped before conversion began.".into()); }
                let entries = match fs::read_dir(&folder) { Ok(entries) => entries, Err(e) => { plan.warn(format!("{}: {e}", folder.display())); continue; } };
                for entry in entries {
                    if cancel.load(Ordering::Acquire) { return Err("Stopped before conversion began.".into()); }
                    let entry = match entry { Ok(e) => e, Err(e) => { plan.warn(e.to_string()); continue; } };
                    let path = entry.path();
                    let meta = match fs::symlink_metadata(&path) { Ok(meta) => meta, Err(e) => { plan.warn(format!("{}: {e}", path.display())); continue; } };
                    if is_link(&meta) { continue; }
                    if meta.is_dir() && request.recursive && !plan.output.as_ref().is_some_and(|output| path.to_string_lossy().eq_ignore_ascii_case(&output.to_string_lossy())) {
                        if depth >= 128 { return Err("The source folder is nested too deeply. Choose a smaller source folder.".into()); }
                        pending.push((path, depth + 1));
                    } else if meta.is_file() && request.direction.accepts(&path) {
                        let relative = path.strip_prefix(&root).map_err(|_| "A label is outside the source folder.")?.to_owned();
                        add_file(&mut plan.jobs, path, relative)?;
                    }
                }
            }
        }
    }
    plan.jobs.sort_by_key(|job| job.relative.to_string_lossy().to_lowercase());
    Ok(plan)
}

fn prepare_target(root: &Path, relative: &Path, extension: &str) -> Result<PathBuf, String> {
    let relative = relative.with_extension(extension);
    let mut parent = root.to_owned();
    if let Some(folders) = relative.parent() {
        for component in folders.components() {
            let std::path::Component::Normal(name) = component else { return Err("Invalid destination subfolder.".into()); };
            parent.push(name);
            match fs::create_dir(&parent) { Ok(()) => {}, Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}, Err(e) => return Err(format!("Cannot create a destination subfolder: {e}")) }
            let meta = fs::symlink_metadata(&parent).map_err(|e| e.to_string())?;
            if !meta.is_dir() || is_link(&meta) { return Err("A destination subfolder is a link or is not a regular folder.".into()); }
            if !fs::canonicalize(&parent).map_err(|e| e.to_string())?.starts_with(root) { return Err("A destination subfolder is outside the chosen folder.".into()); }
        }
    }
    Ok(parent.join(relative.file_name().ok_or("Invalid destination file name.")?))
}

pub fn run(request: Request, cancel: &AtomicBool, notify: impl FnMut(Event), convert: impl FnMut(&Path, &Path, Direction) -> Result<(), String>) {
    run_with_cleanup(request, cancel, notify, convert, crate::recycle::item)
}

fn original_bytes(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut bytes = Vec::new(); fs::File::open(path).map_err(|e| e.to_string())?.take(8_388_609).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.is_empty() || bytes.len() > 8_388_608 { return Err("The original is empty or exceeds the supported size; it will be kept.".into()); }
    Ok(bytes)
}

fn run_with_cleanup(request: Request, cancel: &AtomicBool, mut notify: impl FnMut(Event), mut convert: impl FnMut(&Path, &Path, Direction) -> Result<(), String>, mut recycle: impl FnMut(&Path, Option<&[u8]>) -> Result<(), String>) {
    let mut summary = Summary::default();
    let plan = match plan(&request, cancel) {
        Ok(plan) => plan,
        Err(error) => { summary.cancelled = cancel.load(Ordering::Acquire); summary.detail(error); notify(Event::Finished(summary)); return; }
    };
    summary.total = plan.jobs.len();
    for warning in plan.warnings { summary.detail(warning); }
    if cancel.load(Ordering::Acquire) { summary.cancelled = true; notify(Event::Finished(summary)); return; }
    if plan.new_output && summary.total > 0 {
        if let Err(error) = fs::create_dir(plan.output.as_ref().unwrap()) { summary.detail(format!("Cannot create the new destination folder: {error}")); summary.total = 0; notify(Event::Finished(summary)); return; }
    }
    notify(Event::Progress { processed: 0, total: summary.total });
    let mut emptied_candidates = std::collections::BTreeSet::new();
    for job in plan.jobs {
        if cancel.load(Ordering::Acquire) { summary.cancelled = true; break; }
        let result = (|| {
            let meta = fs::symlink_metadata(&job.source).map_err(|e| format!("Cannot read source: {e}"))?;
            if is_link(&meta) || !meta.is_file() { return Err("Source is not a regular label file.".into()); }
            let (root, relative) = match &plan.output {
                Some(root) => (root.clone(), if request.preserve_folders { job.relative.clone() } else { PathBuf::from(job.relative.file_name().ok_or("Invalid label name.")?) }),
                None => (fs::canonicalize(job.source.parent().ok_or("Source folder missing.")?).map_err(|e| e.to_string())?, PathBuf::from(job.source.file_name().ok_or("Invalid label name.")?)),
            };
            let target = prepare_target(&root, &relative, request.direction.extension())?;
            if fs::symlink_metadata(&target).is_ok() { return Err("Destination already exists; kept unchanged.".into()); }
            let original = if request.recycle_originals { Some(original_bytes(&job.source)?) } else { None };
            convert(&job.source, &target, request.direction)?;
            let target_meta = fs::symlink_metadata(&target).map_err(|e| format!("Converted file could not be verified: {e}"))?;
            if is_link(&target_meta) || !target_meta.is_file() || target_meta.len() == 0 { return Err("Converted file is missing, empty or a link; original kept.".into()); }
            let display_root = plan.output.as_ref().or(plan.source_root.as_ref()).unwrap_or(&root);
            if !summary.output_folders.contains(display_root) { summary.output_folders.push(display_root.clone()); }
            if let Some(original) = original {
                let removal = (|| {
                    if original_bytes(&job.source)? != original { return Err("Original changed during conversion; kept unchanged.".into()); }
                    let canonical = fs::canonicalize(&job.source).map_err(|e| e.to_string())?;
                    recycle(&canonical, Some(&original))?;
                    summary.recycled += 1;
                    if let Some(source_root) = &plan.source_root {
                        let mut parent = canonical.parent();
                        while let Some(folder) = parent.filter(|folder| folder.starts_with(source_root)) {
                            emptied_candidates.insert(folder.to_owned()); if folder == source_root { break; } parent = folder.parent();
                        }
                    }
                    Ok::<_, String>(())
                })();
                if let Err(error) = removal { summary.detail(format!("{}: converted; original removal not confirmed: {error}", job.relative.display())); }
            }
            Ok::<_, String>(())
        })();
        match result { Ok(()) => summary.converted += 1, Err(error) => { summary.skipped += 1; summary.detail(format!("{}: {error}", job.relative.display())); } }
        summary.processed += 1;
        notify(Event::Progress { processed: summary.processed, total: summary.total });
    }
    if request.recycle_originals && request.remove_empty_folders && !summary.cancelled {
        let mut folders = emptied_candidates.into_iter().collect::<Vec<_>>(); folders.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
        for folder in folders {
            if cancel.load(Ordering::Acquire) { summary.cancelled = true; break; }
            let empty = fs::read_dir(&folder).is_ok_and(|mut entries| entries.next().is_none());
            if empty {
                match recycle(&folder, None) { Ok(()) => summary.folders_removed += 1, Err(error) => summary.detail(format!("{}: empty folder kept: {error}", folder.display())) }
            }
        }
    }
    if summary.total == 0 && summary.details.is_empty() { summary.detail("No matching label files were found for the chosen direction.".into()); }
    notify(Event::Finished(summary));
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temporary() -> PathBuf {
        let path = std::env::temp_dir().join(format!("420p-folder-converter-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&path).unwrap(); fs::canonicalize(path).unwrap()
    }
    fn cleanup(path: PathBuf) {
        assert_eq!(path.parent().unwrap(), fs::canonicalize(std::env::temp_dir()).unwrap());
        assert!(path.file_name().unwrap().to_string_lossy().starts_with("420p-folder-converter-")); fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn recursive_folder_conversion_preserves_structure_and_excludes_its_output() {
        let root = temporary(); let nested = root.join("nested"); let output = root.join("converted"); fs::create_dir(&nested).unwrap(); fs::create_dir(&output).unwrap();
        fs::write(root.join("one.label.DYMO"), b"first").unwrap(); fs::write(nested.join("two.dymo"), b"second").unwrap(); fs::write(output.join("excluded.dymo"), b"excluded").unwrap(); fs::write(root.join("other.420p"), b"other format").unwrap();
        let request = Request::folder(root.clone(), output.clone(), Direction::ToProject, true);
        let mut summary = None;
        run(request, &AtomicBool::new(false), |event| if let Event::Finished(value) = event { summary = Some(value); }, |source, target, _| crate::project_io::write_project(target, &fs::read(source).unwrap(), false));
        let summary = summary.unwrap(); assert_eq!(summary.converted, 2); assert_eq!(summary.skipped, 0);
        assert_eq!(fs::read(output.join("one.label.420p")).unwrap(), b"first"); assert_eq!(fs::read(output.join("nested/two.420p")).unwrap(), b"second"); assert!(!output.join("excluded.420p").exists());
        assert_eq!(fs::read(nested.join("two.dymo")).unwrap(), b"second"); cleanup(root);
    }
    #[test]
    fn reverse_conversion_skips_existing_files_continues_errors_and_can_cancel() {
        let root = temporary(); let output = root.join("output"); fs::create_dir(&output).unwrap();
        for name in ["a.420p", "b.420p", "c.420p"] { fs::write(root.join(name), name).unwrap(); }
        fs::write(output.join("a.dymo"), b"must stay").unwrap();
        let request = Request::folder(root.clone(), output.clone(), Direction::ToDymo, false);
        let cancel = AtomicBool::new(false); let mut summary = None;
        run(request, &cancel, |event| if let Event::Finished(value) = event { summary = Some(value); }, |source, _, direction| {
            assert_eq!(source.file_name().unwrap(), "b.420p"); assert!(direction == Direction::ToDymo); cancel.store(true, Ordering::Release); Err("Synthetic invalid label".into())
        });
        let summary = summary.unwrap(); assert_eq!(summary.skipped, 2); assert_eq!(summary.processed, 2); assert_eq!(summary.total, 3); assert!(summary.cancelled);
        assert_eq!(fs::read(output.join("a.dymo")).unwrap(), b"must stay"); assert!(!output.join("c.dymo").exists()); cleanup(root);
    }
    #[test]
    fn nonrecursive_conversion_and_output_subfolder_validation() {
        let root = temporary(); let nested = root.join("nested"); fs::create_dir(&nested).unwrap(); fs::write(root.join("one.dymo"), b"one").unwrap(); fs::write(nested.join("two.dymo"), b"two").unwrap();
        let request = Request::folder(root.clone(), root.clone(), Direction::ToProject, false);
        let plan = plan(&request, &AtomicBool::new(false)).unwrap(); assert_eq!(plan.jobs.len(), 1);
        assert!(prepare_target(&root, Path::new("../escape.dymo"), "420p").is_err());
        fs::write(root.join("not-a-folder"), b"keep").unwrap(); assert!(prepare_target(&root, Path::new("not-a-folder/label.dymo"), "420p").is_err());
        assert_eq!(fs::read(root.join("not-a-folder")).unwrap(), b"keep"); cleanup(root);
    }
    fn copy_run(request: Request) -> Summary {
        let mut summary = None;
        run(request, &AtomicBool::new(false), |event| if let Event::Finished(value) = event { summary = Some(value); }, |source, target, _| crate::project_io::write_project(target, &fs::read(source).unwrap(), false));
        summary.unwrap()
    }
    #[test]
    fn folder_opening_is_optional_and_requires_completed_output() {
        let folder = PathBuf::from("C:\\synthetic output");
        let make = |cancelled| Summary { converted: 1, total: 1, processed: 1, cancelled, output_folders: vec![folder.clone()], ..Default::default() };
        let mut converter = Converter::default();
        converter.finish_with(make(false), |_| panic!("default must not launch Explorer"));
        converter.open_when_finished = true;
        converter.finish_with(make(true), |_| panic!("cancelled batches must not auto-open"));
        converter.finish_with(Summary::default(), |_| panic!("failed or empty batches must not auto-open"));
        let mut opened = Vec::new(); converter.finish_with(make(false), |path| { opened.push(path.to_owned()); Ok(()) }); assert_eq!(opened, vec![folder.clone()]);
        converter.finish_with(make(false), |_| Err("Synthetic Explorer failure".into())); assert!(converter.details.iter().any(|error| error.contains("Explorer failure")));
    }
    #[test]
    fn multiple_destinations_are_offered_without_launching_many_windows() {
        let mut converter = Converter::default(); converter.open_when_finished = true;
        let folders = vec![PathBuf::from("C:\\one"), PathBuf::from("C:\\two")];
        converter.finish_with(Summary { converted: 2, processed: 2, total: 2, output_folders: folders.clone(), ..Default::default() }, |_| panic!("do not launch multiple Explorer windows automatically"));
        assert_eq!(converter.output_folders, folders); assert!(converter.status.contains("Choose an output folder"));
    }
    #[test]
    fn individual_multiple_and_named_folder_destinations_keep_originals() {
        let root = temporary(); let first = root.join("first.dymo"); let second = root.join("second.dymo"); fs::write(&first, b"first").unwrap(); fs::write(&second, b"second").unwrap();
        let mut request = Request { source: Source::Files(vec![first.clone()]), destination: Destination::BesideOriginals, direction: Direction::ToProject, recursive: false, preserve_folders: true, recycle_originals: false, remove_empty_folders: false };
        assert_eq!(copy_run(request.clone()).converted, 1); assert_eq!(fs::read(root.join("first.420p")).unwrap(), b"first");
        request.source = Source::Files(vec![first.clone(), second.clone()]); request.destination = Destination::NewFolder { parent: root.clone(), name: "My converted labels".into() };
        let result = copy_run(request.clone()); assert_eq!(result.converted, 2); assert_eq!(result.recycled, 0);
        assert_eq!(fs::read(root.join("My converted labels/second.420p")).unwrap(), b"second"); assert_eq!(fs::read(&first).unwrap(), b"first"); assert_eq!(fs::read(&second).unwrap(), b"second");
        assert_eq!(copy_run(request).converted, 0, "a named new folder must not silently reuse an existing folder");
        for invalid in ["..", ".", "CON", "bad/name", "trailing.", " leading"] { assert!(new_folder(&root, invalid).is_err()); }
        cleanup(root);
    }
    #[test]
    fn flattened_folders_skip_collisions_and_cancel_does_not_create_output() {
        let root = temporary(); let source = root.join("source"); let output = root.join("output"); fs::create_dir(&source).unwrap(); fs::create_dir(&output).unwrap();
        for name in ["a", "b"] { fs::create_dir(source.join(name)).unwrap(); fs::write(source.join(name).join("same.dymo"), name).unwrap(); }
        let mut request = Request::folder(source.clone(), output.clone(), Direction::ToProject, true); request.preserve_folders = false;
        let summary = copy_run(request.clone()); assert_eq!(summary.converted, 1); assert_eq!(summary.skipped, 1); assert_eq!(fs::read(output.join("same.420p")).unwrap(), b"a"); assert!(!output.join("a").exists());
        request.destination = Destination::NewFolder { parent: root.clone(), name: "Cancelled output".into() };
        run(request, &AtomicBool::new(true), |_| {}, |_, _, _| panic!("cancelled work must not convert")); assert!(!root.join("Cancelled output").exists()); cleanup(root);
    }
    #[test]
    fn removal_keeps_failed_files_unrelated_content_and_only_cleans_empty_source_folders() {
        let root = temporary(); let source = root.join("source"); let output = root.join("output"); let recovery = root.join("simulated-recycle");
        for folder in [&source, &output, &recovery] { fs::create_dir(folder).unwrap(); } fs::create_dir(source.join("nested")).unwrap();
        fs::write(source.join("good.dymo"), b"good").unwrap(); fs::write(source.join("nested/second.dymo"), b"second").unwrap(); fs::write(source.join("bad.dymo"), b"bad").unwrap(); fs::write(source.join("notes.txt"), b"unrelated").unwrap();
        let mut request = Request::folder(source.clone(), output, Direction::ToProject, true); request.recycle_originals = true; request.remove_empty_folders = true;
        let mut summary = None; let mut recycled = 0;
        run_with_cleanup(request, &AtomicBool::new(false), |event| if let Event::Finished(value) = event { summary = Some(value); }, |source, target, _| {
            if source.file_name().unwrap() == "bad.dymo" { return Err("Synthetic invalid file".into()); } crate::project_io::write_project(target, &fs::read(source).unwrap(), false)
        }, |path, expected| {
            if let Some(expected) = expected { assert_eq!(fs::read(path).unwrap(), expected); recycled += 1; fs::rename(path, recovery.join(format!("{recycled}.original"))).unwrap(); }
            else { assert!(path.starts_with(&source)); assert!(fs::read_dir(path).unwrap().next().is_none()); fs::remove_dir(path).unwrap(); } Ok(())
        });
        let summary = summary.unwrap(); assert_eq!(summary.converted, 2); assert_eq!(summary.skipped, 1); assert_eq!(summary.recycled, 2); assert_eq!(summary.folders_removed, 1);
        assert_eq!(fs::read(source.join("bad.dymo")).unwrap(), b"bad"); assert_eq!(fs::read(source.join("notes.txt")).unwrap(), b"unrelated"); assert!(!source.join("nested").exists()); cleanup(root);
    }
    #[test]
    fn source_changes_during_conversion_prevent_removal() {
        let root = temporary(); let source = root.join("original.dymo"); fs::write(&source, b"original").unwrap();
        let request = Request { source: Source::Files(vec![source.clone()]), destination: Destination::BesideOriginals, direction: Direction::ToProject, recursive: false, preserve_folders: true, recycle_originals: true, remove_empty_folders: false };
        let mut summary = None;
        run_with_cleanup(request, &AtomicBool::new(false), |event| if let Event::Finished(value) = event { summary = Some(value); }, |source, target, _| { crate::project_io::write_project(target, b"converted", false)?; fs::write(source, b"edited while converting").unwrap(); Ok(()) }, |_, _| panic!("changed source must never be recycled"));
        let summary = summary.unwrap(); assert_eq!(summary.converted, 1); assert_eq!(summary.recycled, 0); assert!(summary.details.iter().any(|s| s.contains("changed"))); assert_eq!(fs::read(source).unwrap(), b"edited while converting"); cleanup(root);
    }
}
