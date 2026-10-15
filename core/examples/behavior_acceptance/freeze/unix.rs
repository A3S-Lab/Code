//! Descriptor-relative POSIX capture. No source path is opened through a link.

use super::*;
use std::ffi::{CStr, CString};
use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

struct Directory(File);
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    length: u64,
    links: u64,
    mode: u32,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl Stamp {
    fn of(value: &Metadata) -> Self {
        Self {
            device: value.dev(),
            inode: value.ino(),
            length: value.len(),
            links: value.nlink(),
            mode: value.mode(),
            modified: (value.mtime(), value.mtime_nsec()),
            changed: (value.ctime(), value.ctime_nsec()),
        }
    }
}
fn component(name: &str) -> Result<CString> {
    if name.is_empty() || matches!(name, "." | "..") || name.contains('/') {
        return Err(invalid("unsafe path component"));
    }
    CString::new(name).map_err(|_| invalid("NUL path component"))
}
fn opened(parent: &File, name: &str, file_limit: Option<u64>) -> Result<File> {
    let name = component(name)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // The directory descriptor and C string remain live; successful fstatat
    // initializes the complete output. NOFOLLOW inspects the entry itself.
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(FreezeError::io(std::io::Error::last_os_error()));
    }
    // Only reached after successful initialization above.
    let before = unsafe { stat.assume_init() };
    let kind = before.st_mode & libc::S_IFMT;
    match file_limit {
        Some(limit)
            if kind != libc::S_IFREG
                || before.st_nlink != 1
                || before.st_mode & 0o444 == 0
                || before.st_size < 0
                || before.st_size as u64 > limit =>
        {
            return Err(invalid(
                "source is linked, nonregular or oversized before open",
            ))
        }
        None if kind != libc::S_IFDIR => {
            return Err(invalid("path component is not a real directory"))
        }
        _ => {}
    }
    let flags = libc::O_RDONLY
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if file_limit.is_none() {
            libc::O_DIRECTORY
        } else {
            0
        };
    // O_NOFOLLOW defeats entry replacement with a symlink. O_NONBLOCK also
    // prevents a regular-to-FIFO swap between inspection and open from waiting.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(FreezeError::io(std::io::Error::last_os_error()));
    }
    // A successful openat returns one newly owned descriptor.
    let file = unsafe { File::from_raw_fd(fd) };
    let after = file.metadata().map_err(FreezeError::io)?;
    if after.dev() != before.st_dev as u64 || u128::from(after.ino()) != u128::from(before.st_ino) {
        return Err(race("path component changed before open"));
    }
    match file_limit {
        Some(limit) if !after.is_file() || after.nlink() != 1 || after.len() > limit => {
            return Err(race("source changed before regular-file read"))
        }
        None if !after.is_dir() => return Err(race("directory changed before traversal")),
        _ => {}
    }
    Ok(file)
}
impl Directory {
    fn absolute(path: &Path) -> Result<Self> {
        let text = path.to_str().ok_or_else(|| invalid("non-UTF-8 root"))?;
        if !text.starts_with('/') {
            return Err(invalid("root must be absolute"));
        }
        let mut directory = Self(
            OpenOptions::new()
                .read(true)
                .custom_flags(
                    libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
                .open("/")
                .map_err(FreezeError::io)?,
        );
        if text != "/" {
            for name in text[1..].split('/') {
                directory = Self(opened(&directory.0, name, None)?);
            }
        }
        Ok(directory)
    }
    fn identity(&self) -> Result<(u64, u64)> {
        let value = self.0.metadata().map_err(FreezeError::io)?;
        Ok((value.dev(), value.ino()))
    }
    fn parent(&self, path: &str, create: bool) -> Result<(Self, String)> {
        let mut names = path.split('/').collect::<Vec<_>>();
        let last = names
            .pop()
            .ok_or_else(|| invalid("missing filename"))?
            .to_owned();
        let mut directory = Self(self.0.try_clone().map_err(FreezeError::io)?);
        for name in names {
            if create {
                let c = component(name)?;
                // Only called beneath the fresh host-owned destination.
                if unsafe { libc::mkdirat(directory.0.as_raw_fd(), c.as_ptr(), 0o700) } != 0 {
                    let error = std::io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::EEXIST) {
                        return Err(FreezeError::io(error));
                    }
                }
            }
            directory = Self(opened(&directory.0, name, None)?);
        }
        Ok((directory, last))
    }
    fn source(&self, path: &str, limit: u64) -> Result<File> {
        let (directory, name) = self.parent(path, false)?;
        opened(&directory.0, &name, Some(limit))
    }
    fn output(&self, path: &str) -> Result<File> {
        let (directory, name) = self.parent(path, true)?;
        let name = component(&name)?;
        // O_EXCL refuses an existing entry without following it; the descriptor
        // exclusively owns a fresh regular file inside the private new root.
        let fd = unsafe {
            libc::openat(
                directory.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY
                    | libc::O_CREAT
                    | libc::O_EXCL
                    | libc::O_NOFOLLOW
                    | libc::O_NONBLOCK
                    | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(FreezeError::io(std::io::Error::last_os_error()));
        }
        // Successful openat transfers exactly one descriptor to File.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

fn bounded_copy<W: Write>(
    source: &mut File,
    output: &mut W,
    limit: u64,
) -> Result<(u64, DigestV1, Stamp)> {
    let before = Stamp::of(&source.metadata().map_err(FreezeError::io)?);
    let mut hash = Sha256::new();
    let mut count = 0_u64;
    let mut buffer = [0_u8; 32 * 1024];
    loop {
        let read_limit =
            (limit.saturating_sub(count).saturating_add(1)).min(buffer.len() as u64) as usize;
        let read = source
            .read(&mut buffer[..read_limit])
            .map_err(FreezeError::io)?;
        if read == 0 {
            break;
        }
        count = count
            .checked_add(read as u64)
            .filter(|n| *n <= limit)
            .ok_or_else(|| invalid("actual streaming byte limit exceeded"))?;
        output.write_all(&buffer[..read]).map_err(FreezeError::io)?;
        hash.update(&buffer[..read]);
    }
    if count != before.length || Stamp::of(&source.metadata().map_err(FreezeError::io)?) != before {
        return Err(race("source identity changed during byte capture"));
    }
    Ok((
        count,
        canonical_digest(format!("sha256:{:x}", hash.finalize()))?,
        before,
    ))
}

pub(super) fn capture<F: FnOnce() -> Result<()>>(
    maker: &Path,
    candidate: &Path,
    limits: &CandidateDeclarationV1,
    observe_after_copy: F,
) -> Result<(Vec<FrozenFileV1>, u64, u64)> {
    let source = Directory::absolute(maker)?;
    let parent = Directory::absolute(
        candidate
            .parent()
            .ok_or_else(|| invalid("candidate has no parent"))?,
    )?;
    if source.identity()? == parent.identity()? {
        return Err(invalid("candidate parent aliases maker root"));
    }
    let name = candidate
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| invalid("invalid candidate root name"))?;
    let cname = component(name)?;
    // A capture always creates a fresh private root; existing content is never
    // merged, overwritten or reused after a failed capture.
    if unsafe { libc::mkdirat(parent.0.as_raw_fd(), cname.as_ptr(), 0o700) } != 0 {
        return Err(FreezeError::io(std::io::Error::last_os_error()));
    }
    let output = Directory(opened(&parent.0, name, None)?);
    let (device, inode) = output.identity()?;
    let mut paths = limits.allowed_paths.clone();
    paths.sort();
    let mut entries = Vec::new();
    let mut stamps = Vec::new();
    let mut identities = BTreeSet::new();
    let mut total = 0_u64;
    for path in paths {
        let mut file = source.source(&path, limits.max_file_bytes)?;
        let metadata = file.metadata().map_err(FreezeError::io)?;
        if !identities.insert((metadata.dev(), metadata.ino())) {
            return Err(invalid("candidate paths alias one source file"));
        }
        let mut target = output.output(&path)?;
        let remaining = limits
            .max_total_bytes
            .checked_sub(total)
            .ok_or_else(|| invalid("total byte overflow"))?;
        let (byte_count, file_digest, stamp) =
            bounded_copy(&mut file, &mut target, limits.max_file_bytes.min(remaining))?;
        total = total
            .checked_add(byte_count)
            .ok_or_else(|| invalid("total byte overflow"))?;
        target.flush().map_err(FreezeError::io)?;
        target
            .set_permissions(std::fs::Permissions::from_mode(0o444))
            .map_err(FreezeError::io)?;
        target.sync_all().map_err(FreezeError::io)?;
        entries.push(FrozenFileV1 {
            path,
            byte_count,
            file_digest,
            source_mode: metadata.mode(),
        });
        stamps.push(stamp);
    }
    observe_after_copy()?;
    let current = Directory::absolute(maker)
        .map_err(|error| race(format!("maker root changed after copy: {error}")))?;
    if current.identity()? != source.identity()? {
        return Err(race("maker root was replaced"));
    }
    for (entry, expected_stamp) in entries.iter().zip(stamps) {
        let mut file = current
            .source(&entry.path, limits.max_file_bytes)
            .map_err(|error| race(format!("source entry changed after copy: {error}")))?;
        let (count, digest, actual_stamp) =
            bounded_copy(&mut file, &mut std::io::sink(), limits.max_file_bytes)?;
        if count != entry.byte_count
            || digest != entry.file_digest
            || actual_stamp != expected_stamp
        {
            return Err(race("source bytes or file identity changed after capture"));
        }
    }
    verify_tree(output, &entries, limits, true)?;
    Ok((entries, device, inode))
}

pub(super) fn revalidate(
    root: &Path,
    record: &FrozenCandidateV1,
    limits: &CandidateDeclarationV1,
) -> Result<()> {
    let directory = Directory::absolute(root).map_err(|error| {
        FreezeError::new(AcceptanceReasonV1::ArtifactMissing, error.to_string())
    })?;
    if directory.identity()? != (record.frozen_root_device, record.frozen_root_inode) {
        return Err(FreezeError::new(
            AcceptanceReasonV1::StaleCandidate,
            "frozen root identity changed",
        ));
    }
    verify_tree(directory, &record.files, limits, false)
        .map_err(|error| FreezeError::new(AcceptanceReasonV1::StaleCandidate, error.to_string()))
}

fn verify_tree(
    root: Directory,
    files: &[FrozenFileV1],
    limits: &CandidateDeclarationV1,
    seal: bool,
) -> Result<()> {
    let mut stack = vec![(String::new(), root)];
    let mut seen = BTreeSet::new();
    let mut identities = BTreeSet::new();
    while let Some((prefix, directory)) = stack.pop() {
        if seal {
            directory
                .0
                .set_permissions(std::fs::Permissions::from_mode(0o555))
                .map_err(FreezeError::io)?;
        }
        if directory.0.metadata().map_err(FreezeError::io)?.mode() & 0o7777 != 0o555 {
            return Err(invalid("frozen directory is not read-only"));
        }
        for name in directory.names(limits.max_files as usize)? {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            validate_path(&path, limits.max_path_bytes)?;
            if let Some(expected) = files.iter().find(|file| file.path == path) {
                let mut file = opened(&directory.0, &name, Some(limits.max_file_bytes))?;
                let metadata = file.metadata().map_err(FreezeError::io)?;
                if metadata.mode() & 0o7777 != 0o444
                    || !identities.insert((metadata.dev(), metadata.ino()))
                {
                    return Err(invalid("frozen file is writable or aliases another entry"));
                }
                let (count, digest, _) =
                    bounded_copy(&mut file, &mut std::io::sink(), limits.max_file_bytes)?;
                if count != expected.byte_count || digest != expected.file_digest {
                    return Err(invalid("frozen bytes changed"));
                }
                seen.insert(path);
            } else if files
                .iter()
                .any(|file| file.path.starts_with(&format!("{path}/")))
            {
                stack.push((path, Directory(opened(&directory.0, &name, None)?)));
            } else {
                return Err(invalid("unexpected frozen file or directory"));
            }
        }
    }
    if seen.len() != files.len() {
        return Err(invalid("frozen artifact is incomplete"));
    }
    Ok(())
}

struct DirectoryStream(*mut libc::DIR);
impl Drop for DirectoryStream {
    fn drop(&mut self) {
        // The successful fdopendir transferred one private fd to this DIR.
        unsafe {
            libc::closedir(self.0);
        }
    }
}
fn errno() -> *mut libc::c_int {
    #[cfg(target_os = "macos")]
    // libc exposes the calling thread's errno pointer.
    unsafe {
        libc::__error()
    }
    #[cfg(target_os = "linux")]
    // libc exposes the calling thread's errno pointer.
    unsafe {
        libc::__errno_location()
    }
}
impl Directory {
    fn names(&self, limit: usize) -> Result<Vec<String>> {
        // A separately opened directory gives readdir its own cursor. dup would
        // share the original directory offset across repeated revalidation.
        let dot = c".";
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                dot.as_ptr(),
                libc::O_RDONLY
                    | libc::O_DIRECTORY
                    | libc::O_NOFOLLOW
                    | libc::O_NONBLOCK
                    | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(FreezeError::io(std::io::Error::last_os_error()));
        }
        // fdopendir takes ownership on success only.
        let raw = unsafe { libc::fdopendir(fd) };
        if raw.is_null() {
            let error = std::io::Error::last_os_error();
            // No owner was constructed when fdopendir failed.
            unsafe {
                libc::close(fd);
            }
            return Err(FreezeError::io(error));
        }
        let stream = DirectoryStream(raw);
        let mut names = Vec::new();
        loop {
            // readdir uses errno to distinguish end-of-directory from failure.
            unsafe {
                *errno() = 0;
            }
            let entry = unsafe { libc::readdir(stream.0) };
            if entry.is_null() {
                if unsafe { *errno() } != 0 {
                    return Err(FreezeError::io(std::io::Error::last_os_error()));
                }
                break;
            }
            // readdir's live result contains a terminated d_name until its next
            // call. Copy the bytes before advancing the stream.
            let name =
                exact_entry_name(unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes())?;
            if matches!(name, "." | "..") {
                continue;
            }
            if names.len() >= limit {
                return Err(invalid("frozen directory entry limit exceeded"));
            }
            names.push(name.to_owned());
        }
        Ok(names)
    }
}

fn exact_entry_name(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes).map_err(|_| invalid("non-UTF-8 frozen entry"))
}

#[cfg(test)]
mod tests;
