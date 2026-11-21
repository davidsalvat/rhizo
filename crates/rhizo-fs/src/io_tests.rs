//! Host tests of pipes, event counters, `epoll` instances, socket
//! descriptors and socket names.

use super::*;
use crate::tests::exercise;

const IN: u32 = 0x1;
const OUT: u32 = 0x4;
const ERR: u32 = 0x8;
const HUP: u32 = 0x10;

fn no_socket(_: u32) -> u32 {
    0
}

fn read_pipe_all(fs: &mut Fs, fd: Fd) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    fs.read_pipe(fd, usize::MAX, &mut |b| {
        out.extend_from_slice(b);
        true
    })?;
    Ok(out)
}

#[test]
fn pipes_carry_bytes_and_report_their_ends() {
    let mut fs = Fs::new();
    let (r, w) = fs.pipe(false, false).unwrap();
    assert_eq!((r, w), (3, 4), "lowest free descriptors");
    assert_eq!(fs.kind(r), Ok(Kind::Pipe));
    assert_eq!(fs.poll(r, &no_socket), Ok(0));
    assert_eq!(fs.poll(w, &no_socket), Ok(OUT));
    assert_eq!(fs.read_pipe(r, 10, &mut |_| true), Err(Error::Again));
    assert_eq!(fs.write_pipe(w, b"hello"), Ok(5));
    assert_eq!(fs.poll(r, &no_socket), Ok(IN));
    assert_eq!(fs.write_pipe(r, b"x"), Err(Error::BadFd), "the read end");
    assert_eq!(fs.read_pipe(w, 1, &mut |_| true), Err(Error::BadFd));
    let mut first = Vec::new();
    fs.read_pipe(r, 2, &mut |b| {
        first.extend_from_slice(b);
        true
    })
    .unwrap();
    assert_eq!(first, b"he");
    fs.close(w).unwrap();
    assert_eq!(fs.poll(r, &no_socket), Ok(IN | HUP));
    assert_eq!(read_pipe_all(&mut fs, r).unwrap(), b"llo");
    assert_eq!(read_pipe_all(&mut fs, r).unwrap(), b"", "end of the pipe");
    fs.close(r).unwrap();

    let (r, w) = fs.pipe(false, false).unwrap();
    fs.close(r).unwrap();
    assert_eq!(fs.poll(w, &no_socket), Ok(OUT | ERR));
    assert_eq!(fs.write_pipe(w, b"x"), Err(Error::Pipe));
    fs.close(w).unwrap();
    assert!(fs.objects.iter().all(Option::is_none), "both pipes freed");
}

#[test]
fn pipes_hold_64k_and_small_writes_are_atomic() {
    let mut fs = Fs::new();
    let (r, w) = fs.pipe(true, false).unwrap();
    assert_eq!(fs.nonblocking(r), Ok(true));
    assert_eq!(fs.status_flags(w), Ok(1 | O_NONBLOCK_FLAG));
    assert_eq!(
        fs.write_pipe(w, &vec![1; PIPE_CAP - 100]),
        Ok(PIPE_CAP - 100)
    );
    assert_eq!(
        fs.write_pipe(w, &[2; 200]),
        Err(Error::Again),
        "whole or nothing"
    );
    assert_eq!(
        fs.write_pipe(w, &vec![3; PIPE_BUF + 1]),
        Ok(100),
        "a large write is cut"
    );
    assert_eq!(fs.poll(w, &no_socket), Ok(0), "full");
    assert_eq!(read_pipe_all(&mut fs, r).unwrap().len(), PIPE_CAP);
    fs.set_status_flags(r, 0).unwrap();
    assert_eq!(fs.nonblocking(r), Ok(false));
}

#[test]
fn event_counters_add_and_drain() {
    let mut fs = Fs::new();
    let e = fs.eventfd(2, false, false, true).unwrap();
    assert_eq!(fs.cloexec(e, None), Ok(true));
    assert_eq!(fs.poll(e, &no_socket), Ok(IN | OUT));
    fs.write_event(e, 3).unwrap();
    assert_eq!(fs.read_event(e), Ok(5));
    assert_eq!(fs.read_event(e), Err(Error::Again));
    assert_eq!(fs.poll(e, &no_socket), Ok(OUT));
    assert_eq!(fs.write_event(e, u64::MAX), Err(Error::Invalid));
    fs.write_event(e, u64::MAX - 1).unwrap();
    assert_eq!(fs.write_event(e, 1), Err(Error::Again), "would overflow");
    assert_eq!(fs.poll(e, &no_socket), Ok(IN));
    let s = fs.eventfd(2, true, false, false).unwrap();
    assert_eq!(fs.read_event(s), Ok(1));
    assert_eq!(fs.read_event(s), Ok(1));
    assert_eq!(fs.read_event(s), Err(Error::Again));
    assert_eq!(fs.read_event(0), Err(Error::Invalid), "not a counter");
}

