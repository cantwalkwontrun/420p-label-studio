use base64::Engine;
use std::{os::windows::process::CommandExt, path::PathBuf, process::{Command, Stdio}};

pub fn register() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let windows = std::env::var_os("SystemRoot").ok_or("Windows folder unavailable.")?;
    let script = include_str!("../scripts/register-file-type.ps1");
    // The executable path travels as an environment value, never as shell syntax.
    let command = format!("[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)\ntry {{ & {{\n{script}\n}} -ApplicationPath $env:LABEL_STUDIO_APPLICATION }} catch {{ [Console]::Error.WriteLine($_.Exception.Message); exit 1 }}");
    let bytes: Vec<u8> = command.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    let child = Command::new(PathBuf::from(windows).join("System32/WindowsPowerShell/v1.0/powershell.exe"))
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
        .env("LABEL_STUDIO_APPLICATION", &exe).creation_flags(0x08000000)
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("Cannot start file registration: {e}"))?;
    let result = crate::sdk::wait_child(child, std::time::Duration::from_secs(30))
        .map_err(|_| "File registration did not finish in time. Check Windows Open with.".to_string())?;
    if !result.status.success() { return Err(String::from_utf8_lossy(&result.stderr).trim().to_string()); }
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_string())
}
