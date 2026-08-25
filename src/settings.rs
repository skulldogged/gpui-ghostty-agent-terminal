use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub(crate) use crate::terminal_theme::ThemePreset;

pub(crate) const MIN_FONT_SIZE: f32 = 8.;
pub(crate) const MAX_FONT_SIZE: f32 = 48.;
pub(crate) const MIN_OPACITY: f32 = 0.45;
pub(crate) const MAX_OPACITY: f32 = 1.;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KeybindAction {
    OpenSettings,
    CreateSpace,
    CreateTab,
    ClosePane,
    SplitHorizontal,
    SplitVertical,
}

impl KeybindAction {
    pub(crate) const ALL: [Self; 6] = [
        Self::OpenSettings,
        Self::CreateSpace,
        Self::CreateTab,
        Self::ClosePane,
        Self::SplitHorizontal,
        Self::SplitVertical,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::OpenSettings => "Open settings",
            Self::CreateSpace => "New Space",
            Self::CreateTab => "New tab",
            Self::ClosePane => "Close pane",
            Self::SplitHorizontal => "Split right",
            Self::SplitVertical => "Split down",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::OpenSettings => "Show or hide this settings page",
            Self::CreateSpace => "Create a Space in the current directory",
            Self::CreateTab => "Create a tab in the selected Space",
            Self::ClosePane => "Close the focused pane",
            Self::SplitHorizontal => "Place a new pane to the right",
            Self::SplitVertical => "Place a new pane below",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Shortcut {
    pub(crate) key: String,
    #[serde(default)]
    pub(crate) control: bool,
    #[serde(default)]
    pub(crate) alt: bool,
    #[serde(default)]
    pub(crate) shift: bool,
    #[serde(default)]
    pub(crate) platform: bool,
}

impl Shortcut {
    pub(crate) fn is_usable(&self) -> bool {
        !self.key.trim().is_empty()
            && !matches!(
                self.key.to_ascii_lowercase().as_str(),
                "control" | "shift" | "alt" | "command" | "super" | "fn"
            )
    }

    pub(crate) fn display(&self) -> String {
        let mut parts = Vec::with_capacity(5);
        if self.control {
            parts.push("Ctrl".to_owned());
        }
        if self.alt {
            parts.push(
                if cfg!(target_os = "macos") {
                    "Option"
                } else {
                    "Alt"
                }
                .to_owned(),
            );
        }
        if self.shift {
            parts.push("Shift".to_owned());
        }
        if self.platform {
            parts.push(
                if cfg!(target_os = "macos") {
                    "Cmd"
                } else {
                    "Super"
                }
                .to_owned(),
            );
        }
        parts.push(display_key(&self.key));
        parts.join(" + ")
    }
}

fn display_key(key: &str) -> String {
    match key.to_ascii_lowercase().as_str() {
        "escape" => "Esc".to_owned(),
        "backspace" => "Backspace".to_owned(),
        "," => ",".to_owned(),
        key if key.len() == 1 => key.to_ascii_uppercase(),
        _ => key.to_owned(),
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct KeybindingSettings {
    open_settings: Option<Shortcut>,
    create_space: Option<Shortcut>,
    create_tab: Option<Shortcut>,
    close_pane: Option<Shortcut>,
    split_horizontal: Option<Shortcut>,
    split_vertical: Option<Shortcut>,
}

impl KeybindingSettings {
    pub(crate) fn get(&self, action: KeybindAction) -> Shortcut {
        self.custom(action)
            .cloned()
            .unwrap_or_else(|| default_shortcut(action))
    }

    pub(crate) fn custom(&self, action: KeybindAction) -> Option<&Shortcut> {
        match action {
            KeybindAction::OpenSettings => self.open_settings.as_ref(),
            KeybindAction::CreateSpace => self.create_space.as_ref(),
            KeybindAction::CreateTab => self.create_tab.as_ref(),
            KeybindAction::ClosePane => self.close_pane.as_ref(),
            KeybindAction::SplitHorizontal => self.split_horizontal.as_ref(),
            KeybindAction::SplitVertical => self.split_vertical.as_ref(),
        }
    }

    pub(crate) fn set(&mut self, action: KeybindAction, shortcut: Option<Shortcut>) {
        *match action {
            KeybindAction::OpenSettings => &mut self.open_settings,
            KeybindAction::CreateSpace => &mut self.create_space,
            KeybindAction::CreateTab => &mut self.create_tab,
            KeybindAction::ClosePane => &mut self.close_pane,
            KeybindAction::SplitHorizontal => &mut self.split_horizontal,
            KeybindAction::SplitVertical => &mut self.split_vertical,
        } = shortcut;
    }

    pub(crate) fn conflict_for(
        &self,
        action: KeybindAction,
        shortcut: &Shortcut,
    ) -> Option<KeybindAction> {
        KeybindAction::ALL
            .into_iter()
            .find(|candidate| *candidate != action && self.get(*candidate) == *shortcut)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct AppSettings {
    pub(crate) theme: ThemePreset,
    pub(crate) background_opacity: f32,
    pub(crate) font_family: Option<String>,
    pub(crate) font_size: f32,
    pub(crate) keybindings: KeybindingSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreset::TokyoNight,
            background_opacity: 0.65,
            font_family: None,
            font_size: 14.,
            keybindings: KeybindingSettings::default(),
        }
    }
}

impl AppSettings {
    pub(crate) fn load() -> (Self, Option<String>) {
        let Some(path) = settings_path() else {
            return (Self::default(), None);
        };
        match fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str::<Self>(&contents) {
                Ok(mut settings) => {
                    settings.sanitize();
                    (settings, None)
                }
                Err(error) => (
                    Self::default(),
                    Some(format!(
                        "ignored invalid settings at {}: {error}",
                        path.display()
                    )),
                ),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(error) => (
                Self::default(),
                Some(format!(
                    "could not read settings at {}: {error}",
                    path.display()
                )),
            ),
        }
    }

    pub(crate) fn save(&self) -> Result<(), String> {
        let path = settings_path()
            .ok_or_else(|| "no user configuration directory is available".to_owned())?;
        let parent = path
            .parent()
            .ok_or_else(|| "settings path has no parent directory".to_owned())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("create settings directory {}: {error}", parent.display()))?;
        let contents = serde_json::to_vec_pretty(self)
            .map_err(|error| format!("serialize settings: {error}"))?;
        let temporary = path.with_extension("json.tmp");
        let mut file = fs::File::create(&temporary).map_err(|error| {
            format!("create temporary settings {}: {error}", temporary.display())
        })?;
        file.write_all(&contents)
            .and_then(|_| file.sync_all())
            .map_err(|error| {
                format!("write temporary settings {}: {error}", temporary.display())
            })?;
        drop(file);
        replace_file(&temporary, &path)
            .map_err(|error| format!("replace settings {}: {error}", path.display()))
    }

    pub(crate) fn effective_opacity(&self) -> f32 {
        std::env::var("AGENT_TERMINAL_BACKGROUND_OPACITY")
            .ok()
            .and_then(|value| parse_opacity(&value))
            .unwrap_or(self.background_opacity)
    }

    pub(crate) fn effective_font_size(&self) -> f32 {
        std::env::var("AGENT_TERMINAL_FONT_SIZE")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|size| (MIN_FONT_SIZE..=MAX_FONT_SIZE).contains(size))
            .unwrap_or(self.font_size)
    }

    pub(crate) fn effective_font_family(&self) -> Option<String> {
        std::env::var("AGENT_TERMINAL_FONT")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| self.font_family.clone())
    }

    pub(crate) fn sanitize(&mut self) {
        self.background_opacity = if self.background_opacity.is_finite() {
            self.background_opacity.clamp(MIN_OPACITY, MAX_OPACITY)
        } else {
            Self::default().background_opacity
        };
        self.font_size = if self.font_size.is_finite() {
            self.font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
        } else {
            Self::default().font_size
        };
        if self
            .font_family
            .as_ref()
            .is_some_and(|family| family.trim().is_empty())
        {
            self.font_family = None;
        }
        for action in KeybindAction::ALL {
            if self
                .keybindings
                .custom(action)
                .is_some_and(|shortcut| !shortcut.is_usable())
            {
                self.keybindings.set(action, None);
            }
        }
    }
}

pub(crate) fn default_shortcut(action: KeybindAction) -> Shortcut {
    let macos = cfg!(target_os = "macos");
    let mut shortcut = Shortcut {
        key: match action {
            KeybindAction::OpenSettings => ",",
            KeybindAction::CreateSpace => "n",
            KeybindAction::CreateTab => "t",
            KeybindAction::ClosePane => "w",
            KeybindAction::SplitHorizontal => "d",
            KeybindAction::SplitVertical => "e",
        }
        .to_owned(),
        control: !macos,
        alt: false,
        shift: false,
        platform: macos,
    };
    shortcut.shift = matches!(
        action,
        KeybindAction::CreateSpace
            | KeybindAction::ClosePane
            | KeybindAction::SplitHorizontal
            | KeybindAction::SplitVertical
    );
    shortcut
}

pub(crate) fn parse_opacity(value: &str) -> Option<f32> {
    let opacity = value.trim().parse::<f32>().ok()?;
    opacity
        .is_finite()
        .then(|| opacity.clamp(MIN_OPACITY, MAX_OPACITY))
}

fn settings_path() -> Option<PathBuf> {
    if let Some(directory) =
        std::env::var_os("AGENT_TERMINAL_CONFIG_DIR").filter(|value| !value.is_empty())
    {
        return Some(PathBuf::from(directory).join("settings.json"));
    }
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|directory| directory.join("agent-terminal").join("settings.json"))
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .map(|home| home.join(".config"))
            })
            .map(|directory| directory.join("agent-terminal").join("settings.json"))
    }
}

