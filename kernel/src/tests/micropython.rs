//! MicroPython.

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[ktest_case!(micropython_in_a_cell)];

/// MicroPython running `cells/python/rhizo_demo.py`, imported from the
/// cell's own files, prints what it prints on Linux; frozen after its
/// warm-up as a template, every cell forked from it answers without warming
/// up again. Built only when its source is given (`RHIZO_MICROPYTHON_SRC`,
/// set in CI); otherwise the test says it did not run.
fn micropython_in_a_cell() {
    let image = crate::cell::test_image("micropython");
    if image.is_empty() {
        arch::console::BootConsole.write_bytes(b"(MicroPython not built: not run) ");
        return;
    }
    let files = || {
        let mut fs = rhizo_fs::Fs::new();
        fs.put_file(
            b"/lib/rhizo_demo.py",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../cells/python/rhizo_demo.py"
            )),
        )
        .expect("module");
        fs
    };
    let argv: [&[u8]; 3] = [b"micropython", b"-c", expected::PYTHON_COMMAND.as_bytes()];
    let mut host = Host::new();
    let id = host
        .spawn_with_files(image, &argv, files(), true, true)
        .expect("spawn");
    host.run(|_| false);
    assert_eq!(
        ended(&host, id),
        (Ending::Exit(0), expected::PYTHON_STDOUT.into())
    );
    let template = host
        .spawn_with_files(image, &argv, files(), true, false)
        .expect("spawn");
    host.run_to_pause(template, b"warm", Pause::Waits)
        .unwrap_or_else(|why| panic!("{why}"));
    assert!(host.freeze(template));
    let answer = expected::PYTHON_STDOUT
        .strip_prefix("warm\n")
        .expect("warm first");
    let ids: Vec<u32> = (0..3)
        .map(|_| {
            host.spawn_from_template(template, true, true)
                .expect("fork")
        })
        .collect();
    host.run(|_| false);
    for id in ids {
        assert_eq!(ended(&host, id), (Ending::Exit(0), answer.into()));
    }
}
