//! The player's own music natively: one folder (`--music <dir>`, else a `music` folder
//! beside the executable, else the platform's Music folder) scanned at start on a
//! background thread. Each folder in it is one station of every MP3 under it, sub-folders
//! included (an album makes an album station, a genre folder of albums one big station),
//! and the MP3s loose in it make one more, named after the folder: the web's "a picked
//! folder is one station". Titles and lengths come from `audio::music::probe` on the first
//! 64 KiB of each file, as on the web; the radio's seeded shuffle and resume do the rest.
//! Only MP3 plays natively (ADR 0006). Nothing plays until the power key, as on the web.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use audio::music::{self, Track, HEAD_BYTES, MAX_TRACKS};

/// Folders deeper than this below a station's folder are not searched.
const MAX_DEPTH: usize = 8;

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
        return Some(dir.to_path_buf());
    }
    let beside = exe.and_then(Path::parent).map(|d| d.join("music"));
    if let Some(dir) = beside.filter(|d| d.is_dir()) {
        return Some(dir);
    }
    home.map(|h| h.join("Music")).filter(|d| d.is_dir())
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

/// Scans `root` on a background thread; its folders arrive one by one.
pub fn scan_in_background(root: PathBuf) -> Receiver<Folder> {
    let (tx, rx) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name("music-scan".into())
        .spawn(move || {
            let folders = scan(&root);
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

/// The stations of `root`: one per folder in it, then its loose files; folders without
/// MP3s are left out.
pub fn scan(root: &Path) -> Vec<Folder> {
    let mut out = Vec::new();
    let (dirs, files) = list(root);
    for dir in dirs {
        let mut paths = Vec::new();
        collect(&dir, 0, &mut paths);
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned());
        out.extend(folder(&dir, name.unwrap_or_default(), &paths));
    }
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned());
    out.extend(folder(root, name.unwrap_or_default(), &files));
    out
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
        let path = entry.path();
        if kind.is_dir() {
            dirs.push(path);
        } else if is_mp3(&path) && !kind.is_dir() {
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
/// its full path (what `PlayFile` gets back).
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
        let stations = scan(&root);
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
        assert_eq!(scan(&root), stations, "the same folder scans the same");
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
        assert!(scan(Path::new("/nonexistent/skywave/music")).is_empty());
    }
}
