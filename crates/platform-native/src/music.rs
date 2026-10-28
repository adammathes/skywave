//! The player's own music natively, the deck's library: one folder (`--music <dir>`, else
//! a `music` folder beside the executable, else the platform's Music folder), laid out as
//! a music app exports it (`Artist/Album/NN Title.mp3`), scanned at start on a background
//! thread. Each folder in it (an artist's, usually) is one `audio::deck::Folder` of every
//! MP3 under it, each file's path given from the music folder so the deck reads artist and
//! album from the layout as on the web; the MP3s loose in it make one more, an album named
//! after the music folder. Tags and lengths come from `audio::music::probe` on the first
//! 64 KiB of each file, as on the web. Only MP3 plays natively (ADR 0006). Nothing plays
//! until a key, as on the web.
//!
//! At most [`MAX_FOLDERS`] folders are read (by name), and the shell adds them a few per
//! frame, so a large library never stalls a frame. "Remove music folders" on the Audio page
//! writes the folders to an ignore list beside the settings (`music-ignored.txt`, one path
//! a line; delete a line to bring a folder back), which later scans skip.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use audio::deck::{FileInfo, Folder};
use audio::music::{self, HEAD_BYTES, MAX_TRACKS};

/// Folders deeper than this below a folder of the music folder are not searched.
const MAX_DEPTH: usize = 8;
/// Folders of the music folder read at most (artists, usually).
pub const MAX_FOLDERS: usize = 1024;
/// The ignore list's file name, beside `settings.json`.
const IGNORE_FILE: &str = "music-ignored.txt";

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

/// Scans `root` on a background thread, leaving out the folders in `ignored`; the folders
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
            log::info!("music: {} folder(s) from {}", folders.len(), root.display());
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

/// The library's folders in `root`: one per folder in it, then its loose files, at most
/// [`MAX_FOLDERS`]; folders without MP3s and those in `ignored` are left out.
pub fn scan(root: &Path, ignored: &[String]) -> Vec<Folder> {
    let wanted = |p: &Path| !ignored.iter().any(|i| Path::new(i) == p);
    let mut out = Vec::new();
    let (dirs, files) = list(root);
    for dir in dirs.into_iter().filter(|d| wanted(d)) {
        if out.len() == MAX_FOLDERS {
            log::warn!("music: more than {MAX_FOLDERS} folders; the rest are left out");
            return out;
        }
        let mut paths = Vec::new();
        collect(&dir, 0, &mut paths);
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned());
        out.extend(folder(root, &dir, name.unwrap_or_default(), &paths));
        if out.iter().map(|f| f.files.len()).sum::<usize>() >= MAX_TRACKS {
            log::warn!("music: more than {MAX_TRACKS} tracks; the rest are left out");
            return out;
        }
    }
    if out.len() < MAX_FOLDERS && wanted(root) {
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned());
        out.extend(folder(root, root, name.unwrap_or_default(), &files));
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

/// The library folder `dir` (in the music folder `root`) of the files at `paths`.
fn folder(root: &Path, dir: &Path, name: String, paths: &[PathBuf]) -> Option<Folder> {
    let files: Vec<FileInfo> = paths
        .iter()
        .take(MAX_TRACKS)
        .filter_map(|p| file(root, p))
        .collect();
    (!files.is_empty()).then(|| Folder {
        id: dir.to_string_lossy().into_owned(),
        name,
        mixtape: false,
        files,
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

/// The file at `path` as the deck's library takes it: what its head says (tags, length),
/// keyed by its full path (what `PlayFile` gets back), its path from the music folder
/// `root` (`/` between names). A file without a readable MPEG audio frame (empty, or not
/// MP3 after all) is left out.
fn file(root: &Path, path: &Path) -> Option<FileInfo> {
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
    let rel = path.strip_prefix(root).unwrap_or(path);
    let rel: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(FileInfo {
        key: path.to_string_lossy().into_owned(),
        path: rel.join("/"),
        probe,
    })
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
    fn folders_become_the_library_with_their_tags() {
        let root = dir("library");
        let song = |t: &str, a: &str| mp3_file(40, &[("TIT2", t), ("TPE1", a)]);
        put(
            &root.join("Surf Band/Beach Mix/02 b.mp3"),
            &song("Wave", "Surf Band"),
        );
        put(
            &root.join("Surf Band/Beach Mix/01 a.MP3"),
            &mp3_file(40, &[]),
        );
        put(
            &root.join("Surf Band/Beach Mix/Disc 2/c.mp3"),
            &song("Deep", "Surf Band"),
        );
        put(&root.join("Surf Band/Beach Mix/cover.jpg"), b"not music");
        put(&root.join("Empty/notes.txt"), b"nothing");
        put(&root.join("loose.mp3"), &song("Alone", "Solo"));
        put(
            &root.join("Surf Band/Beach Mix/._b.mp3"),
            b"Mac resource fork",
        );
        put(&root.join("Surf Band/Beach Mix/empty.mp3"), b"");
        put(
            &root.join("Surf Band/Beach Mix/renamed.mp3"),
            &[0x55; 20_000],
        );
        put(&root.join(".hidden/x.mp3"), &mp3_file(40, &[]));
        let folders = scan(&root, &[]);
        let names: Vec<&str> = folders.iter().map(|f| f.name.as_str()).collect();
        let root_name = root.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(names, ["Surf Band", root_name.as_str()]);
        let paths: Vec<&str> = folders[0].files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "Surf Band/Beach Mix/01 a.MP3",
                "Surf Band/Beach Mix/02 b.mp3",
                "Surf Band/Beach Mix/Disc 2/c.mp3"
            ]
        );
        // 40 frames of 1152 samples at 44.1 kHz, from the bitrate and the file size.
        let secs = folders[0].files[1].probe.secs.unwrap();
        assert!((secs - 1.04).abs() < 0.05, "{secs}");
        assert!(folders[0].files[0].key.ends_with("01 a.MP3"));
        assert_eq!(folders[1].files[0].path, "loose.mp3");
        // The deck reads artist and album from the layout, the tags over it.
        let lib = audio::deck::Library::build(&folders);
        let albums: Vec<(&str, &str)> = lib
            .albums
            .iter()
            .map(|a| (a.artist.as_str(), a.title.as_str()))
            .collect();
        assert_eq!(
            albums,
            [
                ("Solo", root_name.as_str()),
                ("Surf Band", "Beach Mix"),
                ("Surf Band", "Disc 2")
            ]
        );
        assert_eq!(scan(&root, &[]), folders, "the same folder scans the same");
        // An ignored folder is left out.
        let ignored = [folders[0].id.clone()];
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
    fn a_missing_folder_has_no_music() {
        assert!(scan(Path::new("/nonexistent/skywave/music"), &[]).is_empty());
    }

    #[test]
    fn folders_stop_at_the_cap() {
        let root = dir("many");
        for i in 0..MAX_FOLDERS + 3 {
            put(&root.join(format!("f{i:04}/a.mp3")), &mp3_file(4, &[]));
        }
        put(&root.join("loose.mp3"), &mp3_file(4, &[]));
        let folders = scan(&root, &[]);
        assert_eq!(folders.len(), MAX_FOLDERS);
        assert_eq!(folders[0].name, "f0000");
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
