//! File system calls (spec `linux-abi`: opening and closing files, file
//! input and output, names and metadata), run against the cell's
//! [`rhizo_fs::Fs`]. Descriptors 0–2 start on the console.

use super::*;
use alloc::vec::Vec;
use rhizo_fs::{DirEntry, Error, Kind, OpenFlags, Stat, Whence};

/// `AT_FDCWD`, as the low 32 bits of the argument.
const AT_FDCWD: i32 = -100;
const AT_REMOVEDIR: u64 = 0x200;
const AT_EMPTY_PATH: u64 = 0x1000;
const O_ACCMODE: u64 = 3;
const O_CREAT: u64 = 0x40;
const O_EXCL: u64 = 0x80;
const O_TRUNC: u64 = 0x200;
const O_APPEND: u64 = 0x400;
const O_CLOEXEC: u64 = 0x8_0000;
const PATH_MAX: usize = 4096;
const ERANGE: i64 = 34;
const ENOSPC: i64 = 28;
/// Kernel heap a new name and inode may take (an estimate, on the high
/// side): asked of [`Services::can_store`] before one is made, so names
/// count against memory like file data (spec `isolation`).
const NAME_BYTES: u64 = 512;
/// A file block: the most a write piece adds to what the files hold.
const FILE_BLOCK: u64 = rhizo_fs::BLOCK as u64;
/// Largest `getdents64` answer built at once (a caller asking for more gets
/// fewer entries per call, as Linux allows).
const DIRENTS_MAX: u64 = 64 << 10;

pub(crate) fn err(e: Error) -> i64 {
    -e.errno()
}

impl Arch {
    const fn o_directory(self) -> u64 {
        match self {
            Arch::Aarch64 => 0x4000,
            Arch::X86_64 => 0x1_0000,
        }
    }
}

/// The directory a `*at` call starts from (`None`: the working directory).
fn dirfd(raw: u64) -> Result<Option<u32>, i64> {
    let fd = raw as i32;
    match fd {
        AT_FDCWD => Ok(None),
        fd if fd < 0 => Err(-EBADF),
        fd => Ok(Some(fd as u32)),
    }
}

/// Copies the NUL-terminated path at `addr` (at most `PATH_MAX` bytes with
/// its NUL) without reading past the page that holds the NUL.
fn path<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
) -> Result<Vec<u8>, i64> {
    let mut out = Vec::new();
    let mut buf = [0u8; 256];
    loop {
        let at = addr.wrapping_add(out.len() as u64);
        let to_page = (FRAME_SIZE - at % FRAME_SIZE) as usize;
        let n = to_page.min(buf.len()).min(PATH_MAX - out.len());
        if n == 0 {
            return Err(-(Error::NameTooLong.errno()));
        }
        cx.read(at, &mut buf[..n]).map_err(|_| -EFAULT)?;
        if let Some(end) = buf[..n].iter().position(|&b| b == 0) {
            out.extend_from_slice(&buf[..end]);
            return Ok(out);
        }
        out.extend_from_slice(&buf[..n]);
    }
}

/// Bytes from `addr` to the end of its page, at most `left`.
fn piece(addr: u64, left: u64) -> usize {
    (FRAME_SIZE - addr % FRAME_SIZE).min(left) as usize
}

/// `read`/`pread64` (with `offset`) and the reading half of `readv`.
pub(crate) fn read<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    addr: u64,
    len: u64,
    offset: Option<u64>,
) -> i64 {
    let fd = fd as u32;
    match cx.fs.kind(fd) {
        Err(e) => return err(e),
        Ok(Kind::File | Kind::Dir) => {}
        Ok(_) if offset.is_some() => return -ESPIPE,
        Ok(Kind::Console) => return 0, // standard input is empty
        Ok(Kind::Pipe) => return poll::read_pipe(cx, fd, addr, len),
        Ok(Kind::Event) => return poll::read_event(cx, fd, addr, len),
        Ok(Kind::Socket) => return sockets::recv(cx, fd, addr, len, 0),
        Ok(Kind::Epoll) => return -EINVAL,
    }
    let start = match cx.fs.read_position(fd, offset) {
        Ok(at) => at,
        Err(e) => return err(e),
    };
    // One copy per piece, file block to cell memory; a piece stays within
    // one cell page and one block, so a fault copies nothing of it.
    let mut done = 0u64;
    while done < len {
        let at = addr.wrapping_add(done);
        let n = piece(at, len - done);
        let (pt, mem, pager) = (&cx.pt, &mut *cx.mem, &mut *cx.pager);
        let got = cx.fs.read_with(fd, start + done, n, |src| {
            memory::write(pt, mem, pager, at, src).is_ok()
        });
        match got {
            Ok(Some(0)) => break,
            Ok(Some(got)) => done += got as u64,
            Ok(None) if done == 0 => return -EFAULT,
            Ok(None) => break,
            Err(e) if done == 0 => return err(e),
            Err(_) => break,
        }
    }
    if offset.is_none() {
        let _ = cx.fs.set_offset(fd, start + done);
    }
    done as i64
}

