use super::*;
use proptest::prelude::*;
use std::collections::BTreeMap as Map;

const RW: OpenFlags = OpenFlags {
    read: true,
    write: true,
    create: true,
    exclusive: false,
    truncate: false,
    append: false,
    directory: false,
};
const RDONLY: OpenFlags = OpenFlags {
    read: true,
    write: false,
    create: false,
    exclusive: false,
    truncate: false,
    append: false,
    directory: false,
};

fn read_all(fs: &mut Fs, path: &[u8]) -> Result<Vec<u8>> {
    let fd = fs.open(None, path, RDONLY, 0)?;
    let mut out = Vec::new();
    let mut buf = [0u8; 1000];
    loop {
        let n = fs.read(fd, &mut buf, None)?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
    }
    fs.close(fd)?;
    Ok(out)
}

fn names(fs: &mut Fs, path: &[u8]) -> Vec<Vec<u8>> {
    let dir = OpenFlags {
        directory: true,
        ..RDONLY
    };
    let fd = fs.open(None, path, dir, 0).unwrap();
    let mut out = Vec::new();
    fs.read_dir(fd, |e| {
        out.push(e.name.clone());
        true
    })
    .unwrap();
    fs.close(fd).unwrap();
    out
}

#[test]
fn console_is_open_and_lowest_fd_is_reused() {
    let mut fs = Fs::new();
    for fd in 0..3 {
        assert_eq!(fs.kind(fd), Ok(Kind::Console));
    }
    assert_eq!(fs.open(None, b"/a", RW, 2), Ok(3));
    assert_eq!(fs.open(None, b"/b", RW, 2), Ok(4));
    fs.close(3).unwrap();
    assert_eq!(fs.open(None, b"a", RW, 2), Ok(3));
    assert_eq!(fs.close(9), Err(Error::BadFd));
}

#[test]
fn write_read_seek_and_holes() {
    let mut fs = Fs::new();
    let fd = fs.open(None, b"/f", RW, 2).unwrap();
    assert_eq!(fs.write(fd, b"hello", None), Ok(5));
    assert_eq!(fs.write(fd, b"!", Some(10_000)), Ok(1));
    assert_eq!(fs.fstat(fd).unwrap().unwrap().size, 10_001);
    assert_eq!(fs.seek(fd, 0, Whence::Set), Ok(0));
    let mut buf = vec![7u8; 20_000];
    assert_eq!(fs.read(fd, &mut buf, None), Ok(10_001));
    assert_eq!(&buf[..5], b"hello");
    assert!(buf[5..10_000].iter().all(|&b| b == 0), "holes read as zero");
    assert_eq!(buf[10_000], b'!');
    assert_eq!(fs.read(fd, &mut buf, None), Ok(0));
    assert_eq!(fs.seek(fd, -1, Whence::End), Ok(10_000));
    assert_eq!(fs.seek(fd, -20_000, Whence::Current), Err(Error::Invalid));
    // A zero-length write past the end does not grow the file.
    assert_eq!(fs.write(fd, b"", Some(1 << 20)), Ok(0));
    assert_eq!(fs.stat(None, b"/f").unwrap().size, 10_001);
}

#[test]
fn truncate_zeroes_what_it_cuts() {
    let mut fs = Fs::new();
    fs.put_file(b"/f", &[1u8; 5000]).unwrap();
    let fd = fs.open(None, b"/f", RW, 2).unwrap();
    fs.truncate(fd, 4100).unwrap();
    fs.truncate(fd, 6000).unwrap();
    let data = read_all(&mut fs, b"/f").unwrap();
    assert_eq!(data.len(), 6000);
    assert!(data[..4100].iter().all(|&b| b == 1));
    assert!(data[4100..].iter().all(|&b| b == 0));
}

