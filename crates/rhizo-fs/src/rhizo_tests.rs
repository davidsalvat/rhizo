//! Host tests of `/run/rhizo` (design D9 of `add-cell-hierarchy`).

use super::*;

const RDONLY: OpenFlags = OpenFlags {
    read: true,
    write: false,
    create: false,
    exclusive: false,
    truncate: false,
    append: false,
    directory: false,
};

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

fn list(items: &[&[u8]]) -> Vec<Vec<u8>> {
    items.iter().map(|n| n.to_vec()).collect()
}

/// A tree whose `/run/rhizo` holds an endpoint `chan/tools` (socket 7), a
/// cell handle `cell/peer` (grant 3) and the inbox (socket 9).
fn granted() -> Fs {
    let mut fs = Fs::new();
    fs.rhizo_put(Place::Chan(b"tools"), Kept::Socket(7))
        .unwrap();
    fs.rhizo_put(Place::Cell(b"peer"), Kept::Grant(3)).unwrap();
    fs.rhizo_put(Place::Inbox, Kept::Socket(9)).unwrap();
    fs
}

/// Spec `cell-hierarchy`, "A new cell looks around": with nothing granted
/// the directory holds the generation and what the kernel put there; each
/// subdirectory appears with its first entry, and entries are listed and
/// stat by their labels.
#[test]
fn rhizo_dir_lists_what_was_granted() {
    let mut fs = Fs::new();
    fs.rhizo_dir().unwrap();
    assert_eq!(
        names(&mut fs, b"/run/rhizo"),
        list(&[b".", b"..", b"generation"])
    );
    let generation = fs.stat(None, b"/run/rhizo/generation").unwrap();
    assert_eq!((generation.kind, generation.size), (Kind::File, 8));
    let mut fs = granted();
    assert_eq!(
        names(&mut fs, b"/run/rhizo"),
        list(&[b".", b"..", b"cell", b"chan", b"generation", b"inbox"])
    );
    assert_eq!(
        names(&mut fs, b"/run/rhizo/chan"),
        list(&[b".", b"..", b"tools"])
    );
    assert_eq!(
        fs.stat(None, b"/run/rhizo/chan/tools").unwrap().kind,
        Kind::Socket
    );
    assert_eq!(
        fs.rhizo_entry(None, b"/run/rhizo/chan/tools"),
        Ok(Kept::Socket(7))
    );
    assert_eq!(
        fs.rhizo_entry(None, b"/run/rhizo/cell/peer"),
        Ok(Kept::Grant(3))
    );
    assert_eq!(
        fs.rhizo_entry(None, b"/run/rhizo/generation"),
        Ok(Kept::Generation)
    );
    assert_eq!(
        fs.rhizo_entry(None, b"/run/rhizo/chan"),
        Err(Error::Refused)
    );
    assert_eq!(
        fs.rhizo_entry(None, b"/run/rhizo/chan/none"),
        Err(Error::NotFound)
    );
    fs.chdir(b"/run/rhizo").unwrap();
    assert_eq!(fs.rhizo_entry(None, b"chan/tools"), Ok(Kept::Socket(7)));
    // Opening an entry is the caller's business: it names no file.
    assert_eq!(
        fs.open(None, b"/run/rhizo/chan/tools", RDONLY, 0),
        Err(Error::NoDevice)
    );
    assert_eq!(fs.sockets().collect::<Vec<_>>(), [7, 9]);
    assert!(fs.has_sockets(), "entries hold sockets");
    let mut plain = Fs::new();
    plain.rhizo_dir().unwrap();
    assert!(!plain.has_sockets());
    plain.socket_fd(3, false, false).unwrap();
    assert!(plain.has_sockets(), "a description holds one");
}