#[test]
fn epoll_lists_follow_linux_rules() {
    let mut fs = Fs::new();
    let ep = fs.epoll_create(false).unwrap();
    let (r, w) = fs.pipe(false, false).unwrap();
    assert_eq!(fs.epoll_ctl(ep, EpollOp::Add, r, IN, 7), Ok(()));
    assert_eq!(fs.epoll_ctl(ep, EpollOp::Add, r, IN, 7), Err(Error::Exists));
    assert_eq!(
        fs.epoll_ctl(ep, EpollOp::Modify, w, OUT, 1),
        Err(Error::NotFound)
    );
    assert_eq!(
        fs.epoll_ctl(ep, EpollOp::Add, ep, IN, 0),
        Err(Error::Invalid)
    );
    assert_eq!(fs.epoll_ctl(r, EpollOp::Add, w, IN, 0), Err(Error::Invalid));
    assert_eq!(fs.epoll_ctl(ep, EpollOp::Add, 99, IN, 0), Err(Error::BadFd));
    fs.put_file(b"/f", b"").unwrap();
    let f = fs.open(None, b"/f", OpenFlags::default(), 0).unwrap();
    assert_eq!(fs.epoll_ctl(ep, EpollOp::Add, f, IN, 0), Err(Error::Perm));
    assert_eq!(fs.poll(ep, &no_socket), Ok(0));
    fs.write_pipe(w, b"x").unwrap();
    assert_eq!(fs.poll(ep, &no_socket), Ok(IN), "an entry is ready");
    fs.epoll_mask(ep, r, 0).unwrap();
    assert_eq!(fs.poll(ep, &no_socket), Ok(0), "masked");
    fs.epoll_ctl(ep, EpollOp::Modify, r, IN | OUT, 9).unwrap();
    assert_eq!(
        fs.epoll_interests(ep).unwrap(),
        &[Interest {
            fd: r,
            events: IN | OUT,
            data: 9
        }]
    );
    fs.close(r).unwrap();
    assert!(
        fs.epoll_interests(ep).unwrap().is_empty(),
        "closing removes it"
    );
    assert_eq!(
        fs.epoll_ctl(ep, EpollOp::Delete, r, 0, 0),
        Err(Error::BadFd)
    );
}

#[test]
fn socket_descriptors_hand_back_released_handles() {
    let mut fs = Fs::new();
    let s = fs.socket_fd(42, true, false).unwrap();
    assert_eq!(fs.kind(s), Ok(Kind::Socket));
    assert_eq!(fs.socket(s), Ok(42));
    assert_eq!(fs.socket(0), Err(Error::NotSocket));
    assert_eq!(fs.nonblocking(s), Ok(true));
    assert_eq!(fs.poll(s, &|h| if h == 42 { IN } else { 0 }), Ok(IN));
    let copy = fs.dup(s, 0, false, false).unwrap();
    assert_eq!(
        fs.sockets().collect::<Vec<_>>(),
        [42],
        "one per description"
    );
    fs.close(s).unwrap();
    assert!(!fs.has_released(), "a duplicate still refers to it");
    fs.map_sockets(|h| h + 1);
    assert_eq!(fs.socket(copy), Ok(43));
    fs.close(copy).unwrap();
    assert!(fs.has_released());
    assert_eq!(fs.take_released(), [43]);
    assert!(!fs.has_released());
    let other = fs.socket_fd(1, false, false).unwrap();
    assert_eq!(fs.fstat(other).unwrap().unwrap().kind, Kind::Socket);
}

