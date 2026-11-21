//! The bounds of every structure the hierarchy adds (design D19 of
//! `add-cell-hierarchy`), checked against one another at compile time.

/// Deepest a cell may be below the host: a root is at depth 1 (`EMLINK`).
pub const DEPTH_MAX: u8 = 16;
/// Most children of one cell (`EMLINK`), whatever its descendant budget.
pub const CHILDREN_MAX: u16 = 1024;
/// Most handles one cell holds (`EMFILE`).
pub const HANDLES_MAX: u32 = 256;
/// Longest derivation chain of a grant (`EMLINK`).
pub const CHAIN_MAX: u32 = 16;
/// Most channels one cell holds (`EMFILE`).
pub const CHANNELS_MAX: u32 = 128;
/// Most channels waiting in an inbox (`ECONNREFUSED` for the opener).
pub const INBOX_MAX: u32 = 64;
/// Largest message on a channel, in bytes (`EMSGSIZE`).
pub const MESSAGE_MAX: u32 = 64 << 10;
/// Most handles one message carries (`ETOOMANYREFS`).
pub const MESSAGE_HANDLES_MAX: u32 = 16;
/// Most messages queued on one endpoint (`EAGAIN` or block).
pub const QUEUE_MESSAGES_MAX: u32 = 64;
/// Most bytes queued on one endpoint (`EAGAIN` or block).
pub const QUEUE_BYTES_MAX: u32 = 256 << 10;
/// Most rules in one egress policy (`EINVAL`).
pub const RULES_MAX: usize = 64;
/// Most rules in the host's deny floor (refused by `rhizo up`).
pub const FLOOR_MAX: usize = 64;
/// Longest label, in bytes of `[A-Za-z0-9._-]` (`EINVAL`).
pub const LABEL_MAX: usize = 64;
/// Most `MADV_WIPEONFORK` ranges of one cell (`ENOMEM`).
pub const WIPE_RANGES_MAX: u32 = 64;
/// CPU weights: from 1 to 10,000, 100 when not given (`EINVAL`).
pub const WEIGHT_MIN: u16 = 1;
pub const WEIGHT_MAX: u16 = 10_000;
pub const WEIGHT_DEFAULT: u16 = 100;
/// Bytes of audit records the kernel keeps for the host; past them records
/// are dropped and counted, never blocking.
pub const AUDIT_RING_BYTES: u32 = 64 << 10;

/// Most descriptors any cell may have open (`rhizo_cell::quota::FILES_MAX`,
/// `rhizo_fs::FD_MAX`): the bound the descriptors that stand for handles and
/// channels must fit in.
pub const DESCRIPTORS_MAX: u32 = 1024;

// The bounds fit one another: a cell at every bound still has descriptors
// left for its own files, a full message fits an empty queue, and a
// message never carries more handles than a cell may hold.
const _: () = assert!(HANDLES_MAX + CHANNELS_MAX < DESCRIPTORS_MAX);
const _: () = assert!(MESSAGE_MAX <= QUEUE_BYTES_MAX);
const _: () = assert!(MESSAGE_HANDLES_MAX <= HANDLES_MAX);
const _: () = assert!(INBOX_MAX <= CHANNELS_MAX);
const _: () = assert!(CHAIN_MAX <= DEPTH_MAX as u32);
const _: () = assert!(CHILDREN_MAX as u32 <= DESCRIPTORS_MAX);
const _: () = assert!(FLOOR_MAX <= RULES_MAX);
const _: () = assert!(WEIGHT_MIN <= WEIGHT_DEFAULT && WEIGHT_DEFAULT <= WEIGHT_MAX);
// The audit ring holds a record naming a label for every child of a cell.
const _: () = assert!(AUDIT_RING_BYTES >= CHILDREN_MAX as u32 * LABEL_MAX as u32);