/// `write`/`pwrite64` (with `offset`) to the console or a file.
pub(crate) fn write<F: EntryFormat, M: PhysicalMemory, T, A: Frames, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    addr: u64,
    len: u64,
    offset: Option<u64>,
) -> i64 {
    let fd = fd as u32;
    match cx.fs.kind(fd) {
        Err(e) => return err(e),
        Ok(Kind::File | Kind::Dir) => {}
        Ok(_) if offset.is_some() => return -ESPIPE,
        Ok(Kind::Console) => return console(cx, addr, len),
        Ok(Kind::Pipe) => return poll::write_pipe(cx, fd, addr, len),
        Ok(Kind::Event) => return poll::write_event(cx, fd, addr, len),
        Ok(Kind::Socket) => return sockets::send(cx, fd, addr, len, 0),
        Ok(Kind::Epoll) => return -EINVAL,
    }
    let start = match cx.fs.write_position(fd, offset) {
        Ok(at) => at,
        Err(e) => return err(e),
    };
    // One copy per piece, cell memory to file block (see `read`).
    let mut done = 0u64;
    while done < len {
        let at = addr.wrapping_add(done);
        let n = piece(at, len - done);
        // A piece takes at most one new block, counted against the cell's
        // memory (design D4 of `add-cell-hierarchy`).
        let most = match cx.fs.holds_block(fd, start + done) {
            true => 0,
            false => FILE_BLOCK,
        };
        let before = cx.held();
        let room = match most {
            0 => cx.services.can_store(n as u64),
            _ => cx.store(most),
        };
        if !room {
            if done == 0 {
                return -ENOSPC;
            }
            break;
        }
        let (pt, mem, pager) = (&cx.pt, &mut *cx.mem, &mut *cx.pager);
        let wrote = cx.fs.write_with(fd, start + done, n, |dst| {
            memory::read(pt, mem, pager, at, dst).is_ok()
        });
        cx.settle(most, before);
        match wrote {
            Ok(Some(n)) => done += n as u64,
            Ok(None) if done == 0 => return -EFAULT,
            Ok(None) => break,
            Err(e) if done == 0 => return err(e),
            Err(_) => break,
        }
    }
    if offset.is_none() {
        let _ = cx.fs.set_offset(fd, start + done);
    }
    done as i64
}

/// Console output in small steps, so a fault reports what was written.
fn console<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    base: u64,
    len: u64,
) -> i64 {
    let mut off = 0;
    while off < len {
        let n = (len - off).min(CHUNK as u64) as usize;
        let mut buf = [0u8; CHUNK];
        if cx.read(base.wrapping_add(off), &mut buf[..n]).is_err() {
            return if off == 0 { -EFAULT } else { off as i64 };
        }
        cx.services.write_bytes(&buf[..n]);
        off += n as u64;
    }
    len as i64
}

pub(crate) fn openat<F: EntryFormat, M: PhysicalMemory, T, A: Frames, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    arch: Arch,
    cell: &Cell,
    dir: u64,
    addr: u64,
    flags: u64,
) -> i64 {
    let mut run = || -> Result<i64, i64> {
        let dir = dirfd(dir)?;
        let name = path(cx, addr)?;
        let access = flags & O_ACCMODE;
        if access == 3 {
            return Err(-EINVAL);
        }
        if name == GENERATION {
            return generation(cx, cell, access, flags);
        }
        let how = OpenFlags {
            read: access != 1,
            write: access != 0,
            create: flags & O_CREAT != 0,
            exclusive: flags & O_EXCL != 0,
            truncate: flags & O_TRUNC != 0,
            append: flags & O_APPEND != 0,
            directory: flags & arch.o_directory() != 0,
        };
        let most = if how.create { NAME_BYTES } else { 0 };
        let before = cx.held();
        if most != 0 && !cx.store(most) {
            return Err(-ENOSPC);
        }
        let kept = (flags & (O_ACCMODE | O_APPEND)) as u32;
        let opened = cx.fs.open(dir, &name, how, kept);
        cx.settle(most, before);
        let fd = opened.map_err(err)?;
        if flags & O_CLOEXEC != 0 {
            let _ = cx.fs.cloexec(fd, Some(true));
        }
        Ok(fd as i64)
    };
    run().unwrap_or_else(|e| e)
}

