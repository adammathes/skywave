//! The player's own music natively, the deck's library: the music folder (`--music <dir>`,
//! else a `music` folder beside the executable, else the platform's Music folder) and the
//! folders the player picked ("Music library" on the Audio page opens the platform's folder
//! dialog) or dropped on the window, remembered beside the settings (`music-folders.txt`, one
//! path a line). Each is laid out as a music app exports it (`Artist/Album/NN Title.mp3`;
//! Apple Music's `Music/Media.localized/Music/Artist/...` too) and scanned on a background
//! thread. Each folder in it (an artist's, usually) is one `audio::deck::Folder` of every
//! MP3 under it, each file's path given from the scanned folder so the deck reads artist and
//! album from the layout as on the web; the MP3s loose in it make one more, an album named
//! after it. MP3s dropped loose on the window are one "Mixtape". Tags and lengths come from
//! `audio::music::probe` on the first 64 KiB of each file, as on the web. Only MP3 plays
//! natively (ADR 0006): other files are left out without a word. Nothing plays until a key,
//! as on the web.
//!
//! At most [`MAX_FOLDERS`] folders are read (by name), and the shell adds them a few per
//! frame, so a large library never stalls a frame. "Remove music folders" on the Audio page
//! writes the folders to an ignore list beside the settings (`music-ignored.txt`, one path
//! a line; delete a line to bring a folder back), which later scans skip, and forgets the
//! picked folders; picking a folder again takes it and everything in it off the list.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
/// The picked and dropped folders' file name, beside `settings.json`.
const FOLDERS_FILE: &str = "music-folders.txt";
/// Where Apple Music (and iTunes before it) keeps its files, below the folder scanned: each
/// folder there is an artist's.
const MEDIA: [&str; 6] = [
    "Music/Media.localized/Music",
    "Music/Media/Music",
    "Media.localized/Music",
    "Media/Music",
    "iTunes/iTunes Media/Music",
    "iTunes Media/Music",
];
/// The folder dialog's title.
const PROMPT: &str = "Pick a music folder";

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

/// Scans the folders in `list` (its [`roots`]) on a background thread, leaving out the folders in
/// `ignored`; the folders arrive at once when the scan is done.
pub fn scan_in_background(list: Vec<PathBuf>, ignored: Vec<String>) -> Receiver<Vec<Folder>> {
    in_background("music-scan", move || {
        let mut folders = Vec::new();
        for root in roots(list) {
            if !root.is_dir() {
                log::warn!("music: {} is not a folder", root.display());
                continue;
            }
            let found = scan(&root, &ignored);
            log::info!("music: {} folder(s) from {}", found.len(), root.display());
            folders.extend(found);
        }
        folders
    })
}

/// The MP3s in `files` (dropped on the window) as one mixtape, read on a background thread.
pub fn mixtape_in_background(files: Vec<PathBuf>) -> Receiver<Vec<Folder>> {
    in_background("music-drop", move || mixtape(&files).into_iter().collect())
}

/// Asks the player for a folder with the platform's dialog on a background thread (it
/// blocks until the dialog closes); the answer arrives once.
pub fn pick_in_background() -> Receiver<Picked> {
    in_background("music-dialog", || run_dialog(dialog_command()))
}

/// `work`'s answer, from a thread of its own (none if it cannot start).
fn in_background<T: Send + 'static>(
    name: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Receiver<T> {
    let (tx, rx) = mpsc::channel();
    let spawned = thread::Builder::new().name(name.into()).spawn(move || {
        let _ = tx.send(work());
    });
    if let Err(e) = spawned {
        log::warn!("music: cannot start {name}: {e}");
    }
    rx
}

/// The folders of `list` to scan: each once, and none inside another (it is scanned with
/// it), in order.
pub fn roots(list: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for root in &list {
        let covered = list.iter().any(|r| r != root && root.starts_with(r));
        if !covered && !out.contains(root) {
            out.push(root.clone());
        }
    }
    out
}