#[test]
fn open_flags_follow_linux() {
    let mut fs = Fs::new();
    let excl = OpenFlags {
        exclusive: true,
        ..RW
    };
    assert_eq!(fs.open(None, b"/x", excl, 2), Ok(3));
    assert_eq!(fs.open(None, b"/x", excl, 2), Err(Error::Exists));
    assert_eq!(fs.open(None, b"/nope", RDONLY, 0), Err(Error::NotFound));
    assert_eq!(fs.open(None, b"/x/y", RW, 2), Err(Error::NotDir));
    assert_eq!(fs.open(None, b"/", RW, 2), Err(Error::IsDir));
    let dir = OpenFlags {
        directory: true,
        ..RDONLY
    };
    assert_eq!(fs.open(None, b"/x", dir, 0), Err(Error::NotDir));
    let long = vec![b'a'; NAME_MAX + 1];
    assert_eq!(fs.open(None, &long, RW, 2), Err(Error::NameTooLong));
    // Write-only descriptors cannot read, read-only ones cannot write.
    let ro = fs.open(None, b"/x", RDONLY, 0).unwrap();
    assert_eq!(fs.write(ro, b"a", None), Err(Error::BadFd));
    fs.write(3, b"abc", None).unwrap();
    let app = OpenFlags { append: true, ..RW };
    let a = fs.open(None, b"/x", app, 2 | O_APPEND_FLAG).unwrap();
    fs.write(a, b"de", None).unwrap();
    let trunc = OpenFlags {
        truncate: true,
        ..RW
    };
    assert_eq!(read_all(&mut fs, b"/x").unwrap(), b"abcde");
    let t = fs.open(None, b"/x", trunc, 2).unwrap();
    fs.close(t).unwrap();
    assert_eq!(read_all(&mut fs, b"/x").unwrap(), b"");
}

#[test]
fn directories_paths_and_cwd() {
    let mut fs = Fs::new();
    fs.mkdir(None, b"/a").unwrap();
    fs.mkdir(None, b"a/b").unwrap();
    assert_eq!(fs.mkdir(None, b"/a"), Err(Error::Exists));
    assert_eq!(fs.mkdir(None, b"/q/r"), Err(Error::NotFound));
    fs.chdir(b"/a/b").unwrap();
    assert_eq!(fs.getcwd(), b"/a/b");
    fs.put_file(b"/a/b/f", b"x").unwrap();
    assert_eq!(read_all(&mut fs, b"f").unwrap(), b"x");
    assert_eq!(read_all(&mut fs, b"../b/./f").unwrap(), b"x");
    assert_eq!(fs.stat(None, b"/a").unwrap().links, 3);
    assert_eq!(fs.stat(None, b"f/").map(|_| ()), Err(Error::NotDir));
    // Relative to a directory descriptor.
    let dir = OpenFlags {
        directory: true,
        ..RDONLY
    };
    let a = fs.open(None, b"/a", dir, 0).unwrap();
    assert_eq!(fs.stat(Some(a), b"b/f").unwrap().size, 1);
    let file = fs.open(None, b"f", RDONLY, 0).unwrap();
    assert_eq!(fs.stat(Some(file), b"x").map(|_| ()), Err(Error::NotDir));
    fs.chdir(b"/..").unwrap();
    assert_eq!(fs.getcwd(), b"/");
    assert_eq!(names(&mut fs, b"/a"), [&b"."[..], b"..", b"b"]);
}

#[test]
fn unlink_rmdir_and_open_files() {
    let mut fs = Fs::new();
    fs.mkdir(None, b"/d").unwrap();
    fs.put_file(b"/d/f", b"data").unwrap();
    assert_eq!(fs.unlink(None, b"/d", true), Err(Error::NotEmpty));
    assert_eq!(fs.unlink(None, b"/d", false), Err(Error::IsDir));
    assert_eq!(fs.unlink(None, b"/d/f", true), Err(Error::NotDir));
    let fd = fs.open(None, b"/d/f", RDONLY, 0).unwrap();
    fs.unlink(None, b"/d/f", false).unwrap();
    assert_eq!(fs.stat(None, b"/d/f").map(|_| ()), Err(Error::NotFound));
    let mut buf = [0u8; 8];
    assert_eq!(fs.read(fd, &mut buf, None), Ok(4), "data lives while open");
    assert_eq!(fs.fstat(fd).unwrap().unwrap().links, 0);
    let mut copy = Vec::new();
    fs.encode(&mut copy);
    Fs::decode(&copy).expect("an open unlinked file encodes");
    fs.close(fd).unwrap();
    assert!(fs.inodes.iter().filter(|i| i.is_some()).count() == 2);
    fs.unlink(None, b"/d", true).unwrap();
    assert_eq!(fs.data_bytes(), 0);
}

