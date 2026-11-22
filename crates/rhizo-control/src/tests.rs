use super::*;
use proptest::prelude::*;

/// Everything a decoder finds in `bytes`.
fn items(bytes: &[u8]) -> Vec<Item> {
    let mut d = Decoder::new();
    bytes.iter().filter_map(|&b| d.push(b)).collect()
}

fn frames(bytes: &[u8]) -> Vec<Frame> {
    items(bytes)
        .into_iter()
        .filter_map(|i| match i {
            Item::Frame(f) => Some(f),
            _ => None,
        })
        .collect()
}

fn text(bytes: &[u8]) -> Vec<u8> {
    items(bytes)
        .into_iter()
        .filter_map(|i| match i {
            Item::Text(b) => Some(b),
            _ => None,
        })
        .collect()
}

fn run_request() -> Run {
    match run() {
        Request::Run(r) => r,
        _ => unreachable!(),
    }
}

fn run() -> Request {
    Request::Run(Run {
        image: b"cpython".to_vec(),
        files_from: b"cpython-files".to_vec(),
        argv: vec![
            b"/usr/bin/python3".to_vec(),
            b"-c".to_vec(),
            b"print(1)".to_vec(),
        ],
        envp: vec![b"HOME=/".to_vec()],
        files: vec![(b"/branch".to_vec(), b"1".to_vec())],
        input: true,
        ..Run::default()
    })
}

/// A placement with every field away from its default.
fn placed() -> Placement {
    Placement {
        parent: 3,
        label: b"tools".to_vec(),
        limits: Limits {
            memory: 256 << 20,
            cpu_ns: 10_000_000_000,
            sockets: 16,
            cells: 4,
        },
        egress: Egress::Rules(vec![
            Rule {
                proto: Proto::Tcp,
                addr: [140, 82, 112, 0],
                prefix: 20,
                lo: 443,
                hi: 443,
            },
            Rule {
                proto: Proto::Any,
                addr: [10, 0, 2, 2],
                prefix: 32,
                lo: 1,
                hi: u16::MAX,
            },
        ]),
        ceiling: Some(Egress::Full),
        on_parent_end: OnParentEnd::Promote,
        cpus: 0b1100,
        cpu_weight: 250,
        cpu_max: 1500,
    }
}

/// Everything the host's decoder finds in `bytes`.
fn host_frames(bytes: &[u8]) -> Vec<Frame> {
    let mut d = Decoder::any_version();
    bytes
        .iter()
        .filter_map(|&b| match d.push(b) {
            Some(Item::Frame(f)) => Some(f),
            _ => None,
        })
        .collect()
}

#[test]
fn a_frame_has_its_header_then_its_payload() {
    let bytes = Request::Suspend(7).encode(0x0102_0304);
    assert_eq!(
        bytes,
        [
            // The cell, then its `tree` flag (version 4).
            0x01, b'Z', VERSION, 0x03, 0x04, 0x03, 0x02, 0x01, 5, 0, 0, 0, 7, 0, 0, 0, 0
        ]
    );
}

#[test]
fn text_and_frames_come_apart() {
    let mut stream = b"Rhizo 0.1.0\r\nmemory: 128 MiB\r\n".to_vec();
    stream.extend(Message::Hello { version: VERSION }.encode(0));
    stream.extend(b"net: address 10.0.2.15\r\n");
    stream.extend(Request::List.encode(9));
    assert_eq!(
        text(&stream),
        b"Rhizo 0.1.0\r\nmemory: 128 MiB\r\nnet: address 10.0.2.15\r\n"
    );
    let found = frames(&stream);
    assert_eq!(found.len(), 2);
    assert_eq!(
        Message::decode(&found[0]),
        Ok(Message::Hello { version: VERSION })
    );
    assert_eq!(found[1].id, 9);
    assert_eq!(Request::decode(&found[1]), Ok(Request::List));
}