#[test]
fn socket_names_are_files_of_type_socket() {
    let mut fs = Fs::new();
    fs.bind_name(None, b"/sock", 5).unwrap();
    assert_eq!(fs.bind_name(None, b"/sock", 6), Err(Error::Exists));
    assert_eq!(fs.stat(None, b"/sock").unwrap().kind, Kind::Socket);
    assert_eq!(fs.socket_name(None, b"/sock"), Ok(5));
    assert_eq!(fs.socket_name(None, b"/none"), Err(Error::NotFound));
    fs.put_file(b"/plain", b"").unwrap();
    assert_eq!(fs.socket_name(None, b"/plain"), Err(Error::Refused));
    assert_eq!(
        fs.open(None, b"/sock", OpenFlags::default(), 0),
        Err(Error::NoDevice)
    );
    fs.map_socket_names(|n| if n == 5 { 0 } else { n });
    assert_eq!(
        fs.socket_name(None, b"/sock"),
        Err(Error::Refused),
        "no socket any more"
    );
    assert_eq!(fs.socket_names().count(), 0);
    fs.unlink(None, b"/sock", false).unwrap();
    assert_eq!(fs.stat(None, b"/sock").err(), Some(Error::NotFound));
}

#[test]
fn forks_copy_pipes() {
    let mut parent = Fs::new();
    let (r, w) = parent.pipe(false, false).unwrap();
    parent.write_pipe(w, b"shared").unwrap();
    let mut child = parent.clone();
    assert_eq!(read_pipe_all(&mut parent, r).unwrap(), b"shared");
    assert_eq!(
        read_pipe_all(&mut child, r).unwrap(),
        b"shared",
        "the child's copy"
    );
    child.write_pipe(w, b"mine").unwrap();
    assert_eq!(parent.read_pipe(r, 8, &mut |_| true), Err(Error::Again));
}

#[test]
fn objects_round_trip_and_corruption_is_refused() {
    let mut fs = Fs::new();
    let (r, w) = fs.pipe(true, false).unwrap();
    fs.write_pipe(w, b"in flight").unwrap();
    let e = fs.eventfd(3, true, false, false).unwrap();
    let ep = fs.epoll_create(true).unwrap();
    fs.epoll_ctl(ep, EpollOp::Add, r, IN, 0xfeed).unwrap();
    fs.epoll_ctl(ep, EpollOp::Add, e, IN | OUT, 1).unwrap();
    let s = fs.socket_fd(0, false, false).unwrap();
    fs.bind_name(None, b"/sock", 9).unwrap();
    let mut bytes = Vec::new();
    fs.encode(&mut bytes);
    let mut back = Fs::decode(&bytes).unwrap();
    let mut again = Vec::new();
    back.encode(&mut again);
    assert_eq!(bytes, again);
    assert_eq!(read_pipe_all(&mut back, r).unwrap(), b"in flight");
    assert_eq!(back.read_event(e), Ok(1));
    assert_eq!(back.epoll_interests(ep).unwrap().len(), 2);
    assert_eq!(back.socket(s), Ok(0));
    assert_eq!(back.socket_name(None, b"/sock"), Ok(9));
    for len in 0..bytes.len() {
        assert!(Fs::decode(&bytes[..len]).is_err(), "truncated at {len}");
    }
    for i in 0..bytes.len() {
        let mut bad = bytes.clone();
        bad[i] ^= 0x41;
        if let Ok(mut fs) = Fs::decode(&bad) {
            exercise(&mut fs);
        }
    }
}