/// The library's folders in `root`: one per folder in it, then its loose files, at most
/// [`MAX_FOLDERS`]; folders without MP3s and those in `ignored` are left out. An Apple
/// Music media folder in it ([`MEDIA`]) comes first, each of its folders (an artist's) one
/// of the library's, its files' paths given from it.
pub fn scan(root: &Path, ignored: &[String]) -> Vec<Folder> {
    let mut out = Vec::new();
    let media = MEDIA.iter().map(|m| root.join(m)).find(|m| m.is_dir());
    // The branch of `root` that leads to the media folder is scanned as the media folder.
    let skip = media
        .as_ref()
        .and_then(|m| m.strip_prefix(root).ok()?.components().next())
        .map(|c| root.join(c));
    if let Some(media) = &media {
        if !scan_into(media, ignored, None, &mut out) {
            return out;
        }
    }
    scan_into(root, ignored, skip.as_deref(), &mut out);
    out
}

/// [`scan`] without the media folder: `root`'s folders but `skip`, then its loose files,
/// onto `out`. False once a cap is reached.
fn scan_into(root: &Path, ignored: &[String], skip: Option<&Path>, out: &mut Vec<Folder>) -> bool {
    let wanted = |p: &Path| !ignored.iter().any(|i| Path::new(i) == p);
    let (dirs, files) = list(root);
    for dir in dirs
        .into_iter()
        .filter(|d| wanted(d) && Some(d.as_path()) != skip)
    {
        if out.len() == MAX_FOLDERS {
            log::warn!("music: more than {MAX_FOLDERS} folders; the rest are left out");
            return false;
        }
        let mut paths = Vec::new();
        collect(&dir, 0, &mut paths);
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned());
        out.extend(folder(root, &dir, name.unwrap_or_default(), &paths));
        if out.iter().map(|f| f.files.len()).sum::<usize>() >= MAX_TRACKS {
            log::warn!("music: more than {MAX_TRACKS} tracks; the rest are left out");
            return false;
        }
    }
    if out.len() < MAX_FOLDERS && wanted(root) {
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned());
        out.extend(folder(root, root, name.unwrap_or_default(), &files));
    }
    true
}

/// The MP3s among `files` as the "Mixtape" album (none without one). Its id starts
/// `mixtape:`, and is the same for the same files.
pub fn mixtape(files: &[PathBuf]) -> Option<Folder> {
    let files: Vec<FileInfo> = files
        .iter()
        .filter(|p| is_mp3(p))
        .take(MAX_TRACKS)
        .filter_map(|p| file(p.parent()?, p))
        .collect();
    // FNV-1a over the keys: a name for the drop that fits on a line.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in files.iter().flat_map(|f| f.key.bytes().chain([0])) {
        hash = (hash ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
    }
    (!files.is_empty()).then(|| Folder {
        id: format!("{MIXTAPE_ID}{hash:016x}"),
        name: "Mixtape".to_string(),
        mixtape: true,
        files,
    })
}

/// How a mixtape's id starts: it has no folder to ignore or remember.
pub const MIXTAPE_ID: &str = "mixtape:";

/// What the folder dialog answered.
#[derive(Clone, Debug, PartialEq)]
pub enum Picked {
    Folder(PathBuf),
    Cancelled,
    /// There is no dialog to open (none on this platform, or zenity is not installed).
    NoDialog,
}

/// The folder dialog: AppleScript's `choose folder` on macOS, zenity on other Unixes,
/// none elsewhere. Each prints the folder's path and exits 0, or exits non-zero when
/// cancelled.
pub fn dialog_command() -> Option<Command> {
    if cfg!(target_os = "macos") {
        let mut cmd = Command::new("osascript");
        cmd.arg("-e").arg(format!(
            "POSIX path of (choose folder with prompt \"{PROMPT}\")"
        ));
        Some(cmd)
    } else if cfg!(unix) {
        let mut cmd = Command::new("zenity");
        cmd.args(["--file-selection", "--directory"])
            .arg(format!("--title={PROMPT}"));
        Some(cmd)
    } else {
        None
    }
}