#[test]
fn every_request_round_trips() {
    let requests = [
        run(),
        Request::Run(Run::default()),
        Request::List,
        Request::Suspend(1),
        Request::Resume(2),
        Request::fork(3, 4),
        Request::Fork {
            cell: 3,
            n: 2,
            placement: placed(),
            watch: true,
        },
        Request::Run(Run {
            placement: placed(),
            watch: true,
            ..Run::default()
        }),
        Request::Kill(5),
        Request::KillTree(5),
        Request::SuspendTree(1),
        Request::ResumeTree(2),
        Request::Reparent { cell: 7, to: 3 },
        Request::Reparent { cell: 7, to: 0 },
        Request::Policy {
            cell: 7,
            egress: Egress::Full,
            ceiling: None,
        },
        Request::Policy {
            cell: 7,
            egress: placed().egress,
            ceiling: Some(Egress::None),
        },
        Request::Limits {
            cell: 7,
            limits: placed().limits,
        },
        Request::Floor {
            deny: Vec::new(),
            carved: Vec::new(),
        },
        Request::Floor {
            deny: vec![Rule {
                proto: Proto::Any,
                addr: [192, 168, 0, 0],
                prefix: 16,
                lo: 0,
                hi: u16::MAX,
            }],
            carved: vec![Rule {
                proto: Proto::Tcp,
                addr: [10, 0, 2, 2],
                prefix: 32,
                lo: 11434,
                hi: 11434,
            }],
        },
        Request::Scoped {
            root: 3,
            request: Box::new(Request::KillTree(9)),
        },
        Request::Scoped {
            root: 3,
            request: Box::new(Request::Run(Run {
                placement: placed(),
                ..run_request()
            })),
        },
        Request::Output(6),
        Request::WriteFile {
            cell: 7,
            path: b"/branch".to_vec(),
            data: vec![0, 1, 2, 0xff],
        },
        Request::Stats,
        Request::Shutdown,
        Request::Input {
            cell: 8,
            data: b"hello\n".to_vec(),
        },
        Request::Input {
            cell: 9,
            data: vec![0x01; MAX_INPUT],
        },
        Request::CloseInput(10),
        Request::Freeze(11),
        Request::Save {
            cell: 12,
            host: Host {
                ip: [10, 0, 2, 2],
                port: 40123,
                token: 0x0123_4567_89ab_cdef,
            },
        },
        Request::Load {
            size: 40 << 20,
            template: true,
            host: Host {
                ip: [172, 16, 0, 1],
                port: 1,
                token: u64::MAX,
            },
            placement: placed(),
        },
    ];
    for (id, request) in requests.iter().enumerate() {
        let found = frames(&request.encode(id as u32));
        assert_eq!(found.len(), 1, "{request:?}");
        assert_eq!(found[0].id, id as u32);
        assert_eq!(Request::decode(&found[0]).as_ref(), Ok(request));
    }
}

#[test]
fn every_message_round_trips() {
    let messages = [
        Message::Hello { version: 1 },
        Message::Done {
            ns: 12_345,
            body: Body::None,
        },
        Message::Done {
            ns: 1,
            body: Body::Cell(4),
        },
        Message::Done {
            ns: 2,
            body: Body::Cells(vec![5, 6, 7]),
        },
        Message::Done {
            ns: 3,
            body: Body::Bytes(15 << 20),
        },
        Message::Done {
            ns: 4,
            body: Body::List(vec![
                CellInfo {
                    id: 1,
                    image: b"cpython".to_vec(),
                    state: State::Running,
                    tree: None,
                },
                CellInfo {
                    id: 2,
                    image: b"idle".to_vec(),
                    state: State::Suspended { bytes: 4096 },
                    tree: None,
                },
                CellInfo {
                    id: 3,
                    image: b"exit3".to_vec(),
                    state: State::Exited(-3),
                    tree: None,
                },
                CellInfo {
                    id: 4,
                    image: Vec::new(),
                    state: State::Fault,
                    tree: None,
                },
                CellInfo {
                    id: 5,
                    image: b"hog".to_vec(),
                    state: State::Killed,
                    tree: None,
                },
                CellInfo {
                    id: 6,
                    image: b"cpython".to_vec(),
                    state: State::Template,
                    tree: None,
                },
                CellInfo {
                    id: 7,
                    image: b"python".to_vec(),
                    state: State::Running,
                    tree: Some(TreeInfo {
                        parent: 6,
                        root: 1,
                        label: b"tools".to_vec(),
                        limits: placed().limits,
                        egress: placed().egress,
                    }),
                },
            ]),
        },
        Message::Done {
            ns: 5,
            body: Body::Stats(Stats {
                running: 1,
                suspended: 2,
                ended: 3,
                templates: 4,
                snapshot_bytes: 4,
                memory_total: 128 << 20,
                memory_free: 64 << 20,
            }),
        },
        Message::Failed {
            ns: 6,
            error: Error::NoCell,
            message: b"cell 9".to_vec(),
        },
        Message::Output {
            cell: 8,
            data: b"turn 1\n".to_vec(),
        },
        Message::Ended {
            cell: 8,
            state: State::Exited(0),
        },
        Message::Failed {
            ns: 7,
            error: Error::BadSnapshot,
            message: b"checksum".to_vec(),
        },
    ];
    for message in &messages {
        let found = frames(&message.encode(77));
        assert_eq!(found.len(), 1, "{message:?}");
        assert_eq!(Message::decode(&found[0]).as_ref(), Ok(message));
    }
}