/// Where a cell reads its copy generation (design D22 of
/// `add-cell-hierarchy`), by this absolute path only.
const GENERATION: &[u8] = b"/run/rhizo/generation";

/// Opens the cell's copy generation: 8 bytes, little endian, as they are
/// when it is opened, in a file of its own that no name refers to. Read
/// only (`EROFS`).
fn generation<F, M, T, A: Frames, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    cell: &Cell,
    access: u64,
    flags: u64,
) -> Result<i64, i64> {
    const EROFS: i64 = 30;
    if access != 0 || flags & (O_CREAT | O_TRUNC) != 0 {
        return Err(-EROFS);
    }
    let before = cx.held();
    if !cx.store(FILE_BLOCK) {
        return Err(-ENOSPC);
    }
    let opened = cx.fs.open_data(&cell.generation.to_le_bytes());
    cx.settle(FILE_BLOCK, before);
    let fd = opened.map_err(err)?;
    if flags & O_CLOEXEC != 0 {
        let _ = cx.fs.cloexec(fd, Some(true));
    }
    Ok(fd.into())
}

pub(crate) fn close<F, M, T, A: Frames, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
) -> i64 {
    let before = cx.held();
    let closed = cx.fs.close(fd as u32);
    cx.settle(0, before);
    match closed {
        Ok(()) => 0,
        Err(e) => err(e),
    }
}

pub(crate) fn lseek<F, M, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    offset: u64,
    whence: u64,
) -> i64 {
    let whence = match whence {
        0 => Whence::Set,
        1 => Whence::Current,
        2 => Whence::End,
        _ => return -EINVAL,
    };
    match cx.fs.kind(fd as u32) {
        Err(e) => err(e),
        Ok(Kind::File | Kind::Dir) => match cx.fs.seek(fd as u32, offset as i64, whence) {
            Ok(at) => at as i64,
            Err(e) => err(e),
        },
        Ok(_) => -ESPIPE,
    }
}

pub(crate) fn ftruncate<F, M, T, A: Frames, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    len: u64,
) -> i64 {
    match cx.fs.kind(fd as u32) {
        Err(e) => err(e),
        Ok(Kind::File) if (len as i64) >= 0 => {
            let before = cx.held();
            let cut = cx.fs.truncate(fd as u32, len);
            cx.settle(0, before);
            match cut {
                Ok(()) => 0,
                Err(e) => err(e),
            }
        }
        Ok(_) => -EINVAL,
    }
}

/// `fsync`/`fdatasync`: files live in memory; only the descriptor is checked
/// (pipes, sockets and other objects cannot be synced).
pub(crate) fn fsync<F, M, T, A, S>(cx: &mut Context<'_, F, M, T, A, S>, fd: u64) -> i64 {
    match cx.fs.kind(fd as u32) {
        Ok(Kind::File | Kind::Dir | Kind::Console) => 0,
        Ok(_) => -EINVAL,
        Err(e) => err(e),
    }
}

/// `dup` (`min` 0), `fcntl(F_DUPFD)` (`min`), `dup2`/`dup3` (`exact`).
pub(crate) fn dup<F, M, T, A: Frames, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    min: u64,
    exact: bool,
    cloexec: bool,
) -> i64 {
    if min >= rhizo_fs::FD_MAX as u64 {
        return if exact { -EBADF } else { -EINVAL };
    }
    // `dup2` may close what `min` held.
    let before = cx.held();
    let duped = cx.fs.dup(fd as u32, min as u32, exact, cloexec);
    cx.settle(0, before);
    match duped {
        Ok(new) => new as i64,
        Err(e) => err(e),
    }
}

