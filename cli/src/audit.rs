//! The host's audit log (design D18 of `add-cell-hierarchy`): the
//! background server appends each record the kernel sends as one JSON
//! line to `<state>/audit/audit-<n>.jsonl`, and each line carries the
//! SHA-256 of the line before it, across files and boots. `rhizo audit
//! verify` walks that chain; `rhizo audit tail` shows the records.
//!
//! A file starts with a line naming its boot and the file before it. A
//! new file starts at each `rhizo up`, and when one reaches
//! [`FILE_MAX`]. `audit/head` holds the last line's file and hash, so
//! that cutting the end of the log, or editing its last line, shows too.

use rhizo_control::audit::{Kind, Record};
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Bytes of a file after which the next record starts another.
const FILE_MAX: u64 = 64 << 20;
/// The hash before the first line.
const ZERO: [u8; 32] = [0; 32];

/// SHA-256 (FIPS 180-4) of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            (hh, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(t2));
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0; 32];
    for (chunk, word) in out.chunks_exact_mut(4).zip(h) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The audit directory of the state directory `state`.
pub fn dir(state: &Path) -> PathBuf {
    state.join("audit")
}

/// The log's files, oldest first.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<(u64, PathBuf)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let n = name.strip_prefix("audit-")?.strip_suffix(".jsonl")?;
            Some((n.parse().ok()?, e.path()))
        })
        .collect();
    files.sort();
    files.into_iter().map(|(_, p)| p).collect()
}

fn name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The log, as the background server appends to it.
#[derive(Debug)]
pub struct Log {
    dir: PathBuf,
    boot: u64,
    file: File,
    path: PathBuf,
    index: u64,
    size: u64,
    /// The hash of the last line.
    last: [u8; 32],
    /// Lines written since the last sync, and when that was.
    unsynced: bool,
    synced: Instant,
}

impl Log {
    /// Opens the log in `dir` for the VM booted at `boot` (seconds since
    /// the epoch): a new file, chained to the last line of the one before.
    pub fn open(dir: &Path, boot: u64) -> Result<Self, String> {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let before = files(dir).pop();
        let (index, last) = match &before {
            None => (0, ZERO),
            Some(path) => {
                let text =
                    fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                let line = text
                    .split(|&b| b == b'\n')
                    .rev()
                    .find(|l| !l.is_empty())
                    .unwrap_or_default();
                let index = name(path)
                    .trim_start_matches("audit-")
                    .trim_end_matches(".jsonl")
                    .parse::<u64>()
                    .unwrap_or(0);
                (index + 1, if line.is_empty() { ZERO } else { sha256(line) })
            }
        };
        let after = before.as_deref().map(name).unwrap_or_default();
        let mut log = Self::create(dir, boot, index, last)?;
        log.start(&after)?;
        Ok(log)
    }

    fn create(dir: &Path, boot: u64, index: u64, last: [u8; 32]) -> Result<Self, String> {
        let path = dir.join(format!("audit-{index:06}.jsonl"));
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
        Ok(Log {
            dir: dir.to_path_buf(),
            boot,
            file,
            path,
            index,
            size: 0,
            last,
            unsynced: false,
            synced: Instant::now(),
        })
    }

    /// Writes the first line of a file: its boot and the file before.
    fn start(&mut self, after: &str) -> Result<(), String> {
        self.line(&json!({
            "prev": hex(&self.last),
            "start": { "boot": self.boot, "after": after },
        }))
    }

    /// Appends `record`, before anyone hears of it.
    pub fn append(&mut self, record: &Record) -> Result<(), String> {
        if self.size >= FILE_MAX {
            let after = name(&self.path);
            self.sync();
            *self = Self::create(&self.dir, self.boot, self.index + 1, self.last)?;
            self.start(&after)?;
        }
        self.line(&json!({
            "prev": hex(&self.last),
            "seq": record.seq,
            "ns": record.ns,
            "kind": record.kind.as_str(),
            "cell": record.cell,
            "domain": record.domain,
            "count": record.count,
            "dropped": record.dropped,
            "detail": String::from_utf8_lossy(&record.detail),
        }))
    }