#[cfg(windows)]
fn replace_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{REPLACEFILE_WRITE_THROUGH, ReplaceFileW};

    if !destination.exists() {
        return fs::rename(temporary, destination);
    }

    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let temporary = temporary
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let replaced = unsafe {
        ReplaceFileW(
            destination.as_ptr(),
            temporary.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_WRITE_THROUGH,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if replaced == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(temporary, destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_and_missing_fields_are_filled() {
        let serialized = serde_json::to_string(&AppSettings::default()).unwrap();
        let decoded: AppSettings = serde_json::from_str(&serialized).unwrap();
        assert_eq!(decoded.theme, ThemePreset::TokyoNight);
        let migrated: AppSettings = serde_json::from_str(r#"{"theme":"midnight"}"#).unwrap();
        assert_eq!(migrated.theme, ThemePreset::TokyoNight);
        let partial: AppSettings = serde_json::from_str(r#"{"theme":"nord"}"#).unwrap();
        assert_eq!(partial.theme, ThemePreset::Nord);
        assert_eq!(partial.font_size, 14.);
    }

    #[test]
    fn invalid_numeric_preferences_are_sanitized() {
        let mut settings = AppSettings {
            background_opacity: f32::NAN,
            font_size: 100.,
            ..AppSettings::default()
        };
        settings.sanitize();
        assert_eq!(settings.background_opacity, 0.65);
        assert_eq!(settings.font_size, MAX_FONT_SIZE);
        assert_eq!(parse_opacity("0"), Some(MIN_OPACITY));
        assert_eq!(parse_opacity("NaN"), None);
    }

    #[test]
    fn unusable_persisted_shortcuts_are_reset_to_defaults() {
        let mut settings: AppSettings = serde_json::from_str(
            r#"{"keybindings":{"open_settings":{"key":""},"create_tab":{"key":"control"}}}"#,
        )
        .unwrap();

        settings.sanitize();

        assert_eq!(
            settings.keybindings.get(KeybindAction::OpenSettings),
            default_shortcut(KeybindAction::OpenSettings)
        );
        assert_eq!(
            settings.keybindings.get(KeybindAction::CreateTab),
            default_shortcut(KeybindAction::CreateTab)
        );
        assert!(
            settings
                .keybindings
                .custom(KeybindAction::OpenSettings)
                .is_none()
        );
        assert!(
            settings
                .keybindings
                .custom(KeybindAction::CreateTab)
                .is_none()
        );
    }

    #[test]
    fn custom_shortcuts_override_and_reset_to_defaults() {
        let mut keybindings = KeybindingSettings::default();
        let custom = Shortcut {
            key: "k".to_owned(),
            control: true,
            alt: true,
            shift: false,
            platform: false,
        };
        keybindings.set(KeybindAction::CreateTab, Some(custom.clone()));
        assert_eq!(keybindings.get(KeybindAction::CreateTab), custom);
        keybindings.set(KeybindAction::CreateTab, None);
        assert_eq!(
            keybindings.get(KeybindAction::CreateTab),
            default_shortcut(KeybindAction::CreateTab)
        );
    }

    #[test]
    fn resetting_reports_a_conflict_with_the_default_shortcut() {
        let mut keybindings = KeybindingSettings::default();
        let create_tab_default = default_shortcut(KeybindAction::CreateTab);
        keybindings.set(
            KeybindAction::CreateTab,
            Some(Shortcut {
                key: "k".to_owned(),
                ..create_tab_default.clone()
            }),
        );
        keybindings.set(
            KeybindAction::OpenSettings,
            Some(create_tab_default.clone()),
        );

        assert_eq!(
            keybindings.conflict_for(KeybindAction::CreateTab, &create_tab_default),
            Some(KeybindAction::OpenSettings)
        );
    }

    #[cfg(windows)]
    #[test]
    fn replacing_an_existing_settings_file_installs_the_complete_new_file() {
        let directory = std::env::temp_dir().join(format!(
            "agent-terminal-settings-replace-{}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let destination = directory.join("settings.json");
        let temporary = directory.join("settings.json.tmp");
        fs::write(&destination, b"old").unwrap();
        fs::write(&temporary, b"new").unwrap();

        replace_file(&temporary, &destination).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"new");
        assert!(!temporary.exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
