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
    })
}

#[test]
fn a_frame_has_its_header_then_its_payload() {
    let bytes = Request::Suspend(7).encode(0x0102_0304);
    assert_eq!(
        bytes,
        [
            0x01, b'Z', VERSION, 0x03, 0x04, 0x03, 0x02, 0x01, 4, 0, 0, 0, 7, 0, 0, 0
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
        Request::Fork { cell: 3, n: 4 },
        Request::Kill(5),
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
        Request::Save(12),
        Request::Load {
            size: 40 << 20,
            template: true,
        },
        Request::LoadData {
            cell: 13,
            data: vec![0x01; CHUNK],
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
                },
                CellInfo {
                    id: 2,
                    image: b"idle".to_vec(),
                    state: State::Suspended { bytes: 4096 },
                },
                CellInfo {
                    id: 3,
                    image: b"exit3".to_vec(),
                    state: State::Exited(-3),
                },
                CellInfo {
                    id: 4,
                    image: Vec::new(),
                    state: State::Fault,
                },
                CellInfo {
                    id: 5,
                    image: b"hog".to_vec(),
                    state: State::Killed,
                },
                CellInfo {
                    id: 6,
                    image: b"cpython".to_vec(),
                    state: State::Template,
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
        Message::Data(vec![0x01, b'Z', 0, 0xff]),
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
    // `run`'s input flag is 0 or 1.
    let mut run = Request::Run(Run::default()).encode(1);
    let last = run.len() - 1;
    run[last] = 2;
    assert_eq!(Request::decode(&frames(&run)[0]), Err(Error::Malformed));
    // Snapshot bytes come in chunks of at most `CHUNK`, and `load`'s
    // template flag is 0 or 1.
    let big = Request::LoadData {
        cell: 1,
        data: vec![0; CHUNK + 1],
    };
    assert_eq!(
        Request::decode(&frames(&big.encode(4))[0]),
        Err(Error::TooLarge)
    );
    let mut load = Request::Load {
        size: 1,
        template: false,
    }
    .encode(5);
    let last = load.len() - 1;
    load[last] = 2;
    assert_eq!(Request::decode(&frames(&load)[0]), Err(Error::Malformed));
}

fn bytes() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(any::<u8>(), 0..24)
}

fn request() -> impl Strategy<Value = Request> {
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
                    input
                })
            ),
        Just(Request::List),
        any::<u32>().prop_map(Request::Suspend),
        any::<u32>().prop_map(Request::Resume),
        (any::<u32>(), any::<u32>()).prop_map(|(cell, n)| Request::Fork { cell, n }),
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
        any::<u32>().prop_map(Request::Save),
        (any::<u64>(), any::<bool>()).prop_map(|(size, template)| Request::Load { size, template }),
        (any::<u32>(), bytes()).prop_map(|(cell, data)| Request::LoadData { cell, data }),
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
            (any::<u32>(), bytes(), state()).prop_map(|(id, image, state)| CellInfo {
                id,
                image,
                state
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
        (any::<u64>(), 1u8..=11, bytes()).prop_map(|(ns, code, message)| Message::Failed {
            ns,
            error: Error::from_code(code).expect("a code"),
            message
        }),
        (any::<u32>(), bytes()).prop_map(|(cell, data)| Message::Output { cell, data }),
        (any::<u32>(), state()).prop_map(|(cell, state)| Message::Ended { cell, state }),
        bytes().prop_map(Message::Data),
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
        match Request::decode(&Frame { kind: 0x0b, id: 1, payload }) {
            Ok(Request::Input { data, .. }) => {
                prop_assert!(data.len() <= MAX_INPUT && data.len() == len as usize);
            }
            Ok(other) => prop_assert!(false, "decoded as {:?}", other),
            Err(e) => prop_assert!(e == Error::Malformed || e == Error::TooLarge),
        }
    }

    /// Any `load-data` payload decodes within its bound or is refused.
    #[test]
    fn any_load_data_payload_is_bounded(cell in any::<u32>(), len in any::<u32>(), tail in proptest::collection::vec(any::<u8>(), 0..64)) {
        let mut payload = cell.to_le_bytes().to_vec();
        payload.extend(len.to_le_bytes());
        payload.extend(&tail);
        match Request::decode(&Frame { kind: 0x10, id: 1, payload }) {
            Ok(Request::LoadData { data, .. }) => {
                prop_assert!(data.len() <= CHUNK && data.len() == len as usize);
            }
            Ok(other) => prop_assert!(false, "decoded as {:?}", other),
            Err(e) => prop_assert!(e == Error::Malformed || e == Error::TooLarge),
        }
    }

    /// Any payload under any request or message kind decodes or is refused.
    #[test]
    fn any_payload_never_panics(kind in any::<u8>(), payload in proptest::collection::vec(any::<u8>(), 0..256)) {
        let frame = Frame { kind, id: 1, payload };
        let _ = Request::decode(&frame);
        let _ = Message::decode(&frame);
    }
}