    fn line(&mut self, value: &Value) -> Result<(), String> {
        let line = value.to_string();
        let mut bytes = line.clone().into_bytes();
        bytes.push(b'\n');
        self.file
            .write_all(&bytes)
            .map_err(|e| format!("cannot write {}: {e}", self.path.display()))?;
        self.size += bytes.len() as u64;
        self.last = sha256(line.as_bytes());
        self.unsynced = true;
        let head = format!("{} {}\n", name(&self.path), hex(&self.last));
        let tmp = self.dir.join("head.new");
        fs::write(&tmp, head)
            .and_then(|()| fs::rename(&tmp, self.dir.join("head")))
            .map_err(|e| format!("cannot write {}: {e}", self.dir.join("head").display()))
    }

    /// Whether lines wait to be synced to the disk.
    pub fn unsynced(&self) -> bool {
        self.unsynced
    }

    /// Syncs what was written if a second passed since the last time.
    pub fn sync_due(&mut self) {
        if self.unsynced && self.synced.elapsed() >= Duration::from_secs(1) {
            self.sync();
        }
    }

    /// Syncs what was written to the disk.
    pub fn sync(&mut self) {
        let _ = self.file.sync_data();
        (self.unsynced, self.synced) = (false, Instant::now());
    }
}

/// One line of the log, where it is.
#[derive(Debug)]
struct Line {
    file: String,
    number: usize,
    raw: Vec<u8>,
    value: Option<Value>,
}

impl Line {
    fn at(&self) -> String {
        format!("{}:{}", self.file, self.number)
    }
    fn u64(&self, key: &str) -> Option<u64> {
        self.value.as_ref()?.get(key)?.as_u64()
    }
    fn is_start(&self) -> bool {
        self.value
            .as_ref()
            .is_some_and(|v| v.get("start").is_some())
    }
}

/// Every line of the log, oldest first.
fn lines(dir: &Path) -> Result<Vec<Line>, String> {
    let mut out = Vec::new();
    for path in files(dir) {
        let text = fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let file = name(&path);
        for (k, raw) in text.split(|&b| b == b'\n').enumerate() {
            if raw.is_empty() {
                continue;
            }
            out.push(Line {
                file: file.clone(),
                number: k + 1,
                raw: raw.to_vec(),
                value: serde_json::from_slice(raw).ok(),
            });
        }
    }
    Ok(out)
}

/// What `rhizo audit verify` found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Records read.
    pub records: usize,
    /// Records the kernel dropped, its ring being full.
    pub dropped: u64,
    /// Every break in the chain, each naming where.
    pub problems: Vec<String>,
}

/// Walks the chain of the log in `dir`.
pub fn verify(dir: &Path) -> Result<Report, String> {
    let lines = lines(dir)?;
    let mut report = Report::default();
    let mut prev = ZERO;
    let mut boot: Option<u64> = None;
    let mut seq: Option<u64> = None;
    let mut before: Option<&Line> = None;
    for line in &lines {
        let Some(value) = &line.value else {
            report
                .problems
                .push(format!("{}: edited: not a record of the log", line.at()));
            prev = sha256(&line.raw);
            before = Some(line);
            continue;
        };
        let linked = value.get("prev").and_then(Value::as_str) == Some(hex(&prev).as_str());
        if line.is_start() {
            let start = &value["start"];
            let after = start["after"].as_str().unwrap_or_default();
            let expected = before.map(|b| b.file.as_str()).unwrap_or_default();
            if !linked || after != expected {
                report.problems.push(format!(
                    "{}: the log before this file was edited or removed (it follows `{after}`)",
                    line.at()
                ));
            }
            let this = start["boot"].as_u64();
            if this != boot {
                (boot, seq) = (this, None);
            }
        } else {
            report.records += 1;
            let (s, dropped) = (
                line.u64("seq").unwrap_or(0),
                line.u64("dropped").unwrap_or(0),
            );
            report.dropped += dropped;
            let gap = seq.map(|p| (p, s.checked_sub(p + 1 + dropped)));
            if !linked {
                report.problems.push(match (before, gap) {
                    (Some(b), Some((_, Some(0)))) | (Some(b), None) => {
                        format!(
                            "{}: edited (the next line's hash does not match it)",
                            b.at()
                        )
                    }
                    _ => format!("{}: removed: records before this line", line.at()),
                });
            }
            match gap {
                Some((p, None)) => report
                    .problems
                    .push(format!("{}: reordered: seq {s} after seq {p}", line.at())),
                Some((p, Some(n))) if n > 0 => report.problems.push(format!(
                    "{}: missing: {n} records between seq {p} and {s} never reached the log",
                    line.at()
                )),
                _ => {}
            }
            seq = Some(s);
        }
        prev = sha256(&line.raw);
        before = Some(line);
    }
    match fs::read_to_string(dir.join("head")) {
        Ok(head) => {
            let expected = before
                .map(|b| format!("{} {}", b.file, hex(&prev)))
                .unwrap_or_default();
            if head.trim() != expected {
                report.problems.push(format!(
                    "the end of the log was removed or its last line edited (head: {})",
                    head.trim()
                ));
            }
        }
        Err(_) if lines.is_empty() => {}
        Err(_) => report.problems.push("audit/head is missing".into()),
    }
    Ok(report)
}

