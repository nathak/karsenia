//! The Windows side of the project file editor: save, create, delete (into the project's trash), restore, delete for good, rename and move.
//!
//! The rule is the Linux one: nothing is done to a path that was only checked as text. A folder is opened and kept open (no share for
//! delete or rename, so it stays where it is), its real place is asked of the system and must lie inside the project, and an entry in
//! it is opened by name without following a link (`FILE_FLAG_OPEN_REPARSE_POINT`) and then changed through that handle (rename,
//! delete), so what was looked at is what is changed. A save writes a temporary file in the same folder and swaps it in with
//! `ReplaceFileW`, which keeps the old file's permissions and streams. Network folders (`\\?\UNC\`) are refused: the guarantees above
//! are those of a local NTFS or FAT volume. One gap is left on purpose: the swap by `ReplaceFileW` goes by path, so a file symlink put in
//! the target's place in the instant after the last look (which needs the right to make symlinks) is not caught. None of this could be run where it was written: see the tests at the end, which are for a
//! Windows machine.
use std::ffi::{c_void, OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{
    FileDispositionInfo, FileDispositionInfoEx, FileRenameInfo, GetFinalPathNameByHandleW, ReplaceFileW, SetFileInformationByHandle, DELETE, FILE_ATTRIBUTE_READONLY, FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, SYNCHRONIZE, VOLUME_NAME_DOS,
};

use super::{
    check_change, decode, delete_how, guard, inside, move_check, new_name, parse_record, purge_name_ok, rel_path, text_ok, trash_entry_name, trash_id, version, win_part_ok, Changed, DeleteHow, Guard, Purged, Saved, Tally, Walk, MAX_DEPTH, MAX_PURGE, MAX_TEXT, SAVING,
    TRASH,
};

// ---- the system's answers ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Dir,
    /// A symbolic link, a junction or a mount point: always the link itself, never what it leads to.
    Link,
}

fn kind_of(meta: &std::fs::Metadata) -> Kind {
    if meta.file_type().is_symlink() {
        Kind::Link
    } else if meta.is_dir() {
        Kind::Dir
    } else {
        Kind::File
    }
}

fn io_sentence(e: &std::io::Error, fallback: &str) -> String {
    match e.raw_os_error() {
        Some(80 | 183) => "that name already exists",
        Some(2 | 3) => "the folder does not exist",
        Some(5) => "not allowed on this folder",
        Some(32 | 33) => "another program has it open",
        Some(145) => "the folder is not empty",
        Some(17) => "it is on another disk, so it cannot be moved there (nothing was copied)",
        _ => fallback,
    }
    .to_owned()
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// Opens a file or folder by path, a link as itself (not followed).
fn open_raw(path: &Path, access: u32, share: u32) -> std::io::Result<File> {
    OpenOptions::new().access_mode(access).share_mode(share).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS).open(path)
}

/// Where an open file or folder really is (`\\?\C:\...`, links resolved).
fn final_path(f: &File) -> std::io::Result<PathBuf> {
    let mut buf = vec![0u16; 512];
    loop {
        // SAFETY: the handle is open and the buffer's length is given.
        let n = unsafe { GetFinalPathNameByHandleW(f.as_raw_handle() as HANDLE, buf.as_mut_ptr(), buf.len() as u32, VOLUME_NAME_DOS) } as usize;
        if n == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if n < buf.len() {
            buf.truncate(n);
            return Ok(PathBuf::from(OsString::from_wide(&buf)));
        }
        buf.resize(n + 1, 0);
    }
}