/// Spec `cell-hierarchy`, "Rename refused": creating, renaming, writing or
/// removing anything in the directory fails with `EROFS` (the directory
/// itself is busy), and nothing changes.
#[test]
fn rhizo_dir_is_read_only() {
    let mut fs = granted();
    fs.put_file(b"/mine", b"x").unwrap();
    let create = OpenFlags {
        write: true,
        create: true,
        ..RDONLY
    };
    let write = OpenFlags {
        write: true,
        ..RDONLY
    };
    let before = names(&mut fs, b"/run/rhizo/chan");
    let refused: [(Result<()>, &str); 12] = [
        (
            fs.rename(None, b"/run/rhizo/chan/tools", None, b"/run/rhizo/chan/x"),
            "rename in chan/",
        ),
        (
            fs.rename(None, b"/run/rhizo/chan/tools", None, b"/stolen"),
            "rename out",
        ),
        (
            fs.rename(None, b"/mine", None, b"/run/rhizo/chan/forged"),
            "rename in",
        ),
        (
            fs.rename(None, b"/mine", None, b"/run/rhizo/chan/tools"),
            "rename over",
        ),
        (
            fs.open(None, b"/run/rhizo/new", create, 1).map(drop),
            "create",
        ),
        (
            fs.open(None, b"/run/rhizo/chan/new", create, 1).map(drop),
            "create in chan/",
        ),
        (
            fs.open(None, b"/run/rhizo/generation", write, 1).map(drop),
            "write the generation",
        ),
        (fs.mkdir(None, b"/run/rhizo/chan/d"), "mkdir"),
        (
            fs.unlink(None, b"/run/rhizo/generation", false),
            "unlink generation",
        ),
        (fs.unlink(None, b"/run/rhizo/inbox", false), "unlink inbox"),
        (fs.unlink(None, b"/run/rhizo/chan", true), "rmdir chan/"),
        (fs.bind_name(None, b"/run/rhizo/chan/s", 5), "bind a name"),
    ];
    for (r, what) in refused {
        assert_eq!(r, Err(Error::ReadOnly), "{what}");
    }
    assert_eq!(Error::ReadOnly.errno(), 30, "EROFS");
    // An endpoint opened for writing sends messages: the caller's business.
    assert_eq!(
        fs.open(None, b"/run/rhizo/chan/tools", write, 1),
        Err(Error::NoDevice)
    );
    assert_eq!(
        fs.put_file(b"/run/rhizo/chan/p", b"x"),
        Err(Error::ReadOnly)
    );
    assert_eq!(fs.unlink(None, b"/run/rhizo", true), Err(Error::Busy));
    assert_eq!(fs.rename(None, b"/run", None, b"/moved"), Err(Error::Busy));
    assert_eq!(
        fs.rename(None, b"/run/rhizo", None, b"/moved"),
        Err(Error::Busy)
    );
    assert_eq!(names(&mut fs, b"/run/rhizo/chan"), before, "unchanged");
    assert_eq!(
        fs.rhizo_entry(None, b"/run/rhizo/chan/tools"),
        Ok(Kept::Socket(7))
    );
    // Beside it, `/run` is the cell's own.
    fs.put_file(b"/run/user", b"ok").unwrap();
    fs.rename(None, b"/run/user", None, b"/run/user2").unwrap();
}

/// `unlink` drops a handle: an endpoint's socket goes to the kernel to
/// release, a cell handle's grant to drop; a subdirectory goes with its
/// last entry, and labels are unique in their directory.
#[test]
fn rhizo_dir_unlink_drops() {
    let mut fs = granted();
    fs.rhizo_put(Place::Kernel, Kept::Socket(11)).unwrap();
    assert_eq!(
        fs.rhizo_put(Place::Chan(b"tools"), Kept::Socket(8)),
        Err(Error::Exists),
        "a label once per directory"
    );
    fs.rhizo_put(Place::Cell(b"tools"), Kept::Grant(4)).unwrap();
    for bad in [&b""[..], b".", b"..", b"a/b", b"a\0"] {
        assert_eq!(
            fs.rhizo_put(Place::Chan(bad), Kept::Socket(8)),
            Err(Error::Invalid)
        );
    }
    assert_eq!(
        fs.rhizo_put(Place::Chan(b"g"), Kept::Generation),
        Err(Error::Invalid)
    );
    fs.unlink(None, b"/run/rhizo/chan/tools", false).unwrap();
    assert_eq!(fs.take_released(), [7]);
    assert!(!fs.has_dropped());
    assert_eq!(
        names(&mut fs, b"/run/rhizo"),
        list(&[b".", b"..", b"cell", b"generation", b"inbox", b"kernel"]),
        "chan/ went with its last entry"
    );
    fs.unlink(None, b"/run/rhizo/cell/peer", false).unwrap();
    assert_eq!(fs.take_dropped(), [(3, b"peer".to_vec())], "with its label");
    assert_eq!(
        names(&mut fs, b"/run/rhizo/cell"),
        list(&[b".", b"..", b"tools"])
    );
    fs.unlink(None, b"/run/rhizo/kernel", false).unwrap();
    assert_eq!(fs.take_released(), [11]);
    // The kernel takes an entry away itself: nothing to release then.
    assert_eq!(fs.rhizo_take(Place::Cell(b"tools")), Ok(Kept::Grant(4)));
    assert_eq!(fs.rhizo_take(Place::Cell(b"tools")), Err(Error::NotFound));
    assert!(!fs.has_dropped() && !fs.has_released());
    assert_eq!(
        names(&mut fs, b"/run/rhizo"),
        list(&[b".", b"..", b"generation", b"inbox"])
    );
    // A subdirectory the cell stands in stays, empty, until it leaves.
    fs.rhizo_put(Place::Chan(b"a"), Kept::Socket(1)).unwrap();
    fs.chdir(b"/run/rhizo/chan").unwrap();
    fs.unlink(None, b"a", false).unwrap();
    assert_eq!(names(&mut fs, b"."), list(&[b".", b".."]));
    fs.rhizo_put(Place::Chan(b"b"), Kept::Socket(2)).unwrap();
    assert_eq!(names(&mut fs, b"."), list(&[b".", b"..", b"b"]));
}