#[test]
fn another_version_is_refused_and_skipped() {
    let mut bytes = Request::Kill(1).encode(5);
    bytes[2] = VERSION + 1;
    bytes.extend(Request::List.encode(6));
    let found = items(&bytes);
    assert_eq!(
        found[0],
        Item::Bad {
            id: 5,
            error: Error::Version
        }
    );
    assert_eq!(
        found[1..],
        [Item::Frame(Frame {
            version: VERSION,
            kind: 0x02,
            id: 6,
            payload: Vec::new()
        })]
    );
}

#[test]
fn an_oversized_frame_is_refused_and_the_next_one_found() {
    let mut bytes = encode_frame(0x02, 3, &[]);
    bytes[8..12].copy_from_slice(&((MAX_PAYLOAD as u32) + 1).to_le_bytes());
    bytes.extend(b"garbage");
    bytes.extend(Request::Stats.encode(4));
    let found = items(&bytes);
    assert_eq!(
        found[0],
        Item::Bad {
            id: 3,
            error: Error::TooLarge
        }
    );
    assert_eq!(frames(&bytes).len(), 1);
    assert_eq!(frames(&bytes)[0].id, 4);
}

#[test]
fn a_stray_start_byte_is_dropped() {
    let mut bytes = b"a\x01b".to_vec();
    bytes.extend(Request::List.encode(1));
    assert_eq!(text(&bytes), b"ab");
    assert_eq!(frames(&bytes).len(), 1);
}

#[test]
fn malformed_payloads_are_refused() {
    let frame = |kind, payload: &[u8]| Frame {
        version: VERSION,
        kind,
        id: 1,
        payload: payload.to_vec(),
    };
    // Too short, trailing bytes, unknown kinds and tags.
    assert_eq!(
        Request::decode(&frame(0x03, &[1, 0])),
        Err(Error::Malformed)
    );
    assert_eq!(Request::decode(&frame(0x02, &[0])), Err(Error::Malformed));
    assert_eq!(Request::decode(&frame(0x7f, &[])), Err(Error::UnknownKind));
    assert_eq!(Message::decode(&frame(0x01, &[])), Err(Error::UnknownKind));
    let mut done = 0u64.to_le_bytes().to_vec();
    done.push(9);
    assert_eq!(Message::decode(&frame(0x81, &done)), Err(Error::Malformed));
    // A list whose count is larger than its bytes allocates nothing.
    let mut cells = 0u64.to_le_bytes().to_vec();
    cells.push(2);
    cells.extend(u32::MAX.to_le_bytes());
    assert_eq!(Message::decode(&frame(0x81, &cells)), Err(Error::Malformed));
    // A byte string longer than what is left.
    let mut write = 1u32.to_le_bytes().to_vec();
    write.extend(1000u32.to_le_bytes());
    write.extend(b"short");
    assert_eq!(Request::decode(&frame(0x08, &write)), Err(Error::Malformed));
}