/// Renames the open entry to the whole path `to` (replacing an existing one only when `replace`).
fn rename_handle(f: &File, to: &Path, replace: bool) -> std::io::Result<()> {
    let name = wide(to);
    let bytes = (name.len() - 1) * 2;
    let size = std::mem::size_of::<FILE_RENAME_INFO>() + bytes;
    // 8-byte units: the structure holds a pointer-sized field and so needs that alignment.
    let mut buf = vec![0u64; size.div_ceil(8)];
    let info = buf.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: the buffer is zeroed, aligned and big enough for the header and the name; the name is copied to where the structure's
    // variable-length `FileName` starts (through a raw pointer, not through the one-element array).
    let done = unsafe {
        (*info).Anonymous.ReplaceIfExists = replace;
        (*info).FileNameLength = bytes as u32;
        std::ptr::copy_nonoverlapping(name.as_ptr(), std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(), name.len() - 1);
        SetFileInformationByHandle(f.as_raw_handle() as HANDLE, FileRenameInfo, info.cast::<c_void>(), size as u32)
    };
    if done == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn set_disposition(f: &File, flags: u32) -> std::io::Result<()> {
    let info = FILE_DISPOSITION_INFO_EX { Flags: flags };
    // SAFETY: the handle is open and the structure is a plain value of the size given.
    if unsafe { SetFileInformationByHandle(f.as_raw_handle() as HANDLE, FileDispositionInfoEx, std::ptr::addr_of!(info).cast::<c_void>(), std::mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32) } != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Deletes the open entry (a file, a link or an empty folder) at once, although others have it open. A read-only file is deleted only
/// when `ignore_readonly`; its attribute is never changed (it belongs to the file, whatever other names it has). A system or a volume
/// that does not know the newer requests (before Windows 10 1809, FAT) is asked the older way.
fn dispose(f: &File, ignore_readonly: bool) -> std::io::Result<()> {
    let ignore = if ignore_readonly { FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE } else { 0 };
    // Invalid function, not supported, invalid parameter: the request is not known here.
    let unknown = |e: &std::io::Error| matches!(e.raw_os_error(), Some(1 | 50 | 87));
    for flags in [FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS | ignore, FILE_DISPOSITION_FLAG_DELETE | ignore] {
        match set_disposition(f, flags) {
            Err(e) if unknown(&e) => {}
            done => return done,
        }
    }
    let old = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: as above.
    if unsafe { SetFileInformationByHandle(f.as_raw_handle() as HANDLE, FileDispositionInfo, std::ptr::addr_of!(old).cast::<c_void>(), std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32) } != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Puts the text of `tmp` in the place of `target`, which keeps its permissions and streams.
fn replace_file(target: &Path, tmp: &Path) -> std::io::Result<()> {
    let (target, tmp) = (wide(target), wide(tmp));
    // SAFETY: both names are NUL-terminated; no backup name, no flags, nothing reserved.
    if unsafe { ReplaceFileW(target.as_ptr(), tmp.as_ptr(), std::ptr::null(), 0, std::ptr::null(), std::ptr::null()) } != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

// ---- folders and entries held open ----

/// A folder of the project, held open so that it stays where it is while it is worked in.
struct Dir {
    _handle: File,
    /// Its real place.
    path: PathBuf,
    /// Its place inside the project (what the guard looks at).
    rel: PathBuf,
}

fn pin_dir(root: &Path, real: &Path) -> Result<Dir, String> {
    let handle = open_raw(real, FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE, FILE_SHARE_READ | FILE_SHARE_WRITE).map_err(|e| io_sentence(&e, "the folder cannot be opened"))?;
    if !handle.metadata().is_ok_and(|m| kind_of(&m) == Kind::Dir) {
        return Err("not a folder".to_owned());
    }
    let path = final_path(&handle).map_err(|_| "the folder cannot be verified".to_owned())?;
    let rel = path.strip_prefix(root).map_err(|_| "that path leaves the project folder".to_owned())?.to_owned();
    Ok(Dir { _handle: handle, path, rel })
}

/// A folder made when needed inside `parent` (the trash and its `.from`), a plain folder and not a link.
fn sub_folder(parent: &Dir, name: &str, create: bool) -> Result<Dir, String> {
    let path = parent.path.join(name);
    let bad = || format!("{name} is not a plain folder of the project, so nothing was done");
    if create {
        match std::fs::create_dir(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io_sentence(&e, "the trash folder cannot be made")),
        }
    }
    let handle = open_raw(&path, FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE, FILE_SHARE_READ | FILE_SHARE_WRITE).map_err(|_| bad())?;
    if !handle.metadata().is_ok_and(|m| kind_of(&m) == Kind::Dir) {
        return Err(bad());
    }
    let real = final_path(&handle).map_err(|_| bad())?;
    if real.parent() != Some(parent.path.as_path()) {
        return Err(bad());
    }
    Ok(Dir { _handle: handle, path: real, rel: parent.rel.join(name) })
}

/// What an entry is opened for: it decides the rights asked and which sharing is let go.
#[derive(Clone, Copy)]
enum Access {
    /// Only to see that it is there and what it is.
    Look,
    /// To rename or delete it: nobody else may rename or delete it meanwhile.
    Move,
    /// To read it and then replace it with `ReplaceFileW`, which needs the sharing.
    Save,
}

impl Access {
    fn mask(self) -> u32 {
        match self {
            Access::Look => FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            Access::Move => DELETE | FILE_READ_ATTRIBUTES | FILE_READ_DATA | SYNCHRONIZE,
            Access::Save => FILE_READ_ATTRIBUTES | FILE_READ_DATA | SYNCHRONIZE,
        }
    }

    fn share(self) -> u32 {
        match self {
            Access::Look | Access::Save => FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            Access::Move => FILE_SHARE_READ | FILE_SHARE_WRITE,
        }
    }
}

struct Opened {
    file: File,
    kind: Kind,
    readonly: bool,
}

/// An entry named by a path whose last part is not followed: the folder is held open, the entry is opened by name in it (`None` when it
/// is not there), and `rel` is where the entry really is, with the name as the disk spells it.
struct Place {
    dir: Dir,
    name: OsString,
    rel: PathBuf,
    entry: Option<Opened>,
}

fn open_entry(dir: &Dir, name: &OsStr, access: Access) -> Result<Option<(Opened, OsString)>, String> {
    let file = match open_raw(&dir.path.join(name), access.mask(), access.share()) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_sentence(&e, "it cannot be opened")),
    };
    let meta = file.metadata().map_err(|_| "it cannot be read".to_owned())?;
    // The disk's own name and place (not the text sent: a short 8.3 name or another case is the same entry), which must be in this folder.
    let real = final_path(&file).map_err(|_| "it cannot be verified".to_owned())?;
    if real.parent() != Some(dir.path.as_path()) {
        return Err("that path leaves the project folder".to_owned());
    }
    let name = real.file_name().map(OsStr::to_owned).ok_or_else(|| "that is not an entry".to_owned())?;
    Ok(Some((Opened { file, kind: kind_of(&meta), readonly: meta.file_attributes() & FILE_ATTRIBUTE_READONLY != 0 }, name)))
}

fn check_parts(rel_text: &str) -> Result<(), String> {
    if rel_text.split('/').any(|p| !win_part_ok(p)) {
        return Err("that is not a name this editor uses on Windows".to_owned());
    }
    Ok(())
}

fn place(root: &Path, rel_text: &str, access: Access) -> Result<Place, String> {
    let rel = rel_path(rel_text).map_err(str::to_owned)?;
    let name = rel.file_name().ok_or_else(|| "that is the project folder".to_owned())?.to_owned();
    check_parts(rel_text)?;
    let parent = inside(root, rel.parent().unwrap_or_else(|| Path::new("")))?;
    let dir = pin_dir(root, &parent)?;
    let (entry, name) = match open_entry(&dir, &name, access)? {
        Some((entry, real_name)) => (Some(entry), real_name),
        None => (None, name),
    };
    let rel = dir.rel.join(&name);
    Ok(Place { dir, name, rel, entry })
}

/// A plain file of the project, links on the way and at the end resolved (as the editor opens a file): the place of the file itself.
fn target(root: &Path, rel_text: &str, access: Access) -> Result<Place, String> {
    let rel = rel_path(rel_text).map_err(str::to_owned)?;
    check_parts(rel_text)?;
    let real = inside(root, &rel)?;
    let inner = real.strip_prefix(root).map_err(|_| "that path leaves the project folder".to_owned())?;
    let text = slash(inner).ok_or_else(|| "the path is not valid".to_owned())?;
    if text.is_empty() {
        return Err("that is the project folder, not a file".to_owned());
    }
    let p = place(root, &text, access)?;
    match p.entry.as_ref().map(|e| e.kind) {
        Some(Kind::File) => Ok(p),
        Some(_) => Err("not a plain file".to_owned()),
        None => Err("no such file".to_owned()),
    }
}

fn slash(rel: &Path) -> Option<String> {
    rel.components().map(|c| c.as_os_str().to_str()).collect::<Option<Vec<_>>>().map(|v| v.join("/"))
}

/// Writing is refused where the system's rules are not those of a local volume.
fn local(root: &Path) -> Result<(), String> {
    if root.to_string_lossy().starts_with(r"\\?\UNC\") {
        return Err("a network folder cannot be changed from here on Windows".to_owned());
    }
    Ok(())
}

fn read_capped(mut file: &File) -> Result<Vec<u8>, String> {
    file.seek(SeekFrom::Start(0)).map_err(|_| "the file cannot be read".to_owned())?;
    let mut bytes = Vec::new();
    file.take(MAX_TEXT as u64 + 1).read_to_end(&mut bytes).map_err(|_| "the file cannot be read".to_owned())?;
    if bytes.len() > MAX_TEXT {
        return Err(format!("the file is bigger than {} KiB, which the editor does not open", MAX_TEXT / 1024));
    }
    Ok(bytes)
}

// ---- saving ----

pub fn write_text(project: &Path, rel_text: &str, text: &str, base: &str, confirm: bool) -> Result<Saved, String> {
    write_text_in(&project.canonicalize().map_err(|_| "the project folder cannot be opened".to_owned())?, rel_text, text, base, confirm)
}

/// Replaces an existing plain file's text, if it is still the version `base`.
pub fn write_text_in(root: &Path, rel_text: &str, text: &str, base: &str, confirm: bool) -> Result<Saved, String> {
    let _one_at_a_time = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    text_ok(text)?;
    local(root)?;
    let t = target(root, rel_text, Access::Save)?;
    match guard(&t.rel) {
        Guard::Block(why) => return Err(format!("not saved: {why}")),
        Guard::Confirm(why) if !confirm => return Ok(Saved::NeedsConfirm(why)),
        _ => {}
    }
    let Some(entry) = t.entry.as_ref() else { return Err("no such file".to_owned()) };
    if entry.readonly {
        return Err("the file is read-only".to_owned());
    }
    let current = read_capped(&entry.file)?;
    if version(&current) != base {
        return Ok(Saved::Conflict { version: version(&current), text: decode(current)? });
    }
    let Place { dir, name, entry, .. } = t;
    drop(entry);
    let tmp = dir.path.join(format!(".tenrec-{}.tmp", crate::random_hex(8)));
    let target_path = dir.path.join(&name);
    let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(|_| "a temporary file cannot be made in that folder".to_owned())?;
    let written = file.write_all(text.as_bytes()).and_then(|()| file.sync_all());
    drop(file);
    let remove_tmp = || {
        let _ = std::fs::remove_file(&tmp);
    };
    if written.is_err() {
        remove_tmp();
        return Err("the file could not be written".to_owned());
    }
    // The last look before the swap: whoever wrote in the time the temporary file took is not overwritten without a word.
    match open_entry(&dir, &name, Access::Save) {
        Ok(Some((now, _))) if now.kind == Kind::File => match read_capped(&now.file) {
            Ok(bytes) if version(&bytes) == base => {}
            Ok(bytes) => {
                remove_tmp();
                return Ok(Saved::Conflict { version: version(&bytes), text: decode(bytes)? });
            }
            Err(e) => {
                remove_tmp();
                return Err(e);
            }
        },
        Ok(_) => {
            remove_tmp();
            return Err("the file was replaced while it was being saved".to_owned());
        }
        Err(e) => {
            remove_tmp();
            return Err(e);
        }
    }
    // The handle of that look is gone: `ReplaceFileW` opens the file itself.
    match replace_file(&target_path, &tmp) {
        Ok(()) => Ok(Saved::Saved { version: version(text.as_bytes()) }),
        // Nothing was changed.
        Err(e) if e.raw_os_error() == Some(1175) => {
            remove_tmp();
            Err(io_sentence(&e, "the file could not be replaced; another program may have it open"))
        }
        // The old file is gone and the new text is in the temporary file: it is put under the old name.
        Err(e) if matches!(e.raw_os_error(), Some(1176 | 1177)) => match rename_path(&tmp, &target_path) {
            Ok(()) => Ok(Saved::Saved { version: version(text.as_bytes()) }),
            Err(_) => Err(format!("the file could not be replaced; the new text is in {}", tmp.display())),
        },
        Err(e) => {
            remove_tmp();
            Err(io_sentence(&e, "the file could not be replaced"))
        }
    }
}

/// A rename by path that never replaces what is at `to`.
fn rename_path(from: &Path, to: &Path) -> std::io::Result<()> {
    rename_handle(&open_raw(from, DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE, FILE_SHARE_READ | FILE_SHARE_WRITE)?, to, false)
}

// ---- create, rename, move ----

pub fn create_in(root: &Path, rel_text: &str, kind: &str, confirm: bool) -> Result<Changed, String> {
    let _one_at_a_time = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    local(root)?;
    let rel = rel_path(rel_text).map_err(str::to_owned)?;
    let name = rel.file_name().ok_or_else(|| "that is the project folder".to_owned())?.to_string_lossy().into_owned();
    new_name(&name).map_err(str::to_owned)?;
    if kind != "file" && kind != "dir" {
        return Err("unknown kind".to_owned());
    }
    let p = place(root, rel_text, Access::Look)?;
    if let Some(why) = check_change(&p.rel, confirm)? {
        return Ok(Changed::NeedsConfirm(why));
    }
    if p.entry.is_some() {
        return Err("that name already exists".to_owned());
    }
    let path = p.dir.path.join(&p.name);
    // Both refuse a name that exists, a link in that place included, so nothing is made behind a link.
    if kind == "file" {
        OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| io_sentence(&e, "the file could not be made"))?;
    } else {
        std::fs::create_dir(&path).map_err(|e| io_sentence(&e, "the folder could not be made"))?;
    }
    Ok(Changed::Done)
}

