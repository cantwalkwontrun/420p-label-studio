---
lang: en-GB
---

# 420P Label Studio

A Windows desktop label editor dedicated to the DYMO LabelManager 420P. The interface is written in Rust and egui; a local .NET bridge uses the DYMO SDK for previews and printing. This is an independent project.

## Features

- Light, Dark, System and FireFly Pro appearances with text formatting, image thresholding, metric/imperial sizes and rulers. Choose an appearance in **File → Settings**. Dark is the default for existing installations. System follows Windows. Appearance preferences are local to the user and apply to the editor chrome.
- Smooth design and printer-resolution SDK previews.
- Object selection, dragging, guides, alignment and manual positions retained across object changes.
- Shapes, lines, addresses, barcodes, QR codes, captured dates and counters.
- Open DYMO Connect `.dymo` tape labels, local `.420p` projects, checked `.dymo` export, contacts and CSV/TSV batches.
- Connection and battery status, notification-area restoration, and verified GitHub update downloads and installation.

Print jobs use the same frozen template, image and captured object values as the verified preview. A fresh main preview clears the checked batch row, including after artwork changes at the same path. Batches validate all labels before submission and stop on failure. An accepted job is not confirmation of a physical print.

## Run the Windows release

Extract the complete release ZIP to a folder and run `label-studio-420p.exe`. Keep the `sdk` directory beside it. The app requires 64-bit Windows 10 or later, the x64 .NET 10 Desktop Runtime, and the DYMO 420P Windows printer driver. Printing was physically tested with DYMO Connect services stopped; uninstalling DYMO Connect and restoring the driver has not yet been tested. Read `COPYRIGHT.txt` and the bundled third-party terms before using or redistributing the release; agreement to the included Microsoft component terms is required. The release is unsigned, so Windows may show an unknown-publisher prompt. Existing project files and local preferences are stored separately from the application folder.

Use **File → Settings → Register .420p files with Windows** to register the extracted application for the current user. Windows Explorer uses the label document icon embedded in the executable. If another app is the protected default, Studio appears in **Open with** and Windows lets you choose the default. Registration is scoped to `.420p`. Run it again after moving the application. The same operation is available to source users through `scripts/register-file-type.ps1 -ApplicationPath <exe>`.

## Build and test

Use Windows with Rust 1.95 or later, the .NET 10 SDK, and Visual Studio's Desktop development with C++ workload and Windows SDK. The 420P Windows printer driver is required for hardware printing.

In an x64 Visual Studio developer PowerShell, run:

```powershell
pwsh -File scripts/build.ps1
```

The script restores the pinned dependencies, builds the bridge, supplies synthetic fixtures to the SDK integration tests, runs the release tests and builds `dist/DYMO420P`. It does not print, install or upload anything. `-SkipTests` is available for local iteration; use the default test run before distribution. Restore requires access to the Rust and NuGet package registries.

`-PublicRelease` requires the full tests, remaps local compiler paths, disables debug symbols and stages the x64 runtime files in `dist/DYMO420P-public`. Its `public-bridge` test output and public distribution staging folders must be empty before the build. Binary publication also requires the applicable transitive dependency and embedded-font licence texts/notices, and an audit of the exact archive and metadata. The build script does not publish a release.

The `sdk` directory must remain beside `label-studio-420p.exe`. Local preferences, contacts, projects, diagnostics, package caches and build outputs are not repository content.

Release maintainers can independently scan synthetic QR labels using Python, NumPy, OpenCV and the zxing-cpp Python package. Run `python scripts/check-release-qr.py <extracted-application-folder>`. It verifies the payload in the printer-resolution and smooth previews before and after a DYMO export. The printer is not used. The runtime SDK and printer interfaces are documented in [DYMO's official SDK sample](https://github.com/dymosoftware/DCD-SDK-Sample).

## Existing DYMO labels and conversion

Use **File → Open** or **Open saved label** for DYMO Connect `.dymo` labels and `.420p` projects. Dragging a label onto the window and Windows **Open with** file arguments use the same opening path. Opening a native label preserves its embedded images, formatting and layout; its first text object can be edited. Original files are not changed. LabelWriter formats and other unsupported tape formats produce a clear error.

**File → Open recent** lists up to ten labels opened or saved in the app. The list is stored locally and can be cleared from the menu. Missing files are reported without discarding the current label.

Use **File → Convert labels…** for **DYMO → 420P** or **420P → DYMO** conversion. Choose one file, multiple files or a source folder, optionally including subfolders. Output can go beside the originals, into an existing folder or into a new folder with a chosen name and location. New folders can also use the source/current directory as their location. Folder output can preserve the subfolder structure or place converted labels together; name collisions are skipped without overwriting. After a failed conversion, retrying in the same dialogue can reuse the folder Studio created. Existing outputs are skipped. Folder scans exclude their destination when it is inside the source and skip symbolic links and junctions. Each run is limited to 10,000 matching labels; progress, cancellation and skipped-file details are shown locally.

Originals are kept by default. **Recycle originals after verified conversion** is optional and changes the action button to **Convert and recycle verified originals**. Each output is read back and reopened before an original becomes eligible; changed originals, failed conversions and skipped existing outputs are kept. Optional source-folder removal only considers emptied folders associated with successfully recycled originals and never recursively removes a folder. Windows recycling is restricted to local fixed disks, explicitly requests the Recycle Bin, refuses callbacks without recycling flags and checks the shell's result. It does not fall back to a direct delete. See [Microsoft's file-operation flags](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperation-setoperationflags) and [pre-delete callbacks](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperationprogresssink-predeleteitem). Recycled items can be recovered through the Windows Recycle Bin.

