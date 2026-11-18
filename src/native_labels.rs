//! Native DYMO documents stay embedded locally; conversion never writes the source.
use std::{io::Write, path::{Path, PathBuf}, os::windows::process::CommandExt, process::{Command, Stdio}};
use base64::Engine;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
pub struct NativeLabel { pub xml: String, pub key: String, pub object_names: Vec<String>, pub width: u8, pub text: String, pub font: String, pub bold: bool, pub italic: bool, pub underline: bool, pub align: String, #[cfg(test)] pub reference_preview: Option<String> }
impl NativeLabel {
    pub fn value(&self) -> serde_json::Value { serde_json::json!({"xml":self.xml,"width":self.width,"text":self.text,"font":self.font,"bold":self.bold,"italic":self.italic,"underline":self.underline,"align":self.align}) }
    pub fn from_value(value: &serde_json::Value) -> Result<Self, String> {
        let field = |key| value[key].as_str().map(str::to_owned).ok_or_else(|| format!("Missing native label field: {key}"));
        let xml = field("xml")?;
        if xml.len() > crate::project_io::NATIVE_BASE64_LIMIT { return Err("Embedded DYMO label exceeds 16 MiB.".into()); }
        let bytes = base64::engine::general_purpose::STANDARD.decode(&xml).map_err(|_| "Invalid embedded DYMO label.")?;
        if bytes.is_empty() || bytes.len() > crate::project_io::NATIVE_LIMIT { return Err("Invalid embedded DYMO label size.".into()); }
        let width = value["width"].as_u64().filter(|v| [6,9,12,19].contains(v)).ok_or("Invalid native tape width.")? as u8;
        let text = field("text")?; let font = field("font")?; let align = field("align")?;
        if text.chars().count() > 500 || font.is_empty() || font.len() > 128 || font.chars().any(char::is_control) || !["Left","Center","Right"].contains(&align.as_str()) { return Err("Invalid native label text or formatting.".into()); }
        let boolean = |key| value[key].as_bool().ok_or_else(|| format!("Invalid native label field: {key}"));
        let key = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        let object_names = object_names(&bytes)?;
        Ok(Self { xml, key, object_names, width, text, font, align, bold:boolean("bold")?, italic:boolean("italic")?, underline:boolean("underline")?, #[cfg(test)] reference_preview: value["referencePreview"].as_str().map(str::to_owned) })
    }
}
// Read direct Name children of label objects, including UTF-16 DYMO files and
// legacy projects which only stored the original bytes. Ignore data-table names.
fn object_names(bytes: &[u8]) -> Result<Vec<String>, String> {
    use xml::reader::{ParserConfig, XmlEvent};
    let reader = ParserConfig::new().max_entity_expansion_depth(0).create_reader(bytes);
    let mut names = Vec::new();
    let mut depth = 0usize;
    let mut objects_depth = None;
    let mut current_name = None::<String>;
    for event in reader {
        match event.map_err(|error| format!("Invalid embedded label XML: {error}"))? {
            XmlEvent::StartElement { name, .. } => {
                depth += 1;
                if depth > 128 { return Err("The embedded label is nested too deeply.".into()); }
                if objects_depth.is_none() && name.local_name == "LabelObjects" {
                    objects_depth = Some(depth);
                } else if objects_depth.is_some_and(|list| depth == list + 2) && name.local_name == "Name" {
                    current_name = Some(String::new());
                }
            }
            XmlEvent::Characters(text) | XmlEvent::CData(text) | XmlEvent::Whitespace(text) => {
                if let Some(name) = &mut current_name {
                    if name.len() + text.len() > 512 { return Err("An embedded object name is too long.".into()); }
                    name.push_str(&text);
                }
            }
            XmlEvent::EndElement { .. } => {
                if objects_depth.is_some_and(|list| depth == list + 2) {
                    if let Some(name) = current_name.take() {
                        if names.len() >= 64 { return Err("A label can contain at most 64 objects.".into()); }
                        names.push(name);
                    }
                }
                if objects_depth == Some(depth) { break; }
                depth -= 1;
            }
            _ => {}
        }
    }
    Ok(names)
}

pub fn read(path: &Path) -> Result<NativeLabel, String> {
    #[cfg(test)]
    if let Ok(bridge) = std::env::var("DYMO_TEST_BRIDGE") { return read_with_bridge(path, bridge.into()); }
    let bridge = std::env::current_exe().map_err(|e| e.to_string())?.parent().ok_or("Application folder missing.")?.join("sdk/DymoBridge.exe");
    read_with_bridge(path, bridge)
}
pub(crate) fn read_with_bridge(path: &Path, bridge: PathBuf) -> Result<NativeLabel, String> {
    let bytes = crate::project_io::read_bounded(path, crate::project_io::NATIVE_LIMIT).map_err(|e| format!("Cannot open DYMO label: {e}"))?;
    if bytes.is_empty() { return Err("The DYMO label is empty.".into()); }
    let xml = base64::engine::general_purpose::STANDARD.encode(bytes);
    let body = serde_json::json!({"action":"import-label","templateBase64":xml,"includeDesign":cfg!(test)}).to_string();
    let mut child = Command::new(bridge).creation_flags(0x08000000).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let mut input = child.stdin.take().ok_or("SDK input unavailable.")?;
    let writer = std::thread::spawn(move || input.write_all(body.as_bytes()));
    let output = crate::sdk::wait_child(child, std::time::Duration::from_secs(30));
    let sent = writer.join().map_err(|_| "DYMO input writer stopped.")?;
    let output = output?; sent.map_err(|e| e.to_string())?;
    if !output.status.success() {
        let message = serde_json::from_slice::<serde_json::Value>(&output.stderr).ok().and_then(|v| v["error"].as_str().map(str::to_owned)).unwrap_or_else(|| "The DYMO SDK could not open this label.".into());
        return Err(message);
    }
    let mut value: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    value["xml"] = serde_json::json!(xml);
    NativeLabel::from_value(&value)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> serde_json::Value { serde_json::json!({"xml":base64::engine::general_purpose::STANDARD.encode(b"<DesktopLabel/>"),"width":9,"text":"Synthetic label","font":"Arial","bold":true,"italic":false,"underline":true,"align":"Left"}) }
    #[test] fn native_project_embeds_original_bytes_and_preserves_formatting() {
        let native = NativeLabel::from_value(&fixture()).unwrap(); let reopened = NativeLabel::from_value(&native.value()).unwrap();
        assert_eq!(native.xml, reopened.xml); assert_eq!(native.key, reopened.key); assert_eq!(reopened.text, "Synthetic label"); assert!(reopened.bold && reopened.underline); assert_eq!(reopened.align, "Left");
    }
    #[test] fn malformed_embedded_documents_fail_before_document_state_changes() {
        for (key, value) in [("xml", serde_json::json!("not base64")), ("width", serde_json::json!(24)), ("font", serde_json::json!("invalid\nfont")), ("text", serde_json::json!("x".repeat(501))), ("align", serde_json::json!("invalid"))] {
            let mut input = fixture(); input[key] = value; assert!(NativeLabel::from_value(&input).is_err());
        }
    }
    #[test] fn malformed_xml_and_external_entities_are_rejected_when_bridge_supplied() {
        let Ok(bridge) = std::env::var("DYMO_TEST_BRIDGE") else { return; };
        let path = std::env::temp_dir().join(format!("420p-invalid-native-{}.dymo", std::process::id()));
        for content in ["not xml", "<!DOCTYPE x [<!ENTITY e SYSTEM 'file:///unavailable'>]><DesktopLabel>&e;</DesktopLabel>"] {
            std::fs::write(&path, content).unwrap(); assert!(read_with_bridge(&path, PathBuf::from(&bridge)).is_err());
        }
        std::fs::remove_file(path).unwrap();
    }
}