pub fn rename_in(root: &Path, rel_text: &str, to_text: &str, confirm: bool) -> Result<Changed, String> {
    let _one_at_a_time = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    local(root)?;
    let from_rel = rel_path(rel_text).map_err(str::to_owned)?;
    let to_rel = rel_path(to_text).map_err(str::to_owned)?;
    let to_name = to_rel.file_name().ok_or_else(|| "that is the project folder".to_owned())?.to_string_lossy().into_owned();
    new_name(&to_name).map_err(str::to_owned)?;
    if from_rel == to_rel {
        return Err("it is already there".to_owned());
    }
    let p = place(root, rel_text, Access::Move)?;
    let q = place(root, to_text, Access::Look).map_err(|e| if e == "the folder does not exist" || e == "no such file or folder" { "the destination folder does not exist".to_owned() } else { e })?;
    // The same entry under another spelling (another case, a short name): not a name that is taken, but a rename to the spelling sent.
    let q = if p.entry.is_some() && q.entry.is_some() && p.rel == q.rel { Place { rel: q.dir.rel.join(&to_name), name: OsString::from(&to_name), entry: None, dir: q.dir } } else { q };
    move_place(&p, &q, confirm)
}

/// Moves the entry `p` to the not yet existing `q`: both real places go through the guard, nothing is overwritten and nothing is copied.
fn move_place(p: &Place, q: &Place, confirm: bool) -> Result<Changed, String> {
    let from_ask = check_change(&p.rel, confirm)?;
    let to_ask = check_change(&q.rel, confirm)?;
    let Some(entry) = p.entry.as_ref() else { return Err("no such file or folder".to_owned()) };
    move_check(&p.rel, entry.kind == Kind::Dir, &q.rel, q.entry.is_some())?;
    // Asked last: a move that cannot be done is refused before the person is asked about it.
    if let Some(why) = from_ask.or(to_ask) {
        return Ok(Changed::NeedsConfirm(why));
    }
    rename_handle(&entry.file, &q.dir.path.join(&q.name), false).map_err(|e| io_sentence(&e, "it could not be moved"))?;
    Ok(Changed::Done)
}

