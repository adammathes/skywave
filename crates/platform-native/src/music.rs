//! The player's own music natively: one folder (`--music <dir>`, else a `music` folder
//! beside the executable, else the platform's Music folder) scanned at start on a
//! background thread. Each folder in it is one station of every MP3 under it, sub-folders
//! included (an album makes an album station, a genre folder of albums one big station),
//! and the MP3s loose in it make one more, named after the folder: the web's "a picked
//! folder is one station". Titles and lengths come from `audio::music::probe` on the first
//! 64 KiB of each file, as on the web; the radio's seeded shuffle and resume do the rest.
//! Only MP3 plays natively (ADR 0006). Nothing plays until the power key, as on the web.
//!
//! At most [`MAX_STATIONS`] folders become stations (by name), and the shell adds them a
//! few per frame, so a large library never stalls a frame. "Remove music folders" on the
//! Audio page writes the folders to an ignore list beside the settings
//! (`music-ignored.txt`, one path a line; delete a line to bring a folder back), which
//! later scans skip.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use audio::music::{self, Track, HEAD_BYTES, MAX_TRACKS};

/// Folders deeper than this below a station's folder are not searched.
const MAX_DEPTH: usize = 8;
/// Stations a music folder makes at most: plenty for a dial.
pub const MAX_STATIONS: usize = 64;
/// The ignore list's file name, beside `settings.json`.
const IGNORE_FILE: &str = "music-ignored.txt";

/// A folder of music, ready for `App::add_music_folder`.
#[derive(Clone, Debug, PartialEq)]
pub struct Folder {
    /// The folder's full path: the station's id comes from it.
    pub path: String,
    pub name: String,
    pub tracks: Vec<Track>,
}

/// The music folder: `arg` (`--music`) if given, else `music` beside the executable if
/// there is one, else the Music folder in `home`.
pub fn music_dir(arg: Option<&Path>, exe: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = arg {
        // Absolute, so station ids do not depend on the working directory.
        return Some(fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf()));
    }
    let beside = exe.and_then(Path::parent).map(|d| d.join("music"));
    if let Some(dir) = beside.filter(|d| d.is_dir()) {
        return Some(dir);
    }
    home.filter(|h| h.is_absolute())
        .map(|h| h.join("Music"))
        .filter(|d| d.is_dir())
}

/// [`music_dir`] for this process: the executable's path and `HOME` (`USERPROFILE` on
/// Windows).
pub fn default_dir(arg: Option<&Path>) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok();
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    music_dir(arg, exe.as_deref(), home.as_deref())
}