#[test]
fn input_is_bounded_and_its_flag_checked() {
    let frame = |kind, payload: Vec<u8>| Frame {
        version: VERSION,
        kind,
        id: 1,
        payload,
    };
    // One byte more than an `input` request may carry: refused as too
    // large, while the frame itself is valid.
    let big = Request::Input {
        cell: 1,
        data: vec![b'a'; MAX_INPUT + 1],
    }
    .encode(3);
    let found = frames(&big);
    assert_eq!(found.len(), 1);
    assert_eq!(Request::decode(&found[0]), Err(Error::TooLarge));
    // A length that claims more than the payload holds.
    let mut short = 1u32.to_le_bytes().to_vec();
    short.extend(((MAX_INPUT as u32) + 7).to_le_bytes());
    short.extend(b"abc");
    assert_eq!(Request::decode(&frame(0x0b, short)), Err(Error::Malformed));
    assert_eq!(
        Request::decode(&frame(0x0c, vec![1, 0, 0])),
        Err(Error::Malformed)
    );
    // `run`'s input and watch flags are 0 or 1 (the input flag comes
    // before the placement, one byte by default).
    let run = Request::Run(Run::default()).encode(1);
    for back in [1, 1 + PLACEMENT_BYTES + 1] {
        let mut bad = run.clone();
        let at = bad.len() - back;
        bad[at] = 2;
        assert_eq!(Request::decode(&frames(&bad)[0]), Err(Error::Malformed));
    }
    // `load`'s template flag is 0 or 1.
    let mut load = Request::Load {
        size: 1,
        template: false,
        host: Host::default(),
        placement: Placement::root(),
    }
    .encode(5);
    let flag = load.len() - 15 - PLACEMENT_BYTES;
    load[flag] = 2;
    assert_eq!(Request::decode(&frames(&load)[0]), Err(Error::Malformed));
}

/// Bytes from hexadecimal.
fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// Frames the version-3 code (`2ee4667`) wrote: a `run`, a `fork`, a
/// `kill`, a `load` and a `list` reply.
const V3_RUN: &str = "015a030107000000350000000500000068656c6c6f00000000010000000500000068656c6c6f0100000003000000413d3101000000020000002f66010000007801";
const V3_FORK: &str = "015a030508000000080000000300000002000000";
const V3_KILL: &str = "015a0306090000000400000004000000";
const V3_LOAD: &str = "015a030f0a000000170000000010000000000000010202000a88138877665544332211";
const V3_LIST: &str = "015a03810b00000030000000dc050000000000000402000000010000000500000068656c6c6f00020000000400000069646c65010020000000000000";

/// The host speaks version 3 as the version-3 code did (design D20 of
/// `add-cell-hierarchy`): its requests are the same bytes, and it reads
/// that code's frames, list rows without a place in the tree.
#[test]
fn the_host_speaks_version_3() {
    let load = Request::Load {
        size: 4096,
        template: true,
        host: Host {
            ip: [10, 0, 2, 2],
            port: 5000,
            token: 0x1122_3344_5566_7788,
        },
        placement: Placement::root(),
    };
    let requests = [
        (
            V3_RUN,
            7,
            Request::Run(Run {
                image: b"hello".to_vec(),
                argv: vec![b"hello".to_vec()],
                envp: vec![b"A=1".to_vec()],
                files: vec![(b"/f".to_vec(), b"x".to_vec())],
                input: true,
                ..Run::default()
            }),
        ),
        (V3_FORK, 8, Request::fork(3, 2)),
        (V3_KILL, 9, Request::Kill(4)),
        (V3_LOAD, 10, load),
    ];
    for (hex, id, request) in &requests {
        assert_eq!(
            request.encode_as(PREVIOUS, *id),
            Ok(unhex(hex)),
            "{request:?}"
        );
        // Version 4 says a default placement in one byte, and a `run` or a
        // `fork` adds its watch flag.
        let more = match request {
            Request::Run(_) | Request::Fork { .. } => 2,
            _ => 1, // `load`'s placement, `kill`'s `tree` flag
        };
        assert_eq!(
            request.encode(*id).len(),
            unhex(hex).len() + more,
            "{request:?}"
        );
        let found = host_frames(&unhex(hex));
        assert_eq!(found[0].version, PREVIOUS);
        assert_eq!(Request::decode(&found[0]).as_ref(), Ok(request));
        // The kernel takes only its own version.
        assert_eq!(
            items(&unhex(hex))[0],
            Item::Bad {
                id: *id,
                error: Error::Version
            }
        );
    }
    let list = Message::Done {
        ns: 1500,
        body: Body::List(vec![
            CellInfo {
                id: 1,
                image: b"hello".to_vec(),
                state: State::Running,
                tree: None,
            },
            CellInfo {
                id: 2,
                image: b"idle".to_vec(),
                state: State::Suspended { bytes: 8192 },
                tree: None,
            },
        ]),
    };
    let found = host_frames(&unhex(V3_LIST));
    assert_eq!(Message::decode(&found[0]), Ok(list.clone()));
    assert_eq!(list.encode_as(PREVIOUS, 11), unhex(V3_LIST));
}

