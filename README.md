---
lang: en-GB
---

# 420P Label Studio

A Windows desktop label editor dedicated to the DYMO LabelManager 420P. The interface is written in Rust and egui; a local .NET bridge uses the DYMO SDK for previews and printing. This is an independent project.

## Features

- Dark editor with text formatting, image thresholding, metric/imperial sizes and millimetre rulers.
- Smooth design and printer-resolution SDK previews.
- Object selection, dragging, guides, alignment and manual positions retained across object changes.
- Shapes, lines, addresses, barcodes, QR codes, captured dates and counters.
- Open DYMO Connect `.dymo` tape labels, local `.420p` projects, checked `.dymo` export, contacts and CSV/TSV batches.
- Connection and battery status, notification-area restoration, and verified GitHub update downloads and installation.

Print jobs use the same frozen template, image and captured object values as the verified preview. Batches validate all labels before submission and stop on failure. An accepted job is not confirmation of a physical print.

## Run the Windows release

Extract the complete release ZIP to a folder and run `label-studio-420p.exe`. Keep the `sdk` directory beside it. The app requires 64-bit Windows 10 or later, the x64 .NET 10 Desktop Runtime, and the DYMO 420P Windows printer driver. Printing was physically tested with DYMO Connect services stopped; uninstalling DYMO Connect and restoring the driver has not yet been tested. Read `COPYRIGHT.txt` and the bundled third-party terms before using or redistributing the release; agreement to the included Microsoft component terms is required. The release is unsigned, so Windows may show an unknown-publisher prompt. Existing project files and local preferences are stored separately from the application folder.

## Build and test

Optional Windows file-type registration is provided by `scripts/register-file-type.ps1`, with the installed executable supplied through `-ApplicationPath`. It registers `.420p` for the current user, installs a separate document icon in local application data and keeps one local registration rollback record. It checks Windows' resolved open command and leaves `.dymo` registrations and protected Windows defaults untouched. The app installation path must remain valid; rerun registration after moving the app. This source utility is separate from the application update package.

Use Windows with Rust 1.95 or later, the .NET 10 SDK, and Visual Studio's Desktop development with C++ workload and Windows SDK. The 420P Windows printer driver is required for hardware printing.

In an x64 Visual Studio developer PowerShell, run:

```powershell
pwsh -File scripts/build.ps1
```

The script restores the pinned dependencies, builds the bridge, supplies synthetic fixtures to the SDK integration tests, runs the release tests and builds `dist/DYMO420P`. It does not print, install or upload anything. `-SkipTests` is available for local iteration; use the default test run before distribution. Restore requires access to the Rust and NuGet package registries.

`-PublicRelease` requires the full tests, remaps local compiler paths, disables debug symbols and stages the x64 runtime files in `dist/DYMO420P-public`. Its `public-bridge` test output and public distribution staging folders must be empty before the build. Binary publication also requires the applicable transitive dependency and embedded-font licence texts/notices, and an audit of the exact archive and metadata. The build script does not publish a release.

The `sdk` directory must remain beside `label-studio-420p.exe`. Local preferences, contacts, projects, diagnostics, package caches and build outputs are not repository content.

## Existing DYMO labels and conversion

Use **File → Open** or **Open saved label** for DYMO Connect `.dymo` labels and `.420p` projects. Dragging a label onto the window and Windows **Open with** file arguments use the same opening path. Opening a native label preserves its embedded images, formatting and layout; its first text object can be edited. Original files are not changed. LabelWriter formats and other unsupported tape formats produce a clear error.

**File → Open recent** lists up to ten labels opened or saved in the app. The list is stored locally and can be cleared from the menu. Missing files are reported without discarding the current label.

Use **File → Convert labels…** for **DYMO → 420P** or **420P → DYMO** conversion. Choose one file, multiple files or a source folder, optionally including subfolders. Output can go beside the originals, into an existing folder or into a new folder with a chosen name and location. New folders can also use the source/current directory as their location. Folder output can preserve the subfolder structure or place converted labels together; name collisions are skipped without overwriting. Folder scans exclude their destination when it is inside the source and skip symbolic links and junctions. Each run is limited to 10,000 matching labels; progress, cancellation and skipped-file details are shown locally.

Originals are kept by default. **Recycle originals after verified conversion** is optional and changes the action button to **Convert and recycle verified originals**. Each output is read back and reopened before an original becomes eligible; changed originals, failed conversions and skipped existing outputs are kept. Optional source-folder removal only considers emptied folders associated with successfully recycled originals and never recursively removes a folder. Windows recycling is restricted to local fixed disks, explicitly requests the Recycle Bin, refuses callbacks without recycling flags and checks the shell's result. It does not fall back to a direct delete. See [Microsoft's file-operation flags](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperation-setoperationflags) and [pre-delete callbacks](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperationprogresssink-predeleteitem). Recycled items can be recovered through the Windows Recycle Bin.