#[test]
fn file_data_is_limited_per_cell() {
    let mut fs = Fs::new();
    let fd = fs.open(None, b"/f", RW, 2).unwrap();
    assert_eq!(fs.write(fd, b"x", Some(DATA_MAX)), Err(Error::TooBig));
    assert_eq!(fs.truncate(fd, DATA_MAX + 1), Err(Error::TooBig));
    let chunk = vec![1u8; 1 << 20];
    for i in 0..DATA_MAX / (1 << 20) {
        assert_eq!(fs.write(fd, &chunk, Some(i << 20)), Ok(chunk.len()));
    }
    assert_eq!(fs.data_bytes(), DATA_MAX);
    let other = fs.open(None, b"/g", RW, 2).unwrap();
    assert_eq!(fs.write(other, b"y", None), Err(Error::NoSpace));
    fs.truncate(fd, 4096).unwrap();
    assert_eq!(fs.data_bytes(), 4096);
    assert_eq!(fs.write(other, b"y", None), Ok(1));
    fs.close(fd).unwrap();
    fs.unlink(None, b"/f", false).unwrap();
    assert_eq!(fs.data_bytes(), 4096);
    let trunc = OpenFlags {
        truncate: true,
        ..RW
    };
    let g = fs.open(None, b"/g", trunc, 2).unwrap();
    fs.close(g).unwrap();
    assert_eq!(fs.data_bytes(), 0);
}

#[test]
fn rename_replaces_and_refuses_cycles() {
    let mut fs = Fs::new();
    fs.put_file(b"/a", b"A").unwrap();
    fs.put_file(b"/b", b"B").unwrap();
    fs.rename(None, b"/a", None, b"/b").unwrap();
    assert_eq!(read_all(&mut fs, b"/b").unwrap(), b"A");
    assert_eq!(fs.stat(None, b"/a").map(|_| ()), Err(Error::NotFound));
    fs.mkdir(None, b"/d").unwrap();
    fs.mkdir(None, b"/d/e").unwrap();
    assert_eq!(fs.rename(None, b"/d", None, b"/d/e/x"), Err(Error::Invalid));
    assert_eq!(fs.rename(None, b"/b", None, b"/d"), Err(Error::IsDir));
    assert_eq!(fs.rename(None, b"/d", None, b"/b"), Err(Error::NotDir));
    fs.rename(None, b"/d/e", None, b"/e").unwrap();
    fs.chdir(b"/e").unwrap();
    assert_eq!(fs.getcwd(), b"/e");
    assert_eq!(fs.stat(None, b"..").unwrap().links, 4);
}

#[test]
fn dup_shares_the_offset() {
    let mut fs = Fs::new();
    let fd = fs.open(None, b"/f", RW, 2).unwrap();
    let copy = fs.dup(fd, 0, false, false).unwrap();
    assert_eq!(copy, 4);
    fs.write(fd, b"ab", None).unwrap();
    fs.write(copy, b"cd", None).unwrap();
    assert_eq!(read_all(&mut fs, b"/f").unwrap(), b"abcd");
    assert_eq!(fs.dup(fd, 1, true, true), Ok(1), "dup2 over the console");
    assert_eq!(fs.kind(1), Ok(Kind::File));
    assert_eq!(fs.cloexec(1, None), Ok(true));
    assert_eq!(fs.dup(fd, 2000, true, false), Err(Error::BadFd));
    fs.close(fd).unwrap();
    fs.close(copy).unwrap();
    fs.write(1, b"e", None).unwrap();
    assert_eq!(read_all(&mut fs, b"/f").unwrap(), b"abcde");
    fs.set_append(1, true).unwrap();
    assert_eq!(fs.status_flags(1).unwrap() & O_APPEND_FLAG, O_APPEND_FLAG);
}