/// What version 3 cannot say is not written in it, and frames of other
/// versions are read only as far as naming the kernel goes.
#[test]
fn version_4_requests_stay_in_version_4() {
    let placed_run = Request::Run(Run {
        placement: placed(),
        ..Run::default()
    });
    let watched = Request::Fork {
        cell: 1,
        n: 1,
        placement: Placement::root(),
        watch: true,
    };
    for request in [
        placed_run,
        watched,
        Request::KillTree(1),
        Request::SuspendTree(1),
        Request::ResumeTree(1),
        Request::Reparent { cell: 1, to: 0 },
        Request::Limits {
            cell: 1,
            limits: Limits::NONE,
        },
        Request::Scoped {
            root: 1,
            request: Box::new(Request::List),
        },
        Request::Floor {
            deny: Vec::new(),
            carved: Vec::new(),
        },
    ] {
        assert_eq!(
            request.encode_as(PREVIOUS, 1),
            Err(Error::Version),
            "{request:?}"
        );
        assert!(request.encode_as(VERSION, 1).is_ok());
    }
    assert_eq!(Request::List.encode_as(2, 1), Err(Error::Version));
    // A version-2 kernel's hello and refusals name it; nothing else of it
    // is read.
    let mut hello = Message::Hello { version: 2 }.encode(0);
    hello[2] = 2;
    let mut failed = Message::Failed {
        ns: 0,
        error: Error::Version,
        message: Vec::new(),
    }
    .encode(4);
    failed[2] = 2;
    let mut done = Message::Done {
        ns: 0,
        body: Body::None,
    }
    .encode(5);
    done[2] = 2;
    let mut stream = hello;
    stream.extend(failed);
    stream.extend(done);
    let found = host_frames(&stream);
    assert_eq!(found.len(), 3);
    assert_eq!(
        Message::decode(&found[0]),
        Ok(Message::Hello { version: 2 })
    );
    assert!(matches!(
        Message::decode(&found[1]),
        Ok(Message::Failed {
            error: Error::Version,
            ..
        })
    ));
    assert_eq!(Message::decode(&found[2]), Err(Error::Version));
    assert_eq!(Request::decode(&found[2]), Err(Error::Version));
}

/// Malformed tree requests are refused when read (spec `control-plane`,
/// malformed tree request): an unknown reservation mode, protocol, egress
/// or parent policy, a nested scoped request, and a scoped one wrapping
/// what is not a request.
#[test]
fn malformed_tree_requests_are_refused() {
    let frame = |bytes: Vec<u8>| frames(&bytes).remove(0);
    let run = Request::Run(Run {
        placement: placed(),
        ..Run::default()
    })
    .encode(1);
    // Offsets from the payload's start of the placement's fields: a
    // default `Run` takes 4 + 4 + 4 + 4 + 4 + 1 bytes before it, and the
    // placement starts with its flag.
    let at = HEADER_LEN + 21 + 1;
    let label = 4 + 5; // `tools`
    let mode = at + 4 + label;
    let egress = mode + 1 + 24;
    let proto = egress + 1 + 4;
    let mut bad = run.clone();
    bad[at - 1] = 2;
    assert_eq!(
        Request::decode(&frame(bad)),
        Err(Error::Malformed),
        "placement flag"
    );
    let mut bad = run.clone();
    bad[mode] = 1;
    assert_eq!(Request::decode(&frame(bad)), Err(Error::Malformed), "mode");
    let mut bad = run.clone();
    bad[egress] = 3;
    assert_eq!(
        Request::decode(&frame(bad)),
        Err(Error::Malformed),
        "egress"
    );
    let mut bad = run.clone();
    bad[proto] = 3;
    assert_eq!(
        Request::decode(&frame(bad)),
        Err(Error::Malformed),
        "protocol"
    );
    // The parent policy, after the two rules and the ceiling.
    let ope = proto + 2 * 10 + 2;
    let mut bad = run;
    assert_eq!(bad[ope], 1, "on_parent_end is `promote`");
    bad[ope] = 2;
    assert_eq!(
        Request::decode(&frame(bad)),
        Err(Error::Malformed),
        "parent end"
    );
    let nested = Request::Scoped {
        root: 1,
        request: Box::new(Request::Scoped {
            root: 2,
            request: Box::new(Request::List),
        }),
    }
    .encode(1);
    assert_eq!(Request::decode(&frame(nested)), Err(Error::Malformed));
    let mut unknown = Request::Scoped {
        root: 1,
        request: Box::new(Request::List),
    }
    .encode(1);
    unknown[HEADER_LEN + 4] = 0x7f;
    assert_eq!(Request::decode(&frame(unknown)), Err(Error::UnknownKind));
    let mut reply = Request::Scoped {
        root: 1,
        request: Box::new(Request::List),
    }
    .encode(1);
    reply[HEADER_LEN + 4] = 0x81; // a message kind
    assert_eq!(Request::decode(&frame(reply)), Err(Error::UnknownKind));
}