pub(crate) fn dup3<F, M, T, A: Frames, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    old: u64,
    new: u64,
    flags: u64,
) -> i64 {
    if old == new || flags & !O_CLOEXEC != 0 {
        return -EINVAL;
    }
    dup(cx, old, new, true, flags != 0)
}

/// `ioctl`: the generic descriptor requests (the same numbers on both
/// architectures) that CPython's sockets and `os.set_inheritable` make:
/// `FIONBIO` (non-blocking mode, as `F_SETFL` with `O_NONBLOCK`), `FIOCLEX`
/// and `FIONCLEX` (close-on-exec). There are no terminals or devices: any
/// other request is `ENOTTY`.
pub(crate) fn ioctl<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    request: u64,
    arg: u64,
) -> i64 {
    const FIONBIO: u64 = 0x5421;
    const FIONCLEX: u64 = 0x5450;
    const FIOCLEX: u64 = 0x5451;
    let fd32 = fd as u32;
    let done = |r: Result<(), Error>| r.map_or_else(err, |()| 0);
    match request {
        FIONBIO => {
            let mut on = [0u8; 4];
            if cx.read(arg, &mut on).is_err() {
                return -EFAULT;
            }
            let flags = match cx.fs.status_flags(fd32) {
                Ok(f) => f,
                Err(e) => return err(e),
            };
            let flags = match u32::from_le_bytes(on) {
                0 => flags & !rhizo_fs::O_NONBLOCK_FLAG,
                _ => flags | rhizo_fs::O_NONBLOCK_FLAG,
            };
            done(cx.fs.set_status_flags(fd32, flags))
        }
        FIOCLEX | FIONCLEX => done(cx.fs.cloexec(fd32, Some(request == FIOCLEX)).map(|_| ())),
        _ => -ENOTTY,
    }
}

pub(crate) fn fcntl<F, M, T, A: Frames, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    cmd: u64,
    arg: u64,
) -> i64 {
    const FD_CLOEXEC: u64 = 1;
    let fd32 = fd as u32;
    let done = |r: Result<i64, Error>| r.unwrap_or_else(err);
    match cmd {
        0 => dup(cx, fd, arg, false, false),                 // F_DUPFD
        1030 => dup(cx, fd, arg, false, true),               // F_DUPFD_CLOEXEC
        1 => done(cx.fs.cloexec(fd32, None).map(i64::from)), // F_GETFD
        2 => done(cx.fs.cloexec(fd32, Some(arg & FD_CLOEXEC != 0)).map(|_| 0)), // F_SETFD
        3 => done(cx.fs.status_flags(fd32).map(i64::from)),  // F_GETFL
        4 => done(cx.fs.set_status_flags(fd32, arg as u32).map(|_| 0)), // F_SETFL
        _ => match cx.fs.kind(fd32) {
            Err(e) => err(e),
            Ok(_) => -EINVAL,
        },
    }
}

/// `struct stat` in each architecture's layout; `None` is the console.
/// Inode number, mode, links and size of what `stat` describes (`None`:
/// the console).
fn stat_fields(stat: Option<Stat>) -> (u64, u64, u32, u64) {
    const S_IFIFO: u64 = 0o010_000;
    const S_IFCHR: u64 = 0o020_000;
    const S_IFDIR: u64 = 0o040_000;
    const S_IFREG: u64 = 0o100_000;
    const S_IFSOCK: u64 = 0o140_000;
    match stat {
        None => (1, S_IFCHR | 0o620, 1, 0),
        Some(s) => match s.kind {
            Kind::Dir => (s.ino, S_IFDIR | 0o755, s.links, s.size),
            Kind::Pipe => (s.ino, S_IFIFO | 0o600, s.links, s.size),
            Kind::Socket => (s.ino, S_IFSOCK | 0o777, s.links, s.size),
            Kind::Event | Kind::Epoll => (s.ino, 0o600, s.links, s.size),
            _ => (s.ino, S_IFREG | 0o644, s.links, s.size),
        },
    }
}