Converted `.420p` projects embed the original DYMO document, so they can reopen after the source file moves. These embedded projects require version 0.1.6 or later. Other Studio projects still need access to their referenced template and image files. Conversion preserves the tape width. Studio checks that it can reopen and preview each staged output before completing conversion or recycling an original. Save active counter sequences as `.420p` to preserve their behaviour. Studio reports these files as skipped or failed during DYMO export and keeps the originals.

**QR labels made in v0.1.10.** A QR code added in Studio could encode `123456789` instead of the text you entered. Open the original `.420p` project in v0.1.11 or later, scan the corrected preview and reprint affected labels. If you only have an exported `.dymo` file containing the wrong value, recreate the QR object with the intended text.

**Checking QR codes before printing.** Studio decodes each QR code in the printer-resolution preview and compares the result with its intended text. If a code fails, enlarge it, shorten its contents or choose wider tape. Scan a physical sample before printing a batch.

**Save as…** offers **420P project (*.420p)** and **DYMO label (*.dymo)** in the file type list. Projects retain Studio editing settings and guides. DYMO labels embed images and resolved layout for opening in DYMO Connect; Studio-only settings are not stored in that format. Subsequent **Save / Ctrl+S** updates the chosen format. Saving a DYMO label works without a printer or an already-generated preview: the SDK validates the saved content and layout before writing it. Existing files are replaced only after the save dialogue's confirmation, and externally changed files are protected. The separate **Export DYMO label…** command saves a copy without changing the current document's save destination.

## Updates

The converter offers an opt-in **Open output folder when finished** checkbox and an **Open output folder** button after conversion. Automatic opening requires completed output and does not run for cancelled or empty batches. A shared destination opens once; when files are saved beside originals in different directories, the converter offers individual folder buttons rather than launching many Explorer windows.

Settings and Help check this project's public GitHub repository for the latest stable app release. Fresh installations default to `cantwalkwontrun/420p-label-studio`. Saved settings using the previous `noswimmingplease/420p-label-studio`, `noswimmingplease/dymo-420p-rust` or `cantwalkwontrun/dymo-420p-rust` addresses migrate to this address; custom repositories and disabled updates are preserved.

The app also checks once in the background when it opens. A dismissible notice appears only when a newer stable version is available; **View update** opens App updates. Connection failures and an up-to-date result do not interrupt editing. Disabled updates and unreadable preferences skip the automatic check. Startup checks do not rewrite saved settings, discard verified downloads, download packages or install anything automatically. Updates already downloaded remain ready to install.

For an official release, choose **Download update**, then **Restart and install**. The app checks the archive against GitHub's SHA-256 asset digest and the release checksum file, extracts only managed application files, checks the executable's x64 format and version, and verifies the staged files again before restart. Downloads can be cancelled and completed downloads remain available after reopening the app. Custom repositories retain release checks and download links; automatic installation is restricted to this project's official repository.

Installation requires a writable application folder. It waits for printer operations and exports to finish, checks unsaved labels and contacts, and waits for the old process to exit. Close other Studio windows using the same installation, including windows in the notification area, before restarting to install. A separate helper replaces the managed files, preserving unrelated files and the separate project, contact and preference stores. It retains one local rollback set and restores the previous files if installation fails or the new app does not acknowledge its first window. An interrupted transaction is recovered on the next launch; if a file has changed independently, Studio preserves it and offers the recovery folder and an explicit retry. Cleanup after a verified restart rotates the previous rollback. Temporary updates and recovery files stay under the application's `.420p-updater` directory and are never uploaded.

Release binaries remain unsigned. Checksums detect corruption but do not protect against a compromised release publisher. The updater does not install drivers, prerequisites or firmware, request administrator privileges, or silently install an update while the editor is in use.

For versions 0.1.1 through 0.1.5, install a current release manually once. Versions 0.1.4 and 0.1.5 recognise the previous repository owner for automatic installation and cannot follow the owner's redirect. Version 0.1.10 and later recognise the current owner and support subsequent in-app installations. [Download 420P Label Studio](https://github.com/cantwalkwontrun/420p-label-studio/releases)

## Limitations

The 420P supports 6, 9, 12 and 19 mm D1 tape and has a manual cutter. When the SDK cannot produce a high-resolution design for an object such as a barcode, the editor can show the complete printer-resolution preview. Enlarging it exposes the printer dots. Rulers measure the SDK canvas. The physical cut length can differ.

Geometry editing, smooth rendering and resolved export depend on guarded private SDK members. Export is accepted only after a saved/reloaded label reproduces the resolved XML and printer-resolution bitmap. Unsupported SDK changes fail safely. Gross template-width mismatches are rejected when geometry is available.

The app reads the 420P's battery percentage, charging state, detected tape width and reported printer errors through DYMO's legacy status interface. Refresh reads these details again; background checks run every 15 seconds and pause during print jobs. Status reads and print submission are serialised. Missing or invalid details appear as unavailable. A reported cassette, power or printer fault blocks printing. An unavailable status reading and a detected tape-width mismatch are advisory. A live connected/idle battery reply has been checked; active charging and error combinations have synthetic test coverage and still need broader physical validation.

Imported live date/time objects and embedded counters must be recreated through the app to keep previews and printing consistent. Static template dates remain supported. Removing a replacement image from an imported template restores any image embedded in the original template.

The basic 9 mm text/image workflow has physical validation. Wider tapes, advanced objects and batches still need broader physical verification. Automated tests and code review do not establish a bug-free application or full DYMO Connect parity. Mirror, collate, chain marks and graphics-quality controls remain pending device-specific preview/print validation. Firmware and hidden-setting modification are not supported.

## Dependencies

See `THIRD-PARTY-NOTICES.txt`. Vendor applications, SDK binaries and reference executables are not included in this source repository.