/// Bytes of a default placement.
const PLACEMENT_BYTES: usize = 1;

fn bytes() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(any::<u8>(), 0..24)
}

fn host() -> impl Strategy<Value = Host> {
    (any::<[u8; 4]>(), any::<u16>(), any::<u64>()).prop_map(|(ip, port, token)| Host {
        ip,
        port,
        token,
    })
}

fn limits() -> impl Strategy<Value = Limits> {
    (any::<u64>(), any::<u64>(), any::<u32>(), any::<u32>()).prop_map(
        |(memory, cpu_ns, sockets, cells)| Limits {
            memory,
            cpu_ns,
            sockets,
            cells,
        },
    )
}

fn egress() -> impl Strategy<Value = Egress> {
    let rule = (
        0u8..3,
        any::<[u8; 4]>(),
        any::<u8>(),
        any::<u16>(),
        any::<u16>(),
    )
        .prop_map(|(p, addr, prefix, lo, hi)| Rule {
            proto: [Proto::Tcp, Proto::Udp, Proto::Any][p as usize],
            addr,
            prefix,
            lo,
            hi,
        });
    prop_oneof![
        Just(Egress::None),
        Just(Egress::Full),
        proptest::collection::vec(rule, 0..4).prop_map(Egress::Rules),
    ]
}

fn placement() -> impl Strategy<Value = Placement> {
    (
        any::<u32>(),
        bytes(),
        limits(),
        egress(),
        proptest::option::of(egress()),
        any::<bool>(),
        (any::<u64>(), any::<u16>(), any::<u32>()),
    )
        .prop_map(
            |(parent, label, limits, egress, ceiling, promote, (cpus, cpu_weight, cpu_max))| {
                Placement {
                    parent,
                    label,
                    limits,
                    egress,
                    ceiling,
                    on_parent_end: if promote {
                        OnParentEnd::Promote
                    } else {
                        OnParentEnd::Cascade
                    },
                    cpus,
                    cpu_weight,
                    cpu_max,
                }
            },
        )
}

/// Requests of version 3 too.
fn request_v3() -> impl Strategy<Value = Request> {
    let list = || proptest::collection::vec(bytes(), 0..4);
    prop_oneof![
        (
            bytes(),
            bytes(),
            list(),
            list(),
            proptest::collection::vec((bytes(), bytes()), 0..3),
            any::<bool>(),
        )
            .prop_map(
                |(image, files_from, argv, envp, files, input)| Request::Run(Run {
                    image,
                    files_from,
                    argv,
                    envp,
                    files,
                    input,
                    ..Run::default()
                })
            ),
        Just(Request::List),
        any::<u32>().prop_map(Request::Suspend),
        any::<u32>().prop_map(Request::Resume),
        (any::<u32>(), any::<u32>()).prop_map(|(cell, n)| Request::fork(cell, n)),
        any::<u32>().prop_map(Request::Kill),
        any::<u32>().prop_map(Request::Output),
        (any::<u32>(), bytes(), bytes()).prop_map(|(cell, path, data)| Request::WriteFile {
            cell,
            path,
            data
        }),
        Just(Request::Stats),
        Just(Request::Shutdown),
        (any::<u32>(), bytes()).prop_map(|(cell, data)| Request::Input { cell, data }),
        any::<u32>().prop_map(Request::CloseInput),
        any::<u32>().prop_map(Request::Freeze),
        (any::<u32>(), host()).prop_map(|(cell, host)| Request::Save { cell, host }),
        (any::<u64>(), any::<bool>(), host()).prop_map(|(size, template, host)| Request::Load {
            size,
            template,
            host,
            placement: Placement::root(),
        }),
    ]
}

