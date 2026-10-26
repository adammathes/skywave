//! The settings document on disk: `$XDG_CONFIG_HOME/skywave/settings.json`, or
//! `~/.config/skywave/settings.json` when that is unset (the XDG default, also used on
//! macOS for simplicity).

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use app::SettingsStore;

const DIR: &str = "skywave";
const FILE: &str = "settings.json";

/// Where the settings live, from the values of `XDG_CONFIG_HOME` and `HOME`. Relative or
/// empty values are ignored, as the XDG spec asks.
pub fn settings_path(xdg_config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let absolute = |v: Option<OsString>| v.map(PathBuf::from).filter(|p| p.is_absolute());
    let config = absolute(xdg_config_home).or_else(|| absolute(home).map(|h| h.join(".config")))?;
    Some(config.join(DIR).join(FILE))
}

/// Settings in a file. Without a usable path nothing is loaded or saved.
pub struct FileStore {
    path: Option<PathBuf>,
}

impl FileStore {
    pub fn new(path: Option<PathBuf>) -> FileStore {
        FileStore { path }
    }

    pub fn from_env() -> FileStore {
        FileStore::new(settings_path(
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("HOME"),
        ))
    }
}

impl SettingsStore for FileStore {
    fn load(&self) -> Option<String> {
        let path = self.path.as_ref()?;
        match fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                log::warn!("cannot read {}: {e}", path.display());
                None
            }
        }
    }

    /// Writes a sibling temporary file and renames it over the old one, so a crash never
    /// leaves half a file.
    fn save(&mut self, json: &str) {
        let Some(path) = &self.path else { return };
        let tmp = path.with_extension("json.tmp");
        let result = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&tmp, json))
            .and_then(|()| fs::rename(&tmp, path));
        if let Err(e) = result {
            log::warn!("cannot save settings to {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_follows_xdg_then_home() {
        let os = |s: &str| Some(OsString::from(s));
        assert_eq!(
            settings_path(os("/cfg"), os("/home/u")),
            Some(PathBuf::from("/cfg/skywave/settings.json"))
        );
        assert_eq!(
            settings_path(None, os("/home/u")),
            Some(PathBuf::from("/home/u/.config/skywave/settings.json"))
        );
        assert_eq!(
            settings_path(os("relative"), os("/home/u")),
            Some(PathBuf::from("/home/u/.config/skywave/settings.json"))
        );
        assert_eq!(settings_path(os(""), None), None);
    }

    #[test]
    fn saves_and_loads_through_a_file() {
        let dir = std::env::temp_dir().join(format!("skywave-settings-{}", std::process::id()));
        let path = dir.join("nested").join(FILE);
        let mut store = FileStore::new(Some(path.clone()));
        assert_eq!(store.load(), None);
        store.save("{\"version\": 1}");
        store.save("{\"version\": 2}");
        assert_eq!(store.load().as_deref(), Some("{\"version\": 2}"));
        assert!(!path.with_extension("json.tmp").exists());
        fs::remove_dir_all(&dir).unwrap();
        let mut nowhere = FileStore::new(None);
        nowhere.save("x");
        assert_eq!(nowhere.load(), None);
    }
}
