//! Serialise direct SDK access to the connected 420P.
use std::sync::Mutex;

pub(crate) static PRINTER_ACCESS: Mutex<()> = Mutex::new(());

/// USB presence remains authoritative if optional device details cannot be read.
pub fn status() -> Result<crate::sdk::DeviceStatus, String> {
    let _access = PRINTER_ACCESS.lock().map_err(|_| "Printer access state unavailable.")?;
    crate::sdk::device_status()
}