#[test]
fn forks_share_blocks_until_written() {
    let mut parent = Fs::new();
    parent.put_file(b"/big", &vec![3u8; 64 * BLOCK]).unwrap();
    let mut child = parent.clone();
    assert!(child.shares_with(&parent), "a fork copies no table");
    let fd = child.open(None, b"/big", RW, 2).unwrap();
    child.write(fd, b"x", Some(5)).unwrap();
    // The child copied the tables, the file's inode and one block only.
    assert!(!child.shares_with(&parent));
    let (Some(Some(p)), Some(Some(c))) = (parent.inodes.get(1), child.inodes.get(1)) else {
        panic!()
    };
    let (Node::File(pf), Node::File(cf)) = (&p.node, &c.node) else {
        panic!()
    };
    assert!(!Arc::ptr_eq(
        &pf.blocks[0].clone().unwrap(),
        &cf.blocks[0].clone().unwrap()
    ));
    assert!(Arc::ptr_eq(
        &pf.blocks[1].clone().unwrap(),
        &cf.blocks[1].clone().unwrap()
    ));
    assert_eq!(read_all(&mut parent, b"/big").unwrap()[5], 3);
    assert_eq!(read_all(&mut child, b"/big").unwrap()[5], b'x');
}

#[test]
fn decode_round_trips_and_refuses_corruption() {
    let mut fs = Fs::new();
    fs.mkdir(None, b"/d").unwrap();
    fs.put_file(b"/d/f", b"contents").unwrap();
    fs.chdir(b"/d").unwrap();
    let fd = fs.open(None, b"f", RW, 2).unwrap();
    fs.seek(fd, 3, Whence::Set).unwrap();
    let mut bytes = Vec::new();
    fs.encode(&mut bytes);
    let mut back = Fs::decode(&bytes).unwrap();
    let mut again = Vec::new();
    back.encode(&mut again);
    assert_eq!(bytes, again);
    let mut buf = [0u8; 5];
    assert_eq!(back.read(fd, &mut buf, None), Ok(5));
    assert_eq!(&buf, b"tents");
    assert_eq!(back.getcwd(), b"/d");
    for len in 0..bytes.len() {
        assert!(Fs::decode(&bytes[..len]).is_err(), "truncated at {len}");
    }
    let mut extra = bytes.clone();
    extra.push(0);
    assert_eq!(Fs::decode(&extra).err(), Some(DecodeError::Truncated));
    // Flipping any single byte never yields a state that breaks invariants:
    // either it is refused, or every operation below still works.
    for i in 0..bytes.len() {
        let mut bad = bytes.clone();
        bad[i] ^= 0x41;
        if let Ok(mut fs) = Fs::decode(&bad) {
            exercise(&mut fs);
        }
    }
}

/// Runs operations that would panic on an inconsistent state.
fn exercise(fs: &mut Fs) {
    let _ = fs.getcwd();
    for fd in 0..8 {
        let _ = fs.fstat(fd);
        let _ = fs.read(fd, &mut [0u8; 64], None);
        let _ = fs.read_dir(fd, |_| true);
        let _ = fs.close(fd);
    }
    let _ = fs.stat(None, b"/d/f");
    let _ = fs.unlink(None, b"/d/f", false);
    let _ = fs.unlink(None, b"/d", true);
    let _ = fs.put_file(b"/n/m", b"z");
}

#[derive(Debug, Clone)]
enum Op {
    Write {
        cell: usize,
        file: u8,
        at: u16,
        len: u16,
        byte: u8,
    },
    Truncate {
        cell: usize,
        file: u8,
        size: u16,
    },
    Unlink {
        cell: usize,
        file: u8,
    },
    Rename {
        cell: usize,
        from: u8,
        to: u8,
    },
    Fork {
        cell: usize,
    },
    Snapshot {
        cell: usize,
    },
}