// ---- delete, restore ----

/// Removes the record `.from/<id>` of the trash entry `name`; a record that stays is harmless.
fn drop_record(trash: &Dir, name: &str) {
    let Some(id) = trash_id(name) else { return };
    if let Ok(froms) = sub_folder(trash, ".from", false) {
        let _ = std::fs::remove_file(froms.path.join(id));
    }
}

pub fn delete_in(root: &Path, rel_text: &str, base: Option<&str>, confirm: bool) -> Result<Changed, String> {
    let _one_at_a_time = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    local(root)?;
    let p = place(root, rel_text, Access::Move)?;
    let Some(entry) = p.entry.as_ref() else { return Err("no such file or folder".to_owned()) };
    if let Some(why) = check_change(&p.rel, confirm)? {
        return Ok(Changed::NeedsConfirm(why));
    }
    let how = delete_how(&rel_path(rel_text).map_err(str::to_owned)?, &p.rel, entry.kind == Kind::File, entry.readonly)?;
    if let (Some(base), Kind::File) = (base, entry.kind) {
        if version(&read_capped(&entry.file)?) != base {
            return Err("the file changed since it was opened, so it was not deleted".to_owned());
        }
    }
    if how == DeleteHow::ToTrash {
        return trash_entry(root, &p, entry);
    }
    // A folder's read-only attribute only advises (desktop.ini folders carry it); a file's was checked by `delete_how`.
    dispose(&entry.file, entry.kind == Kind::Dir).map_err(|e| io_sentence(&e, "it could not be deleted"))?;
    // The record of where it came from goes with it (kept out of a leftover pile).
    if p.rel.parent() == Some(Path::new(TRASH)) {
        drop_record(&p.dir, &p.name.to_string_lossy());
    }
    Ok(Changed::Done)
}

