use eframe::egui::{self, Color32, Theme, ThemePreference, Visuals};
use std::{fs, io::Read, path::{Path, PathBuf}};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    System,
    Light,
    #[default]
    Dark,
    Firefly,
}

impl Appearance {
    pub const OPTIONS: [Self; 4] = [Self::System, Self::Light, Self::Dark, Self::Firefly];

    pub fn name(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::Firefly => "FireFly Pro",
        }
    }

    pub fn apply(self, ctx: &egui::Context) {
        for theme in [Theme::Light, Theme::Dark] {
            let mut style = (*ctx.style_of(theme)).clone();
            style.visuals = match theme {
                Theme::Light => Visuals::light(),
                Theme::Dark if self == Self::Firefly => firefly(),
                Theme::Dark => Visuals::dark(),
            };
            style.spacing.item_spacing = egui::vec2(8.0, 8.0);
            style.spacing.button_padding = egui::vec2(10.0, 6.0);
            for (key, size) in [(egui::TextStyle::Body, 14.0), (egui::TextStyle::Button, 14.0),
                (egui::TextStyle::Heading, 20.0), (egui::TextStyle::Small, 12.0)] {
                style.text_styles.insert(key, egui::FontId::proportional(size));
            }
            ctx.set_style_of(theme, style);
        }
        ctx.set_theme(match self {
            Self::System => ThemePreference::System,
            Self::Light => ThemePreference::Light,
            Self::Dark | Self::Firefly => ThemePreference::Dark,
        });
    }
}

// Studio's UI mapping of the FireFly Pro editor, sidebar, foreground and accent colours.
fn firefly() -> Visuals {
    let mut v = Visuals::dark();
    let canvas = Color32::from_rgb(10, 15, 23);
    let panel = Color32::from_rgb(14, 20, 33);
    let text = Color32::from_rgb(176, 186, 221);
    let accent = Color32::from_rgb(145, 179, 98);
    v.panel_fill = canvas;
    v.window_fill = panel;
    v.extreme_bg_color = canvas;
    v.faint_bg_color = panel;
    v.hyperlink_color = accent;
    v.selection.bg_fill = Color32::from_rgb(54, 67, 36);
    v.selection.stroke.color = Color32::from_rgb(218, 235, 180);
    v.text_cursor.stroke.color = Color32::from_rgb(189, 229, 241);
    v.widgets.noninteractive.fg_stroke.color = text;
    for (widget, fill) in [
        (&mut v.widgets.inactive, Color32::from_rgb(31, 40, 57)),
        (&mut v.widgets.hovered, Color32::from_rgb(46, 57, 76)),
        (&mut v.widgets.active, Color32::from_rgb(64, 78, 42)),
        (&mut v.widgets.open, Color32::from_rgb(46, 57, 76)),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.fg_stroke.color = text;
    }
    v.widgets.active.fg_stroke.color = Color32::WHITE;
    v.widgets.hovered.bg_stroke.color = accent;
    v
}

fn path() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA").ok_or("Windows local app data is unavailable.")?;
    Ok(PathBuf::from(base).join("420P Label Studio").join("appearance.txt"))
}

fn read(path: &Path) -> Result<Appearance, String> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Appearance::default()),
        Err(error) => return Err(format!("Cannot read the appearance setting: {error}")),
    };
    let mut value = String::new();
    file.take(64).read_to_string(&mut value).map_err(|e| e.to_string())?;
    Appearance::OPTIONS.into_iter().find(|option| option.name() == value.trim())
        .ok_or_else(|| "Unrecognised appearance setting. Choose a theme in Settings.".into())
}

pub fn load() -> Result<Appearance, String> { read(&path()?) }

pub fn save(value: Appearance) -> Result<(), String> {
    let file = path()?;
    fs::create_dir_all(file.parent().ok_or("Appearance folder is missing.")?).map_err(|e| e.to_string())?;
    crate::project_io::write_project(&file, value.name().as_bytes(), file.exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn four_modes_apply_and_switch_back_without_leaking_the_custom_palette() {
        let ctx = egui::Context::default();
        for mode in [Appearance::Light, Appearance::Firefly, Appearance::Dark, Appearance::System] {
            mode.apply(&ctx);
            let system = ctx.options(|options| options.theme_preference);
            assert_eq!(system == ThemePreference::System, mode == Appearance::System);
            let dark = ctx.style_of(Theme::Dark);
            assert_eq!(dark.visuals.panel_fill == firefly().panel_fill, mode == Appearance::Firefly);
            assert_eq!(ctx.style_of(Theme::Light).visuals.panel_fill, Visuals::light().panel_fill);
            if mode == Appearance::Light { assert_eq!(ctx.theme(), Theme::Light); }
        }
        for system_theme in [Theme::Light, Theme::Dark] {
            let input = egui::RawInput { system_theme: Some(system_theme), ..Default::default() };
            let _ = ctx.run_ui(input, |ui| assert_eq!(ui.ctx().theme(), system_theme));
        }
    }
    #[test]
    fn stored_choices_round_trip_and_existing_users_default_to_dark() {
        let file = std::env::temp_dir().join(format!("420p-appearance-test-{}", std::process::id()));
        assert_eq!(read(&file).unwrap(), Appearance::Dark);
        for value in Appearance::OPTIONS {
            fs::write(&file, value.name()).unwrap();
            assert_eq!(read(&file).unwrap(), value);
        }
        fs::write(&file, "unknown").unwrap();
        assert!(read(&file).is_err());
        fs::remove_file(file).unwrap();
    }
}