fn stat_bytes(arch: Arch, stat: Option<Stat>) -> ([u8; 144], usize) {
    let (ino, mode, links, size) = stat_fields(stat);
    let blocks = size.div_ceil(512);
    let mut st = [0u8; 144];
    let mut set =
        |at: usize, v: u64, n: usize| st[at..at + n].copy_from_slice(&v.to_le_bytes()[..n]);
    set(8, ino, 8);
    set(48, size, 8);
    set(64, blocks, 8);
    let len = match arch {
        // st_dev, st_ino, st_nlink (8), st_mode (4), ..., st_blksize at 56
        Arch::X86_64 => {
            set(16, u64::from(links), 8);
            set(24, mode, 4);
            set(56, 4096, 8);
            144
        }
        // st_dev, st_ino, st_mode (4) at 16, st_nlink (4), ..., st_blksize (4) at 56
        Arch::Aarch64 => {
            set(16, mode, 4);
            set(20, u64::from(links), 4);
            set(56, 4096, 4);
            128
        }
    };
    (st, len)
}

pub(crate) fn fstat<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    arch: Arch,
    fd: u64,
    addr: u64,
) -> i64 {
    match cx.fs.fstat(fd as u32) {
        Ok(stat) => {
            let (st, len) = stat_bytes(arch, stat);
            put(cx, addr, &st[..len])
        }
        Err(e) => err(e),
    }
}

/// `newfstatat`, and `stat`/`lstat` (with `AT_FDCWD`): no links exist, so
/// `AT_SYMLINK_NOFOLLOW` changes nothing.
pub(crate) fn newfstatat<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    arch: Arch,
    dir: u64,
    addr: u64,
    out: u64,
    flags: u64,
) -> i64 {
    let mut run = || -> Result<i64, i64> {
        let stat = stat_at(cx, dir, addr, flags)?;
        let (st, len) = stat_bytes(arch, stat);
        Ok(put(cx, out, &st[..len]))
    };
    run().unwrap_or_else(|e| e)
}

/// What `dir` and the path at `addr` name (`AT_EMPTY_PATH` in `flags`:
/// `dir` itself).
fn stat_at<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    dir: u64,
    addr: u64,
    flags: u64,
) -> Result<Option<Stat>, i64> {
    let dir = dirfd(dir)?;
    let name = path(cx, addr)?;
    if name.is_empty() && flags & AT_EMPTY_PATH != 0 {
        return match dir {
            Some(fd) => cx.fs.fstat(fd).map_err(err),
            None => Ok(Some(cx.fs.stat(None, b".").map_err(err)?)),
        };
    }
    Ok(Some(cx.fs.stat(dir, &name).map_err(err)?))
}

/// `statx`: the fields of `newfstatat` (`STATX_BASIC_STATS`); times are
/// zero, as there, and no attribute is known.
pub(crate) fn statx<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    a: [u64; 6],
) -> i64 {
    const STATX_BASIC_STATS: u32 = 0x7ff;
    let mut run = || -> Result<i64, i64> {
        let (ino, mode, links, size) = stat_fields(stat_at(cx, a[0], a[1], a[2])?);
        let mut stx = [0u8; 256];
        stx[..4].copy_from_slice(&STATX_BASIC_STATS.to_le_bytes());
        stx[4..8].copy_from_slice(&4096u32.to_le_bytes()); // stx_blksize
        stx[16..20].copy_from_slice(&links.to_le_bytes());
        stx[28..30].copy_from_slice(&(mode as u16).to_le_bytes());
        stx[32..40].copy_from_slice(&ino.to_le_bytes());
        stx[40..48].copy_from_slice(&size.to_le_bytes());
        stx[48..56].copy_from_slice(&size.div_ceil(512).to_le_bytes());
        Ok(put(cx, a[4], &stx))
    };
    run().unwrap_or_else(|e| e)
}

/// `faccessat`/`access`: cells run as root, so only execute permission on a
/// file (whose mode has no execute bit) is refused.
pub(crate) fn faccessat<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    dir: u64,
    addr: u64,
    mode: u64,
) -> i64 {
    const X_OK: u64 = 1;
    let mut run = || -> Result<i64, i64> {
        let dir = dirfd(dir)?;
        let name = path(cx, addr)?;
        if mode & !7 != 0 {
            return Err(-EINVAL);
        }
        let stat = cx.fs.stat(dir, &name).map_err(err)?;
        Ok(if mode & X_OK != 0 && stat.kind == Kind::File {
            -EACCES
        } else {
            0
        })
    };
    run().unwrap_or_else(|e| e)
}