/// Moves the entry into the trash: the place it came from is written down first (`.from/<id>`), then it is renamed to `<id>-<name>`.
fn trash_entry(root: &Path, p: &Place, entry: &Opened) -> Result<Changed, String> {
    let from = slash(&p.rel).ok_or_else(|| "the place cannot be recorded, so it cannot go to the trash".to_owned())?;
    let home = pin_dir(root, root)?;
    let trash = sub_folder(&home, TRASH, true)?;
    let froms = sub_folder(&trash, ".from", true)?;
    // A `.gitignore` that ignores everything, itself included: git status stays clean and `.git` is not touched. One that is already
    // there was made or changed by the person and is left alone.
    if let Ok(mut f) = OpenOptions::new().write(true).create_new(true).open(trash.path.join(".gitignore")) {
        let _ = f.write_all(b"*\n");
    }
    let mut made = None;
    for _ in 0..3 {
        let id = format!("{}-{}", crate::now_ms(), crate::random_hex(4));
        if let Ok(f) = OpenOptions::new().write(true).create_new(true).open(froms.path.join(&id)) {
            made = Some((f, id));
            break;
        }
    }
    let (mut file, id) = made.ok_or_else(|| "the trash cannot be written to".to_owned())?;
    let record = froms.path.join(&id);
    if file.write_all(from.as_bytes()).and_then(|()| file.sync_all()).is_err() {
        drop(file);
        let _ = std::fs::remove_file(&record);
        return Err("the trash cannot be written to".to_owned());
    }
    drop(file);
    let entry_name = trash_entry_name(&id, &p.name.to_string_lossy());
    if let Err(e) = rename_handle(&entry.file, &trash.path.join(&entry_name), false) {
        let _ = std::fs::remove_file(&record);
        return Err(io_sentence(&e, "it could not go to the trash").replace("cannot be moved there", "cannot go to the trash").replace("(nothing was copied)", "(nothing was deleted)"));
    }
    Ok(Changed::Moved(format!("{TRASH}/{entry_name}")))
}