/// The directory follows the cell's tree into a copy and a snapshot: its
/// sockets with the descriptions' (each one reference), its cell handles
/// left out, and read only after a decode; a forged entry outside it is
/// refused.
#[test]
fn rhizo_dir_in_copies_and_snapshots() {
    let mut fs = granted();
    let mut copy = fs.clone();
    copy.map_sockets(|h| h + 100);
    assert_eq!(copy.sockets().collect::<Vec<_>>(), [107, 109]);
    assert_eq!(
        fs.sockets().collect::<Vec<_>>(),
        [7, 9],
        "the source keeps its own"
    );
    let mut bytes = Vec::new();
    fs.encode(&mut bytes);
    let mut back = Fs::decode(&bytes).unwrap();
    assert_eq!(back.sockets().collect::<Vec<_>>(), [7, 9]);
    assert_eq!(
        names(&mut back, b"/run/rhizo"),
        list(&[b".", b"..", b"cell", b"chan", b"generation", b"inbox"])
    );
    assert_eq!(names(&mut back, b"/run/rhizo/cell"), list(&[b".", b".."]));
    assert_eq!(
        back.rename(None, b"/run/rhizo/chan/tools", None, b"/x"),
        Err(Error::ReadOnly)
    );
    back.rhizo_dir().unwrap();
    back.rhizo_put(Place::Cell(b"peer"), Kept::Grant(5))
        .unwrap();
    // Entries of the kernel in a directory that is not its own.
    fs.rhizo = None;
    let mut forged = Vec::new();
    fs.encode(&mut forged);
    assert_eq!(Fs::decode(&forged).err(), Some(DecodeError::Corrupt));
    // A tree from before `/run/rhizo` existed gets one.
    let mut old = Fs::new();
    old.mkdir(None, b"/run").unwrap();
    old.mkdir(None, b"/run/rhizo").unwrap();
    old.rhizo_dir().unwrap();
    assert_eq!(
        names(&mut old, b"/run/rhizo"),
        list(&[b".", b"..", b"generation"])
    );
    assert_eq!(old.mkdir(None, b"/run/rhizo/x"), Err(Error::ReadOnly));
}