/// Scans `root` on a background thread, leaving out the folders in `ignored`; the stations
/// arrive when the scan is done.
pub fn scan_in_background(root: PathBuf, ignored: Vec<String>) -> Receiver<Folder> {
    let (tx, rx) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name("music-scan".into())
        .spawn(move || {
            if !root.is_dir() {
                log::warn!("music: {} is not a folder", root.display());
                return;
            }
            let folders = scan(&root, &ignored);
            log::info!(
                "music: {} station(s) from {}",
                folders.len(),
                root.display()
            );
            for f in folders {
                if tx.send(f).is_err() {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        log::warn!("music: cannot scan: {e}");
    }
    rx
}

/// The stations of `root`: one per folder in it, then its loose files, at most
/// [`MAX_STATIONS`]; folders without MP3s and those in `ignored` are left out.
pub fn scan(root: &Path, ignored: &[String]) -> Vec<Folder> {
    let wanted = |p: &Path| !ignored.iter().any(|i| Path::new(i) == p);
    let mut out = Vec::new();
    let (dirs, files) = list(root);
    for dir in dirs.into_iter().filter(|d| wanted(d)) {
        if out.len() == MAX_STATIONS {
            log::warn!("music: more than {MAX_STATIONS} folders; the rest are left out");
            return out;
        }
        let mut paths = Vec::new();
        collect(&dir, 0, &mut paths);
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned());
        out.extend(folder(&dir, name.unwrap_or_default(), &paths));
    }
    if out.len() < MAX_STATIONS && wanted(root) {
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned());
        out.extend(folder(root, name.unwrap_or_default(), &files));
    }
    out
}

/// Where the ignore list lives: beside the settings file.
pub fn ignore_file() -> Option<PathBuf> {
    crate::settings::settings_path(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
    .map(|p| p.with_file_name(IGNORE_FILE))
}

/// The folders in the ignore list at `file` (none when it is missing).
pub fn load_ignored(file: &Path) -> Vec<String> {
    fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

/// Adds `paths` to the ignore list at `file`.
pub fn ignore(file: &Path, paths: &[String]) {
    let mut list = load_ignored(file);
    for p in paths {
        if !list.contains(p) {
            list.push(p.clone());
        }
    }
    let text: String = list.iter().map(|p| format!("{p}\n")).collect();
    let written = file
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::write(file, text));
    if let Err(e) = written {
        log::warn!("music: cannot write {}: {e}", file.display());
    }
}

fn folder(dir: &Path, name: String, paths: &[PathBuf]) -> Option<Folder> {
    let tracks: Vec<Track> = paths
        .iter()
        .take(MAX_TRACKS)
        .filter_map(|p| track(p))
        .collect();
    (!tracks.is_empty()).then(|| Folder {
        path: dir.to_string_lossy().into_owned(),
        name,
        tracks,
    })
}

/// The sub-folders and MP3 files directly in `dir`, each sorted by name. Symbolic links
/// to folders are not followed (no loops).
fn list(dir: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut dirs, mut files) = (Vec::new(), Vec::new());
    let Ok(entries) = fs::read_dir(dir) else {
        return (dirs, files);
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        // Hidden entries: dot folders, and the `._*.mp3` resource forks a Mac leaves on
        // cards it fills.
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            dirs.push(path);
        } else if is_mp3(&path) {
            files.push(path);
        }
    }
    dirs.sort();
    files.sort();
    (dirs, files)
}

/// Every MP3 under `dir`, its own files first, then each sub-folder's, by name.
fn collect(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || out.len() >= MAX_TRACKS {
        return;
    }
    let (dirs, files) = list(dir);
    out.extend(files);
    for d in dirs {
        collect(&d, depth + 1, out);
    }
}

fn is_mp3(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("mp3"))
}