/// The text of the record `id` in the folder `froms` (up to a limit): a plain file, opened as itself, in a folder that is not a link.
pub(super) fn read_record(froms: &Path, id: &str) -> Option<Vec<u8>> {
    if !std::fs::symlink_metadata(froms).ok()?.is_dir() {
        return None;
    }
    let file = open_raw(&froms.join(id), FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).ok()?;
    if kind_of(&file.metadata().ok()?) != Kind::File {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// The place a trash entry came from, as `trash_entry` wrote it down (`None` when the record is missing or not usable).
fn recorded_from(froms: &Dir, id: &str) -> Option<String> {
    read_record(&froms.path, id).and_then(parse_record)
}

pub fn restore_in(root: &Path, rel_text: &str, confirm: bool) -> Result<Changed, String> {
    restore_in_to(root, rel_text, confirm).map(|(changed, _)| changed)
}

/// Brings an entry that is directly in the trash back to where it came from, and tells where it would go when a confirmation is asked.
pub fn restore_in_to(root: &Path, rel_text: &str, confirm: bool) -> Result<(Changed, Option<String>), String> {
    let _one_at_a_time = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    local(root)?;
    let p = place(root, rel_text, Access::Move)?;
    if p.entry.is_none() {
        return Err("no such file or folder".to_owned());
    }
    let name = p.name.to_string_lossy().into_owned();
    let id = trash_id(&name).filter(|_| p.rel.parent() == Some(Path::new(TRASH))).ok_or_else(|| format!("only an entry directly in {TRASH} can be restored"))?.to_owned();
    let unknown = || "where it came from is not known; move it out with rename".to_owned();
    let froms = sub_folder(&p.dir, ".from", false).map_err(|_| unknown())?;
    let from = recorded_from(&froms, &id).ok_or_else(unknown)?;
    let q = place(root, &from, Access::Look).map_err(|e| if e == "the folder does not exist" || e == "no such file or folder" { "the folder it came from no longer exists".to_owned() } else { e })?;
    if q.entry.is_some() {
        return Err("something has that name now; move it out of the trash with rename".to_owned());
    }
    match move_place(&p, &q, confirm)? {
        Changed::Done => {
            let _ = std::fs::remove_file(froms.path.join(&id));
            Ok((Changed::Moved(from), None))
        }
        other => Ok((other, Some(from))),
    }
}

// ---- deleting from the trash for good ----

fn over(mode: Walk, tally: &mut Tally, why: String) -> Result<bool, String> {
    if mode == Walk::Count {
        Err(why)
    } else {
        tally.left += 1;
        Ok(false)
    }
}

/// The names in a folder, all collected before anything is changed (`cap` and one more at most).
fn read_names(path: &Path, cap: usize) -> Result<Vec<OsString>, String> {
    let mut names = Vec::new();
    for item in std::fs::read_dir(path).map_err(|_| "a folder cannot be read".to_owned())? {
        names.push(item.map_err(|_| "a folder cannot be read".to_owned())?.file_name());
        if names.len() > cap {
            break;
        }
    }
    Ok(names)
}

/// One entry of a folder, counted or removed: a link or a file is deleted (a link as a link, never followed), a folder is gone into
/// through its own handle and removed when it is empty. What cannot be removed (in use, refused) is counted as left. A read-only
/// file is deleted all the same, as it is on Linux, and its attribute is not changed.
fn purge_one(dir: &Path, name: &OsStr, depth: usize, mode: Walk, tally: &mut Tally) -> Result<bool, String> {
    if tally.items >= MAX_PURGE {
        return over(mode, tally, format!("more than {MAX_PURGE} entries"));
    }
    let path = dir.join(name);
    let Ok(file) = open_raw(&path, DELETE | FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY | SYNCHRONIZE, FILE_SHARE_READ | FILE_SHARE_WRITE) else {
        tally.left += 1;
        return Ok(false);
    };
    // What was opened is in this folder, under that name.
    let Ok(meta) = file.metadata() else {
        tally.left += 1;
        return Ok(false);
    };
    let real = final_path(&file).ok().filter(|real| real.parent() == Some(dir));
    let Some(real) = real else {
        tally.left += 1;
        return Ok(false);
    };
    tally.items += 1;
    let kind = kind_of(&meta);
    if kind != Kind::Dir {
        if kind == Kind::File {
            tally.bytes += meta.len();
        }
        if mode == Walk::Delete {
            if dispose(&file, true).is_ok() {
                tally.removed += 1;
                return Ok(true);
            }
            tally.left += 1;
        }
        return Ok(false);
    }
    if depth >= MAX_DEPTH {
        return over(mode, tally, format!("folders deeper than {MAX_DEPTH}"));
    }
    let names = match read_names(&real, MAX_PURGE.saturating_sub(tally.items) + 1) {
        Ok(names) => names,
        // While deleting, a folder that cannot be read is left (and counted) like any other; the count refuses it before.
        Err(_) if mode == Walk::Delete => {
            tally.left += 1;
            return Ok(false);
        }
        Err(e) => return Err(e),
    };
    if names.len() > MAX_PURGE.saturating_sub(tally.items) {
        return over(mode, tally, format!("more than {MAX_PURGE} entries"));
    }
    for inner in names {
        purge_one(&real, &inner, depth + 1, mode, tally)?;
    }
    if mode == Walk::Delete {
        if dispose(&file, true).is_ok() {
            tally.removed += 1;
            return Ok(true);
        }
        tally.left += 1;
    }
    Ok(false)
}

fn open_trash(root: &Path) -> Result<Dir, String> {
    sub_folder(&pin_dir(root, root)?, TRASH, false).map_err(|_| "there is no trash".to_owned())
}

pub fn purge_in(root: &Path, rel_text: &str, confirm: bool) -> Result<Purged, String> {
    let _one_at_a_time = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    local(root)?;
    let rel = rel_path(rel_text).map_err(str::to_owned)?;
    if rel.parent() != Some(Path::new(TRASH)) {
        return Err(format!("only an entry directly in {TRASH} can be deleted for good"));
    }
    let p = place(root, rel_text, Access::Look)?;
    // The same path as it really is: a link that leads into the trash is not a way to delete for good under other words.
    purge_name_ok(&p.rel, &p.name.to_string_lossy(), p.entry.is_some())?;
    let trash = open_trash(root)?;
    let mut count = Tally::default();
    purge_one(&trash.path, &p.name, 0, Walk::Count, &mut count)?;
    if !confirm {
        return Ok(Purged::Summary { entries: 1, items: count.items, bytes: count.bytes, left: count.left });
    }
    let mut done = Tally::default();
    if purge_one(&trash.path, &p.name, 0, Walk::Delete, &mut done)? {
        drop_record(&trash, &p.name.to_string_lossy());
    }
    Ok(Purged::Done { removed: done.removed, left: done.left })
}

pub fn empty_trash_in(root: &Path, rel_text: &str, confirm: bool) -> Result<Purged, String> {
    let _one_at_a_time = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    local(root)?;
    if rel_path(rel_text).map_err(str::to_owned)? != Path::new(TRASH) {
        return Err(format!("only {TRASH} can be emptied"));
    }
    let trash = open_trash(root)?;
    let names: Vec<OsString> = read_names(&trash.path, MAX_PURGE + 2)?.into_iter().filter(|n| n != ".from" && n != ".gitignore").collect();
    let mut count = Tally::default();
    for name in &names {
        purge_one(&trash.path, name, 0, Walk::Count, &mut count)?;
    }
    if !confirm {
        if names.len() > MAX_PURGE {
            return Err(format!("more than {MAX_PURGE} entries in the trash: delete the biggest in a file manager or a shell"));
        }
        return Ok(Purged::Summary { entries: names.len(), items: count.items, bytes: count.bytes, left: count.left });
    }
    let mut done = Tally::default();
    for name in &names {
        if purge_one(&trash.path, name, 0, Walk::Delete, &mut done)? {
            drop_record(&trash, &name.to_string_lossy());
        }
    }
    // A record whose entry is gone (deleted for good or moved out some other way) is no use to anybody.
    let kept: Vec<String> = read_names(&trash.path, 1_000_000).unwrap_or_default().iter().filter_map(|n| trash_id(&n.to_string_lossy()).map(str::to_owned)).collect();
    if let Ok(froms) = sub_folder(&trash, ".from", false) {
        for record in read_names(&froms.path, 1_000_000).unwrap_or_default() {
            if !kept.iter().any(|id| record == id.as_str()) {
                let _ = std::fs::remove_file(froms.path.join(&record));
            }
        }
    }
    Ok(Purged::Done { removed: done.removed, left: done.left })
}

#[cfg(test)]
mod tests {
    //! For a Windows machine (`cargo test` there): nothing here has run yet.
    use super::*;

    fn project() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        (dir, root)
    }

    fn base_of(root: &Path, rel: &str) -> String {
        super::super::read_text_in(root, rel).unwrap().version
    }

    #[test]
    fn a_file_is_saved_made_renamed_deleted_to_the_trash_and_restored() {
        let (_keep, root) = project();
        assert_eq!(create_in(&root, "a.txt", "file", false), Ok(Changed::Done));
        assert!(create_in(&root, "a.txt", "file", false).is_err(), "an existing name is not made again");
        let base = base_of(&root, "a.txt");
        assert!(matches!(write_text_in(&root, "a.txt", "one\r\ntwo", &base, false), Ok(Saved::Saved { .. })));
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "one\r\ntwo");
        // A save from a version that is out of date is a conflict, not an overwrite.
        assert!(matches!(write_text_in(&root, "a.txt", "late", &base, false), Ok(Saved::Conflict { .. })));
        assert_eq!(create_in(&root, "sub", "dir", false), Ok(Changed::Done));
        assert_eq!(rename_in(&root, "a.txt", "sub/b.txt", false), Ok(Changed::Done));
        assert!(rename_in(&root, "sub/b.txt", "sub", false).is_err(), "nothing is overwritten");
        let trashed = match delete_in(&root, "sub/b.txt", None, false) {
            Ok(Changed::Moved(to)) => to,
            other => panic!("{other:?}"),
        };
        assert!(!root.join("sub/b.txt").exists() && root.join(&trashed).exists());
        assert_eq!(restore_in(&root, &trashed, false), Ok(Changed::Moved("sub/b.txt".to_owned())));
        assert_eq!(std::fs::read_to_string(root.join("sub/b.txt")).unwrap(), "one\r\ntwo");
    }

    #[test]
    fn a_junction_in_the_trash_is_deleted_as_a_link_and_what_it_points_to_stays() {
        let (_keep, root) = project();
        let outside = root.parent().unwrap().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep.txt"), "mine").unwrap();
        create_in(&root, "x", "file", false).unwrap();
        let trashed = match delete_in(&root, "x", None, false) {
            Ok(Changed::Moved(to)) => to,
            other => panic!("{other:?}"),
        };
        // A junction made inside the trash (no privilege needed).
        let junction = root.join(TRASH).join("1-aaaaaaaa-link");
        assert!(std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&junction).arg(&outside).output().unwrap().status.success());
        let purged = purge_in(&root, &format!("{TRASH}/1-aaaaaaaa-link"), true).unwrap();
        assert_eq!(purged, Purged::Done { removed: 1, left: 0 });
        assert_eq!(std::fs::read_to_string(outside.join("keep.txt")).unwrap(), "mine");
        assert!(matches!(empty_trash_in(&root, TRASH, true), Ok(Purged::Done { .. })));
        assert!(!root.join(&trashed).exists());
    }

    #[test]
    fn a_read_only_file_is_not_saved_and_a_guarded_folder_is_refused_in_any_case() {
        let (_keep, root) = project();
        create_in(&root, "r.txt", "file", false).unwrap();
        let base = base_of(&root, "r.txt");
        let mut perms = std::fs::metadata(root.join("r.txt")).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(root.join("r.txt"), perms).unwrap();
        assert!(write_text_in(&root, "r.txt", "x", &base, false).is_err());
        std::fs::create_dir(root.join(".git")).unwrap();
        assert!(create_in(&root, ".GIT/x", "file", false).is_err());
    }
}