/// Requests of version 4: those of version 3, placed, and the tree's.
fn request() -> impl Strategy<Value = Request> {
    let leaf = prop_oneof![
        request_v3(),
        (request_v3(), placement(), any::<bool>()).prop_map(|(r, p, watch)| match r {
            Request::Run(run) => Request::Run(Run {
                placement: p,
                watch,
                ..run
            }),
            Request::Fork { cell, n, .. } => Request::Fork {
                cell,
                n,
                placement: p,
                watch,
            },
            Request::Load {
                size,
                template,
                host,
                ..
            } => Request::Load {
                size,
                template,
                host,
                placement: p,
            },
            other => other,
        }),
        any::<u32>().prop_map(Request::KillTree),
        any::<u32>().prop_map(Request::SuspendTree),
        any::<u32>().prop_map(Request::ResumeTree),
        (any::<u32>(), any::<u32>()).prop_map(|(cell, to)| Request::Reparent { cell, to }),
        (any::<u32>(), egress(), proptest::option::of(egress())).prop_map(
            |(cell, egress, ceiling)| Request::Policy {
                cell,
                egress,
                ceiling
            }
        ),
        (any::<u32>(), limits()).prop_map(|(cell, limits)| Request::Limits { cell, limits }),
        (egress(), egress()).prop_map(|(d, c)| {
            let rules = |e: Egress| match e {
                Egress::Rules(r) => r,
                _ => Vec::new(),
            };
            Request::Floor {
                deny: rules(d),
                carved: rules(c),
            }
        }),
    ];
    prop_oneof![
        3 => leaf.clone(),
        1 => (any::<u32>(), leaf).prop_map(|(root, r)| Request::Scoped {
            root,
            request: Box::new(r)
        }),
    ]
}

fn state() -> impl Strategy<Value = State> {
    prop_oneof![
        Just(State::Running),
        any::<u64>().prop_map(|bytes| State::Suspended { bytes }),
        any::<i32>().prop_map(State::Exited),
        Just(State::Fault),
        Just(State::Killed),
        Just(State::Template),
    ]
}

fn message() -> impl Strategy<Value = Message> {
    let body = prop_oneof![
        Just(Body::None),
        any::<u32>().prop_map(Body::Cell),
        proptest::collection::vec(any::<u32>(), 0..5).prop_map(Body::Cells),
        any::<u64>().prop_map(Body::Bytes),
        proptest::collection::vec(
            (
                any::<u32>(),
                bytes(),
                state(),
                proptest::option::of((any::<[u32; 2]>(), bytes(), limits(), egress()))
            )
                .prop_map(|(id, image, state, tree)| CellInfo {
                    id,
                    image,
                    state,
                    tree: tree.map(|([parent, root], label, limits, egress)| TreeInfo {
                        parent,
                        root,
                        label,
                        limits,
                        egress,
                    }),
                }),
            0..4
        )
        .prop_map(Body::List),
        any::<[u32; 3]>().prop_map(|[running, suspended, ended]| Body::Stats(Stats {
            running,
            suspended,
            ended,
            templates: 4,
            snapshot_bytes: 1,
            memory_total: 2,
            memory_free: 3,
        })),
    ];
    prop_oneof![
        any::<u8>().prop_map(|version| Message::Hello { version }),
        (any::<u64>(), body).prop_map(|(ns, body)| Message::Done { ns, body }),
        (any::<u64>(), 1u8..=13, bytes()).prop_map(|(ns, code, message)| Message::Failed {
            ns,
            error: Error::from_code(code).expect("a code"),
            message
        }),
        (any::<u32>(), bytes()).prop_map(|(cell, data)| Message::Output { cell, data }),
        (any::<u32>(), state()).prop_map(|(cell, state)| Message::Ended { cell, state }),
    ]
}

/// Text as the kernel's console prints it: never the start byte.
fn console_text() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(
        any::<u8>().prop_filter("not a frame start", |&b| b != 1),
        0..16,
    )
}