/// Which records `rhizo audit tail` shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub cell: Option<u32>,
    pub domain: Option<u32>,
    pub kind: Option<Kind>,
}

impl Filter {
    fn keeps(&self, v: &Value) -> bool {
        let is = |key: &str, want: Option<u32>| {
            want.is_none_or(|w| v.get(key).and_then(Value::as_u64) == Some(u64::from(w)))
        };
        is("cell", self.cell)
            && is("domain", self.domain)
            && self
                .kind
                .is_none_or(|k| v.get("kind").and_then(Value::as_str) == Some(k.as_str()))
    }
}

/// A record for people.
fn show(v: &Value) -> String {
    let n = |key: &str| v.get(key).and_then(Value::as_u64).unwrap_or(0);
    let mut out = format!(
        "{:>6}  {:>12.3}s  {:<16}  cell {:<4} domain {:<4}",
        n("seq"),
        n("ns") as f64 / 1e9,
        v.get("kind").and_then(Value::as_str).unwrap_or("?"),
        n("cell"),
        n("domain"),
    );
    if n("count") > 1 {
        out.push_str(&format!(" x{}", n("count")));
    }
    if n("dropped") > 0 {
        out.push_str(&format!(" ({} dropped before)", n("dropped")));
    }
    out.push_str("  ");
    out.push_str(v.get("detail").and_then(Value::as_str).unwrap_or(""));
    out
}