/// A cell's files decoded against the tree they started from share every
/// inode encoded the same (its `/run/rhizo` among them) and are otherwise
/// what a plain decode gives.
#[test]
fn decode_shares_what_the_base_has() {
    let mut base = Fs::new();
    base.rhizo_dir().unwrap();
    base.rhizo_put(Place::Inbox, Kept::Inbox).unwrap();
    let shared_base = Base::new(base.clone());
    let base_bytes = shared_base.bytes().to_vec();
    let mut fs = base.clone();
    fs.put_file(b"/notes", b"mine").unwrap();
    fs.socket_fd(7, false, false).unwrap();
    let mut bytes = Vec::new();
    fs.encode(&mut bytes);
    let shared = Fs::decode_trusted_sharing(&bytes, &shared_base).unwrap();
    let plain = Fs::decode_trusted(&bytes).unwrap();
    let mut again = Vec::new();
    shared.encode(&mut again);
    assert_eq!(again, bytes, "the same file system");
    assert!(Fs::decode(&again).is_ok(), "a consistent one");
    let ptr = |f: &Fs, i: usize| f.0.inodes[i].as_ref().map(Arc::as_ptr);
    let rhizo = base.rhizo.unwrap() as usize;
    for i in 1..base.inodes.len() {
        assert_eq!(ptr(&shared, i), ptr(&base, i), "inode {i} shared");
        assert_ne!(ptr(&plain, i), ptr(&base, i), "a plain decode copies");
    }
    assert_ne!(ptr(&shared, 0), ptr(&base, 0), "its root changed");
    assert_eq!(shared.rhizo, Some(rhizo as Ino));
    assert_eq!(
        shared.rhizo_entry(None, b"/run/rhizo/inbox"),
        Ok(Kept::Inbox)
    );
    assert_eq!(shared.sockets().collect::<Vec<_>>(), [7]);
    // Unchanged, even the root is shared; a write copies only what it
    // changes, as in a fork.
    let mut same = Fs::decode_trusted_sharing(&base_bytes, &shared_base).unwrap();
    assert_eq!(ptr(&same, 0), ptr(&base, 0));
    assert_eq!(
        (same.used, same.names, same.rhizo),
        (base.used, base.names, base.rhizo)
    );
    same.put_file(b"/x", b"y").unwrap();
    assert_eq!(
        names(&mut base.clone(), b"/").len(),
        3,
        "the base is untouched"
    );
}

/// Spec `cell-hierarchy`, "A new cell looks around": the inbox the kernel
/// puts in every cell's tree is one entry for all of them, holding no
/// socket of its own: it goes only at `inbox`, stays (`EROFS`), survives
/// copies and snapshots, and costs a fork no walk of the tree.
#[test]
fn rhizo_dir_keeps_the_inbox() {
    let mut fs = Fs::new();
    fs.rhizo_dir().unwrap();
    assert!(fs.has_rhizo());
    assert_eq!(
        fs.rhizo_put(Place::Chan(b"inbox"), Kept::Inbox),
        Err(Error::Invalid),
        "only at its place"
    );
    fs.rhizo_put(Place::Inbox, Kept::Inbox).unwrap();
    assert_eq!(
        names(&mut fs, b"/run/rhizo"),
        list(&[b".", b"..", b"generation", b"inbox"])
    );
    assert_eq!(
        fs.stat(None, b"/run/rhizo/inbox").unwrap().kind,
        Kind::Socket
    );
    assert_eq!(fs.rhizo_entry(None, b"/run/rhizo/inbox"), Ok(Kept::Inbox));
    assert!(!fs.has_sockets(), "no socket of its own");
    assert_eq!(fs.sockets().count(), 0);
    assert_eq!(
        fs.unlink(None, b"/run/rhizo/inbox", false),
        Err(Error::ReadOnly)
    );
    let copy = fs.clone();
    assert!(copy.shares_with(&fs), "a copy shares the tree");
    let mut bytes = Vec::new();
    fs.encode(&mut bytes);
    let back = Fs::decode(&bytes).unwrap();
    assert_eq!(back.rhizo_entry(None, b"/run/rhizo/inbox"), Ok(Kept::Inbox));
    assert!(back.has_rhizo() && !back.has_sockets());
    // Entries holding sockets are counted as they come and go.
    fs.rhizo_put(Place::Chan(b"a"), Kept::Socket(4)).unwrap();
    assert!(fs.has_sockets());
    let mut bytes = Vec::new();
    fs.encode(&mut bytes);
    assert!(Fs::decode(&bytes).unwrap().has_sockets());
    assert_eq!(fs.rhizo_take(Place::Chan(b"a")), Ok(Kept::Socket(4)));
    assert!(!fs.has_sockets());
    fs.rhizo_put(Place::Chan(b"b"), Kept::Socket(5)).unwrap();
    fs.unlink(None, b"/run/rhizo/chan/b", false).unwrap();
    assert!(!fs.has_sockets());
    assert_eq!(fs.take_released(), [5]);
}