Converted `.420p` projects embed the original DYMO document, so they do not depend on the source file remaining in its original folder. These embedded projects require version 0.1.6 or later; earlier project files remain supported. Reverse conversion uses the same checked SDK export as **Save as DYMO**. Other `.420p` projects still need their referenced template and image files to be accessible. Tape width is retained rather than silently resizing or discarding imported objects. Existing restrictions on dynamic dates and counters still apply before printing.

**Save as…** offers **420P project (*.420p)** and **DYMO label (*.dymo)** in the file type list. Projects retain Studio editing settings and guides. DYMO labels embed images and resolved layout for opening in DYMO Connect; Studio-only settings are not stored in that format. Subsequent **Save / Ctrl+S** updates the chosen format. Saving a DYMO label works without a printer or an already-generated preview: the SDK validates the saved content and layout before writing it. Existing files are replaced only after the save dialogue's confirmation, and externally changed files are protected. The separate **Export DYMO label…** command saves a copy without changing the current document's save destination.

## Updates

The converter offers an opt-in **Open output folder when finished** checkbox and an **Open output folder** button after conversion. Automatic opening requires completed output and does not run for cancelled or empty batches. A shared destination opens once; when files are saved beside originals in different directories, the converter offers individual folder buttons rather than launching many Explorer windows.

Settings and Help check this project's public GitHub repository for the latest stable app release. Fresh installations default to `cantwalkwontrun/420p-label-studio`. Saved settings using the previous `noswimmingplease/420p-label-studio`, `noswimmingplease/dymo-420p-rust` or `cantwalkwontrun/dymo-420p-rust` addresses migrate to this address; custom repositories and disabled updates are preserved.

The app also checks once in the background when it opens. A dismissible notice appears only when a newer stable version is available; **View update** opens App updates. Connection failures and an up-to-date result do not interrupt editing. Disabled updates and unreadable preferences skip the automatic check. Startup checks do not rewrite saved settings, discard verified downloads, download packages or install anything automatically. Updates already downloaded remain ready to install.

For an official release, choose **Download update**, then **Restart and install**. The app checks the archive against GitHub's SHA-256 asset digest and the release checksum file, extracts only managed application files, checks the executable's x64 format and version, and verifies the staged files again before restart. Downloads can be cancelled and completed downloads remain available after reopening the app. Custom repositories retain release checks and download links; automatic installation is restricted to this project's official repository.

Installation requires a writable application folder. It waits for printer operations and exports to finish, checks unsaved labels and contacts, and waits for the old process to exit. Other windows in the same Windows session must be closed, including tray windows. A separate helper replaces the managed files, preserving unrelated files and the separate project, contact and preference stores. It retains one local rollback set and restores the previous files if installation fails or the new app does not acknowledge its first window. An interrupted transaction is recovered on the next launch; conflicting externally modified files are preserved for manual recovery. Cleanup after a verified restart rotates the previous rollback. Temporary updates and recovery files stay under the application's `.420p-updater` directory and are never uploaded.

Release binaries remain unsigned. Checksums detect corruption but do not protect against a compromised release publisher. The updater does not install drivers, prerequisites or firmware, request administrator privileges, or silently install an update while the editor is in use.

Versions 0.1.1 and 0.1.2 reject repository redirects; change their update repository in Settings to the new address or upgrade manually. Version 0.1.3 has release checks but no installer. Install version 0.1.4 or later once from [420P Label Studio releases](https://github.com/cantwalkwontrun/420p-label-studio/releases) to enable future in-app installation.

## Limitations

The 420P supports 6, 9, 12 and 19 mm D1 tape and has a manual cutter. Enlarging a printer-resolution preview exposes its dots. The smooth view is a design aid; barcodes can fall back to an approximate view and require checking the regular SDK preview. Rulers describe the SDK canvas, not the physical cut length.

Geometry editing, smooth rendering and resolved export depend on guarded private SDK members. Export is accepted only after a saved/reloaded label reproduces the resolved XML and printer-resolution bitmap. Unsupported SDK changes fail safely. Gross template-width mismatches are rejected when geometry is available.

The app reads the 420P's battery percentage, charging state, detected tape width and reported printer errors through DYMO's legacy status interface. Refresh reads these details again; background checks run every 15 seconds and pause during print jobs. Status reads and print submission are serialised. Missing or invalid details appear as unavailable. Detected tape width is advisory and does not change the label's selected width. A live connected/idle battery reply has been checked; active charging and error combinations have synthetic test coverage and still need broader physical validation.

Imported live date/time objects and embedded counters must be recreated through the app to keep previews and printing consistent. Static template dates remain supported. Removing a replacement image from an imported template restores any image embedded in the original template.

The basic 9 mm text/image workflow has physical validation. Wider tapes, advanced objects and batches still need broader physical verification. Automated tests and code review do not establish a bug-free application or full DYMO Connect parity. Mirror, collate, chain marks and graphics-quality controls remain pending device-specific preview/print validation. Firmware and hidden-setting modification are not supported.

## Dependencies

See `THIRD-PARTY-NOTICES.txt`. Vendor applications, SDK binaries and reference executables are not included in this source repository.