pub(crate) fn getdents64<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    addr: u64,
    count: u64,
) -> i64 {
    const DT_FIFO: u8 = 1;
    const DT_CHR: u8 = 2;
    const DT_DIR: u8 = 4;
    const DT_REG: u8 = 8;
    const DT_SOCK: u8 = 12;
    let count = count.min(DIRENTS_MAX);
    let mut out = Vec::new();
    let mut next = 0u64;
    let mut full = false;
    let emit = |e: &DirEntry| {
        // d_ino, d_off, d_reclen, d_type, name, NUL, padded to 8 bytes.
        let reclen = (19 + e.name.len() + 1).next_multiple_of(8);
        if (out.len() + reclen) as u64 > count {
            full = true;
            return false;
        }
        next += 1;
        out.extend_from_slice(&e.ino.to_le_bytes());
        out.extend_from_slice(&next.to_le_bytes());
        out.extend_from_slice(&(reclen as u16).to_le_bytes());
        out.push(match e.kind {
            Kind::Dir => DT_DIR,
            Kind::File => DT_REG,
            Kind::Console => DT_CHR,
            Kind::Socket => DT_SOCK,
            Kind::Pipe => DT_FIFO,
            Kind::Event | Kind::Epoll => 0,
        });
        out.extend_from_slice(&e.name);
        out.resize(out.len() + reclen - 19 - e.name.len(), 0);
        true
    };
    match cx.fs.read_dir(fd as u32, emit) {
        Err(e) => err(e),
        Ok(0) if full => -EINVAL, // the next entry does not fit
        Ok(_) => match cx.write(addr, &out) {
            Ok(()) => out.len() as i64,
            Err(_) => -EFAULT,
        },
    }
}

pub(crate) fn mkdirat<F: EntryFormat, M: PhysicalMemory, T, A: Frames, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    dir: u64,
    addr: u64,
) -> i64 {
    let mut run = || -> Result<i64, i64> {
        let dir = dirfd(dir)?;
        let name = path(cx, addr)?;
        let before = cx.held();
        if !cx.store(NAME_BYTES) {
            return Err(-ENOSPC);
        }
        let made = cx.fs.mkdir(dir, &name);
        cx.settle(NAME_BYTES, before);
        made.map_err(err)?;
        Ok(0)
    };
    run().unwrap_or_else(|e| e)
}

pub(crate) fn unlinkat<F: EntryFormat, M: PhysicalMemory, T, A: Frames, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    dir: u64,
    addr: u64,
    flags: u64,
) -> i64 {
    let mut run = || -> Result<i64, i64> {
        if flags & !AT_REMOVEDIR != 0 {
            return Err(-EINVAL);
        }
        let dir = dirfd(dir)?;
        let name = path(cx, addr)?;
        let before = cx.held();
        let gone = cx.fs.unlink(dir, &name, flags & AT_REMOVEDIR != 0);
        cx.settle(0, before);
        gone.map_err(err)?;
        Ok(0)
    };
    run().unwrap_or_else(|e| e)
}

pub(crate) fn renameat<F: EntryFormat, M: PhysicalMemory, T, A: Frames, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    a: [u64; 6],
    flags: u64,
) -> i64 {
    let mut run = || -> Result<i64, i64> {
        if flags != 0 {
            return Err(-EINVAL);
        }
        let (from_dir, to_dir) = (dirfd(a[0])?, dirfd(a[2])?);
        let (from, to) = (path(cx, a[1])?, path(cx, a[3])?);
        // The new name may be longer than the old one.
        let before = cx.held();
        if !cx.store(NAME_BYTES) {
            return Err(-ENOSPC);
        }
        let moved = cx.fs.rename(from_dir, &from, to_dir, &to);
        cx.settle(NAME_BYTES, before);
        moved.map_err(err)?;
        Ok(0)
    };
    run().unwrap_or_else(|e| e)
}

pub(crate) fn getcwd<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
    size: u64,
) -> i64 {
    let mut cwd = cx.fs.getcwd();
    cwd.push(0);
    if (cwd.len() as u64) > size {
        return -ERANGE;
    }
    match cx.write(addr, &cwd) {
        Ok(()) => cwd.len() as i64,
        Err(_) => -EFAULT,
    }
}

pub(crate) fn chdir<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
) -> i64 {
    let mut run = || -> Result<i64, i64> {
        let name = path(cx, addr)?;
        cx.fs.chdir(&name).map_err(err)?;
        Ok(0)
    };
    run().unwrap_or_else(|e| e)
}