fn op() -> impl Strategy<Value = Op> {
    let cell = 0usize..4;
    prop_oneof![
        4 => (cell.clone(), 0u8..4, 0u16..20_000, 0u16..6000, any::<u8>())
            .prop_map(|(cell, file, at, len, byte)| Op::Write { cell, file, at, len, byte }),
        1 => (cell.clone(), 0u8..4, 0u16..20_000).prop_map(|(cell, file, size)| Op::Truncate { cell, file, size }),
        1 => (cell.clone(), 0u8..4).prop_map(|(cell, file)| Op::Unlink { cell, file }),
        1 => (cell.clone(), 0u8..4, 0u8..4).prop_map(|(cell, from, to)| Op::Rename { cell, from, to }),
        1 => cell.clone().prop_map(|cell| Op::Fork { cell }),
        1 => cell.prop_map(|cell| Op::Snapshot { cell }),
    ]
}

fn path(file: u8) -> Vec<u8> {
    format!("/dir/f{file}").into_bytes()
}

proptest! {
    /// Several cells forked from each other, each against a model of its own
    /// files: a change in one cell is never seen by another.
    #[test]
    fn forked_cells_match_their_models(ops in prop::collection::vec(op(), 1..60)) {
        let mut root = Fs::new();
        root.mkdir(None, b"/dir").unwrap();
        let mut cells = vec![(root, Map::<u8, Vec<u8>>::new())];
        for op in ops {
            let n = cells.len();
            match op {
                Op::Write { cell, file, at, len, byte } => {
                    let (fs, model) = &mut cells[cell % n];
                    let fd = fs.open(None, &path(file), RW, 2).unwrap();
                    let data = vec![byte; len as usize];
                    prop_assert_eq!(fs.write(fd, &data, Some(at as u64)), Ok(len as usize));
                    fs.close(fd).unwrap();
                    let m = model.entry(file).or_default();
                    if len > 0 {
                        let end = at as usize + len as usize;
                        if m.len() < end {
                            m.resize(end, 0);
                        }
                        m[at as usize..end].fill(byte);
                    }
                }
                Op::Truncate { cell, file, size } => {
                    let (fs, model) = &mut cells[cell % n];
                    if let Some(m) = model.get_mut(&file) {
                        let fd = fs.open(None, &path(file), RW, 2).unwrap();
                        fs.truncate(fd, size as u64).unwrap();
                        fs.close(fd).unwrap();
                        m.resize(size as usize, 0);
                    }
                }
                Op::Unlink { cell, file } => {
                    let (fs, model) = &mut cells[cell % n];
                    let expected = if model.remove(&file).is_some() { Ok(()) } else { Err(Error::NotFound) };
                    prop_assert_eq!(fs.unlink(None, &path(file), false), expected);
                }
                Op::Rename { cell, from, to } => {
                    let (fs, model) = &mut cells[cell % n];
                    let result = fs.rename(None, &path(from), None, &path(to));
                    match model.remove(&from) {
                        Some(data) => {
                            prop_assert_eq!(result, Ok(()));
                            model.insert(to, data);
                        }
                        None => prop_assert_eq!(result, Err(Error::NotFound)),
                    }
                }
                Op::Fork { cell } => {
                    let copy = cells[cell % n].clone();
                    cells.push(copy);
                }
                Op::Snapshot { cell } => {
                    let (fs, _) = &mut cells[cell % n];
                    let mut bytes = Vec::new();
                    fs.encode(&mut bytes);
                    *fs = Fs::decode(&bytes).unwrap();
                }
            }
            for (fs, model) in &mut cells {
                let mut bytes = Vec::new();
                fs.encode(&mut bytes);
                prop_assert_eq!(Fs::decode(&bytes).unwrap().data_bytes(), fs.data_bytes());
                for file in 0..4 {
                    let got = read_all(fs, &path(file)).ok();
                    prop_assert_eq!(got.as_ref(), model.get(&file));
                }
                let listed: Vec<Vec<u8>> = names(fs, b"/dir").into_iter().skip(2).collect();
                let expected: Vec<Vec<u8>> = model.keys().map(|&f| path(f)[5..].to_vec()).collect();
                prop_assert_eq!(listed, expected);
            }
        }
    }
}