#[test]
fn the_host_writes_standard_input() {
    let mut fs = Fs::new();
    assert!(!fs.has_input());
    assert_eq!(fs.push_input(b"x"), Err(Error::BadFd), "no host input yet");
    fs.host_input().unwrap();
    assert_eq!(fs.host_input(), Err(Error::Exists));
    assert_eq!(fs.kind(0), Ok(Kind::Pipe));
    assert_eq!(fs.kind(1), Ok(Kind::Console), "output stays on the console");
    assert_eq!(fs.read_pipe(0, 10, &mut |_| true), Err(Error::Again));
    assert_eq!(fs.poll(0, &no_socket), Ok(0));
    fs.push_input(b"hello\n").unwrap();
    assert_eq!(fs.poll(0, &no_socket), Ok(IN));
    assert_eq!(read_pipe_all(&mut fs, 0).unwrap(), b"hello\n");
    // Whole or not at all, within the pipe's capacity.
    fs.push_input(&vec![1; PIPE_CAP - 1]).unwrap();
    assert_eq!(fs.push_input(b"ab"), Err(Error::Again));
    fs.push_input(b"a").unwrap();
    assert_eq!(read_pipe_all(&mut fs, 0).unwrap().len(), PIPE_CAP);
    // A duplicate reads the same pipe; closing the host's end is the end
    // of file once the unread bytes are read.
    let dup = fs.dup(0, 0, false, false).unwrap();
    fs.push_input(b"last").unwrap();
    fs.close_input().unwrap();
    assert_eq!(fs.close_input(), Err(Error::BadFd));
    assert_eq!(fs.push_input(b"more"), Err(Error::BadFd));
    assert_eq!(fs.poll(dup, &no_socket), Ok(IN | HUP));
    assert_eq!(read_pipe_all(&mut fs, dup).unwrap(), b"last");
    assert_eq!(read_pipe_all(&mut fs, 0).unwrap(), b"", "end of file");
    fs.close(0).unwrap();
    fs.close(dup).unwrap();
    assert!(fs.objects.iter().all(Option::is_none), "the pipe is freed");
}

#[test]
fn input_to_a_cell_that_closed_it_is_refused() {
    let mut fs = Fs::new();
    fs.host_input().unwrap();
    fs.close(0).unwrap();
    assert_eq!(fs.push_input(b"x"), Err(Error::Pipe));
    fs.close_input().unwrap();
    assert!(fs.objects.iter().all(Option::is_none), "the pipe is freed");
}

#[test]
fn forks_and_snapshots_keep_their_own_input() {
    let mut parent = Fs::new();
    parent.host_input().unwrap();
    parent.push_input(b"before ").unwrap();
    let mut child = parent.clone();
    child.push_input(b"child").unwrap();
    parent.push_input(b"parent").unwrap();
    assert_eq!(read_pipe_all(&mut parent, 0).unwrap(), b"before parent");
    assert_eq!(read_pipe_all(&mut child, 0).unwrap(), b"before child");
    child.push_input(b"unread").unwrap();
    let mut bytes = Vec::new();
    child.encode(&mut bytes);
    let mut back = Fs::decode(&bytes).unwrap();
    assert!(back.has_input());
    back.push_input(b" and more").unwrap();
    assert_eq!(read_pipe_all(&mut back, 0).unwrap(), b"unread and more");
    back.close_input().unwrap();
    let mut closed = Vec::new();
    back.encode(&mut closed);
    let mut back = Fs::decode(&closed).unwrap();
    assert!(!back.has_input());
    assert_eq!(read_pipe_all(&mut back, 0).unwrap(), b"", "still closed");
    // The host's end must name a pipe it holds: anything else is refused.
    let at = bytes.len() - 4;
    for bad in [1u32, 7, 1 << 20, u32::MAX] {
        let mut corrupt = bytes.clone();
        corrupt[at..].copy_from_slice(&bad.to_le_bytes());
        assert!(Fs::decode(&corrupt).is_err(), "input {bad}");
    }
}

#[test]
fn what_a_waiting_thread_may_wait_for_is_counted() {
    let mut fs = Fs::new();
    let (r, w) = fs.pipe(false, false).unwrap();
    let e = fs.eventfd(0, false, false, false).unwrap();
    let before = fs.changes();
    assert_eq!(read_pipe_all(&mut fs, r), Err(Error::Again));
    assert_eq!(fs.read_event(e), Err(Error::Again));
    assert_eq!(fs.changes(), before, "nothing happened");
    let mut seen = fs.changes();
    let mut changed = |fs: &Fs, what: &str| {
        assert!(fs.changes() > seen, "{what}");
        seen = fs.changes();
    };
    fs.write_pipe(w, b"x").unwrap();
    changed(&fs, "data in");
    read_pipe_all(&mut fs, r).unwrap();
    changed(&fs, "room for a writer");
    fs.write_event(e, 1).unwrap();
    changed(&fs, "a count");
    fs.read_event(e).unwrap();
    changed(&fs, "room for a count");
    let ep = fs.epoll_create(false).unwrap();
    fs.epoll_ctl(ep, EpollOp::Add, r, IN, 0).unwrap();
    changed(&fs, "an interest");
    fs.close(w).unwrap();
    changed(&fs, "the end of the pipe");
}