proptest! {
    #[test]
    fn requests_round_trip_between_text(
        requests in proptest::collection::vec((console_text(), request(), any::<u32>()), 0..6),
    ) {
        let mut stream = Vec::new();
        let mut all_text = Vec::new();
        for (text, request, id) in &requests {
            stream.extend(text);
            all_text.extend(text);
            stream.extend(request.encode(*id));
        }
        let found = frames(&stream);
        prop_assert_eq!(found.len(), requests.len());
        for (frame, (_, request, id)) in found.iter().zip(&requests) {
            prop_assert_eq!(frame.id, *id);
            prop_assert_eq!(&Request::decode(frame).unwrap(), request);
        }
        prop_assert_eq!(text(&stream), all_text);
    }

    #[test]
    fn version_3_requests_round_trip(request in request_v3(), id in any::<u32>()) {
        let bytes = request.encode_as(PREVIOUS, id).unwrap();
        let found = host_frames(&bytes);
        prop_assert_eq!(found.len(), 1);
        prop_assert_eq!(Request::decode(&found[0]).unwrap(), request);
    }

    #[test]
    fn version_3_messages_round_trip(message in message(), id in any::<u32>()) {
        // Version 3 has no place in the tree: rows come back without one.
        let found = host_frames(&message.encode_as(PREVIOUS, id));
        let expected = match message {
            Message::Done { ns, body: Body::List(cells) } => Message::Done {
                ns,
                body: Body::List(
                    cells.into_iter().map(|c| CellInfo { tree: None, ..c }).collect(),
                ),
            },
            other => other,
        };
        prop_assert_eq!(Message::decode(&found[0]).unwrap(), expected);
    }

    #[test]
    fn messages_round_trip(message in message(), id in any::<u32>()) {
        let found = frames(&message.encode(id));
        prop_assert_eq!(found.len(), 1);
        prop_assert_eq!(found[0].id, id);
        prop_assert_eq!(Message::decode(&found[0]).unwrap(), message);
    }

    /// Any bytes at all: the decoder and both decoders of payloads never
    /// panic, and never return a payload longer than the bytes given.
    #[test]
    fn any_bytes_never_panic(input in proptest::collection::vec(any::<u8>(), 0..512)) {
        for item in items(&input) {
            if let Item::Frame(frame) = item {
                prop_assert!(frame.payload.len() <= input.len());
                let _ = Request::decode(&frame);
                let _ = Message::decode(&frame);
            }
        }
        // The host's decoder reads frames of every version.
        for frame in host_frames(&input) {
            prop_assert!(frame.payload.len() <= input.len());
            let _ = Request::decode(&frame);
            let _ = Message::decode(&frame);
        }
    }

    /// Valid frames with one corrupted byte: no panic, and while the
    /// damage leaves the length readable, the next frame is found.
    #[test]
    fn damaged_frames_are_survived(
        request in request(),
        at in any::<prop::sample::Index>(),
        byte in any::<u8>(),
    ) {
        let mut bytes = request.encode(1);
        let at = at.index(bytes.len());
        bytes[at] = byte;
        // Its start and its length must be readable.
        let length_intact = at >= 2 && !(8..12).contains(&at);
        bytes.extend(Request::List.encode(2));
        let found = frames(&bytes);
        for frame in &found {
            let _ = Request::decode(frame);
            let _ = Message::decode(frame);
        }
        if length_intact {
            prop_assert!(found.iter().any(|f| f.id == 2 && f.payload.is_empty()));
        }
    }

    /// Any `input` payload decodes within its bound or is refused.
    #[test]
    fn any_input_payload_is_bounded(cell in any::<u32>(), len in any::<u32>(), tail in proptest::collection::vec(any::<u8>(), 0..64)) {
        let mut payload = cell.to_le_bytes().to_vec();
        payload.extend(len.to_le_bytes());
        payload.extend(&tail);
        match Request::decode(&Frame { version: VERSION, kind: 0x0b, id: 1, payload }) {
            Ok(Request::Input { data, .. }) => {
                prop_assert!(data.len() <= MAX_INPUT && data.len() == len as usize);
            }
            Ok(other) => prop_assert!(false, "decoded as {:?}", other),
            Err(e) => prop_assert!(e == Error::Malformed || e == Error::TooLarge),
        }
    }

    /// Any payload under any request or message kind decodes or is refused.
    #[test]
    fn any_payload_never_panics(kind in any::<u8>(), payload in proptest::collection::vec(any::<u8>(), 0..256)) {
        let frame = Frame { version: VERSION, kind, id: 1, payload };
        let _ = Request::decode(&frame);
        let _ = Message::decode(&frame);
    }
}

mod audit;