/// Runs the dialog `cmd` and reads its answer; blocks until it closes. A command that does
/// not start (not installed) has no dialog.
pub fn run_dialog(cmd: Option<Command>) -> Picked {
    let Some(mut cmd) = cmd else {
        return Picked::NoDialog;
    };
    match cmd.stdin(Stdio::null()).stderr(Stdio::null()).output() {
        Ok(out) => parse_dialog(out.status.success(), &out.stdout),
        Err(e) => {
            log::info!("music: no folder dialog: {e}");
            Picked::NoDialog
        }
    }
}

/// The dialog's answer from its exit status and what it printed: the path, less the line
/// end (osascript ends a folder's with `/`, which a path takes as it is).
pub fn parse_dialog(ok: bool, stdout: &[u8]) -> Picked {
    let line = stdout.strip_suffix(b"\n").unwrap_or(stdout);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    if !ok || line.is_empty() {
        return Picked::Cancelled;
    }
    #[cfg(unix)]
    let path = PathBuf::from(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(line));
    #[cfg(not(unix))]
    let path = PathBuf::from(String::from_utf8_lossy(line).into_owned());
    Picked::Folder(path)
}

/// The file `name` beside the settings file.
fn config_file(name: &str) -> Option<PathBuf> {
    crate::settings::settings_path(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
    .map(|p| p.with_file_name(name))
}

/// Where the ignore list lives: beside the settings file.
pub fn ignore_file() -> Option<PathBuf> {
    config_file(IGNORE_FILE)
}

/// Where the picked and dropped folders are remembered: beside the settings file.
pub fn folders_file() -> Option<PathBuf> {
    config_file(FOLDERS_FILE)
}

/// The paths in the list at `file`, one a line (none when it is missing).
pub fn load_list(file: &Path) -> Vec<String> {
    fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

/// Adds `paths` to the list at `file`.
pub fn add_to_list(file: &Path, paths: &[String]) {
    let mut list = load_list(file);
    for p in paths {
        if !list.contains(p) {
            list.push(p.clone());
        }
    }
    write_list(file, &list);
}

/// Takes the paths at or inside `dir` off the list at `file` (all of them with no `dir`).
pub fn remove_from_list(file: &Path, dir: Option<&Path>) {
    let list = load_list(file);
    let kept: Vec<String> = list
        .iter()
        .filter(|p| dir.is_some_and(|d| !Path::new(p).starts_with(d)))
        .cloned()
        .collect();
    if kept.len() != list.len() {
        write_list(file, &kept);
    }
}

fn write_list(file: &Path, list: &[String]) {
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
        assert!(load_list(&file).is_empty());
        add_to_list(&file, &["/m/a".into(), "/m/b".into()]);
        add_to_list(&file, &["/m/b".into(), "/m/c".into()]);
        assert_eq!(load_list(&file), ["/m/a", "/m/b", "/m/c"]);
    }

    #[test]
    fn picked_folders_are_remembered_and_forgotten() {
        let file = dir("remember").join("cfg/music-folders.txt");
        add_to_list(&file, &["/Users/a/Tunes".into()]);
        add_to_list(&file, &["/Volumes/Card/MP3 Player".into()]);
        add_to_list(&file, &["/Users/a/Tunes".into()]);
        add_to_list(&file, &["/Users/a/Tunes/Surf Band".into()]);
        assert_eq!(
            load_list(&file),
            [
                "/Users/a/Tunes",
                "/Volumes/Card/MP3 Player",
                "/Users/a/Tunes/Surf Band"
            ]
        );
        // Picked again: off the list, everything in it too (the ignore list's use).
        remove_from_list(&file, Some(Path::new("/Users/a/Tunes")));
        assert_eq!(load_list(&file), ["/Volumes/Card/MP3 Player"]);
        // "Remove music folders": all of them.
        remove_from_list(&file, None);
        assert!(load_list(&file).is_empty());
        assert!(file.exists(), "written empty, so it sticks");
    }

    #[test]
    fn each_folder_is_scanned_once() {
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(
            roots(vec![
                p("/u/Music"),
                p("/u/Music/Mix"),
                p("/d"),
                p("/d"),
                p("/u")
            ]),
            [p("/d"), p("/u")]
        );
    }

    #[test]
    fn the_apple_music_layout_reads_as_artist_and_album() {
        let home = dir("apple");
        let root = home.join("Music");
        let media = root.join("Music/Media.localized/Music");
        let song = mp3_file(4, &[]);
        put(&media.join("Surf Band/Beach Mix/01 Wave.mp3"), &song);
        put(
            &media.join("Surf Band/Beach Mix/02 Tide.m4a"),
            b"AAC, not played",
        );
        put(&media.join("Night Shift/Coastline/1-03 Neon.mp3"), &song);
        put(&media.join("Compilations/Drive/01 Road.mp3"), &song);
        put(&root.join("Music/Music Library.musiclibrary/x.mp3"), &song);
        put(&root.join("Other/Mix/a.mp3"), &song);
        put(&root.join("Lossless/Album/01 a.flac"), b"FLAC, not played");
        let names = |f: &[Folder]| f.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
        let folders = scan(&root, &[]);
        assert_eq!(
            names(&folders),
            ["Compilations", "Night Shift", "Surf Band", "Other"],
            "the media folder's artists, then the rest; nothing in Music/ but the media"
        );
        assert_eq!(folders[2].files.len(), 1, "MP3 only");
        assert_eq!(folders[2].files[0].path, "Surf Band/Beach Mix/01 Wave.mp3");
        let lib = audio::deck::Library::build(&folders);
        let albums: Vec<(&str, &str)> = lib
            .albums
            .iter()
            .map(|a| (a.artist.as_str(), a.title.as_str()))
            .collect();
        assert_eq!(
            albums,
            [
                ("Compilations", "Drive"),
                ("Night Shift", "Coastline"),
                ("Other", "Mix"),
                ("Surf Band", "Beach Mix")
            ]
        );
        // Picking the Music app's own folder reads the same; so does the plain `Media` name
        // and the old iTunes one. The home folder is one folder ("Music") whose albums
        // still read from the layout's last two folders.
        let artists = ["Compilations", "Night Shift", "Surf Band"];
        assert_eq!(names(&scan(&root.join("Music"), &[]))[..3], artists);
        let lib = audio::deck::Library::build(&scan(&home, &[]));
        assert!(lib
            .albums
            .iter()
            .any(|a| (a.artist.as_str(), a.title.as_str()) == ("Surf Band", "Beach Mix")));
        let plain = dir("apple-plain");
        put(&plain.join("Media/Music/A/B/01 x.mp3"), &song);
        put(&plain.join("iTunes/iTunes Media/Music/C/D/01 y.mp3"), &song);
        assert_eq!(names(&scan(&plain, &[])), ["A", "iTunes"]);
        // An ignored artist stays out.
        let ignored = [media.join("Surf Band").to_string_lossy().into_owned()];
        assert_eq!(
            names(&scan(&root, &ignored)),
            ["Compilations", "Night Shift", "Other"]
        );
    }

    #[test]
    fn a_large_media_folder_fills_the_library() {
        // 600 artists of 7 tracks: past the library's 4096 tracks, read by artist name,
        // not cut short by the folder cap or the depth.
        let root = dir("apple-large");
        let media = root.join("Music/Media.localized/Music");
        let song = mp3_file(1, &[]);
        for a in 0..600 {
            for t in 0..7 {
                put(&media.join(format!("A{a:03}/Album/{t:02} t.mp3")), &song);
            }
        }
        let folders = scan(&root, &[]);
        let tracks: usize = folders.iter().map(|f| f.files.len()).sum();
        assert!(tracks >= MAX_TRACKS, "{tracks}");
        assert_eq!(folders.len(), MAX_TRACKS.div_ceil(7));
        assert_eq!(folders[0].name, "A000");
    }

    #[test]
    fn folders_without_mp3s_have_no_music() {
        let root = dir("lossless");
        put(&root.join("Band/Album/01 a.flac"), b"FLAC");
        put(&root.join("Band/Album/02 b.m4a"), b"AAC");
        put(&root.join("loose.aac"), b"AAC");
        put(&root.join("loose.ogg"), b"Vorbis");
        assert!(scan(&root, &[]).is_empty());
    }

    #[test]
    fn loose_dropped_mp3s_are_a_mixtape() {
        let d = dir("drop");
        let song = |t: &str| mp3_file(4, &[("TIT2", t)]);
        put(&d.join("a/one.mp3"), &song("One"));
        put(&d.join("b/two.MP3"), &song("Two"));
        put(&d.join("b/three.m4a"), b"AAC");
        put(&d.join("b/notes.txt"), b"text");
        let files = ["a/one.mp3", "b/two.MP3", "b/three.m4a", "b/notes.txt"].map(|f| d.join(f));
        let tape = mixtape(&files).unwrap();
        assert!(tape.mixtape);
        assert!(tape.id.starts_with(MIXTAPE_ID));
        let paths: Vec<&str> = tape.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["one.mp3", "two.MP3"]);
        assert_eq!(
            mixtape(&files).unwrap().id,
            tape.id,
            "the same drop, the same id"
        );
        assert_ne!(mixtape(&files[..1]).unwrap().id, tape.id);
        let lib = audio::deck::Library::build(&[tape]);
        assert_eq!(lib.albums[0].title, "Mixtape");
        assert_eq!(mixtape(&files[2..]), None, "no MP3, no mixtape");
    }

    #[test]
    fn the_dialog_answer_is_read() {
        // osascript ends a folder's POSIX path with `/`; zenity does not.
        assert_eq!(
            parse_dialog(true, b"/Users/a/My Music/\n"),
            Picked::Folder("/Users/a/My Music/".into())
        );
        assert_eq!(
            parse_dialog(true, b"/home/a/Tunes\r\n"),
            Picked::Folder("/home/a/Tunes".into())
        );
        // Cancelled: osascript exits 1 ("User canceled. (-128)" on stderr), zenity 1.
        assert_eq!(parse_dialog(false, b""), Picked::Cancelled);
        assert_eq!(parse_dialog(true, b"\n"), Picked::Cancelled);
        assert_eq!(parse_dialog(false, b"/partial\n"), Picked::Cancelled);
    }

    #[test]
    #[cfg(unix)]
    fn the_dialog_runs_as_a_command() {
        let sh = |script: &str| {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", script]);
            Some(cmd)
        };
        assert_eq!(
            run_dialog(sh("printf '/Volumes/Card/MP3s/\\n'")),
            Picked::Folder("/Volumes/Card/MP3s/".into())
        );
        assert_eq!(
            run_dialog(sh(
                "echo 'execution error: User canceled. (-128)' >&2; exit 1"
            )),
            Picked::Cancelled
        );
        assert_eq!(
            run_dialog(Some(Command::new("skywave-no-such-dialog"))),
            Picked::NoDialog
        );
        assert_eq!(run_dialog(None), Picked::NoDialog);
        // Each platform's dialog prints the folder: the arguments it gets.
        let cmd = dialog_command().unwrap();
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        if cfg!(target_os = "macos") {
            assert_eq!(cmd.get_program(), "osascript");
            assert_eq!(
                args,
                [
                    "-e",
                    "POSIX path of (choose folder with prompt \"Pick a music folder\")"
                ]
            );
        } else {
            assert_eq!(cmd.get_program(), "zenity");
            assert_eq!(
                args,
                [
                    "--file-selection",
                    "--directory",
                    "--title=Pick a music folder"
                ]
            );
        }
    }
}