/// The track of the file at `path`: its title, artist and length from its head, keyed by
/// its full path (what `PlayFile` gets back). A file without a readable MPEG audio frame
/// (empty, or not MP3 after all) is left out.
fn track(path: &Path) -> Option<Track> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut head = Vec::with_capacity(HEAD_BYTES);
    file.by_ref()
        .take(HEAD_BYTES as u64)
        .read_to_end(&mut head)
        .ok()?;
    let at = music::audio_start(&head);
    let mut audio = Vec::new();
    if at + 4096 > head.len() as u64 && at < len {
        file.seek(SeekFrom::Start(at)).ok()?;
        file.take(4096).read_to_end(&mut audio).ok()?;
    }
    let probe = music::probe(&head, &audio, len);
    probe.secs?;
    let key = path.to_string_lossy().into_owned();
    let name = path.file_name()?.to_string_lossy().into_owned();
    Some(Track::new(key, &name, &probe))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radio::file::tests::mp3_file;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("skywave-music-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn folders_become_stations_with_their_tags() {
        let root = dir("library");
        let song = |t: &str, a: &str| mp3_file(40, &[("TIT2", t), ("TPE1", a)]);
        put(&root.join("Beach Mix/b.mp3"), &song("Wave", "Surf Band"));
        put(&root.join("Beach Mix/a.MP3"), &mp3_file(40, &[]));
        put(
            &root.join("Beach Mix/Disc 2/c.mp3"),
            &song("Deep", "Surf Band"),
        );
        put(&root.join("Beach Mix/cover.jpg"), b"not music");
        put(&root.join("Empty/notes.txt"), b"nothing");
        put(&root.join("loose.mp3"), &song("Alone", "Solo"));
        put(&root.join("Beach Mix/._b.mp3"), b"Mac resource fork");
        put(&root.join("Beach Mix/empty.mp3"), b"");
        put(&root.join("Beach Mix/renamed.mp3"), &[0x55; 20_000]);
        put(&root.join(".hidden/x.mp3"), &mp3_file(40, &[]));
        let stations = scan(&root, &[]);
        let names: Vec<&str> = stations.iter().map(|f| f.name.as_str()).collect();
        let root_name = root.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(names, ["Beach Mix", root_name.as_str()]);
        let labels: Vec<&str> = stations[0]
            .tracks
            .iter()
            .map(|t| t.label.as_str())
            .collect();
        assert_eq!(labels, ["a", "Surf Band - Wave", "Surf Band - Deep"]);
        // 40 frames of 1152 samples at 44.1 kHz, from the bitrate and the file size.
        assert!(
            (stations[0].tracks[1].secs - 1.04).abs() < 0.05,
            "{}",
            stations[0].tracks[1].secs
        );
        assert!(stations[0].tracks[0].key.ends_with("a.MP3"));
        assert_eq!(stations[1].tracks[0].label, "Solo - Alone");
        // Each becomes a files station with a stable id.
        let ch = audio::Channel::files(
            &stations[0].path,
            &stations[0].name,
            stations[0].tracks.clone(),
        )
        .unwrap();
        assert!(ch.id.starts_with("files_") && ch.is_files());
        assert_eq!(scan(&root, &[]), stations, "the same folder scans the same");
        // An ignored folder is left out.
        let ignored = [stations[0].path.clone()];
        let names: Vec<String> = scan(&root, &ignored).into_iter().map(|f| f.name).collect();
        assert_eq!(names, [root_name]);
    }

    #[test]
    fn the_music_folder_is_the_flag_then_beside_the_executable_then_home() {
        let root = dir("where");
        let exe = root.join("bin/skywave");
        let home = root.join("home");
        put(&exe, b"");
        fs::create_dir_all(home.join("Music")).unwrap();
        let flag = Path::new("/my/music");
        assert_eq!(
            music_dir(Some(flag), Some(&exe), Some(&home)),
            Some(flag.to_path_buf())
        );
        // A relative flag becomes absolute; a relative home is ignored.
        let rel = music_dir(Some(Path::new(".")), None, None).unwrap();
        assert!(rel.is_absolute(), "{}", rel.display());
        assert_eq!(music_dir(None, None, Some(Path::new("relative"))), None);
        assert_eq!(
            music_dir(None, Some(&exe), Some(&home)),
            Some(home.join("Music"))
        );
        fs::create_dir_all(root.join("bin/music")).unwrap();
        assert_eq!(
            music_dir(None, Some(&exe), Some(&home)),
            Some(root.join("bin/music"))
        );
        assert_eq!(music_dir(None, None, Some(&root.join("nobody"))), None);
    }

    #[test]
    fn a_missing_folder_has_no_stations() {
        assert!(scan(Path::new("/nonexistent/skywave/music"), &[]).is_empty());
    }

    #[test]
    fn stations_stop_at_the_cap() {
        let root = dir("many");
        for i in 0..MAX_STATIONS + 3 {
            put(&root.join(format!("f{i:03}/a.mp3")), &mp3_file(4, &[]));
        }
        put(&root.join("loose.mp3"), &mp3_file(4, &[]));
        let stations = scan(&root, &[]);
        assert_eq!(stations.len(), MAX_STATIONS);
        assert_eq!(stations[0].name, "f000");
    }

    #[test]
    fn the_ignore_list_round_trips() {
        let file = dir("ignore").join("cfg/music-ignored.txt");
        assert!(load_ignored(&file).is_empty());
        ignore(&file, &["/m/a".into(), "/m/b".into()]);
        ignore(&file, &["/m/b".into(), "/m/c".into()]);
        assert_eq!(load_ignored(&file), ["/m/a", "/m/b", "/m/c"]);
    }
}