/// The records of the log in `dir` that `filter` keeps, for people, from
/// line `from` of the whole log on; and the number of lines.
pub fn tail(dir: &Path, filter: &Filter, from: usize) -> Result<(Vec<String>, usize), String> {
    let lines = lines(dir)?;
    let shown = lines
        .iter()
        .skip(from)
        .filter(|l| !l.is_start())
        .filter_map(|l| l.value.as_ref())
        .filter(|v| filter.keeps(v))
        .map(show)
        .collect();
    Ok((shown, lines.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rhizo-audit-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn record(seq: u64, kind: Kind, cell: u32, dropped: u64) -> Record {
        Record {
            seq,
            ns: seq * 1000,
            kind,
            cell,
            domain: 1,
            count: 1,
            dropped,
            detail: format!("record {seq}").into_bytes(),
        }
    }

    /// A log of a grant-like policy change, a move and an egress denial,
    /// then a restart of `rhizo` (a new boot) and two more.
    fn written(dir: &Path) {
        let mut log = Log::open(dir, 100).unwrap();
        log.append(&record(1, Kind::Policy, 3, 0)).unwrap();
        log.append(&record(2, Kind::Moved, 4, 0)).unwrap();
        log.append(&record(3, Kind::Egress, 4, 0)).unwrap();
        log.sync();
        drop(log);
        let mut log = Log::open(dir, 200).unwrap();
        log.append(&record(1, Kind::Scope, 5, 0)).unwrap();
        log.append(&record(2, Kind::Created, 6, 0)).unwrap();
    }

    fn edit(path: &Path, line: usize, f: impl FnOnce(&mut String)) {
        let text = fs::read_to_string(path).unwrap();
        let mut lines: Vec<String> = text.lines().map(String::from).collect();
        f(&mut lines[line - 1]);
        fs::write(path, lines.join("\n") + "\n").unwrap();
    }

    #[test]
    fn sha256_matches_fips_180_vectors() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            hex(&sha256(&[b'a'; 1_000_000])),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn audit_log_verifies_and_survives_a_restart() {
        let dir = temp("restart");
        written(&dir);
        let report = verify(&dir).unwrap();
        assert_eq!(report.problems, Vec::<String>::new());
        assert_eq!((report.records, report.dropped), (5, 0));
        // The second boot's file is chained to the first's last line.
        let second = fs::read_to_string(dir.join("audit-000001.jsonl")).unwrap();
        let first = fs::read_to_string(dir.join("audit-000000.jsonl")).unwrap();
        let start: Value = serde_json::from_str(second.lines().next().unwrap()).unwrap();
        assert_eq!(
            start["prev"],
            hex(&sha256(first.lines().last().unwrap().as_bytes()))
        );
        assert_eq!(start["start"]["after"], "audit-000000.jsonl");
        let (shown, _) = tail(&dir, &Filter::default(), 0).unwrap();
        assert_eq!(shown.len(), 5);
        assert!(shown[2].contains("egress-denied") && shown[2].contains("record 3"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn audit_tail_filters() {
        let dir = temp("tail");
        written(&dir);
        let only = |filter: Filter| tail(&dir, &filter, 0).unwrap().0;
        assert_eq!(
            only(Filter {
                cell: Some(4),
                ..Filter::default()
            })
            .len(),
            2
        );
        let scope = only(Filter {
            kind: Some(Kind::Scope),
            ..Filter::default()
        });
        assert_eq!(scope.len(), 1);
        assert!(scope[0].contains("scope-denied") && scope[0].contains("cell 5"));
        assert_eq!(
            only(Filter {
                domain: Some(9),
                ..Filter::default()
            })
            .len(),
            0
        );
        // From a line on: what `--follow` prints next.
        let (_, n) = tail(&dir, &Filter::default(), 0).unwrap();
        assert_eq!(tail(&dir, &Filter::default(), n).unwrap().0.len(), 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_edited_audit_record_is_named() {
        let dir = temp("edited");
        written(&dir);
        edit(&dir.join("audit-000000.jsonl"), 3, |l| {
            *l = l.replace("\"cell\":4", "\"cell\":9");
        });
        let report = verify(&dir).unwrap();
        assert_eq!(
            report.problems,
            ["audit-000000.jsonl:3: edited (the next line's hash does not match it)"]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_removed_audit_record_is_found() {
        let dir = temp("removed");
        written(&dir);
        let path = dir.join("audit-000000.jsonl");
        let text = fs::read_to_string(&path).unwrap();
        let kept: Vec<&str> = text
            .lines()
            .enumerate()
            .filter(|(k, _)| *k != 2)
            .map(|(_, l)| l)
            .collect();
        fs::write(&path, kept.join("\n") + "\n").unwrap();
        let report = verify(&dir).unwrap();
        assert!(
            report
                .problems
                .iter()
                .any(|p| p.starts_with("audit-000000.jsonl:3: removed")),
            "{:?}",
            report.problems
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_removed_audit_file_and_a_cut_end_are_found() {
        let dir = temp("file");
        written(&dir);
        let mut log = Log::open(&dir, 300).unwrap();
        log.append(&record(1, Kind::Limits, 7, 0)).unwrap();
        drop(log);
        fs::remove_file(dir.join("audit-000001.jsonl")).unwrap();
        let report = verify(&dir).unwrap();
        assert!(
            report
                .problems
                .iter()
                .any(|p| p.starts_with("audit-000002.jsonl:1: the log before")),
            "{:?}",
            report.problems
        );
        fs::remove_dir_all(&dir).unwrap();
        // The last line removed.
        let dir = temp("cut");
        written(&dir);
        let path = dir.join("audit-000001.jsonl");
        let text = fs::read_to_string(&path).unwrap();
        let kept: Vec<&str> = text.lines().take(2).collect();
        fs::write(&path, kept.join("\n") + "\n").unwrap();
        let report = verify(&dir).unwrap();
        assert!(
            report
                .problems
                .iter()
                .any(|p| p.starts_with("the end of the log")),
            "{:?}",
            report.problems
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Records lost between the kernel and the host show as a gap; records
    /// the kernel dropped show as its count, and are no gap.
    #[test]
    fn missing_and_dropped_audit_records_are_told_apart() {
        let dir = temp("missing");
        let mut log = Log::open(&dir, 100).unwrap();
        log.append(&record(1, Kind::Created, 1, 0)).unwrap();
        log.append(&record(5, Kind::Created, 1, 3)).unwrap(); // 3 dropped
        log.append(&record(9, Kind::Created, 1, 0)).unwrap(); // 3 missing
        log.append(&record(8, Kind::Created, 1, 0)).unwrap();
        drop(log);
        let report = verify(&dir).unwrap();
        assert_eq!(report.dropped, 3);
        assert_eq!(
            report.problems,
            [
                "audit-000000.jsonl:4: missing: 3 records between seq 5 and 9 never reached the log",
                "audit-000000.jsonl:5: reordered: seq 8 after seq 9",
            ]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_full_audit_file_continues_in_the_next() {
        let dir = temp("rotate");
        let mut log = Log::open(&dir, 100).unwrap();
        log.append(&record(1, Kind::Created, 1, 0)).unwrap();
        log.size = FILE_MAX; // as if it were full
        log.append(&record(2, Kind::Created, 1, 0)).unwrap();
        drop(log);
        assert_eq!(files(&dir).len(), 2);
        assert_eq!(verify(&dir).unwrap().problems, Vec::<String>::new());
        fs::remove_dir_all(&dir).unwrap();
    }

    fn reference(data: &[u8]) -> [u8; 32] {
        use sha2::Digest;
        sha2::Sha256::digest(data).into()
    }

    /// The lengths where padding goes wrong: one block or two, the length
    /// fitting after the 0x80 byte or not.
    #[test]
    fn sha256_matches_rustcrypto_at_padding_boundaries() {
        for len in [0, 1, 55, 56, 57, 63, 64, 65, 119, 120, 127, 128, 129, 1000] {
            let data: Vec<u8> = (0..len).map(|k| (k * 31 + 7) as u8).collect();
            assert_eq!(sha256(&data), reference(&data), "{len} bytes");
        }
    }

    proptest! {
        #[test]
        fn sha256_matches_rustcrypto(data in proptest::collection::vec(any::<u8>(), 0..=1000)) {
            prop_assert_eq!(sha256(&data), reference(&data));
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// Any change of any byte of the log, or a line removed or two
        /// swapped, is reported.
        #[test]
        fn any_audit_edit_is_detected(
            at in any::<prop::sample::Index>(),
            byte in any::<u8>(),
            how in 0u8..3,
        ) {
            let dir = temp("prop");
            written(&dir);
            let path = files(&dir)[at.index(2)].clone();
            let text = fs::read(&path).unwrap();
            let mut lines: Vec<Vec<u8>> =
                text.split(|&b| b == b'\n').filter(|l| !l.is_empty()).map(<[u8]>::to_vec).collect();
            let k = at.index(lines.len());
            match how {
                0 => {
                    let line = &mut lines[k];
                    let i = at.index(line.len());
                    prop_assume!(line[i] != byte && byte != b'\n');
                    line[i] = byte;
                }
                1 => {
                    lines.remove(k);
                }
                _ => {
                    let j = (k + 1) % lines.len();
                    lines.swap(k, j);
                }
            }
            let mut out = lines.join(&b'\n');
            out.push(b'\n');
            fs::write(&path, out).unwrap();
            let report = verify(&dir).unwrap();
            fs::remove_dir_all(&dir).unwrap();
            prop_assert!(!report.problems.is_empty());
        }
    }
}
