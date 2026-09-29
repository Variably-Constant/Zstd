//! Zstd: Zstandard (RFC 8878) compression for Windows PowerShell 5.1
//! and PowerShell 7, with libzstd compiled into the module through the
//! zstd crate.
//!
//! `Compress-Zstd` and `Expand-Zstd` work on the files `-Path` or
//! `-LiteralPath` names, or that are piped to them, each written beside
//! itself, into a folder, or to the file `-DestinationPath` names. A file
//! is compressed from a map of it, or read 1 MiB at a time when Windows
//! will not map it or its map does not fit `-MaxMemory`, and expanded
//! 1 MiB at a time, or in place when a frame's window needs it. On the
//! bytes given to `-InputObject`, from the pipeline or by name, the result
//! is written as one `byte[]` when the pipeline ends: `Compress-Zstd`
//! compresses a single `byte[]` in one piece and feeds a longer pipeline
//! to libzstd as its records arrive, and `Expand-Zstd` expands a first
//! record whose frames record their content size straight into the
//! `byte[]` it writes and feeds the other records as they arrive.
//! `-MaxMemory` bounds what either may commit.
//! `New-ZstdDictionary` trains a dictionary from sample files with
//! libzstd's trainer.

use std::cell::Cell;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use pwrs::prelude::*;
use zstd::zstd_safe::{self, zstd_sys, CParameter, DParameter, InBuffer, OutBuffer};

/// The bytes a file operation reads or writes per step. The pipeline's
/// stop flag is checked once per step.
const CHUNK: usize = 1 << 20;

/// What `-Level` and `-Threads` ask libzstd for.
#[derive(Clone, Copy, Debug)]
struct Settings {
    /// The compression level; the binder holds `-Level` to 1..=22.
    level: i32,
    /// Worker threads; 0 compresses on the calling thread.
    threads: u32,
}

impl Settings {
    /// The settings for the parameters as bound, with libzstd's own
    /// default level, 3, when `-Level` was not given.
    fn from_parameters(level: Option<i32>, threads: Option<i32>) -> PsResult<Settings> {
        let threads = match threads {
            None => 0,
            Some(n) => u32::try_from(n).map_err(|e| {
                PsError::new(ErrorCategory::InvalidArgument, "ZstdThreadsInvalid", format!("-Threads {n} is not a thread count: {e}"))
            })?,
        };
        Ok(Settings { level: level.unwrap_or(zstd::DEFAULT_COMPRESSION_LEVEL), threads })
    }
}

/// libzstd's limit on the window of a frame it expands in a stream when
/// no -MaxMemory is given: 2^27 bytes, its ZSTD_WINDOWLOG_LIMIT_DEFAULT.
const WINDOW_LIMIT_DEFAULT: u64 = 1 << 27;

/// The largest window log libzstd expands at all in a 64-bit build, its
/// ZSTD_WINDOWLOG_MAX_64; setting it lifts libzstd's own window limit.
const WINDOW_LOG_MAX: u32 = 31;

/// The job log libzstd's worker threads never exceed in a 64-bit build,
/// its ZSTDMT_JOBLOG_MAX.
const JOB_LOG_MAX: u32 = 30;

/// libzstd's largest block, ZSTD_BLOCKSIZE_MAX.
const BLOCK_MAX: u64 = 1 << 17;

/// The input size up to which libzstd compresses on the calling thread
/// whatever the number of worker threads asked for, its
/// ZSTDMT_JOBSIZE_MIN.
const ONE_THREAD_UP_TO: u64 = 512 << 10;

/// The command's own share of any one operation, which every estimate
/// counts: its buffers, a chunk of input and a chunk of output at most,
/// and 2 MiB for what the host commits while the command runs. Every
/// peak measured in PowerShell 7.6 and Windows PowerShell 5.1 stayed
/// within the estimates this is part of.
const OWN_SHARE: u64 = 2 * CHUNK as u64 + (2 << 20);

/// What each worker thread takes beyond libzstd's sizes, its stack and
/// its job's own state: measured at about 0.4 MB a worker at level 19,
/// with 8 and with 24 workers, and counted as 1 MiB.
const WORKER_SHARE: u64 = 1 << 20;

/// How much of a mapped file one page table maps, 2 MiB, and what that
/// page table takes, 4 KiB: Windows charges the page tables of a mapping
/// to the process's commit as the mapping is touched, 1/512 of it.
const PAGE_TABLE_SPAN: u64 = 2 << 20;
const PAGE_TABLE: u64 = 4 << 10;

/// The page tables a view of `len` bytes of a file can take: one for each
/// 2 MiB it spans, and one more, since a view need not start on a 2 MiB
/// boundary.
fn page_tables(len: u64) -> u64 {
    if len == 0 {
        return 0;
    }
    len.div_ceil(PAGE_TABLE_SPAN).saturating_add(1).saturating_mul(PAGE_TABLE)
}

/// Whether `code`, a libzstd return value, is one of its error codes.
fn is_error(code: usize) -> bool {
    // SAFETY: ZSTD_isError reads nothing but its argument.
    unsafe { zstd_sys::ZSTD_isError(code) != 0 }
}

/// libzstd's error `code` as an I/O error carrying its name.
fn libzstd_error(code: usize) -> io::Error {
    io::Error::other(zstd_safe::get_error_name(code))
}

/// A usize from libzstd as a byte count, or its error.
fn libzstd_size(code: usize) -> io::Result<u64> {
    if is_error(code) {
        return Err(libzstd_error(code));
    }
    u64::try_from(code).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// The compression parameters libzstd chooses for `level` and an input
/// of `size` bytes, or of a size not known in advance. libzstd takes a
/// size of 0 as one not known, so an empty input is asked for as an
/// input of one byte, whose parameters are the smallest.
fn compression_parameters(level: i32, size: Option<u64>) -> zstd_sys::ZSTD_compressionParameters {
    let size = match size {
        Some(0) => 1,
        Some(size) => size,
        None => 0,
    };
    // SAFETY: ZSTD_getCParams computes from its arguments alone.
    unsafe { zstd_sys::ZSTD_getCParams(level, size, 0) }
}

/// Whether libzstd turns long-distance matching on by itself for
/// `parameters`, as its ZSTD_resolveEnableLdm does: a strategy of btopt
/// or above with a window of 2^27 bytes or more, which is level 22 on an
/// input larger than 64 MiB or of a size not known.
fn long_distance(parameters: &zstd_sys::ZSTD_compressionParameters) -> bool {
    parameters.strategy as u32 >= zstd_sys::ZSTD_strategy::ZSTD_btopt as u32 && parameters.windowLog >= 27
}

/// libzstd's estimate, in bytes, of what compressing at `level` on the
/// calling thread takes, for an input of `size` bytes or of a size not
/// known: its context, with the long-distance tables when libzstd turns
/// them on, and its buffers. `stable` input stays where it is for the
/// whole frame, as a mapped file or a pinned `byte[]` does, so libzstd
/// keeps no input buffer for it, only its output buffer of a block.
fn calling_thread_estimate(level: i32, size: Option<u64>, stable: bool) -> io::Result<u64> {
    let parameters = compression_parameters(level, size);
    if !stable {
        // SAFETY: computes from its argument alone.
        return libzstd_size(unsafe { zstd_sys::ZSTD_estimateCStreamSize_usingCParams(parameters) });
    }
    // SAFETY: computes from its argument alone.
    let context = libzstd_size(unsafe { zstd_sys::ZSTD_estimateCCtxSize_usingCParams(parameters) })?;
    let block = BLOCK_MAX.min(1 << parameters.windowLog);
    let output = zstd_safe::compress_bound(usize::try_from(block).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?);
    Ok(context + libzstd_size(output)? + 1)
}

/// The memory, in bytes, compressing at `level` with `workers` worker
/// threads takes, for an input of `size` bytes or of a size not known.
/// libzstd estimates the calling thread only, so this follows how its
/// worker threads allocate (zstdmt_compress.c), from its own sizes:
/// - the round buffer every byte of input is copied into, allocated whole
///   at the start: a job for each worker, or with long-distance matching
///   the window when that is larger, and three jobs more;
/// - for each job running, one per worker and no more than the input has
///   jobs, a compression context, and with long-distance matching a
///   buffer for the sequences it finds, 12 bytes for every 32 or 64 bytes
///   of the job;
/// - for each job from its start until its output is written, an output
///   buffer, for as many jobs as libzstd's table of jobs holds;
/// - with long-distance matching, one table of its hashes;
/// - for each worker, `WORKER_SHARE`.
///
/// An input of `ONE_THREAD_UP_TO` bytes or less is compressed on the
/// calling thread, as libzstd does whatever the workers asked for.
fn workers_estimate(level: i32, size: Option<u64>, workers: u32, stable: bool) -> io::Result<u64> {
    if size.is_some_and(|size| size <= ONE_THREAD_UP_TO) {
        return calling_thread_estimate(level, size, stable);
    }
    let parameters = compression_parameters(level, size);
    let strategy = parameters.strategy as u32;
    let long = long_distance(&parameters);
    let job_log = if long {
        // ZSTD_cycleLog: the chain log, one less for the binary-tree
        // strategies.
        let cycle_log = parameters.chainLog - u32::from(strategy >= zstd_sys::ZSTD_strategy::ZSTD_btlazy2 as u32);
        (cycle_log + 3).max(21)
    } else {
        (parameters.windowLog + 2).max(20)
    };
    let job = 1u64 << job_log.min(JOB_LOG_MAX);
    // SAFETY: computes from its argument alone.
    let context = libzstd_size(unsafe { zstd_sys::ZSTD_estimateCCtxSize_usingCParams(parameters) })?;
    let bound = libzstd_size(zstd_safe::compress_bound(usize::try_from(job).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?))?;
    let workers = u64::from(workers);
    let jobs = size.map_or(u64::MAX, |size| size.div_ceil(job));
    // libzstd's table of jobs holds twice the highest power of two in
    // workers + 2.
    let table = 2u64 << (workers + 2).ilog2();
    let window = if long { 1u64 << parameters.windowLog } else { 0 };
    let round = window.max(workers * job) + 3 * job;
    let (sequences, hashes) = if long {
        // ZSTD_ldm_adjustParameters: a hash log of the window log less 7 -
        // strategy / 3, buckets of 2^strategy entries at most 2^8, and
        // matches of 32 bytes from btultra on, 64 below; ldmEntry_t is 8
        // bytes and rawSeq 12.
        let hash_log = parameters.windowLog.saturating_sub(7 - strategy / 3).clamp(6, 30);
        let bucket_log = strategy.clamp(4, 8).min(hash_log);
        let min_match = if strategy >= zstd_sys::ZSTD_strategy::ZSTD_btultra as u32 { 32 } else { 64 };
        (job / min_match * 12, (8u64 << hash_log) + (1u64 << (hash_log - bucket_log)))
    } else {
        (0, 0)
    };
    Ok(workers.min(jobs) * (context + sequences) + round + table.min(jobs) * bound + hashes + workers * WORKER_SHARE)
}

/// What the first bytes of a frame say about expanding it in a stream.
#[derive(Debug, PartialEq, Eq)]
enum FrameStart {
    /// The header is this many bytes long in all, and fewer have arrived.
    Needs(usize),
    /// A frame, or a skippable frame, whose window is `window` bytes and
    /// which libzstd estimates takes `memory` bytes to expand in a stream,
    /// `u64::MAX` when its window is beyond anything libzstd expands; it
    /// expands to `content` bytes when its header records that.
    Header { window: u64, memory: u64, content: Option<u64> },
    /// Bytes libzstd reads no frame header from; given them, it says why.
    Unreadable,
}

/// Reads the frame header at the start of `bytes`.
fn frame_start(bytes: &[u8]) -> FrameStart {
    let mut header = std::mem::MaybeUninit::<zstd_sys::ZSTD_FrameHeader>::uninit();
    // SAFETY: libzstd reads at most `bytes.len()` bytes from `bytes` and
    // writes one frame header into `header`.
    let code = unsafe { zstd_sys::ZSTD_getFrameHeader(header.as_mut_ptr(), bytes.as_ptr().cast(), bytes.len()) };
    if is_error(code) {
        return FrameStart::Unreadable;
    }
    if code > 0 {
        return FrameStart::Needs(code);
    }
    // SAFETY: a return of 0 means libzstd filled the header in.
    let header = unsafe { header.assume_init() };
    if header.frameType == zstd_sys::ZSTD_FrameType_e::ZSTD_skippableFrame {
        return FrameStart::Header { window: 0, memory: 0, content: Some(0) };
    }
    // SAFETY: as above, reading the same bytes.
    let memory = unsafe { zstd_sys::ZSTD_estimateDStreamSize_fromFrame(bytes.as_ptr().cast(), bytes.len()) };
    let memory = if is_error(memory) { u64::MAX } else { memory as u64 };
    let content = Some(header.frameContentSize).filter(|&size| size != zstd_safe::CONTENTSIZE_UNKNOWN);
    FrameStart::Header { window: header.windowSize, memory, content }
}

/// A dictionary to compress or expand with, as its file or bytes held
/// it. libzstd takes eight or more bytes that begin with zstd's
/// dictionary magic number as a dictionary in zstd's format, and any
/// other bytes as raw content.
#[derive(Debug)]
struct Dictionary {
    bytes: Vec<u8>,
    /// The id a dictionary in zstd's format carries, the four bytes after
    /// its magic number; raw content has none.
    id: Option<u32>,
}

impl Dictionary {
    fn new(bytes: Vec<u8>) -> Dictionary {
        // 0xEC30A437, zstd's dictionary magic number, little-endian (RFC
        // 8878, section 5).
        let id = match bytes.as_slice() {
            [0x37, 0xA4, 0x30, 0xEC, a, b, c, d, ..] => Some(u32::from_le_bytes([*a, *b, *c, *d])),
            _ => None,
        };
        Dictionary { bytes, id }
    }

    /// The bytes to load into libzstd, or none for no dictionary.
    fn bytes_of(dictionary: Option<&Dictionary>) -> &[u8] {
        match dictionary {
            Some(d) => &d.bytes,
            None => &[],
        }
    }

    /// What compressing with `dictionary` at `level` takes besides the
    /// compression itself: the command's copy of its bytes, and libzstd's
    /// copy with the tables it builds from it, as ZSTD_estimateCDictSize
    /// estimates them, one for all the worker threads. No dictionary takes
    /// nothing.
    fn compression_share(dictionary: Option<&Dictionary>, level: i32) -> u64 {
        let Some(dictionary) = dictionary else {
            return 0;
        };
        // SAFETY: computes from its arguments alone, and returns a size,
        // never an error code.
        let libzstd = unsafe { zstd_sys::ZSTD_estimateCDictSize(dictionary.bytes.len(), level) };
        (dictionary.bytes.len() as u64).saturating_add(libzstd as u64)
    }

    /// What expanding with `dictionary` takes besides the expansion
    /// itself: the command's copy of its bytes, and libzstd's copy, as
    /// ZSTD_estimateDDictSize estimates it. No dictionary takes nothing.
    fn expansion_share(dictionary: Option<&Dictionary>) -> u64 {
        let Some(dictionary) = dictionary else {
            return 0;
        };
        // SAFETY: computes from its arguments alone, and returns a size,
        // never an error code.
        let libzstd = unsafe { zstd_sys::ZSTD_estimateDDictSize(dictionary.bytes.len(), zstd_sys::ZSTD_dictLoadMethod_e::ZSTD_dlm_byCopy) };
        (dictionary.bytes.len() as u64).saturating_add(libzstd as u64)
    }

    /// The dictionary `bytes` hold, once libzstd has taken one in zstd's
    /// format both to compress nothing and to expand an empty frame, which
    /// makes it read the dictionary's tables each way; raw content needs no
    /// check. `Err` carries libzstd's reason for refusing it, or is an
    /// `OutOfMemory` error when a context cannot be allocated.
    fn checked(bytes: Vec<u8>) -> io::Result<Dictionary> {
        let dictionary = Dictionary::new(bytes);
        if dictionary.id.is_none() {
            return Ok(dictionary);
        }
        let refused = |code: usize| io::Error::other(zstd::zstd_safe::get_error_name(code));
        let unallocated = || io::Error::new(io::ErrorKind::OutOfMemory, "libzstd could not allocate a context");
        let mut compressor = zstd::zstd_safe::CCtx::try_create().ok_or_else(unallocated)?;
        let mut frame = [0u8; 64];
        compressor.compress_using_dict(&mut frame[..], &[], &dictionary.bytes, zstd::DEFAULT_COMPRESSION_LEVEL).map_err(refused)?;
        let mut decompressor = zstd::zstd_safe::DCtx::try_create().ok_or_else(unallocated)?;
        let mut nothing = [0u8; 1];
        decompressor.decompress_using_dict(&mut nothing[..], &EMPTY_FRAME, &dictionary.bytes).map_err(refused)?;
        Ok(dictionary)
    }
}

/// A frame of nothing, built by hand from RFC 8878: the magic number, a
/// single-segment header with a one-byte content size of 0, and one last
/// raw block of no bytes.
const EMPTY_FRAME: [u8; 9] = [0x28, 0xB5, 0x2F, 0xFD, 0x20, 0x00, 0x01, 0x00, 0x00];

/// The dictionary an expansion was given, as a frame whose header names
/// another is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Given {
    /// No dictionary.
    Nothing,
    /// Raw content, which carries no id.
    RawContent,
    /// A dictionary in zstd's format, which carries this id.
    WithId(u32),
}

impl Given {
    fn of(dictionary: Option<&Dictionary>) -> Given {
        match dictionary {
            None => Given::Nothing,
            Some(Dictionary { id: None, .. }) => Given::RawContent,
            Some(Dictionary { id: Some(id), .. }) => Given::WithId(*id),
        }
    }
}

/// Why a stream stopped short. The variant decides the error record's
/// id and category.
#[derive(Debug)]
enum Failure {
    /// Reading the input failed.
    Read(io::Error),
    /// Writing the output failed, or memory for it was refused.
    Write(io::Error),
    /// libzstd refused a setting or the data.
    Codec(io::Error),
    /// A frame names a dictionary by id, and the dictionary given, if
    /// any, is another. `needed` is `None` when the frame's header could
    /// not be read back for its id.
    DictionaryMismatch { needed: Option<u32>, given: Given },
    /// A frame's window of `window` bytes does not fit: expanding it in a
    /// stream takes more than `budget`, or, with no budget, the window is
    /// larger than libzstd's default limit. `memory` is what expanding it
    /// takes: libzstd's estimate for the frame, the command's own share and
    /// the dictionary's, `dictionary` bytes, and, with no budget, a result
    /// held in memory twice when the frame records its size; `u64::MAX`
    /// when the window is beyond anything libzstd expands. `besides` is
    /// whether a result held in memory, of a size the frame does not
    /// record, comes on top of `memory`.
    WindowTooLarge { window: u64, memory: u64, dictionary: u64, budget: Option<i64>, besides: bool },
    /// The dictionary and what expanding takes before any frame, `needed`
    /// bytes, `dictionary` of them for the dictionary, do not fit `budget`.
    DictionaryTooLarge { needed: u64, dictionary: u64, budget: i64 },
    /// A result held in memory does not fit `limit`: it would be `result`
    /// bytes, as the frames' headers record, or, with `None`, it outgrew
    /// the limit as it was built.
    ResultTooLarge { result: Option<u64>, limit: Limit },
    /// Expanding straight into a byte[] of `result` bytes, as the frames'
    /// headers record, needs `needed` bytes, `dictionary` of them for the
    /// dictionary, more than `budget`; the result is held in memory once.
    HeldOnceTooLarge { needed: u64, result: u64, dictionary: u64, budget: i64 },
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Read(e) => write!(f, "reading the input failed: {e}"),
            Failure::Write(e) => write!(f, "writing the output failed: {e}"),
            Failure::Codec(e) => write!(f, "libzstd failed: {e}"),
            Failure::DictionaryMismatch { needed, given } => write!(f, "a frame needs dictionary {needed:?} and was given {given:?}"),
            Failure::WindowTooLarge { window, memory, budget, .. } => write!(f, "a frame's window of {window} bytes takes {memory} bytes, over {budget:?}"),
            Failure::ResultTooLarge { result, limit } => write!(f, "a result of {result:?} bytes held twice, over {} bytes of result within {}", limit.most, limit.budget),
            Failure::DictionaryTooLarge { needed, dictionary, budget } => write!(f, "a dictionary taking {dictionary} of {needed} bytes, over {budget}"),
            Failure::HeldOnceTooLarge { needed, result, budget, .. } => write!(f, "a result of {result} bytes held once, needing {needed} bytes, over {budget}"),
        }
    }
}

impl std::error::Error for Failure {}

/// The message libzstd gives when a frame's dictionary id is not the id
/// of the dictionary loaded, none loaded included (its
/// `ZSTD_error_dictionary_wrong`).
const DICTIONARY_WRONG: &str = "Dictionary mismatch";

/// The largest frame header RFC 8878 allows: the magic number, the frame
/// header descriptor, the window descriptor, a four-byte dictionary id
/// and an eight-byte content size.
const FRAME_HEADER_MAX: usize = 18;

/// An I/O error raised by the file or buffer at one end of a stream.
/// The zstd crate passes these back as `io::Error`, the same type as
/// libzstd's own errors, so the endpoints wrap theirs in this to keep
/// the two apart.
#[derive(Debug)]
struct EndpointError(io::Error);

impl std::fmt::Display for EndpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for EndpointError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// `e` marked as an endpoint's error, keeping its kind.
fn endpoint(e: io::Error) -> io::Error {
    io::Error::new(e.kind(), EndpointError(e))
}

/// Whether `e` was raised by an endpoint rather than by libzstd.
fn from_endpoint(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|inner| inner.is::<EndpointError>())
}

/// A file a stream writes to, counting the bytes written. Its errors
/// are marked as the endpoint's.
struct FileSink {
    file: File,
    written: u64,
}

impl Write for FileSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.file.write(buf).map_err(endpoint)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush().map_err(endpoint)
    }
}

/// How far a result held in memory may grow within -MaxMemory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Limit {
    /// The most bytes the result may hold: half of what `budget` leaves
    /// once the work itself is counted, since the result is held twice, as
    /// it is built and as the `byte[]` written.
    most: u64,
    /// -MaxMemory.
    budget: i64,
}

impl Limit {
    /// The limit `budget` leaves once `working` bytes are counted.
    fn left(budget: i64, working: u64) -> Limit {
        Limit { most: budget.max(0).unsigned_abs().saturating_sub(working) / 2, budget }
    }
}

/// The error for a result held in memory that would outgrow its limit.
#[derive(Debug)]
struct OverBudget(Limit);

impl std::fmt::Display for OverBudget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the result would outgrow the {} bytes -MaxMemory of {} bytes leaves it", self.0.most, self.0.budget)
    }
}

impl std::error::Error for OverBudget {}

/// Where a stream's output goes, as -MaxMemory sees it.
trait Output: Write {
    /// The bytes held so far, for output held in memory, which the budget
    /// counts; `None` for output written to a file.
    fn held(&self) -> Option<u64>;

    /// From this call on holds no more than `limit` lets it, when there is a
    /// limit, and makes room for `total` bytes in all, when that is known.
    fn prepare(&mut self, limit: Option<Limit>, total: Option<u64>) -> io::Result<()>;
}

impl Output for FileSink {
    fn held(&self) -> Option<u64> {
        None
    }

    fn prepare(&mut self, _: Option<Limit>, _: Option<u64>) -> io::Result<()> {
        Ok(())
    }
}

impl<T: Output + ?Sized> Output for &mut T {
    fn held(&self) -> Option<u64> {
        (**self).held()
    }

    fn prepare(&mut self, limit: Option<Limit>, total: Option<u64>) -> io::Result<()> {
        (**self).prepare(limit, total)
    }
}

/// Memory a stream writes to: the result a command writes as one
/// `byte[]`. It grows as `Vec` grows, to twice what it holds at a time,
/// but never past its limit, so the buffer it grows from and the one it
/// grows into never hold more than twice the limit between them; writing
/// past the limit is an `OverBudget` error. An allocation the allocator
/// refuses is an `OutOfMemory` error, where `Vec`'s own `Write` would end
/// the process. Both are marked as the endpoint's.
#[derive(Default)]
struct BufferSink {
    bytes: Vec<u8>,
    limit: Option<Limit>,
}

impl BufferSink {
    fn new(limit: Option<Limit>) -> BufferSink {
        BufferSink { bytes: Vec::new(), limit }
    }

    /// Makes room for `total` bytes in all.
    fn reserve_total(&mut self, total: u64) -> io::Result<()> {
        let more = usize::try_from(total.saturating_sub(self.bytes.len() as u64)).map_err(|e| io::Error::new(io::ErrorKind::OutOfMemory, e))?;
        self.bytes.try_reserve_exact(more).map_err(|e| io::Error::new(io::ErrorKind::OutOfMemory, e))
    }
}

impl Write for BufferSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let wanted = self.bytes.len() as u64 + buf.len() as u64;
        if let Some(limit) = self.limit.filter(|limit| wanted > limit.most) {
            return Err(endpoint(io::Error::other(OverBudget(limit))));
        }
        let capacity = self.bytes.capacity() as u64;
        if wanted > capacity {
            let grown = wanted.max(capacity.saturating_mul(2));
            self.reserve_total(self.limit.map_or(grown, |limit| grown.min(limit.most))).map_err(endpoint)?;
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Output for BufferSink {
    fn held(&self) -> Option<u64> {
        Some(self.bytes.len() as u64)
    }

    fn prepare(&mut self, limit: Option<Limit>, total: Option<u64>) -> io::Result<()> {
        self.limit = limit;
        match total {
            Some(total) => self.reserve_total(total),
            None => Ok(()),
        }
    }
}

/// The limit a result outgrew, when `e` is the error for that.
fn outgrown(e: &io::Error) -> Option<Limit> {
    let endpoint = e.get_ref()?.downcast_ref::<EndpointError>()?;
    endpoint.0.get_ref()?.downcast_ref::<OverBudget>().map(|over| over.0)
}

/// The failure for an error writing a stream's output.
fn write_failure(e: io::Error) -> Failure {
    match outgrown(&e) {
        Some(limit) => Failure::ResultTooLarge { result: None, limit },
        None => Failure::Write(e),
    }
}

/// An error the encoder passed back: the output's own, or libzstd's.
fn encoder_failure(e: io::Error) -> Failure {
    if from_endpoint(&e) {
        write_failure(e)
    } else {
        Failure::Codec(e)
    }
}

/// Compresses what `input` yields into one zstd frame on `output`.
///
/// The frame carries a content checksum. When `size` is given its
/// header records the content size, and `input` must yield exactly
/// that many bytes; fewer is a read failure. With a dictionary in zstd's
/// format, the header records its id. `stopping` is asked before each
/// chunk, and `Ok(false)` means it answered yes and the frame was left
/// unfinished.
fn compress<R: Read, W: Write>(
    input: &mut R,
    output: W,
    size: Option<u64>,
    settings: Settings,
    dictionary: Option<&Dictionary>,
    stopping: &dyn Fn() -> bool,
) -> Result<bool, Failure> {
    let mut encoder = zstd::stream::write::Encoder::with_dictionary(output, settings.level, Dictionary::bytes_of(dictionary)).map_err(Failure::Codec)?;
    encoder.include_checksum(true).map_err(Failure::Codec)?;
    encoder.set_pledged_src_size(size).map_err(Failure::Codec)?;
    if settings.threads > 0 {
        encoder.multithread(settings.threads).map_err(Failure::Codec)?;
    }
    let mut chunk = vec![0u8; CHUNK];
    let mut taken: u64 = 0;
    loop {
        if stopping() {
            return Ok(false);
        }
        let n = match input.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(Failure::Read(e)),
        };
        taken += n as u64;
        encoder.write_all(&chunk[..n]).map_err(encoder_failure)?;
    }
    if let Some(expected) = size.filter(|&expected| taken < expected) {
        return Err(Failure::Read(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("the input ended after {taken} of the {expected} bytes it held when it was opened"),
        )));
    }
    encoder.finish().map_err(encoder_failure)?;
    Ok(true)
}

/// Decompresses every zstd frame `input` yields, in order, onto
/// `output`, a chunk at a time through an `Expansion`, each frame
/// checked against `budget` as `Expansion` checks it.
///
/// libzstd refuses bytes after a frame that do not begin another, a frame
/// whose checksum does not match, and a frame whose header names a
/// dictionary other than the one given, and input that ends inside a
/// frame is refused once it ends; what was decoded before the fault is
/// already in `output` then.
/// `stopping` is asked before each chunk, and `Ok(false)` means it
/// answered yes.
fn expand<R: Read, W: Output>(input: &mut R, output: W, dictionary: Option<&Dictionary>, budget: Option<i64>, stopping: &dyn Fn() -> bool) -> Result<bool, Failure> {
    let mut expansion = Expansion::new(output, dictionary, budget)?;
    let mut chunk = vec![0u8; CHUNK];
    loop {
        if stopping() {
            return Ok(false);
        }
        let n = match input.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(Failure::Read(e)),
        };
        if !expansion.feed(&chunk[..n], stopping)? {
            return Ok(false);
        }
    }
    expansion.finish()?;
    Ok(true)
}

/// Compresses `input`, which stays where it is for the whole frame (a
/// file mapped into memory, or a pinned `byte[]`), into one zstd frame
/// on `output`, with its content size and a checksum, as `compress` does.
/// On the calling thread libzstd reads the input where it lies, as a
/// stable buffer, and keeps no input buffer of its own; with worker
/// threads it copies each job's input into buffers of its own. The input
/// is offered a chunk more at a
/// time, with `stopping` asked before each, and `Ok(false)` means it
/// answered yes; the last chunk ends the frame, offered with the rest of
/// the input still where libzstd left it, since libzstd holds back input
/// short of a block until it is told to end.
fn compress_stable<W: Write>(input: &[u8], mut output: W, settings: Settings, dictionary: Option<&Dictionary>, stopping: &dyn Fn() -> bool) -> Result<bool, Failure> {
    use zstd_sys::ZSTD_EndDirective::{ZSTD_e_continue, ZSTD_e_end};
    let codec = |code: usize| Failure::Codec(libzstd_error(code));
    let mut context = zstd_safe::CCtx::try_create().ok_or_else(|| Failure::Write(io::Error::new(io::ErrorKind::OutOfMemory, "libzstd could not allocate a compression context")))?;
    context.set_parameter(CParameter::CompressionLevel(settings.level)).map_err(codec)?;
    context.set_parameter(CParameter::ChecksumFlag(true)).map_err(codec)?;
    context.set_parameter(CParameter::StableInBuffer(true)).map_err(codec)?;
    if settings.threads > 0 {
        context.set_parameter(CParameter::NbWorkers(settings.threads)).map_err(codec)?;
    }
    context.load_dictionary(Dictionary::bytes_of(dictionary)).map_err(codec)?;
    context.set_pledged_src_size(Some(input.len() as u64)).map_err(codec)?;
    let mut chunk = Vec::new();
    chunk.try_reserve_exact(CHUNK).map_err(|e| Failure::Write(io::Error::new(io::ErrorKind::OutOfMemory, e)))?;
    chunk.resize(CHUNK, 0);
    let mut offered = 0;
    let mut consumed = 0;
    loop {
        if stopping() {
            return Ok(false);
        }
        offered = input.len().min(offered + CHUNK);
        let last = offered == input.len();
        // The same buffer each time, grown by a chunk, with the position
        // where libzstd left it, as a stable input must be.
        let mut buffer = InBuffer::around(&input[..offered]);
        buffer.set_pos(consumed);
        loop {
            let mut out = OutBuffer::around(&mut chunk[..]);
            let directive = if last { ZSTD_e_end } else { ZSTD_e_continue };
            let remaining = context.compress_stream2(&mut out, &mut buffer, directive).map_err(codec)?;
            let produced = out.pos();
            output.write_all(&chunk[..produced]).map_err(write_failure)?;
            if last && remaining == 0 {
                return Ok(true);
            }
            if !last && buffer.pos() == offered {
                break;
            }
        }
        consumed = buffer.pos();
    }
}

/// Expands every frame in `input`, a file mapped into memory or a
/// `byte[]`, into `output`, whose length is exactly what the frames'
/// content sizes add up to. libzstd writes into `output` in place, as a
/// stable buffer, and keeps no window of its own, so a frame of any window
/// fits. Each frame is given as many bytes of `output` as its header
/// records: one that holds more is refused as libzstd's destination buffer
/// too small, and one that holds fewer as a size mismatch, `misrecorded`
/// telling the two apart when libzstd refuses the frame for its size. It
/// refuses what `expand` refuses besides. `stopping` is asked before each
/// chunk of input, and `Ok(false)` means it answered yes.
fn expand_mapped(input: &[u8], output: &mut [u8], dictionary: Option<&Dictionary>, stopping: &dyn Fn() -> bool) -> Result<bool, Failure> {
    use zstd::stream::raw::Operation;
    let given = Given::of(dictionary);
    let mut decoder = zstd::stream::raw::Decoder::with_dictionary(Dictionary::bytes_of(dictionary)).map_err(Failure::Codec)?;
    decoder.set_parameter(DParameter::StableOutBuffer(true)).map_err(Failure::Codec)?;
    decoder.set_parameter(DParameter::WindowLogMax(WINDOW_LOG_MAX)).map_err(Failure::Codec)?;
    let expected = output.len();
    // The frame being expanded: where it begins in `input` and in
    // `output`, the content size its header records, and the space libzstd
    // expands it into, as long as that size.
    let mut frame_start = 0;
    let mut frame_output = 0;
    let mut recorded = recorded_size(input)?;
    let mut out = OutBuffer::around(frame_space(output, 0, recorded));
    let mut at_frame_end = false;
    let mut refused = None;
    let mut at = 0;
    'input: while at < input.len() {
        if stopping() {
            return Ok(false);
        }
        let end = input.len().min(at + CHUNK);
        let mut piece = InBuffer::around(&input[at..end]);
        while piece.pos() < end - at {
            if at_frame_end {
                // A frame ended and more data follows, which begins the
                // next frame.
                decoder.reinit().map_err(Failure::Codec)?;
                frame_start = at + piece.pos();
                frame_output += out.pos();
                recorded = recorded_size(&input[frame_start..])?;
                out = OutBuffer::around(frame_space(output, frame_output, recorded));
            }
            let (read, written) = (piece.pos(), out.pos());
            match decoder.run(&mut piece, &mut out) {
                Ok(hint) => at_frame_end = hint == 0,
                Err(e) => {
                    refused = Some(e);
                    break 'input;
                }
            }
            if at_frame_end {
                // libzstd does not check the size at a frame that ends with
                // an empty block.
                let held = out.pos() as u64;
                if let Some(recorded) = recorded.filter(|&recorded| recorded != held) {
                    return Err(size_mismatch(recorded, held));
                }
            }
            if piece.pos() == read && out.pos() == written && !at_frame_end {
                return Err(Failure::Codec(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("libzstd stopped at byte {} of {} with its output at byte {} of the {expected} the content sizes add up to", at + read, input.len(), frame_output + written),
                )));
            }
        }
        at = end;
    }
    let written = frame_output + out.pos();
    if let Some(e) = refused {
        let failure = refusal(e, &input[frame_start..], given);
        if matches!(failure, Failure::Codec(_)) && let Some(told) = misrecorded(&input[frame_start..], &mut output[frame_output..], dictionary)? {
            return Err(told);
        }
        return Err(failure);
    }
    if !at_frame_end {
        return Err(Failure::Codec(io::Error::new(io::ErrorKind::UnexpectedEof, "the data ends inside a frame")));
    }
    if written != expected {
        return Err(Failure::Codec(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the frames' content sizes add up to {expected} bytes, and they expand to {written}"),
        )));
    }
    Ok(true)
}

/// The space in `output` the frame that begins at `start` expands into:
/// as many bytes as its header records, `recorded`, or all that is left
/// when it records none or more than is left.
fn frame_space(output: &mut [u8], start: usize, recorded: Option<u64>) -> &mut [u8] {
    let rest = &mut output[start..];
    match recorded {
        Some(recorded) if recorded < rest.len() as u64 => &mut rest[..recorded as usize],
        _ => rest,
    }
}

/// The content size the header at the start of `frame` records, `None`
/// when it records none; a header libzstd cannot read is refused.
fn recorded_size(frame: &[u8]) -> Result<Option<u64>, Failure> {
    zstd_safe::get_frame_content_size(frame).map_err(|e| Failure::Codec(io::Error::new(io::ErrorKind::InvalidData, e.to_string())))
}

/// The failure for a frame whose header records a content size of
/// `recorded` bytes and which holds `held`.
fn size_mismatch(recorded: u64, held: u64) -> Failure {
    Failure::Codec(io::Error::new(io::ErrorKind::InvalidData, format!("a frame records a content size of {recorded} bytes and holds {held}")))
}

/// A copy of the header at the start of `frame` that records no content
/// size, with the length of the header it stands for; `None` when the
/// header records none or is cut short. A single-segment frame's window
/// is its content size, so its copy asks for a window of the smallest
/// power of two that holds the `recorded` size, 128 KiB at least, which
/// holds libzstd's largest block, and 2 GiB at most, the largest libzstd
/// expands.
fn unrecorded_header(frame: &[u8], recorded: u64) -> Option<(Vec<u8>, usize)> {
    let descriptor = *frame.get(4)?;
    let single = descriptor & 0x20 != 0;
    let dictionary_len = [0, 1, 2, 4][usize::from(descriptor & 0x03)];
    let size_len = match descriptor >> 6 {
        0 if single => 1,
        0 => 0,
        1 => 2,
        2 => 4,
        _ => 8,
    };
    let window_len = usize::from(!single);
    let header_len = 5 + window_len + dictionary_len + size_len;
    if size_len == 0 || frame.len() < header_len {
        return None;
    }
    let mut header = frame[..4].to_vec();
    // The content size's flag and the single-segment flag cleared.
    header.push(descriptor & 0x1F);
    if single {
        let log = (u64::BITS - recorded.saturating_sub(1).leading_zeros()).clamp(17, WINDOW_LOG_MAX);
        header.push(((log - 10) << 3) as u8);
    } else {
        header.push(frame[5]);
    }
    header.extend_from_slice(&frame[5 + window_len..5 + window_len + dictionary_len]);
    Some((header, header_len))
}

/// What to report for the frame at the start of `frame`, which libzstd
/// refused with the content size its header records: the frame is
/// expanded once more, from a copy of its header that records no size,
/// into `space`, the output from where the frame's own begins, with room
/// for as many bytes as it records. A frame that then holds fewer bytes is
/// a size mismatch; one that holds more is refused by libzstd as its
/// destination too small, and one with a fault of its own for that fault.
/// `Ok(None)` when the frame records no size, expands to exactly that
/// size, or does not end, and libzstd's first reason stands.
fn misrecorded(frame: &[u8], space: &mut [u8], dictionary: Option<&Dictionary>) -> Result<Option<Failure>, Failure> {
    use zstd::stream::raw::Operation;
    let Some(recorded) = recorded_size(frame)? else {
        return Ok(None);
    };
    let Some((header, header_len)) = unrecorded_header(frame, recorded) else {
        return Ok(None);
    };
    let room = if (space.len() as u64) < recorded { space.len() } else { recorded as usize };
    let mut decoder = zstd::stream::raw::Decoder::with_dictionary(Dictionary::bytes_of(dictionary)).map_err(Failure::Codec)?;
    decoder.set_parameter(DParameter::StableOutBuffer(true)).map_err(Failure::Codec)?;
    decoder.set_parameter(DParameter::WindowLogMax(WINDOW_LOG_MAX)).map_err(Failure::Codec)?;
    let mut out = OutBuffer::around(&mut space[..room]);
    for part in [&header[..], &frame[header_len..]] {
        let mut piece = InBuffer::around(part);
        while piece.pos() < part.len() {
            let (read, written) = (piece.pos(), out.pos());
            match decoder.run(&mut piece, &mut out) {
                Ok(0) => {
                    let held = out.pos() as u64;
                    return Ok((held < recorded).then(|| size_mismatch(recorded, held)));
                }
                Ok(_) if piece.pos() == read && out.pos() == written => return Ok(None),
                Ok(_) => {}
                Err(e) => return Ok(Some(Failure::Codec(e))),
            }
        }
    }
    Ok(None)
}

/// The first `size` bytes of `file` mapped into memory, for reading.
fn map_input(file: &File, size: u64) -> io::Result<memmap2::Mmap> {
    let len = usize::try_from(size).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    // SAFETY: `file` stays open for as long as the map lives. Windows
    // refuses to shorten a file while a map of it exists, so every mapped
    // byte stays readable; a byte another process rewrites meanwhile is
    // read as rewritten, as a read of the file would read it.
    unsafe { memmap2::MmapOptions::new().len(len).map(file) }
}

/// Compresses bytes handed over in pieces, as `-InputObject` records
/// arrive, into one frame held in memory, which grows no further than
/// `limit` lets it. The frame starts before its length is known, so it
/// carries a content checksum and no content size.
struct Compression {
    encoder: zstd::stream::write::Encoder<'static, BufferSink>,
    /// The bytes fed so far.
    taken: u64,
    /// The level and worker threads it compresses with.
    settings: Settings,
}

impl Compression {
    fn new(settings: Settings, dictionary: Option<&Dictionary>, limit: Option<Limit>) -> Result<Compression, Failure> {
        let mut encoder = zstd::stream::write::Encoder::with_dictionary(BufferSink::new(limit), settings.level, Dictionary::bytes_of(dictionary)).map_err(Failure::Codec)?;
        encoder.include_checksum(true).map_err(Failure::Codec)?;
        if settings.threads > 0 {
            encoder.multithread(settings.threads).map_err(Failure::Codec)?;
        }
        Ok(Compression { encoder, taken: 0, settings })
    }

    /// Feeds `data` to libzstd a chunk at a time, asking `stopping`
    /// before each chunk; `Ok(false)` means it answered yes.
    fn feed(&mut self, data: &[u8], stopping: &dyn Fn() -> bool) -> Result<bool, Failure> {
        for piece in data.chunks(CHUNK) {
            if stopping() {
                return Ok(false);
            }
            self.encoder.write_all(piece).map_err(encoder_failure)?;
            self.taken += piece.len() as u64;
        }
        Ok(true)
    }

    /// Ends the frame and returns it with the number of bytes it holds
    /// and the settings it was compressed with.
    fn finish(self) -> Result<(Vec<u8>, u64, Settings), Failure> {
        let (taken, settings) = (self.taken, self.settings);
        let sink = self.encoder.finish().map_err(encoder_failure)?;
        Ok((sink.bytes, taken, settings))
    }
}

/// The failure for an error libzstd returned while expanding the frame
/// whose first bytes are `header`: a frame whose header names another
/// dictionary than the one `given`, with the id it names, or libzstd's
/// error as it gave it.
fn refusal(e: io::Error, header: &[u8], given: Given) -> Failure {
    if e.to_string() == DICTIONARY_WRONG {
        let header = &header[..header.len().min(FRAME_HEADER_MAX)];
        let needed = zstd_safe::get_dict_id_from_frame(header).map(|id| id.get());
        return Failure::DictionaryMismatch { needed, given };
    }
    Failure::Codec(e)
}

/// Expands zstd data handed over in pieces, a file's chunks or
/// `-InputObject` records, onto `sink`: every frame, in order. Each
/// frame's header is read before libzstd is given the frame, and a frame
/// whose window does not fit is refused: one that takes more memory to
/// expand than the budget, or, with no budget, one whose window is larger
/// than libzstd's default limit. When `sink` holds the result in memory,
/// the result counts against the budget too, held twice: a frame whose
/// header records a content size the result cannot take is refused
/// before libzstd sees it, and a result that outgrows what is left stops
/// the expansion. libzstd refuses a frame whose data is corrupt or whose
/// checksum does not match as the pieces arrive, and a frame that expands
/// to other than the content size its header records is refused when it
/// ends; `finish` refuses data that stops inside a frame. A frame whose
/// header names another dictionary than the one given is reported with
/// the id it names.
struct Expansion<W: Output> {
    decoder: zstd::stream::raw::Decoder<'static>,
    sink: W,
    /// libzstd's output for one step, before it is written to `sink`.
    step: Vec<u8>,
    /// Whether the last step ended a frame and flushed all of its output,
    /// so the data so far is whole frames.
    at_frame_end: bool,
    /// Whether the header of the frame being expanded has been read and
    /// checked, and handed to libzstd.
    checked: bool,
    /// The header of the frame being expanded: its first `header_len`
    /// bytes, gathered until libzstd can read it whole.
    header: [u8; FRAME_HEADER_MAX],
    header_len: usize,
    /// The dictionary given, as a frame that names another is told.
    given: Given,
    /// -MaxMemory, the most the expansion may take.
    budget: Option<i64>,
    /// What the dictionary takes, counted with every frame.
    dictionary: u64,
    /// The most any frame so far takes to expand, the dictionary and the
    /// command's own buffers included: libzstd keeps the buffers of the
    /// largest window it has expanded.
    working: u64,
    /// The bytes fed so far.
    taken: u64,
    /// The content size the header of the frame being expanded records,
    /// and the bytes it has expanded to so far.
    recorded: Option<u64>,
    held: u64,
}

impl<W: Output> Expansion<W> {
    fn new(sink: W, dictionary: Option<&Dictionary>, budget: Option<i64>) -> Result<Expansion<W>, Failure> {
        let share = Dictionary::expansion_share(dictionary);
        if let Some(budget) = budget {
            dictionary_fits(share, budget)?;
        }
        let mut decoder = zstd::stream::raw::Decoder::with_dictionary(Dictionary::bytes_of(dictionary)).map_err(Failure::Codec)?;
        if budget.is_some() {
            // The budget is checked against each frame's header before
            // libzstd sees it, in place of libzstd's own window limit.
            decoder.set_parameter(DParameter::WindowLogMax(WINDOW_LOG_MAX)).map_err(Failure::Codec)?;
        }
        let mut step = Vec::new();
        step.try_reserve_exact(CHUNK).map_err(|e| Failure::Write(io::Error::new(io::ErrorKind::OutOfMemory, e)))?;
        step.resize(CHUNK, 0);
        Ok(Expansion {
            decoder,
            sink,
            step,
            at_frame_end: false,
            checked: false,
            header: [0; FRAME_HEADER_MAX],
            header_len: 0,
            given: Given::of(dictionary),
            budget,
            dictionary: share,
            working: 0,
            taken: 0,
            recorded: None,
            held: 0,
        })
    }

    /// An expansion that goes on after `taken` bytes of whole frames,
    /// whose result `sink` holds already.
    fn after_frames(sink: W, taken: u64, dictionary: Option<&Dictionary>, budget: Option<i64>) -> Result<Expansion<W>, Failure> {
        let mut expansion = Expansion::new(sink, dictionary, budget)?;
        expansion.at_frame_end = true;
        expansion.taken = taken;
        Ok(expansion)
    }

    /// Feeds `data` to libzstd a chunk at a time, asking `stopping`
    /// before each chunk; `Ok(false)` means it answered yes. Each frame's
    /// header is gathered and checked first, and then the frame is fed up
    /// to its end.
    fn feed(&mut self, data: &[u8], stopping: &dyn Fn() -> bool) -> Result<bool, Failure> {
        use zstd::stream::raw::Operation;
        for piece in data.chunks(CHUNK) {
            if stopping() {
                return Ok(false);
            }
            let mut rest = piece;
            while !rest.is_empty() {
                if self.at_frame_end {
                    // A frame ended and more data follows, which begins
                    // the next frame.
                    self.decoder.reinit().map_err(Failure::Codec)?;
                    self.at_frame_end = false;
                    self.checked = false;
                    self.header_len = 0;
                    self.recorded = None;
                    self.held = 0;
                }
                if !self.checked {
                    let taken = self.read_header(rest)?;
                    rest = &rest[taken..];
                    if !self.checked {
                        // The header goes on in the next piece.
                        break;
                    }
                    let header = self.header;
                    self.run(&header[..self.header_len])?;
                    continue;
                }
                let used = self.run(rest)?;
                rest = &rest[used..];
            }
            self.taken += piece.len() as u64;
        }
        Ok(true)
    }

    /// Moves the bytes the frame header needs from the front of `bytes`
    /// into `header`, and once it is whole, checks the frame. Returns how
    /// many bytes it took. Bytes that begin no frame are passed on
    /// unchecked, for libzstd to say what they are.
    fn read_header(&mut self, bytes: &[u8]) -> Result<usize, Failure> {
        let mut taken = 0;
        loop {
            match frame_start(&self.header[..self.header_len]) {
                FrameStart::Needs(total) if total > self.header_len && total <= FRAME_HEADER_MAX => {
                    let take = (total - self.header_len).min(bytes.len() - taken);
                    if take == 0 {
                        return Ok(taken);
                    }
                    self.header[self.header_len..self.header_len + take].copy_from_slice(&bytes[taken..taken + take]);
                    self.header_len += take;
                    taken += take;
                }
                FrameStart::Header { window, memory, content } => {
                    self.check_frame(window, memory, content)?;
                    self.checked = true;
                    return Ok(taken);
                }
                FrameStart::Needs(_) | FrameStart::Unreadable => {
                    self.checked = true;
                    return Ok(taken);
                }
            }
        }
    }

    /// Checks a frame before libzstd sees it: its window of `window`
    /// bytes, which libzstd estimates takes `memory` bytes to expand, and,
    /// for a result held in memory within a budget, the `content` bytes it
    /// expands to when its header records that, which room is then made
    /// for.
    fn check_frame(&mut self, window: u64, memory: u64, content: Option<u64>) -> Result<(), Failure> {
        self.recorded = content;
        let dictionary = self.dictionary;
        let needed = memory.saturating_add(OWN_SHARE).saturating_add(dictionary);
        let held = self.sink.held();
        let total = held.zip(content).map(|(held, content)| held.saturating_add(content));
        let budget = match self.budget {
            None if window > WINDOW_LIMIT_DEFAULT => {
                // What expanding it takes: the frame, and a result held in
                // memory twice when its size is known.
                let memory = needed.saturating_add(total.unwrap_or(0).saturating_mul(2));
                let besides = held.is_some() && total.is_none();
                return Err(Failure::WindowTooLarge { window, memory, dictionary, budget: None, besides });
            }
            // With no budget, a result held in memory grows as it is
            // written, whatever size a header claims.
            None => return Ok(()),
            Some(budget) if !fits(needed, budget) => return Err(Failure::WindowTooLarge { window, memory: needed, dictionary, budget: Some(budget), besides: false }),
            Some(budget) => budget,
        };
        self.working = self.working.max(needed);
        if held.is_none() {
            return Ok(());
        }
        let limit = Limit::left(budget, self.working);
        if let Some(total) = total.filter(|&total| total > limit.most) {
            return Err(Failure::ResultTooLarge { result: Some(total), limit });
        }
        self.sink.prepare(Some(limit), total).map_err(Failure::Write)
    }

    /// Feeds `bytes` to libzstd and writes what it expands to `sink`,
    /// until they are all consumed with no output held back, or a frame
    /// ends before them. Returns how many bytes it took.
    fn run(&mut self, bytes: &[u8]) -> Result<usize, Failure> {
        use zstd::stream::raw::Operation;
        let mut input = InBuffer::around(bytes);
        loop {
            let stepped = {
                let mut output = OutBuffer::around(&mut self.step[..]);
                self.decoder.run(&mut input, &mut output).map(|hint| (hint, output.pos()))
            };
            let (hint, produced) = stepped.map_err(|e| refusal(e, &self.header[..self.header_len], self.given))?;
            self.sink.write_all(&self.step[..produced]).map_err(write_failure)?;
            self.held += produced as u64;
            self.at_frame_end = hint == 0;
            if self.at_frame_end {
                // libzstd does not check the size at a frame that ends
                // with an empty block.
                if let Some(recorded) = self.recorded.filter(|&recorded| recorded != self.held) {
                    return Err(size_mismatch(recorded, self.held));
                }
            }
            if self.at_frame_end && input.pos() < bytes.len() {
                return Ok(input.pos());
            }
            if input.pos() == bytes.len() && (self.at_frame_end || produced < self.step.len()) {
                return Ok(bytes.len());
            }
        }
    }

    /// The sink, once the data fed so far is whole frames.
    fn finish(self) -> Result<W, Failure> {
        if !self.at_frame_end {
            return Err(Failure::Codec(io::Error::new(io::ErrorKind::UnexpectedEof, "the data ends inside a frame")));
        }
        Ok(self.sink)
    }
}

/// Which cmdlet a failure belongs to, which decides what a libzstd
/// failure means.
#[derive(Clone, Copy)]
enum Operation {
    Compress,
    Expand,
}

/// The two ends of an operation as its error records name them: the
/// files' paths, or `None` for bytes on the pipeline.
#[derive(Clone, Copy)]
struct Ends<'a> {
    from: Option<&'a str>,
    to: Option<&'a str>,
}

/// The ends of an operation on bytes from the pipeline.
const PIPED: Ends<'static> = Ends { from: None, to: None };

impl Ends<'_> {
    fn input(&self) -> String {
        self.from.map_or_else(|| "the input".to_string(), |p| format!("'{p}'"))
    }

    fn output(&self) -> String {
        self.to.map_or_else(|| "the output".to_string(), |p| format!("'{p}'"))
    }
}

/// `e` with `target` as its record's target object.
fn targeted(e: PsError, target: &str) -> PsError {
    match target.into_ps() {
        Ok(object) => e.with_target(object),
        Err(lost) => {
            let message = format!("{} The target '{target}' could not be attached: {}", e.message, lost.message);
            PsError { message, ..e }
        }
    }
}

/// `e` targeted at `path` when there is one.
fn at(e: PsError, path: Option<&str>) -> PsError {
    match path {
        Some(p) => targeted(e, p),
        None => e,
    }
}

/// The error category for an I/O error of `e`'s kind, or `otherwise`.
fn category_of(e: &io::Error, otherwise: ErrorCategory) -> ErrorCategory {
    match e.kind() {
        io::ErrorKind::NotFound => ErrorCategory::ObjectNotFound,
        io::ErrorKind::PermissionDenied => ErrorCategory::PermissionDenied,
        io::ErrorKind::AlreadyExists => ErrorCategory::ResourceExists,
        io::ErrorKind::OutOfMemory => ErrorCategory::ResourceUnavailable,
        _ => otherwise,
    }
}

/// The error record for a failed operation.
fn failed(operation: Operation, failure: Failure, ends: Ends<'_>) -> PsError {
    match failure {
        Failure::Read(e) => at(
            PsError::new(category_of(&e, ErrorCategory::ReadError), "ZstdReadFailed", format!("Cannot read {}: {e}", ends.input())),
            ends.from,
        ),
        Failure::Write(e) if e.kind() == io::ErrorKind::OutOfMemory => {
            PsError::new(ErrorCategory::ResourceUnavailable, "ZstdOutOfMemory", format!("Cannot hold {} in memory: {e}", ends.output()))
        }
        Failure::Write(e) => at(
            PsError::new(category_of(&e, ErrorCategory::WriteError), "ZstdWriteFailed", format!("Cannot write {}: {e}", ends.output())),
            ends.to,
        ),
        Failure::Codec(e) => match operation {
            Operation::Compress => at(
                PsError::new(ErrorCategory::InvalidOperation, "ZstdCompressFailed", format!("Cannot compress {}: libzstd reported {e}", ends.input())),
                ends.from,
            ),
            Operation::Expand => at(
                PsError::new(ErrorCategory::InvalidData, "ZstdInvalidData", format!("Cannot expand {}: it is not valid zstd data ({e}).", ends.input())),
                ends.from,
            ),
        },
        Failure::DictionaryMismatch { needed, given } => {
            let needs = match needed {
                Some(id) => format!("a frame in it needs dictionary {id}"),
                None => "a frame in it needs another dictionary".to_string(),
            };
            let was_given = match given {
                Given::Nothing => "no dictionary was given".to_string(),
                Given::RawContent => "the dictionary given is raw content, which carries no id".to_string(),
                Given::WithId(id) => format!("dictionary {id} was given"),
            };
            at(
                PsError::new(ErrorCategory::InvalidArgument, "ZstdDictionaryMismatch", format!("Cannot expand {}: {needs}, and {was_given}.", ends.input())),
                ends.from,
            )
        }
        Failure::WindowTooLarge { window, memory, dictionary, budget, besides } => {
            let besides = if besides { ", with room besides for its result, which is held in memory twice" } else { "" };
            let share = if dictionary > 0 { format!(", {dictionary} of them for the dictionary") } else { String::new() };
            let why = match budget {
                _ if memory == u64::MAX => format!("a frame in it has a window of {window} bytes, more than libzstd expands at all"),
                Some(budget) => format!("a frame in it has a window of {window} bytes, which takes {memory} bytes to expand{share}, more than -MaxMemory of {budget} bytes"),
                None => format!(
                    "a frame in it has a window of {window} bytes, more than the {WINDOW_LIMIT_DEFAULT} bytes libzstd expands without -MaxMemory; -MaxMemory of {memory} bytes or more lets it{share}{besides}"
                ),
            };
            at(PsError::new(ErrorCategory::LimitsExceeded, "ZstdWindowTooLarge", format!("Cannot expand {}: {why}.", ends.input())), ends.from)
        }
        Failure::DictionaryTooLarge { needed, dictionary, budget } => at(
            PsError::new(
                ErrorCategory::LimitsExceeded,
                "ZstdBudgetTooSmall",
                format!("Cannot expand {} within -MaxMemory of {budget} bytes: with its dictionary, expanding needs about {needed} bytes before any frame, {dictionary} of them for the dictionary.", ends.input()),
            ),
            ends.from,
        ),
        Failure::ResultTooLarge { result, limit } => {
            let verb = match operation {
                Operation::Compress => "compress",
                Operation::Expand => "expand",
            };
            let why = match result {
                Some(result) => format!("its result of {result} bytes, as its frames record, would be held in memory twice, as it is built and as the byte[] written, and {} bytes of result is the most that fits", limit.most),
                None => format!("its result is held in memory twice, as it is built and as the byte[] written, and it outgrew {} bytes, the most that fits", limit.most),
            };
            at(
                PsError::new(ErrorCategory::LimitsExceeded, "ZstdBudgetTooSmall", format!("Cannot {verb} {} within -MaxMemory of {} bytes: {why}.", ends.input(), limit.budget)),
                ends.from,
            )
        }
        Failure::HeldOnceTooLarge { needed, result, dictionary, budget } => {
            let shares = if dictionary > 0 { format!("{dictionary} of them for the dictionary and {result} for its result") } else { format!("{result} of them for its result") };
            at(
                PsError::new(
                    ErrorCategory::LimitsExceeded,
                    "ZstdBudgetTooSmall",
                    format!(
                        "Cannot expand {} within -MaxMemory of {budget} bytes: it needs about {needed} bytes, {shares}, as its frames record, which is held in memory once, as the byte[] it is expanded into.",
                        ends.input()
                    ),
                ),
                ends.from,
            )
        }
    }
}

/// Whether `needed` bytes fit a budget of `budget` bytes.
fn fits(needed: u64, budget: i64) -> bool {
    i128::from(needed) <= i128::from(budget)
}

/// Whether what expanding with a dictionary takes before any frame fits
/// `budget`: the dictionary's `share`, libzstd's context and the command's
/// own buffers. Without a dictionary nothing is checked before the first
/// frame, whose own check counts the context.
fn dictionary_fits(share: u64, budget: i64) -> Result<(), Failure> {
    if share == 0 {
        return Ok(());
    }
    // SAFETY: computes from nothing outside it.
    let context = unsafe { zstd_sys::ZSTD_estimateDCtxSize() } as u64;
    let needed = share.saturating_add(context).saturating_add(OWN_SHARE);
    if fits(needed, budget) {
        Ok(())
    } else {
        Err(Failure::DictionaryTooLarge { needed, dictionary: share, budget })
    }
}

/// What writing a file in place takes beside the two files: the page
/// tables of the input's map, of `size` bytes, and of the output's, of
/// `total` bytes; libzstd's context and a block of input held back when a
/// block spans two chunks; the dictionary's `share`; and the command's
/// own share. libzstd writes into the output's map and keeps no window.
fn in_place_needs(size: u64, total: u64, share: u64) -> u64 {
    // SAFETY: computes from nothing outside it.
    let context = unsafe { zstd_sys::ZSTD_estimateDCtxSize() } as u64;
    page_tables(size)
        .saturating_add(page_tables(total))
        .saturating_add(context)
        .saturating_add(BLOCK_MAX)
        .saturating_add(share)
        .saturating_add(OWN_SHARE)
}

/// The error record for a compression that does not fit -MaxMemory of
/// `budget` bytes: even one worker, or the calling thread, is estimated
/// to take `working` bytes, `dictionary` of them for the dictionary, and
/// a result of up to `result` bytes held in memory takes it twice over.
fn budget_too_small(working: u64, dictionary: u64, result: u64, budget: i64, settings: Settings, ends: Ends<'_>) -> PsError {
    let who = if settings.threads == 0 { "on the calling thread" } else { "with one worker thread" };
    let twice = result.saturating_mul(2);
    let needs = working.saturating_add(twice);
    let held = "which is held in memory twice, as it is built and as the byte[] written";
    let shares = match (dictionary > 0, result > 0) {
        (false, false) => String::new(),
        (true, false) => format!(", {dictionary} of them for the dictionary"),
        (false, true) => format!(", {twice} of them for its result of up to {result} bytes, {held}"),
        (true, true) => format!(", {dictionary} of them for the dictionary and {twice} for its result of up to {result} bytes, {held}"),
    };
    at(
        PsError::new(
            ErrorCategory::LimitsExceeded,
            "ZstdBudgetTooSmall",
            format!("Cannot compress {} within -MaxMemory of {budget} bytes: at level {} {who} it needs about {needs} bytes{shares}.", ends.input(), settings.level),
        ),
        ends.from,
    )
}

/// The error record for input with no bytes in it, which holds no
/// frame and so is not zstd data.
fn empty_input(ends: Ends<'_>) -> PsError {
    at(
        PsError::new(ErrorCategory::InvalidData, "ZstdInvalidData", format!("Cannot expand {}: it is empty, and zstd data holds at least one frame.", ends.input())),
        ends.from,
    )
}

/// The file-system path `path` names in this session: relative to the
/// current PowerShell location and through its drives, with wildcard
/// characters taken as written.
fn provider_path(ps: &Pipeline<'_>, path: &str) -> PsResult<PathBuf> {
    let resolved = ps.resolve_path(path, true).map_err(|e| {
        targeted(
            PsError::new(ErrorCategory::InvalidArgument, "ZstdPathNotResolved", format!("Cannot resolve '{path}': {}", e.message)),
            path,
        )
    })?;
    match resolved.as_slice() {
        [one] => Ok(PathBuf::from(one)),
        other => Err(targeted(
            PsError::new(ErrorCategory::InvalidArgument, "ZstdPathNotResolved", format!("'{path}' resolves to {} paths, and one file is needed.", other.len())),
            path,
        )),
    }
}

/// The paths one `-Path` value names: every file or folder its wildcard
/// characters match, in the provider's order, each to be taken on its
/// own, and a folder refused when it is opened. A value that names
/// nothing is an error record of its own; the runtime reports an engine
/// exception as its type's name and message, and the engine raises
/// `ItemNotFoundException` for a path or pattern that names nothing.
fn matched_files(ps: &Pipeline<'_>, pattern: &str) -> Vec<PsResult<PathBuf>> {
    let nothing = || targeted(PsError::new(ErrorCategory::ObjectNotFound, "ZstdInputNotFound", format!("Cannot find '{pattern}'.")), pattern);
    match ps.resolve_path(pattern, false) {
        Ok(found) if found.is_empty() => vec![Err(nothing())],
        Ok(found) => found.into_iter().map(|p| Ok(PathBuf::from(p))).collect(),
        Err(e) if e.message.starts_with("ItemNotFoundException") => vec![Err(nothing())],
        Err(e) => vec![Err(targeted(
            PsError::new(ErrorCategory::InvalidArgument, "ZstdPathNotResolved", format!("Cannot resolve '{pattern}': {}", e.message)),
            pattern,
        ))],
    }
}

/// A file a command was given, with the parameter that named it, which a
/// refusal of the file names.
struct Named {
    path: PathBuf,
    parameter: &'static str,
}

/// The files a command's `-Path` and `-LiteralPath` values name, each to
/// be taken on its own: `-Path` values through their wildcard characters,
/// `-LiteralPath` values exactly as written.
fn named_files(ps: &Pipeline<'_>, paths: &[String], literal_paths: &[String]) -> Vec<PsResult<Named>> {
    let mut found = Vec::new();
    for pattern in paths {
        found.extend(matched_files(ps, pattern).into_iter().map(|path| path.map(|path| Named { path, parameter: "-Path" })));
    }
    for literal in literal_paths {
        found.push(provider_path(ps, literal).map(|path| Named { path, parameter: "-LiteralPath" }));
    }
    found
}

/// The name an input's output takes when `-DestinationPath` gives no
/// file: the input's name with `.zst` added when compressing, and without
/// its `.zst`, in any case, when expanding. An input to expand whose name
/// does not end in `.zst`, or is nothing but `.zst`, has none.
fn default_name(src: &Path, operation: Operation) -> Option<OsString> {
    let name = src.file_name()?;
    match operation {
        Operation::Compress => {
            let mut named = name.to_os_string();
            named.push(".zst");
            Some(named)
        }
        Operation::Expand => match name.to_str() {
            Some(text) if text.len() > 4 && text.to_ascii_lowercase().ends_with(".zst") => Some(OsString::from(&text[..text.len() - 4])),
            _ => None,
        },
    }
}

/// `src`'s default name, or the `ZstdNoDestination` error record for an
/// input that has none.
fn named_default(src: &Path, operation: Operation) -> PsResult<OsString> {
    default_name(src, operation).ok_or_else(|| {
        let shown = src.display().to_string();
        targeted(
            PsError::new(
                ErrorCategory::InvalidArgument,
                "ZstdNoDestination",
                format!("'{shown}' does not end in .zst, so it has no default destination. Name one with -DestinationPath."),
            ),
            &shown,
        )
    })
}

/// Where the output for `src` is written. Without `-DestinationPath`,
/// beside `src` under its default name; when it names an existing
/// folder, in that folder under the default name; otherwise that file,
/// which only the first input of the command may take: `taken` records
/// that it has been, and a later input is refused with
/// `ZstdDestinationReused`.
fn destination(ps: &Pipeline<'_>, given: Option<&str>, taken: &mut bool, src: &Path, operation: Operation) -> PsResult<PathBuf> {
    let Some(given) = given else {
        return Ok(src.with_file_name(named_default(src, operation)?));
    };
    let dest = provider_path(ps, given)?;
    let shown = dest.display().to_string();
    let is_folder = match fs::metadata(&dest) {
        Ok(meta) => meta.is_dir(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(output_error(&e, &shown)),
    };
    if is_folder {
        return Ok(dest.join(named_default(src, operation)?));
    }
    if *taken {
        let input = src.display().to_string();
        return Err(targeted(
            PsError::new(
                ErrorCategory::InvalidOperation,
                "ZstdDestinationReused",
                format!("-DestinationPath '{shown}' is one file, and an earlier input of this command took it, so '{input}' was not written."),
            ),
            &input,
        ));
    }
    *taken = true;
    Ok(dest)
}

/// Writes the file at `path` to the pipeline as `Get-Item` gives it: a
/// `FileInfo` carrying the provider's own properties, `PSPath` among
/// them, so it pipes on like any file the engine lists.
fn pass_through(ps: &Pipeline<'_>, path: &str) -> PsResult<()> {
    for item in ps.invoke("Get-Item", &[("LiteralPath", path.into_ps()?)])? {
        ps.write_object(&item)?;
    }
    Ok(())
}

/// The error record for an input file that could not be opened or read.
fn input_error(e: &io::Error, shown: &str) -> PsError {
    let error = if e.kind() == io::ErrorKind::NotFound {
        PsError::new(ErrorCategory::ObjectNotFound, "ZstdInputNotFound", format!("Cannot find '{shown}'."))
    } else {
        PsError::new(category_of(e, ErrorCategory::ReadError), "ZstdReadFailed", format!("Cannot read '{shown}': {e}"))
    };
    targeted(error, shown)
}

/// The error record for an output file that could not be written.
fn output_error(e: &io::Error, shown: &str) -> PsError {
    targeted(PsError::new(category_of(e, ErrorCategory::WriteError), "ZstdWriteFailed", format!("Cannot write '{shown}': {e}")), shown)
}

/// Opens the file at `path` for reading, with its length. A directory
/// is refused, naming `parameter`, the parameter that named it.
fn open_input(path: &Path, parameter: &str) -> PsResult<(File, u64)> {
    let shown = path.display().to_string();
    let meta = fs::metadata(path).map_err(|e| input_error(&e, &shown))?;
    if meta.is_dir() {
        return Err(targeted(
            PsError::new(ErrorCategory::InvalidArgument, "ZstdInputIsDirectory", format!("'{shown}' is a directory, and {parameter} takes a file.")),
            &shown,
        ));
    }
    let file = File::open(path).map_err(|e| input_error(&e, &shown))?;
    let size = file.metadata().map_err(|e| input_error(&e, &shown))?.len();
    Ok((file, size))
}

/// The error record for a dictionary libzstd would not take: one in
/// zstd's format whose tables it refused, `what` naming where it came
/// from, or one it had no memory to load.
fn dictionary_refused(e: &io::Error, what: &str, target: Option<&str>) -> PsError {
    let error = if e.kind() == io::ErrorKind::OutOfMemory {
        PsError::new(ErrorCategory::ResourceUnavailable, "ZstdOutOfMemory", format!("Cannot load {what} as a dictionary: {e}"))
    } else {
        PsError::new(
            ErrorCategory::InvalidData,
            "ZstdDictionaryInvalid",
            format!("Cannot use {what} as a dictionary: it begins with zstd's dictionary magic number, and libzstd refused its tables ({e})."),
        )
    };
    at(error, target)
}

/// The dictionary in the file `path` names, taken as written, read whole
/// and checked as `Dictionary::checked` checks it.
fn dictionary_from_file(ps: &Pipeline<'_>, path: &str) -> PsResult<Dictionary> {
    let file_path = provider_path(ps, path)?;
    let shown = file_path.display().to_string();
    let (file, size) = open_input(&file_path, "-DictionaryPath")?;
    let unheld = |e: &dyn std::fmt::Display| {
        targeted(
            PsError::new(ErrorCategory::ResourceUnavailable, "ZstdOutOfMemory", format!("Cannot hold the dictionary '{shown}' ({size} bytes) in memory: {e}")),
            &shown,
        )
    };
    let wanted = usize::try_from(size).map_err(|e| unheld(&e))?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(wanted).map_err(|e| unheld(&e))?;
    file.take(size).read_to_end(&mut bytes).map_err(|e| input_error(&e, &shown))?;
    if bytes.len() != wanted {
        let ended = io::Error::new(io::ErrorKind::UnexpectedEof, format!("it ended after {} of the {size} bytes it held when it was opened", bytes.len()));
        return Err(input_error(&ended, &shown));
    }
    Dictionary::checked(bytes).map_err(|e| dictionary_refused(&e, &format!("'{shown}'"), Some(&shown)))
}

/// The dictionary the `byte[]` given to -Dictionary holds, copied out of
/// its pinned array and checked as `Dictionary::checked` checks it.
fn dictionary_from_bytes(given: &PsObject) -> PsResult<Dictionary> {
    let pinned = given.pin::<u8>()?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(pinned.len()).map_err(|e| {
        PsError::new(ErrorCategory::ResourceUnavailable, "ZstdOutOfMemory", format!("Cannot hold the -Dictionary bytes ({} bytes) in memory: {e}", pinned.len()))
    })?;
    bytes.extend_from_slice(&pinned);
    drop(pinned);
    Dictionary::checked(bytes).map_err(|e| dictionary_refused(&e, "the bytes given to -Dictionary", None))
}

/// The dictionary a command was given, `-Dictionary` or `-DictionaryPath`,
/// loaded before any input is read; `None` when it was given neither.
fn given_dictionary(ps: &Pipeline<'_>, bytes: &PsObject, path: Option<&str>) -> PsResult<Option<Dictionary>> {
    if ps.parameter_is_bound("Dictionary") {
        return dictionary_from_bytes(bytes).map(Some);
    }
    match path {
        Some(path) => dictionary_from_file(ps, path).map(Some),
        None => Ok(None),
    }
}

/// Refuses a destination that is a directory, or that exists when
/// `force` is off.
fn check_destination(dest: &Path, force: bool) -> PsResult<()> {
    let shown = dest.display().to_string();
    match fs::metadata(dest) {
        Ok(meta) if meta.is_dir() => Err(targeted(
            PsError::new(ErrorCategory::InvalidArgument, "ZstdDestinationIsDirectory", format!("'{shown}' is a directory, and -DestinationPath takes a file.")),
            &shown,
        )),
        Ok(_) if !force => Err(targeted(
            PsError::new(ErrorCategory::ResourceExists, "ZstdDestinationExists", format!("'{shown}' already exists. Use -Force to replace it.")),
            &shown,
        )),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(output_error(&e, &shown)),
    }
}

/// Numbers the temporary files this process creates, so two operations
/// in one process never pick the same name.
static TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// Where the output for `dest` is written before it is renamed into
/// place: the same folder, so the rename stays on one volume. `None`
/// when `dest` names no file.
fn temporary_path(dest: &Path) -> Option<PathBuf> {
    let mut name = dest.file_name()?.to_os_string();
    name.push(format!(".{}-{}.zstd-partial", std::process::id(), TEMPORARY.fetch_add(1, Ordering::Relaxed)));
    Some(dest.with_file_name(name))
}

/// Removes the partial output at `temp` after `error`. When that fails
/// too, `error`'s message names the file left behind and why.
fn discard(temp: &Path, mut error: PsError) -> PsError {
    if let Err(e) = fs::remove_file(temp) {
        error.message = format!("{} The partial output '{}' could not be removed: {e}", error.message, temp.display());
    }
    error
}

/// Writes `dest` through a temporary file beside it, which `fill`
/// writes and which is renamed onto `dest` only once `fill` has
/// finished. A failure or a stop removes the temporary file, so no
/// partial `dest` is left, and a file already at `dest` survives both.
///
/// Returns the bytes written, or `None` when `fill` stopped.
fn write_through_temporary<E>(
    dest: &Path,
    fill: impl FnOnce(&mut FileSink) -> Result<bool, E>,
    report: impl FnOnce(E) -> PsError,
) -> PsResult<Option<u64>> {
    let shown = dest.display().to_string();
    let Some(temp) = temporary_path(dest) else {
        return Err(targeted(
            PsError::new(ErrorCategory::InvalidArgument, "ZstdWriteFailed", format!("'{shown}' does not name a file.")),
            &shown,
        ));
    };
    let file = OpenOptions::new().write(true).create_new(true).open(&temp).map_err(|e| output_error(&e, &shown))?;
    let mut sink = FileSink { file, written: 0 };
    let outcome = fill(&mut sink);
    let FileSink { file, written } = sink;
    drop(file);
    match outcome {
        Ok(true) => match fs::rename(&temp, dest) {
            Ok(()) => Ok(Some(written)),
            Err(e) => Err(discard(&temp, output_error(&e, &shown))),
        },
        Ok(false) => match fs::remove_file(&temp) {
            Ok(()) => Ok(None),
            Err(e) => Err(output_error(&e, &temp.display().to_string())),
        },
        Err(failure) => Err(discard(&temp, report(failure))),
    }
}

/// How a mapped write ended short of its destination.
enum Unmapped {
    /// The temporary file could not be given its length or mapped. It is
    /// removed and nothing was written, so the output can be written
    /// another way.
    Refused(io::Error),
    /// The temporary file could not be created, or, after a refused map,
    /// removed; filling the output failed; or renaming it onto its
    /// destination did. The error record says which.
    Failed(PsError),
}

/// Writes `dest` as `write_through_temporary` does, through a temporary
/// file beside it that is `len` bytes long from the start and mapped
/// into memory for `fill` to write in place. Returns the bytes written,
/// or `None` when `fill` stopped.
fn write_mapped_through_temporary<E>(
    dest: &Path,
    len: u64,
    fill: impl FnOnce(&mut [u8]) -> Result<bool, E>,
    report: impl FnOnce(E) -> PsError,
) -> Result<Option<u64>, Unmapped> {
    let shown = dest.display().to_string();
    let Some(temp) = temporary_path(dest) else {
        return Err(Unmapped::Failed(targeted(
            PsError::new(ErrorCategory::InvalidArgument, "ZstdWriteFailed", format!("'{shown}' does not name a file.")),
            &shown,
        )));
    };
    let file = OpenOptions::new().read(true).write(true).create_new(true).open(&temp).map_err(|e| Unmapped::Failed(output_error(&e, &shown)))?;
    let mapped = file.set_len(len).and_then(|()| {
        // SAFETY: this process created the temporary file with
        // create_new, under a name no other is given, and nothing else
        // maps it.
        unsafe { memmap2::MmapMut::map_mut(&file) }
    });
    let mut map = match mapped {
        Ok(map) => map,
        Err(e) => {
            drop(file);
            return match fs::remove_file(&temp) {
                Ok(()) => Err(Unmapped::Refused(e)),
                Err(removing) => Err(Unmapped::Failed(output_error(
                    &io::Error::new(removing.kind(), format!("{e}; the temporary file '{}' could not be removed: {removing}", temp.display())),
                    &shown,
                ))),
            };
        }
    };
    let outcome = fill(&mut map[..]);
    let flushed = map.flush();
    drop(map);
    drop(file);
    match outcome {
        Ok(true) => match flushed.and_then(|()| fs::rename(&temp, dest)) {
            Ok(()) => Ok(Some(len)),
            Err(e) => Err(Unmapped::Failed(discard(&temp, output_error(&e, &shown)))),
        },
        Ok(false) => match fs::remove_file(&temp) {
            Ok(()) => Ok(None),
            Err(e) => Err(Unmapped::Failed(output_error(&e, &temp.display().to_string()))),
        },
        Err(failure) => Err(Unmapped::Failed(discard(&temp, report(failure)))),
    }
}

/// How compression fits a budget.
#[derive(Clone, Copy, Debug)]
enum Fit {
    /// The settings to compress with, with the worker threads asked for or
    /// as many of them as fit, and what the work is estimated to take with
    /// them.
    Within(Settings, u64),
    /// Not even the calling thread, or one worker, fits: what that is
    /// estimated to take.
    Beyond(u64),
}

/// Fits `settings` to `budget`: the worker threads asked for, or as many
/// of them as fit. The work is estimated as libzstd's share for an input
/// of `size` bytes, `stable` or not, the dictionary's `share`, the
/// command's own share, and `mapped` bytes besides for the page tables of
/// a mapped input; beside it, a result of up to `result` bytes is held in
/// memory twice, 0 for a result written to a file.
fn fitting(settings: Settings, size: Option<u64>, stable: bool, mapped: u64, result: u64, share: u64, budget: i64) -> io::Result<Fit> {
    let working = |workers: u32| -> io::Result<u64> {
        let libzstd = if workers == 0 { calling_thread_estimate(settings.level, size, stable) } else { workers_estimate(settings.level, size, workers, stable) }?;
        Ok(libzstd.saturating_add(OWN_SHARE).saturating_add(share).saturating_add(mapped))
    };
    let within = |estimate: u64| fits(estimate.saturating_add(result.saturating_mul(2)), budget);
    if settings.threads == 0 {
        let estimate = working(0)?;
        return Ok(if within(estimate) { Fit::Within(settings, estimate) } else { Fit::Beyond(estimate) });
    }
    for workers in (1..=settings.threads).rev() {
        let estimate = working(workers)?;
        if within(estimate) {
            return Ok(Fit::Within(Settings { threads: workers, ..settings }, estimate));
        }
    }
    Ok(Fit::Beyond(working(1)?))
}

/// The error record for memory that could not be estimated.
fn unestimated(e: &io::Error, ends: Ends<'_>) -> PsError {
    at(
        PsError::new(ErrorCategory::InvalidOperation, "ZstdCompressFailed", format!("Cannot estimate the memory compressing {} takes: {e}", ends.input())),
        ends.from,
    )
}

/// The settings to compress input that is not mapped with within
/// `budget`, as `fitting` fits them, and what the work itself is
/// estimated to take with them. With no budget, the settings as they are.
/// Refused with `ZstdBudgetTooSmall` when the calling thread, or one
/// worker, does not fit.
fn fit(settings: Settings, size: Option<u64>, stable: bool, result: u64, dictionary: Option<&Dictionary>, budget: Option<i64>, ends: Ends<'_>) -> PsResult<(Settings, Option<u64>)> {
    let Some(budget) = budget else {
        return Ok((settings, None));
    };
    let share = Dictionary::compression_share(dictionary, settings.level);
    match fitting(settings, size, stable, 0, result, share, budget).map_err(|e| unestimated(&e, ends))? {
        Fit::Within(settings, estimate) => Ok((settings, Some(estimate))),
        Fit::Beyond(working) => Err(budget_too_small(working, share, result, budget, settings, ends)),
    }
}

/// Runs `use_bytes` over the bytes of one `-InputObject` record, the
/// `byte[]` the binder made of it, through a pinned borrow, uncopied. A
/// record whose bytes cannot be pinned is refused with
/// `ZstdInputNotBytes`.
fn with_record_bytes<T>(record: &PsObject, use_bytes: impl FnOnce(&[u8]) -> T) -> PsResult<T> {
    let bytes = record.pin::<u8>().map_err(|e| not_bytes(record, &e))?;
    Ok(use_bytes(&bytes))
}

/// What `Compress-Zstd -InputObject` holds between records.
#[derive(Default)]
enum Piped {
    /// No record has arrived.
    #[default]
    Waiting,
    /// One record has arrived and is held as the pipeline delivered it,
    /// uncopied, so that a single `byte[]` is compressed in one piece with
    /// its length recorded in the frame header.
    Holding(PsObject),
    /// Two or more records have arrived, each fed to libzstd as it came.
    Streaming(Compression),
    /// A record could not be compressed. Its error was reported, and the
    /// records after it are passed over.
    Failed,
}

/// What `Expand-Zstd -InputObject` holds between records.
#[derive(Default)]
enum Unpacking {
    /// No record has arrived.
    #[default]
    Waiting,
    /// The first record, whose frames all record their content size,
    /// expanded straight into a byte[] of the size they add up to, made by
    /// the host: the byte[], the bytes the record held, and the bytes it
    /// expanded to.
    Expanded(PsObject, u64, u64),
    /// Records have arrived, each fed to libzstd as it came.
    Streaming(Expansion<BufferSink>),
    /// A record could not be expanded. Its error was reported, and the
    /// records after it are passed over.
    Failed,
}

/// Space a result is expanded into in place, as long as its frames record
/// it to be.
trait Space: Sized {
    /// Space of `len` bytes, or why it cannot be made.
    fn make(len: u64) -> Result<Self, String>;

    /// Runs `fill` over the space as a slice.
    fn fill<T>(&mut self, fill: impl FnOnce(&mut [u8]) -> T) -> PsResult<T>;
}

/// A byte[] the host makes, through System.Array.CreateInstance, and
/// lends pinned; it is written to the pipeline as it is.
impl Space for PsObject {
    fn make(len: u64) -> Result<PsObject, String> {
        let len = i32::try_from(len).map_err(|e| format!("its length does not fit an Int32: {e}"))?;
        let made = || -> PsResult<PsObject> {
            let byte = PsType::from_name("System.Type").call_static("GetType", &["System.Byte".into_ps()?])?;
            PsType::from_name("System.Array").call_static("CreateInstance", &[byte, len.into_ps()?])
        };
        made().map_err(|e| e.message)
    }

    fn fill<T>(&mut self, fill: impl FnOnce(&mut [u8]) -> T) -> PsResult<T> {
        let mut pinned = self.pin::<u8>()?;
        Ok(fill(&mut pinned))
    }
}

/// What expanding bytes straight into space of their recorded size did.
enum Direct<S> {
    /// The frames expanded into the space, which they fill exactly.
    Expanded(S),
    /// The space could not be made, for this reason.
    Unmade(String),
    /// `stopping` answered yes; nothing is to be written.
    Stopped,
}

/// Expands `bytes`, whose frames all record their content size, `total`
/// bytes in all, straight into space of that size as `S` makes it, as
/// `expand_mapped` expands into a file's map: libzstd writes into the
/// space as a stable buffer and keeps no window of its own, and it
/// refuses what `expand_mapped` refuses, so the space is returned only
/// when the frames fill it exactly.
fn expand_direct<S: Space>(bytes: &[u8], total: u64, dictionary: Option<&Dictionary>, stopping: &dyn Fn() -> bool) -> PsResult<Result<Direct<S>, Failure>> {
    let mut space = match S::make(total) {
        Ok(space) => space,
        Err(reason) => return Ok(Ok(Direct::Unmade(reason))),
    };
    Ok(match space.fill(|slice| expand_mapped(bytes, slice, dictionary, stopping))? {
        Ok(true) => Ok(Direct::Expanded(space)),
        Ok(false) => Ok(Direct::Stopped),
        Err(failure) => Err(failure),
    })
}

/// Whether expanding straight into a byte[] of `total` bytes fits
/// `budget`: the result, held in memory once; libzstd's context and a
/// block of input it holds back, and no window, since libzstd writes
/// into the byte[] itself; the dictionary's share, which `dictionary_fits`
/// checks first; and the command's own share.
fn direct_fits(total: u64, dictionary: Option<&Dictionary>, budget: i64) -> Result<(), Failure> {
    let share = Dictionary::expansion_share(dictionary);
    dictionary_fits(share, budget)?;
    // SAFETY: computes from nothing outside it.
    let context = unsafe { zstd_sys::ZSTD_estimateDCtxSize() } as u64;
    let needed = context.saturating_add(BLOCK_MAX).saturating_add(share).saturating_add(OWN_SHARE).saturating_add(total);
    if fits(needed, budget) {
        Ok(())
    } else {
        Err(Failure::HeldOnceTooLarge { needed, result: total, dictionary: share, budget })
    }
}

/// How the first `-InputObject` record of `Expand-Zstd` was taken.
enum Whole<S> {
    /// Every frame in it records its content size, and they expanded
    /// straight into space of the size they add up to, the `u64` bytes.
    Expanded(S, u64),
    /// It is to be fed to libzstd a chunk at a time, as the records after
    /// it are; when that is not because a frame records no content size,
    /// with the reason.
    Streamed(Option<String>),
    /// `stopping` answered yes; nothing is to be written.
    Stopped,
}

/// Takes the bytes of the first `-InputObject` record whole when every
/// frame in them records its content size: they are expanded straight
/// into space of the size the frames add up to, as `expand_direct`
/// expands them, within `budget` as `direct_fits` counts it. Bytes with a
/// frame that records no size, bytes whose sizes libzstd cannot add up,
/// such as bytes that end inside a frame, and space that cannot be made
/// leave the record to be fed to libzstd a chunk at a time instead.
fn expand_whole<S: Space>(bytes: &[u8], dictionary: Option<&Dictionary>, budget: Option<i64>, stopping: &dyn Fn() -> bool) -> PsResult<Result<Whole<S>, Failure>> {
    if bytes.is_empty() {
        return Ok(Ok(Whole::Streamed(None)));
    }
    let total = match zstd_safe::find_decompressed_size(bytes) {
        Ok(Some(total)) => total,
        Ok(None) => return Ok(Ok(Whole::Streamed(None))),
        Err(e) => return Ok(Ok(Whole::Streamed(Some(format!("libzstd cannot add up the content sizes its frames record ({e})"))))),
    };
    if let Some(budget) = budget && let Err(failure) = direct_fits(total, dictionary, budget) {
        return Ok(Err(failure));
    }
    Ok(match expand_direct::<S>(bytes, total, dictionary, stopping)? {
        Ok(Direct::Expanded(space)) => Ok(Whole::Expanded(space, total)),
        Ok(Direct::Unmade(reason)) => Ok(Whole::Streamed(Some(format!("a byte[] of {total} bytes could not be made ({reason})")))),
        Ok(Direct::Stopped) => Ok(Whole::Stopped),
        Err(failure) => Err(failure),
    })
}

/// The expansion the records after a first one taken whole go on with: a
/// chunk at a time, after the `taken` bytes of whole frames that first
/// record held, into memory that starts with a copy of their result,
/// `space`, of `result` bytes, within `budget` as `Expansion` counts it.
/// From here the result is held twice, as it is built and as the byte[]
/// written, so the copy is made only when the result so far fits that
/// way.
fn go_on_from<S: Space>(space: &mut S, taken: u64, result: u64, dictionary: Option<&Dictionary>, budget: Option<i64>) -> Result<Expansion<BufferSink>, Failure> {
    if let Some(budget) = budget {
        let share = Dictionary::expansion_share(dictionary);
        dictionary_fits(share, budget)?;
        let limit = Limit::left(budget, share.saturating_add(OWN_SHARE));
        if result > limit.most {
            return Err(Failure::ResultTooLarge { result: Some(result), limit });
        }
    }
    let copied = space.fill(|held| {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(held.len()).map(|()| {
            bytes.extend_from_slice(held);
            bytes
        })
    });
    let bytes = match copied {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(e)) => return Err(Failure::Write(io::Error::new(io::ErrorKind::OutOfMemory, e))),
        Err(e) => return Err(Failure::Write(io::Error::other(e.message))),
    };
    Expansion::after_frames(BufferSink { bytes, limit: None }, taken, dictionary, budget)
}

/// The error record for bytes that never arrived: the InputObject set chosen
/// with no -InputObject, or a `$null` record. It ends nothing; the records
/// around a `$null` are taken as if it had not been sent.
fn no_input() -> PsError {
    PsError::new(ErrorCategory::InvalidArgument, "ZstdNoInput", "No bytes were given: -InputObject takes a byte[], from the pipeline or by name.")
}

/// The error record for a record `pin` refused. A `byte[]` that would
/// not pin reports the pin's own error; anything else is named by type.
fn not_bytes(record: &PsObject, pin_error: &PsError) -> PsError {
    let message = match record.type_name() {
        Ok(name) if name == "System.Byte[]" => format!("Cannot read this byte[]: {}", pin_error.message),
        Ok(name) => format!("-InputObject takes a byte[], and this record reached it as a {name}."),
        Err(e) => format!("-InputObject takes a byte[], and this record's type could not be read: {}", e.message),
    };
    PsError::new(ErrorCategory::InvalidType, "ZstdInputNotBytes", message)
}

/// Compresses files, or bytes, with Zstandard.
///
/// With -Path or -LiteralPath, each file named is compressed on its own:
/// a file that fails gets its own error record and the rest go on. Files
/// piped from Get-ChildItem or Get-Item bind to -LiteralPath by their
/// PSPath. A file is mapped into memory and handed to libzstd 1 MiB more at
/// a time, or read in 1 MiB chunks when Windows will not map it or, within
/// -MaxMemory, its map does not fit, and written to a temporary file beside
/// its destination that is renamed onto it only when complete. The destination is the file's name with .zst added,
/// beside it, unless -DestinationPath names a folder to write into or one
/// file, which only the first input of the command may take. With
/// -InputObject, a single byte[] is compressed in one
/// piece, and the records of a longer pipeline are fed to libzstd as they
/// arrive; the result is written as one byte[] when the pipeline ends.
/// Every result is one zstd frame with a content checksum, whose header
/// records the content size for a file or a single byte[].
///
/// # Examples
/// Compress-Zstd -Path .\app.log
/// Compress-Zstd -Path .\logs\*.log -DestinationPath .\archive -PassThru
/// Get-ChildItem .\logs -Filter *.log | Compress-Zstd -Level 19
/// Compress-Zstd -Path .\app.log -DestinationPath .\app.log.zst -Level 19 -Threads 4 -Force
/// $packed = Compress-Zstd -InputObject ([System.Text.Encoding]::UTF8.GetBytes('hello'))
#[cmdlet(verb = "Compress", noun = "Zstd", default_parameter_set = "Path", supports_should_process, output = ["System.IO.FileInfo", "System.Byte[]"])]
#[derive(Default)]
pub struct CompressZstd {
    /// The files to compress. Wildcard characters select every file they match, and a relative path is resolved against the current PowerShell location.
    #[param(mandatory, position = 0, set = ["Path", "PathDictionary"], validate_not_null_or_empty)]
    pub path: Vec<String>,
    /// The files to compress, each named exactly as written, wildcard characters included. A file piped from Get-ChildItem or Get-Item binds here by its PSPath.
    #[param(mandatory, set = ["LiteralPath", "LiteralPathDictionary"], literal_path, validate_not_null_or_empty)]
    pub literal_path: Vec<String>,
    /// Where to write. Without it, each file is written beside itself under its name with .zst added. An existing folder receives each file under that name; any other path is one file, which only the first input of the command may take.
    #[param(position = 1, set = ["Path", "LiteralPath", "PathDictionary", "LiteralPathDictionary"])]
    pub destination_path: Option<String>,
    /// The bytes to compress, as a byte[]. A single byte piped from an enumerated array arrives as a byte[] of one, and an empty byte[] compresses to a frame of nothing. A $null record, or no -InputObject at all where the bytes are expected, is refused with ZstdNoInput.
    #[param(set = ["InputObject", "InputObjectDictionary"], value_from_pipeline, clr = "byte[]", allow_empty_collection)]
    pub input_object: Option<PsObject>,
    /// A dictionary file, named exactly as written: one in zstd's format, whose id each frame's header then records, or any other file, whose bytes are used as raw content. Expanding the output needs the same dictionary.
    #[param(set = ["Path", "LiteralPath", "InputObject"], validate_not_null_or_empty)]
    pub dictionary_path: Option<String>,
    /// A dictionary as a byte[], taken as -DictionaryPath takes a file's bytes.
    #[param(mandatory, set = ["PathDictionary", "LiteralPathDictionary", "InputObjectDictionary"], clr = "byte[]")]
    pub dictionary: PsObject,
    /// The compression level, 1 to 22. Higher levels trade speed for a smaller output. The default is 3, libzstd's own default.
    #[param(validate_range(1, 22))]
    pub level: Option<i32>,
    /// Worker threads libzstd compresses with, 0 to 256. The default, 0, compresses on the calling thread.
    #[param(validate_range(0, 256))]
    pub threads: Option<i32>,
    /// The most memory, in bytes, the command may commit, 1MB or more: what libzstd takes, estimated before any work starts, a dictionary, the command's own buffers, the page tables of a file's map, and on bytes the result, which is held in memory twice. A file whose map does not fit is read a chunk at a time. Compression uses as many of the -Threads worker threads as fit, and is refused when the calling thread, or one worker, does not fit. Without it, compression uses the worker threads asked for.
    #[param(validate_range(1048576, 9223372036854775807))]
    pub max_memory: Option<i64>,
    /// Replaces a destination file that exists.
    #[param(set = ["Path", "LiteralPath", "PathDictionary", "LiteralPathDictionary"])]
    pub force: bool,
    /// Writes the FileInfo of each file written.
    #[param(set = ["Path", "LiteralPath", "PathDictionary", "LiteralPathDictionary"])]
    pub pass_thru: bool,
    /// What -InputObject has delivered so far.
    piped: Piped,
    /// Whether an input of this command has taken a -DestinationPath that names one file.
    destination_taken: bool,
    /// The dictionary -Dictionary or -DictionaryPath gave, loaded before any input.
    loaded: Option<Dictionary>,
    /// Whether that dictionary was refused; the command then does nothing more.
    refused: bool,
}

/// Feeds the bytes of one `-InputObject` record to `stream`. The outer
/// result is reading the record; the inner one is libzstd's, as `feed`
/// gives it.
fn feed_record(stream: &mut Compression, record: &PsObject, stopping: &dyn Fn() -> bool) -> PsResult<Result<bool, Failure>> {
    with_record_bytes(record, |bytes| stream.feed(bytes, stopping))
}

/// Reports what a compression within -MaxMemory was fitted to: a warning
/// when fewer of the `asked` worker threads fit than were asked for,
/// naming both, the estimate and the budget; and in the verbose stream
/// the workers it uses, the memory estimated for the work, and, for a
/// result held in memory, the most that result may hold.
fn report_fit(ps: &Pipeline<'_>, asked: u32, settings: Settings, fitted: Option<(u64, i64)>, result: Option<u64>, ends: Ends<'_>) -> PsResult<()> {
    let Some((estimate, budget)) = fitted else {
        return Ok(());
    };
    let held = match result {
        Some(most) => format!(", and its result of up to {most} bytes, held in memory twice"),
        None => String::new(),
    };
    if settings.threads < asked {
        // Written whatever -WarningAction says, as PowerShell's own
        // commands write theirs, so -WarningVariable collects it.
        ps.warning(&format!(
            "Compressing {} with {} of the {asked} worker threads asked for, the most that fit -MaxMemory of {budget} bytes: estimated at {estimate} bytes{held}.",
            ends.input(),
            settings.threads
        ))?;
    }
    pwrs::verbose!(ps, "Within -MaxMemory of {budget} bytes: level {} with {} worker threads, estimated at {estimate} bytes{held}.", settings.level, settings.threads)?;
    Ok(())
}

/// Compresses `bytes`, the whole of the one `-InputObject` record, an
/// empty `byte[]` included, into one frame whose header records their
/// length, and writes it as one `byte[]`, within `budget`, which counts
/// the frame at its largest, compressBound of the length, held twice.
fn compress_whole(ps: &Pipeline<'_>, bytes: &[u8], settings: Settings, dictionary: Option<&Dictionary>, budget: Option<i64>) -> PsResult<()> {
    let bound = libzstd_size(zstd_safe::compress_bound(bytes.len())).map_err(|e| failed(Operation::Compress, Failure::Codec(e), PIPED))?;
    let asked = settings.threads;
    let (settings, estimate) = fit(settings, Some(bytes.len() as u64), true, bound, dictionary, budget, PIPED)?;
    report_fit(ps, asked, settings, estimate.zip(budget), Some(bound), PIPED)?;
    let mut sink = BufferSink::new(budget.map(|budget| Limit { most: bound, budget }));
    match compress_stable(bytes, &mut sink, settings, dictionary, &|| ps.stopping()) {
        Ok(true) => {
            pwrs::verbose!(ps, "Compressed {} bytes to {} bytes at level {} with {} worker threads.", bytes.len(), sink.bytes.len(), settings.level, settings.threads)?;
            ps.write(PsArray(sink.bytes))
        }
        Ok(false) => Ok(()),
        Err(failure) => Err(failed(Operation::Compress, failure, PIPED)),
    }
}

impl CompressZstd {
    /// Takes one -InputObject record: the first is held, and from the
    /// second on every record is fed to libzstd as it arrives. A record
    /// that is not bytes ends the command, because the frame would
    /// otherwise go on without its bytes.
    fn take_record(&mut self, ps: &Pipeline<'_>, record: &PsObject) -> PsResult<()> {
        with_record_bytes(record, |_| ()).map_err(PsError::terminating)?;
        let stopping = || ps.stopping();
        let mut stream = match std::mem::take(&mut self.piped) {
            Piped::Waiting => {
                self.piped = Piped::Holding(record.clone());
                return Ok(());
            }
            Piped::Failed => {
                self.piped = Piped::Failed;
                return Ok(());
            }
            Piped::Streaming(stream) => stream,
            Piped::Holding(first) => {
                let settings = Settings::from_parameters(self.level, self.threads)?;
                let asked = settings.threads;
                let (settings, estimate) = match fit(settings, None, false, 0, self.loaded.as_ref(), self.max_memory, PIPED) {
                    Ok(fitted) => fitted,
                    Err(error) => {
                        self.piped = Piped::Failed;
                        return Err(error);
                    }
                };
                // The frame's length is not known, so it may grow into what
                // the work leaves of the budget, held twice.
                let limit = self.max_memory.zip(estimate).map(|(budget, working)| Limit::left(budget, working));
                report_fit(ps, asked, settings, estimate.zip(self.max_memory), limit.map(|limit| limit.most), PIPED)?;
                let mut stream = match Compression::new(settings, self.loaded.as_ref(), limit) {
                    Ok(stream) => stream,
                    Err(failure) => return self.fail(failure),
                };
                match feed_record(&mut stream, &first, &stopping).map_err(PsError::terminating)? {
                    Ok(true) => stream,
                    Ok(false) => return Ok(()),
                    Err(failure) => return self.fail(failure),
                }
            }
        };
        match feed_record(&mut stream, record, &stopping).map_err(PsError::terminating)? {
            Ok(true) => {
                self.piped = Piped::Streaming(stream);
                Ok(())
            }
            Ok(false) => Ok(()),
            Err(failure) => self.fail(failure),
        }
    }

    /// Reports `failure` as the error record for the piped bytes, and
    /// passes over the records after it.
    fn fail(&mut self, failure: Failure) -> PsResult<()> {
        self.piped = Piped::Failed;
        Err(failed(Operation::Compress, failure, PIPED))
    }

    /// Writes the frame for everything -InputObject delivered: one piece
    /// with its length for one record, an empty byte[] included, and the
    /// streamed frame for more. When no record reached the cmdlet, such as
    /// when the binder refused every one, nothing is written.
    fn finish_piped(&mut self, ps: &Pipeline<'_>) -> PsResult<()> {
        let settings = Settings::from_parameters(self.level, self.threads)?;
        let dictionary = self.loaded.as_ref();
        let budget = self.max_memory;
        match std::mem::take(&mut self.piped) {
            Piped::Waiting => Ok(()),
            Piped::Holding(first) => with_record_bytes(&first, |bytes| compress_whole(ps, bytes, settings, dictionary, budget))?,
            Piped::Streaming(stream) => match stream.finish() {
                Ok((frame, taken, settings)) => {
                    pwrs::verbose!(ps, "Compressed {taken} bytes to {} bytes at level {} with {} worker threads.", frame.len(), settings.level, settings.threads)?;
                    ps.write(PsArray(frame))
                }
                Err(failure) => Err(failed(Operation::Compress, failure, PIPED)),
            },
            Piped::Failed => Ok(()),
        }
    }

    /// Whether a file set is bound. -Path and -LiteralPath are mandatory
    /// in theirs, where the binder refuses an empty list, and both stay
    /// empty in the InputObject set.
    fn file_mode(&self) -> bool {
        !self.path.is_empty() || !self.literal_path.is_empty()
    }

    /// Compresses every file -Path or -LiteralPath names, each on its own:
    /// one that fails gets its own error record, and the rest go on.
    fn compress_files(&mut self, ps: &Pipeline<'_>) -> PsResult<()> {
        let settings = Settings::from_parameters(self.level, self.threads)?;
        for source in named_files(ps, &self.path, &self.literal_path) {
            if ps.stopping() {
                return Ok(());
            }
            if let Err(error) = source.and_then(|src| self.compress_file(ps, &src.path, src.parameter, settings)) {
                ps.write_error(&error)?;
            }
        }
        Ok(())
    }

    fn compress_file(&mut self, ps: &Pipeline<'_>, src: &Path, parameter: &str, settings: Settings) -> PsResult<()> {
        let (file, size) = open_input(src, parameter)?;
        let dest = destination(ps, self.destination_path.as_deref(), &mut self.destination_taken, src, Operation::Compress)?;
        check_destination(&dest, self.force)?;
        let from = src.display().to_string();
        let to = dest.display().to_string();
        if !ps.should_process(&format!("Item: {from} Destination: {to}"), "Compress File")? {
            return Ok(());
        }
        let ends = Ends { from: Some(&from), to: Some(&to) };
        let stopping = || ps.stopping();
        let dictionary = self.loaded.as_ref();
        let budget = self.max_memory;
        let asked = settings.threads;
        let share = Dictionary::compression_share(dictionary, settings.level);
        // A file with bytes in it is read from a map of it, the first
        // `size` bytes, which libzstd takes as a stable input. Within a
        // budget the map's page tables count too, and a file whose map
        // does not fit is read a chunk at a time, as one Windows will not
        // map is.
        let mut fitted = None;
        let mut beyond = None;
        let mapped = if size == 0 {
            None
        } else {
            let fits_mapped = match budget {
                None => true,
                Some(budget) => match fitting(settings, Some(size), true, page_tables(size), 0, share, budget).map_err(|e| unestimated(&e, ends))? {
                    Fit::Within(settings, estimate) => {
                        fitted = Some((settings, estimate));
                        true
                    }
                    Fit::Beyond(working) => {
                        beyond = Some(working);
                        false
                    }
                },
            };
            if fits_mapped {
                match map_input(&file, size) {
                    Ok(map) => Some(map),
                    Err(e) => {
                        pwrs::verbose!(ps, "Cannot map '{from}' into memory ({e}); reading it a chunk at a time instead.")?;
                        fitted = None;
                        None
                    }
                }
            } else {
                None
            }
        };
        let (settings, estimate) = match (fitted, budget) {
            (Some((settings, estimate)), _) => (settings, Some(estimate)),
            (None, None) => (settings, None),
            (None, Some(budget)) => match fitting(settings, Some(size), false, 0, 0, share, budget).map_err(|e| unestimated(&e, ends))? {
                Fit::Within(settings, estimate) => {
                    if let Some(working) = beyond {
                        pwrs::verbose!(
                            ps,
                            "Reading '{from}' a chunk at a time: mapping it takes {} bytes of page tables, and with them compressing it needs about {working} bytes, more than -MaxMemory of {budget} bytes.",
                            page_tables(size)
                        )?;
                    }
                    (settings, Some(estimate))
                }
                // The smaller of what the file needs read a chunk at a
                // time and what it needs mapped.
                Fit::Beyond(working) => return Err(budget_too_small(beyond.map_or(working, |mapped| mapped.min(working)), share, 0, budget, settings, ends)),
            },
        };
        report_fit(ps, asked, settings, estimate.zip(budget), None, ends)?;
        let written = match &mapped {
            Some(map) => write_through_temporary(
                &dest,
                move |sink| compress_stable(map, sink, settings, dictionary, &stopping),
                move |failure| failed(Operation::Compress, failure, ends),
            )?,
            None => {
                // The frame's pledged size is the length when opened, so
                // bytes appended while it is read are not compressed.
                let mut input = file.take(size);
                write_through_temporary(
                    &dest,
                    move |sink| compress(&mut input, sink, Some(size), settings, dictionary, &stopping),
                    move |failure| failed(Operation::Compress, failure, ends),
                )?
            }
        };
        if let Some(written) = written {
            pwrs::verbose!(ps, "Compressed '{from}' ({size} bytes) to '{to}' ({written} bytes) at level {} with {} worker threads.", settings.level, settings.threads)?;
            if self.pass_thru {
                pass_through(ps, &to)?;
            }
        }
        Ok(())
    }
}

impl Cmdlet for CompressZstd {
    /// Loads the dictionary given, if any, before any input is read. One
    /// that cannot be loaded is reported once, and the command then does
    /// nothing more.
    fn begin(&mut self, ps: &Pipeline<'_>) -> PsResult<()> {
        match given_dictionary(ps, &self.dictionary, self.dictionary_path.as_deref()) {
            Ok(loaded) => {
                self.loaded = loaded;
                Ok(())
            }
            Err(error) => {
                self.refused = true;
                Err(error)
            }
        }
    }

    fn process(&mut self, ps: &Pipeline<'_>) -> PsResult<()> {
        if self.refused {
            return Ok(());
        }
        if self.file_mode() {
            return self.compress_files(ps);
        }
        // A $null record binds as a null object, and no -InputObject at all
        // leaves the parameter unset; both are bytes that never arrived.
        match self.input_object.take() {
            Some(record) if !record.is_null() => self.take_record(ps, &record),
            _ => Err(no_input()),
        }
    }

    fn end(&mut self, ps: &Pipeline<'_>) -> PsResult<()> {
        if self.refused || self.file_mode() {
            return Ok(());
        }
        self.finish_piped(ps)
    }
}

/// Expands Zstandard-compressed data from files, or bytes.
///
/// With -Path or -LiteralPath, each file named is expanded on its own: a
/// file that fails gets its own error record and the rest go on. Files
/// piped from Get-ChildItem or Get-Item bind to -LiteralPath by their
/// PSPath. A file is expanded in 1 MiB chunks into a temporary file. A
/// frame whose window does not fit that way, over 128 MiB without
/// -MaxMemory or taking more than -MaxMemory to expand, sends the file to
/// be written in place instead, through a map
/// of it and of a temporary file as long as its frames' content sizes add
/// up to, when every frame records its content size. The temporary file is
/// beside the destination and is renamed onto it only when complete. The
/// destination is the file's name without its .zst, beside it, unless
/// -DestinationPath names a folder to write into or one file, which only
/// the first input of the command may take; a file not named .zst needs
/// -DestinationPath. With -InputObject, the result is written as one
/// byte[] when the pipeline ends. A first record whose frames all record
/// their content size, as the frames Compress-Zstd writes from a file or
/// a single byte[] do, is expanded straight into a byte[] of the size they
/// add up to, which takes a frame of any window; other bytes, and the
/// records after the first, are fed to libzstd as they arrive. Every frame
/// in the input is expanded, in order. Input that is empty, ends inside a
/// frame, holds bytes after a frame that do not begin another, or fails a
/// frame's checksum is refused with an error record, and nothing is
/// written.
///
/// # Examples
/// Expand-Zstd -Path .\app.log.zst
/// Expand-Zstd -Path .\archive\*.zst -DestinationPath .\restored -PassThru
/// Get-ChildItem .\archive -Filter *.zst | Expand-Zstd -DestinationPath .\restored
/// $bytes = Expand-Zstd -InputObject $packed
#[cmdlet(verb = "Expand", noun = "Zstd", default_parameter_set = "Path", supports_should_process, output = ["System.IO.FileInfo", "System.Byte[]"])]
#[derive(Default)]
pub struct ExpandZstd {
    /// The files to expand. Wildcard characters select every file they match, and a relative path is resolved against the current PowerShell location.
    #[param(mandatory, position = 0, set = ["Path", "PathDictionary"], validate_not_null_or_empty)]
    pub path: Vec<String>,
    /// The files to expand, each named exactly as written, wildcard characters included. A file piped from Get-ChildItem or Get-Item binds here by its PSPath.
    #[param(mandatory, set = ["LiteralPath", "LiteralPathDictionary"], literal_path, validate_not_null_or_empty)]
    pub literal_path: Vec<String>,
    /// Where to write. Without it, each file is written beside itself under its name without .zst. An existing folder receives each file under that name; any other path is one file, which only the first input of the command may take.
    #[param(position = 1, set = ["Path", "LiteralPath", "PathDictionary", "LiteralPathDictionary"])]
    pub destination_path: Option<String>,
    /// The zstd data to expand, as a byte[]. A single byte piped from an enumerated array arrives as a byte[] of one. A $null record, or no -InputObject at all where the bytes are expected, is refused with ZstdNoInput.
    #[param(set = ["InputObject", "InputObjectDictionary"], value_from_pipeline, clr = "byte[]", allow_empty_collection)]
    pub input_object: Option<PsObject>,
    /// The dictionary the data was compressed with, as a file named exactly as written. A frame whose header names a dictionary other than this one is refused.
    #[param(set = ["Path", "LiteralPath", "InputObject"], validate_not_null_or_empty)]
    pub dictionary_path: Option<String>,
    /// The dictionary the data was compressed with, as a byte[], taken as -DictionaryPath takes a file's bytes.
    #[param(mandatory, set = ["PathDictionary", "LiteralPathDictionary", "InputObjectDictionary"], clr = "byte[]")]
    pub dictionary: PsObject,
    /// The most memory, in bytes, the command may commit, 1MB or more: what libzstd takes for a frame's window, a dictionary, the command's own buffers, the page tables of a file's map, and on bytes the result. The result is held in memory once when the bytes are one record whose frames all record their content size, expanded straight into the byte[] written with no window, and twice otherwise, as it is built and as the byte[] written. A frame that does not fit a chunk at a time sends a file to be written in place, which takes a frame of any window, when all its frames record their content size and the page tables of both files' maps fit; otherwise the frame is refused. Without it, a frame expanded a chunk at a time may have a window of up to 128 MiB, libzstd's own limit.
    #[param(validate_range(1048576, 9223372036854775807))]
    pub max_memory: Option<i64>,
    /// Replaces a destination file that exists.
    #[param(set = ["Path", "LiteralPath", "PathDictionary", "LiteralPathDictionary"])]
    pub force: bool,
    /// Writes the FileInfo of each file written.
    #[param(set = ["Path", "LiteralPath", "PathDictionary", "LiteralPathDictionary"])]
    pub pass_thru: bool,
    /// What -InputObject has delivered so far.
    unpacking: Unpacking,
    /// Whether an input of this command has taken a -DestinationPath that names one file.
    destination_taken: bool,
    /// The dictionary -Dictionary or -DictionaryPath gave, loaded before any input.
    loaded: Option<Dictionary>,
    /// Whether that dictionary was refused; the command then does nothing more.
    refused: bool,
}

impl ExpandZstd {
    /// Takes one -InputObject record. The first, when every frame in it
    /// records its content size, is expanded straight into a byte[] of the
    /// size they add up to, which the host makes, as `expand_whole` takes
    /// it. Otherwise, and from the second record on, each record is fed to
    /// libzstd as it arrives, after a copy of the first record's result
    /// when that was taken whole, as `go_on_from` goes on. A record that is
    /// not bytes ends the command, because the data would otherwise go on
    /// without its bytes.
    fn take_record(&mut self, ps: &Pipeline<'_>, record: &PsObject) -> PsResult<()> {
        with_record_bytes(record, |_| ()).map_err(PsError::terminating)?;
        let stopping = || ps.stopping();
        let mut expansion = match std::mem::take(&mut self.unpacking) {
            Unpacking::Failed => {
                self.unpacking = Unpacking::Failed;
                return Ok(());
            }
            Unpacking::Streaming(expansion) => expansion,
            Unpacking::Expanded(mut array, taken, total) => match go_on_from(&mut array, taken, total, self.loaded.as_ref(), self.max_memory) {
                Ok(expansion) => expansion,
                Err(failure) => return self.fail(failure),
            },
            Unpacking::Waiting => {
                let (taken, whole) =
                    with_record_bytes(record, |bytes| (bytes.len() as u64, expand_whole::<PsObject>(bytes, self.loaded.as_ref(), self.max_memory, &stopping))).map_err(PsError::terminating)?;
                match whole {
                    Ok(Ok(Whole::Expanded(array, total))) => {
                        self.unpacking = Unpacking::Expanded(array, taken, total);
                        return Ok(());
                    }
                    Ok(Ok(Whole::Stopped)) => {
                        self.unpacking = Unpacking::Failed;
                        return Ok(());
                    }
                    Ok(Ok(Whole::Streamed(reason))) => {
                        if let Some(reason) = reason {
                            pwrs::verbose!(ps, "Expanding the input a chunk at a time: {reason}.")?;
                        }
                    }
                    Ok(Err(failure)) => return self.fail(failure),
                    Err(error) => {
                        self.unpacking = Unpacking::Failed;
                        return Err(error);
                    }
                }
                match Expansion::new(BufferSink::default(), self.loaded.as_ref(), self.max_memory) {
                    Ok(expansion) => expansion,
                    Err(failure) => return self.fail(failure),
                }
            }
        };
        match with_record_bytes(record, |bytes| expansion.feed(bytes, &stopping)).map_err(PsError::terminating)? {
            Ok(true) => {
                self.unpacking = Unpacking::Streaming(expansion);
                Ok(())
            }
            Ok(false) => Ok(()),
            Err(failure) => self.fail(failure),
        }
    }

    /// Reports `failure` as the error record for the piped data, and
    /// passes over the records after it.
    fn fail(&mut self, failure: Failure) -> PsResult<()> {
        self.unpacking = Unpacking::Failed;
        Err(failed(Operation::Expand, failure, PIPED))
    }

    /// Writes everything -InputObject delivered, expanded, as one
    /// `byte[]`, once the data is whole frames: the byte[] the first
    /// record expanded into when it was taken whole and no record followed
    /// it, and otherwise the result built a chunk at a time. Records that
    /// held no bytes at all are refused, as they hold no frame. When no
    /// record reached the cmdlet, such as when the binder refused every
    /// one, nothing is written, output or error.
    fn finish_piped(&mut self, ps: &Pipeline<'_>) -> PsResult<()> {
        match std::mem::take(&mut self.unpacking) {
            Unpacking::Waiting => Ok(()),
            Unpacking::Failed => Ok(()),
            Unpacking::Expanded(array, taken, total) => {
                pwrs::verbose!(ps, "Expanded {taken} bytes to {total} bytes.")?;
                ps.write_object(&array)
            }
            Unpacking::Streaming(expansion) => {
                let taken = expansion.taken;
                if taken == 0 {
                    return Err(empty_input(PIPED));
                }
                match expansion.finish() {
                    Ok(BufferSink { bytes, .. }) => {
                        pwrs::verbose!(ps, "Expanded {taken} bytes to {} bytes.", bytes.len())?;
                        ps.write(PsArray(bytes))
                    }
                    Err(failure) => Err(failed(Operation::Expand, failure, PIPED)),
                }
            }
        }
    }

    /// Whether a file set is bound; see `CompressZstd::file_mode`.
    fn file_mode(&self) -> bool {
        !self.path.is_empty() || !self.literal_path.is_empty()
    }

    /// Expands every file -Path or -LiteralPath names, each on its own:
    /// one that fails gets its own error record, and the rest go on.
    fn expand_files(&mut self, ps: &Pipeline<'_>) -> PsResult<()> {
        for source in named_files(ps, &self.path, &self.literal_path) {
            if ps.stopping() {
                return Ok(());
            }
            if let Err(error) = source.and_then(|src| self.expand_file(ps, &src.path, src.parameter)) {
                ps.write_error(&error)?;
            }
        }
        Ok(())
    }

    fn expand_file(&mut self, ps: &Pipeline<'_>, src: &Path, parameter: &str) -> PsResult<()> {
        let (file, size) = open_input(src, parameter)?;
        let dest = destination(ps, self.destination_path.as_deref(), &mut self.destination_taken, src, Operation::Expand)?;
        check_destination(&dest, self.force)?;
        let from = src.display().to_string();
        let to = dest.display().to_string();
        let ends = Ends { from: Some(&from), to: Some(&to) };
        if size == 0 {
            return Err(empty_input(ends));
        }
        if !ps.should_process(&format!("Item: {from} Destination: {to}"), "Expand File")? {
            return Ok(());
        }
        let stopping = || ps.stopping();
        let dictionary = self.loaded.as_ref();
        let budget = self.max_memory;
        let share = Dictionary::expansion_share(dictionary);
        // The dictionary counts whichever way the file is expanded.
        if let Some(budget) = budget {
            dictionary_fits(share, budget).map_err(|failure| failed(Operation::Expand, failure, ends))?;
        }
        // The file is expanded a chunk at a time, each frame's window checked
        // before libzstd sees it. A frame whose window does not fit that way
        // sends the file to be written in place instead.
        let refused = Cell::new(None);
        let streamed = {
            let mut input = &file;
            write_through_temporary(
                &dest,
                |sink| expand(&mut input, sink, dictionary, budget, &stopping),
                |failure| {
                    if let Failure::WindowTooLarge { window, .. } = &failure {
                        refused.set(Some(*window));
                    }
                    failed(Operation::Expand, failure, ends)
                },
            )
        };
        // Writes the file in place, through a map of it and a map of a
        // temporary file as long as its frames' content sizes add up to,
        // which takes a frame of any window. `Ok(None)` when it cannot be:
        // a frame records no content size, Windows will not map a file, or,
        // within a budget, what writing in place takes does not fit.
        let in_place = |window: u64| -> PsResult<Option<Option<u64>>> {
            if let Some(budget) = budget {
                let needed = page_tables(size).saturating_add(share).saturating_add(OWN_SHARE);
                if !fits(needed, budget) {
                    pwrs::verbose!(
                        ps,
                        "Cannot expand '{from}' in place: mapping it takes {} bytes of page tables, and with them it needs about {needed} bytes, more than -MaxMemory of {budget} bytes.",
                        page_tables(size)
                    )?;
                    return Ok(None);
                }
            }
            let input = match map_input(&file, size) {
                Ok(map) => map,
                Err(e) => {
                    pwrs::verbose!(ps, "Cannot expand '{from}' in place: Windows will not map it ({e}).")?;
                    return Ok(None);
                }
            };
            let total = match zstd_safe::find_decompressed_size(&input) {
                Ok(Some(total)) if total > 0 => total,
                Ok(Some(_)) => {
                    pwrs::verbose!(ps, "Cannot expand '{from}' in place: its frames record no content to write.")?;
                    return Ok(None);
                }
                Ok(None) => {
                    pwrs::verbose!(ps, "Cannot expand '{from}' in place: a frame in it does not record its content size.")?;
                    return Ok(None);
                }
                Err(e) => {
                    pwrs::verbose!(ps, "Cannot expand '{from}' in place: libzstd cannot add up the content sizes of its frames ({e}).")?;
                    return Ok(None);
                }
            };
            if let Some(budget) = budget {
                let needed = in_place_needs(size, total, share);
                if !fits(needed, budget) {
                    pwrs::verbose!(
                        ps,
                        "Cannot expand '{from}' in place: with the page tables of both files, it needs about {needed} bytes, more than -MaxMemory of {budget} bytes."
                    )?;
                    return Ok(None);
                }
            }
            pwrs::verbose!(ps, "Expanding '{from}' in place: a frame in it has a window of {window} bytes, which does not fit a chunk at a time.")?;
            match write_mapped_through_temporary(&dest, total, |output| expand_mapped(&input, output, dictionary, &stopping), |failure| failed(Operation::Expand, failure, ends)) {
                Ok(written) => Ok(Some(written)),
                Err(Unmapped::Failed(error)) => Err(error),
                Err(Unmapped::Refused(e)) => {
                    pwrs::verbose!(ps, "Cannot write '{to}' in place ({e}).")?;
                    Ok(None)
                }
            }
        };
        let written = match (streamed, refused.get()) {
            (Ok(written), _) => written,
            // Written in place, or the chunked refusal stands.
            (Err(error), Some(window)) => match in_place(window)? {
                Some(written) => written,
                None => return Err(error),
            },
            (Err(error), None) => return Err(error),
        };
        if let Some(written) = written {
            pwrs::verbose!(ps, "Expanded '{from}' ({size} bytes) to '{to}' ({written} bytes).")?;
            if self.pass_thru {
                pass_through(ps, &to)?;
            }
        }
        Ok(())
    }
}

impl Cmdlet for ExpandZstd {
    /// Loads the dictionary given, if any, as `CompressZstd::begin` does.
    fn begin(&mut self, ps: &Pipeline<'_>) -> PsResult<()> {
        match given_dictionary(ps, &self.dictionary, self.dictionary_path.as_deref()) {
            Ok(loaded) => {
                self.loaded = loaded;
                Ok(())
            }
            Err(error) => {
                self.refused = true;
                Err(error)
            }
        }
    }

    fn process(&mut self, ps: &Pipeline<'_>) -> PsResult<()> {
        if self.refused {
            return Ok(());
        }
        if self.file_mode() {
            return self.expand_files(ps);
        }
        // As in CompressZstd::process: a null object or an unset parameter.
        match self.input_object.take() {
            Some(record) if !record.is_null() => self.take_record(ps, &record),
            _ => Err(no_input()),
        }
    }

    fn end(&mut self, ps: &Pipeline<'_>) -> PsResult<()> {
        if self.refused || self.file_mode() {
            return Ok(());
        }
        self.finish_piped(ps)
    }
}

/// The largest dictionary New-ZstdDictionary trains when -MaxSize is
/// not given, in bytes: the zstd command line tool's default.
const DICTIONARY_SIZE_DEFAULT: i32 = 112_640;

/// Trains a dictionary of at most `max_size` bytes from `samples`, the
/// samples end to end, whose lengths `sizes` gives in order and which
/// add up to the length of `samples`. libzstd's refusal comes back with
/// its reason, and memory the allocator refuses is an `OutOfMemory`
/// error.
fn train(samples: &[u8], sizes: &[usize], max_size: usize) -> io::Result<Vec<u8>> {
    let mut dictionary = Vec::new();
    dictionary.try_reserve_exact(max_size).map_err(|e| io::Error::new(io::ErrorKind::OutOfMemory, e))?;
    zstd::zstd_safe::train_from_buffer(&mut dictionary, samples, sizes).map_err(|code| io::Error::other(zstd::zstd_safe::get_error_name(code)))?;
    Ok(dictionary)
}

/// The error record for training that failed on `count` samples of
/// `total` bytes in all.
fn training_failed(e: &io::Error, count: usize, total: usize, to: &str) -> PsError {
    let error = if e.kind() == io::ErrorKind::OutOfMemory {
        PsError::new(ErrorCategory::ResourceUnavailable, "ZstdOutOfMemory", format!("Cannot hold the dictionary for '{to}' in memory: {e}"))
    } else {
        PsError::new(
            ErrorCategory::InvalidOperation,
            "ZstdDictionaryTrainingFailed",
            format!(
                "Cannot train a dictionary from {count} samples of {total} bytes in all: libzstd reported '{e}'. Its trainer needs at least 7 samples and 8 bytes of them in all, and refuses 4294967295 bytes or more in all."
            ),
        )
    };
    targeted(error, to)
}

/// Samples read for training: each file's bytes, end to end, and each
/// one's length.
#[derive(Default)]
struct Samples {
    bytes: Vec<u8>,
    sizes: Vec<usize>,
}

impl Samples {
    /// Reads the file at `src`, which `parameter` named, as one more
    /// sample, a chunk at a time, asking `stopping` before each chunk;
    /// `Ok(false)` means it answered yes. A sample that is not read whole
    /// is left out.
    fn read(&mut self, src: &Path, parameter: &str, stopping: &dyn Fn() -> bool) -> PsResult<bool> {
        let (file, size) = open_input(src, parameter)?;
        let start = self.bytes.len();
        match self.append(file, size, src, stopping) {
            Ok(true) => {
                self.sizes.push(self.bytes.len() - start);
                Ok(true)
            }
            other => {
                self.bytes.truncate(start);
                other
            }
        }
    }

    /// Appends the `size` bytes `file` held when it was opened, after
    /// reserving room for all of them.
    fn append(&mut self, mut file: File, size: u64, src: &Path, stopping: &dyn Fn() -> bool) -> PsResult<bool> {
        let shown = src.display().to_string();
        let unheld = |e: &dyn std::fmt::Display| {
            targeted(PsError::new(ErrorCategory::ResourceUnavailable, "ZstdOutOfMemory", format!("Cannot hold the sample '{shown}' ({size} bytes) in memory: {e}")), &shown)
        };
        let wanted = usize::try_from(size).map_err(|e| unheld(&e))?;
        self.bytes.try_reserve(wanted).map_err(|e| unheld(&e))?;
        let mut taken = 0;
        while taken < wanted {
            if stopping() {
                return Ok(false);
            }
            let at = self.bytes.len();
            self.bytes.resize(at + (wanted - taken).min(CHUNK), 0);
            let read = file.read(&mut self.bytes[at..]);
            let n = match read {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                    self.bytes.truncate(at);
                    continue;
                }
                Err(e) => return Err(input_error(&e, &shown)),
            };
            self.bytes.truncate(at + n);
            if n == 0 {
                let ended = io::Error::new(io::ErrorKind::UnexpectedEof, format!("it ended after {taken} of the {size} bytes it held when it was opened"));
                return Err(input_error(&ended, &shown));
            }
            taken += n;
        }
        Ok(true)
    }
}

/// Trains a Zstandard dictionary from sample files.
///
/// Each file -Path or -LiteralPath names is one sample, and files piped
/// from Get-ChildItem or Get-Item bind to -LiteralPath by their PSPath.
/// The samples are read into memory
/// together, since libzstd's trainer takes them as one buffer, and the
/// dictionary it trains, at most -MaxSize bytes, is written to
/// -DestinationPath through a temporary file beside it that is renamed
/// onto it only when complete. A sample that cannot be read gets its own
/// error record, and training goes on with the rest. libzstd's trainer
/// needs at least 7 samples and 8 bytes of them in all, and refuses
/// 4294967295 bytes or more in all. Training runs in libzstd without a
/// pause, so a stop during it takes effect when it returns, and nothing is
/// written.
///
/// # Examples
/// New-ZstdDictionary -Path .\samples\*.json -DestinationPath .\json.dict
/// Get-ChildItem .\samples -Filter *.json | New-ZstdDictionary -DestinationPath .\json.dict
/// New-ZstdDictionary -Path .\logs\*.log -DestinationPath .\logs.dict -MaxSize 65536 -Force -PassThru
#[cmdlet(verb = "New", noun = "ZstdDictionary", default_parameter_set = "Path", supports_should_process, output = ["System.IO.FileInfo"])]
#[derive(Default)]
pub struct NewZstdDictionary {
    /// The sample files, each one sample. Wildcard characters select every file they match, and a relative path is resolved against the current PowerShell location.
    #[param(mandatory, position = 0, set = "Path", validate_not_null_or_empty)]
    pub path: Vec<String>,
    /// The sample files, each one sample, named exactly as written, wildcard characters included. A file piped from Get-ChildItem or Get-Item binds here by its PSPath.
    #[param(mandatory, set = "LiteralPath", literal_path, validate_not_null_or_empty)]
    pub literal_path: Vec<String>,
    /// The file to write the dictionary to. A folder is refused.
    #[param(mandatory, position = 1, validate_not_null_or_empty)]
    pub destination_path: String,
    /// The largest dictionary to train, in bytes, 256 to 2147483647. The default, 112640, is the zstd command line tool's.
    #[param(validate_range(256, 2147483647))]
    pub max_size: Option<i32>,
    /// Replaces a destination file that exists.
    #[param]
    pub force: bool,
    /// Writes the FileInfo of the dictionary written.
    #[param]
    pub pass_thru: bool,
    /// The dictionary's file, once -DestinationPath is accepted and the write confirmed; without it the command does nothing more.
    destination: Option<PathBuf>,
    /// The samples read so far.
    samples: Samples,
}

impl NewZstdDictionary {
    /// Reads every file -Path or -LiteralPath names as a sample, each on
    /// its own: one that fails gets its own error record, and the rest go
    /// on.
    fn read_samples(&mut self, ps: &Pipeline<'_>) -> PsResult<()> {
        let stopping = || ps.stopping();
        for source in named_files(ps, &self.path, &self.literal_path) {
            if ps.stopping() {
                return Ok(());
            }
            match source.and_then(|src| self.samples.read(&src.path, src.parameter, &stopping)) {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(error) => ps.write_error(&error)?,
            }
        }
        Ok(())
    }

    /// Trains the dictionary from the samples read and writes it to `dest`.
    fn train_into(&mut self, ps: &Pipeline<'_>, dest: &Path) -> PsResult<()> {
        let to = dest.display().to_string();
        let Samples { bytes, sizes } = std::mem::take(&mut self.samples);
        let max_size = self.max_size.unwrap_or(DICTIONARY_SIZE_DEFAULT);
        let capacity = usize::try_from(max_size).map_err(|e| {
            targeted(PsError::new(ErrorCategory::InvalidOperation, "ZstdDictionaryTrainingFailed", format!("-MaxSize {max_size} is not a size in bytes: {e}")), &to)
        })?;
        pwrs::verbose!(ps, "Training a dictionary of at most {max_size} bytes from {} samples of {} bytes in all.", sizes.len(), bytes.len())?;
        let trained = train(&bytes, &sizes, capacity).map_err(|e| training_failed(&e, sizes.len(), bytes.len(), &to))?;
        drop(bytes);
        if ps.stopping() {
            return Ok(());
        }
        let dictionary = Dictionary::new(trained);
        let written = write_through_temporary(dest, |sink| sink.write_all(&dictionary.bytes).map(|()| true), |e| output_error(&e, &to))?;
        if let Some(written) = written {
            match dictionary.id {
                Some(id) => pwrs::verbose!(ps, "Wrote dictionary {id}, {written} bytes, to '{to}'.")?,
                None => pwrs::verbose!(ps, "Wrote a dictionary of {written} bytes without an id to '{to}'.")?,
            }
            if self.pass_thru {
                pass_through(ps, &to)?;
            }
        }
        Ok(())
    }
}

impl Cmdlet for NewZstdDictionary {
    /// Resolves -DestinationPath and refuses it before any sample is read:
    /// a folder, or a file that exists without -Force.
    fn begin(&mut self, ps: &Pipeline<'_>) -> PsResult<()> {
        let dest = provider_path(ps, &self.destination_path)?;
        check_destination(&dest, self.force)?;
        let to = dest.display().to_string();
        if ps.should_process(&format!("Destination: {to}"), "New Dictionary")? {
            self.destination = Some(dest);
        }
        Ok(())
    }

    fn process(&mut self, ps: &Pipeline<'_>) -> PsResult<()> {
        if self.destination.is_none() {
            return Ok(());
        }
        self.read_samples(ps)
    }

    fn end(&mut self, ps: &Pipeline<'_>) -> PsResult<()> {
        let Some(dest) = self.destination.take() else {
            return Ok(());
        };
        if ps.stopping() {
            return Ok(());
        }
        self.train_into(ps, &dest)
    }
}

pwrs::export_module! {
    name: "Zstd",
    cmdlets: [CompressZstd, ExpandZstd, NewZstdDictionary],
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const LEVEL_3: Settings = Settings { level: 3, threads: 0 };

    fn never() -> bool {
        false
    }

    fn always() -> bool {
        true
    }

    /// `data` compressed in memory with `dictionary`, in one piece with
    /// its length recorded.
    fn packed_with(data: &[u8], settings: Settings, dictionary: Option<&Dictionary>) -> Result<Vec<u8>, Failure> {
        let mut sink = BufferSink::default();
        let finished = compress(&mut &data[..], &mut sink, Some(data.len() as u64), settings, dictionary, &never)?;
        assert!(finished, "compression stopped without being asked to");
        Ok(sink.bytes)
    }

    fn packed(data: &[u8], settings: Settings) -> Result<Vec<u8>, Failure> {
        packed_with(data, settings, None)
    }

    /// `data` expanded in memory with `dictionary`, read a chunk at a
    /// time as a file is.
    fn unpacked_with(data: &[u8], dictionary: Option<&Dictionary>) -> Result<Vec<u8>, Failure> {
        let mut sink = BufferSink::default();
        let finished = expand(&mut &data[..], &mut sink, dictionary, None, &never)?;
        assert!(finished, "expansion stopped without being asked to");
        Ok(sink.bytes)
    }

    fn unpacked(data: &[u8]) -> Result<Vec<u8>, Failure> {
        unpacked_with(data, None)
    }

    /// Log-shaped text that compresses well and spans several of
    /// libzstd's 128 KiB blocks.
    fn text(lines: usize) -> Vec<u8> {
        (0..lines)
            .map(|i| format!("01:{:02}:{:02}Z INFO worker-{} handled request {} in {} ms\n", i % 60, (i * 7) % 60, i % 8, (i * 7919) % 100_003, i % 997))
            .collect::<String>()
            .into_bytes()
    }

    /// Bytes from xorshift64, which libzstd cannot compress.
    fn noise(len: usize, seed: u64) -> Vec<u8> {
        let mut state = seed | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state.to_le_bytes()[0]
            })
            .collect()
    }

    /// A writer whose every write fails as an endpoint's error.
    struct RefusingSink;

    impl Write for RefusingSink {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(endpoint(io::Error::other("the sink refuses every write")))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Output for RefusingSink {
        fn held(&self) -> Option<u64> {
            None
        }

        fn prepare(&mut self, _: Option<Limit>, _: Option<u64>) -> io::Result<()> {
            Ok(())
        }
    }

    /// A reader whose every read fails as an endpoint's error.
    struct RefusingSource;

    impl Read for RefusingSource {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(endpoint(io::Error::other("the source refuses every read")))
        }
    }

    #[test]
    fn round_trips_empty_single_byte_text_and_noise() -> TestResult {
        let cases = [Vec::new(), vec![0x5A], text(40_000), noise(300_000, 7)];
        for data in &cases {
            for settings in [LEVEL_3, Settings { level: 1, threads: 0 }] {
                assert_eq!(&unpacked(&packed(data, settings)?)?, data, "{} bytes at level {}", data.len(), settings.level);
            }
        }
        let small = text(500);
        assert_eq!(unpacked(&packed(&small, Settings { level: 22, threads: 0 })?)?, small);
        Ok(())
    }

    #[test]
    fn round_trips_with_worker_threads() -> TestResult {
        let data = [text(40_000), noise(1 << 20, 3)].concat();
        assert_eq!(unpacked(&packed(&data, Settings { level: 1, threads: 4 })?)?, data);
        Ok(())
    }

    #[test]
    fn writes_one_frame_with_a_checksum_and_the_content_size() -> TestResult {
        let frame = packed(b"hello", LEVEL_3)?;
        assert_eq!(&frame[..4], &[0x28, 0xB5, 0x2F, 0xFD], "zstd magic number");
        let descriptor = frame[4];
        assert_ne!(descriptor & 0x04, 0, "Content_Checksum_flag");
        assert!((descriptor >> 6) != 0 || (descriptor & 0x20) != 0, "Frame_Content_Size present");
        Ok(())
    }

    #[test]
    fn expands_a_frame_built_by_hand_from_rfc_8878() -> TestResult {
        // Magic number; a single-segment header with a one-byte content
        // size of 5; one last raw block of 5 bytes; the bytes.
        let frame = [0x28, 0xB5, 0x2F, 0xFD, 0x20, 0x05, 0x29, 0x00, 0x00, b'h', b'e', b'l', b'l', b'o'];
        assert_eq!(unpacked(&frame)?, b"hello");
        Ok(())
    }

    #[test]
    fn expands_concatenated_frames_in_order() -> TestResult {
        let joined = [packed(b"hel", LEVEL_3)?, packed(b"lo", LEVEL_3)?].concat();
        assert_eq!(unpacked(&joined)?, b"hello");
        Ok(())
    }

    #[test]
    fn refuses_input_that_is_not_whole_frames() -> TestResult {
        let good = packed(&text(20_000), LEVEL_3)?;
        let mut flipped = good.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 0xFF;
        let cases = [
            ("empty", Vec::new()),
            ("not zstd", noise(1000, 9)),
            ("truncated", good[..good.len() - 10].to_vec()),
            ("checksum", flipped),
            ("trailing bytes", [good.clone(), noise(64, 5)].concat()),
        ];
        for (name, input) in &cases {
            let result = unpacked(input);
            assert!(matches!(result, Err(Failure::Codec(_))), "{name}: {result:?}");
        }
        Ok(())
    }

    #[test]
    fn a_stop_leaves_the_stream_unfinished() -> TestResult {
        let mut sink = BufferSink::default();
        assert!(!compress(&mut &b"abc"[..], &mut sink, Some(3), LEVEL_3, None, &always)?);
        let frame = packed(b"abc", LEVEL_3)?;
        assert!(!expand(&mut &frame[..], &mut BufferSink::default(), None, None, &always)?);
        Ok(())
    }

    #[test]
    fn tells_endpoint_errors_from_libzstd_errors() -> TestResult {
        let written = compress(&mut &b"abc"[..], RefusingSink, Some(3), LEVEL_3, None, &never);
        assert!(matches!(written, Err(Failure::Write(_))), "{written:?}");
        let frame = packed(b"abc", LEVEL_3)?;
        let expanded = expand(&mut &frame[..], &mut RefusingSink, None, None, &never);
        assert!(matches!(expanded, Err(Failure::Write(_))), "{expanded:?}");
        let read = expand(&mut RefusingSource, &mut BufferSink::default(), None, None, &never);
        assert!(matches!(read, Err(Failure::Read(_))), "{read:?}");
        Ok(())
    }

    #[test]
    fn an_input_shorter_than_its_pledged_size_is_a_read_failure() {
        let result = compress(&mut &b"abc"[..], BufferSink::default(), Some(10), LEVEL_3, None, &never);
        assert!(matches!(&result, Err(Failure::Read(e)) if e.kind() == io::ErrorKind::UnexpectedEof), "{result:?}");
    }

    /// `data` fed to a `Compression` in pieces of `size`, then finished.
    fn streamed(data: &[u8], size: usize, settings: Settings) -> Result<Vec<u8>, Failure> {
        let mut stream = Compression::new(settings, None, None)?;
        for piece in data.chunks(size.max(1)) {
            assert!(stream.feed(piece, &never)?, "compression stopped without being asked to");
        }
        let (frame, taken, _) = stream.finish()?;
        assert_eq!(taken, data.len() as u64);
        Ok(frame)
    }

    /// `data` fed to an `Expansion` with `dictionary` in pieces of `size`,
    /// then finished.
    fn unstreamed_with(data: &[u8], size: usize, dictionary: Option<&Dictionary>) -> Result<Vec<u8>, Failure> {
        let mut expansion = Expansion::new(BufferSink::default(), dictionary, None)?;
        for piece in data.chunks(size.max(1)) {
            assert!(expansion.feed(piece, &never)?, "expansion stopped without being asked to");
        }
        assert_eq!(expansion.taken, data.len() as u64);
        expansion.finish().map(|sink| sink.bytes)
    }

    fn unstreamed(data: &[u8], size: usize) -> Result<Vec<u8>, Failure> {
        unstreamed_with(data, size, None)
    }

    /// The content size a frame's header records, or why it could not
    /// be read.
    fn content_size(frame: &[u8]) -> Result<Option<u64>, String> {
        zstd::zstd_safe::get_frame_content_size(frame).map_err(|e| format!("{e:?}"))
    }

    /// The libzstd error `result` holds; anything else fails the test,
    /// naming `what` and what the result held.
    fn codec_error<T: std::fmt::Debug>(result: Result<T, Failure>, what: &str) -> io::Error {
        match result {
            Err(Failure::Codec(e)) => e,
            other => panic!("{what}: expected libzstd to refuse it, and got {other:?}"),
        }
    }

    #[test]
    fn a_streamed_frame_has_a_checksum_and_no_content_size() -> TestResult {
        let small = text(300);
        let data = [text(40_000), noise(300_000, 11)].concat();
        let cases = [(&small, 1, LEVEL_3), (&data, 7, LEVEL_3), (&data, 4096, LEVEL_3), (&data, CHUNK + 1, LEVEL_3), (&data, 65_536, Settings { level: 1, threads: 4 })];
        for (input, size, settings) in cases {
            let frame = streamed(input, size, settings)?;
            assert_eq!(&frame[..4], &[0x28, 0xB5, 0x2F, 0xFD], "zstd magic number");
            assert_ne!(frame[4] & 0x04, 0, "Content_Checksum_flag, pieces of {size}");
            assert_eq!(content_size(&frame)?, None, "pieces of {size}");
            assert_eq!(&unpacked(&frame)?, input, "pieces of {size}");
        }
        assert_eq!(content_size(&packed(&data, LEVEL_3)?)?, Some(data.len() as u64), "one piece records its length");
        Ok(())
    }

    #[test]
    fn expands_data_fed_in_pieces_of_any_size() -> TestResult {
        // Text whose frame expands to many of libzstd's output steps, then
        // noise, which libzstd stores raw.
        let big = [text(20_000), noise(300_000, 13)].concat();
        let frames = [packed(&big, LEVEL_3)?, packed(b"", LEVEL_3)?, packed(b"tail", LEVEL_3)?].concat();
        let expected = [big.as_slice(), b"tail"].concat();
        for size in [1, 3, 5, 4096, 131_072, CHUNK, frames.len()] {
            assert_eq!(unstreamed(&frames, size)?, expected, "pieces of {size}");
        }
        let by_hand = [0x28, 0xB5, 0x2F, 0xFD, 0x20, 0x05, 0x29, 0x00, 0x00, b'h', b'e', b'l', b'l', b'o'];
        for size in [1, 2, by_hand.len()] {
            assert_eq!(unstreamed(&by_hand, size)?, b"hello", "hand-built frame in pieces of {size}");
        }
        let without_size = streamed(&expected, 1000, LEVEL_3)?;
        assert_eq!(unstreamed(&without_size, 777)?, expected, "a frame without a content size");
        Ok(())
    }

    #[test]
    fn refuses_streamed_input_that_is_not_whole_frames() -> TestResult {
        let good = packed(&text(20_000), LEVEL_3)?;
        let mut flipped = good.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 0xFF;
        let cases = [
            ("not zstd", noise(1000, 9)),
            ("truncated", good[..good.len() - 10].to_vec()),
            ("header only", good[..6].to_vec()),
            ("checksum", flipped),
            ("trailing bytes", [good.clone(), noise(64, 5)].concat()),
        ];
        for (name, input) in &cases {
            for size in [1, 64, input.len()] {
                let refused = codec_error(unstreamed(input, size), &format!("{name} in pieces of {size}"));
                assert!(!refused.to_string().is_empty(), "{name}: libzstd's reason is empty");
            }
        }
        Ok(())
    }

    #[test]
    fn a_stop_leaves_a_stream_unfinished() -> TestResult {
        let mut stream = Compression::new(LEVEL_3, None, None)?;
        assert!(!stream.feed(b"abc", &always)?);
        let mut expansion = Expansion::new(BufferSink::default(), None, None)?;
        assert!(!expansion.feed(&packed(b"abc", LEVEL_3)?, &always)?);
        Ok(())
    }

    #[test]
    fn names_the_default_destination() {
        let named = |path: &str, operation| default_name(Path::new(path), operation).map(|name| name.to_string_lossy().into_owned());
        assert_eq!(named(r"C:\logs\app.log", Operation::Compress).as_deref(), Some("app.log.zst"));
        assert_eq!(named(r"C:\logs\app.log.zst", Operation::Expand).as_deref(), Some("app.log"));
        assert_eq!(named(r"C:\logs\APP.LOG.ZST", Operation::Expand).as_deref(), Some("APP.LOG"));
        assert_eq!(named(r"C:\logs\archive.tar.zst", Operation::Expand).as_deref(), Some("archive.tar"));
        for without in [r"C:\logs\app.log", r"C:\logs\.zst", r"C:\logs\app.zstd", r"C:\logs\app.log.gz"] {
            assert_eq!(named(without, Operation::Expand), None, "{without}");
        }
    }

    /// Records shaped like one small document each, alike in their keys
    /// and different in their values, as a dictionary's samples are meant
    /// to be.
    fn records(count: usize, seed: usize) -> Vec<Vec<u8>> {
        (0..count)
            .map(|i| {
                let n = i * 7919 + seed * 104_729;
                format!(
                    "{{\"id\":{n},\"user\":\"user-{}\",\"region\":\"{}\",\"status\":\"{}\",\"items\":[{},{},{}],\"note\":\"order {n} left warehouse {} for region {}\"}}\n",
                    n % 1000,
                    ["eu-west", "us-east", "ap-south"][n % 3],
                    ["open", "paid", "shipped", "closed"][n % 4],
                    n % 97,
                    n % 89,
                    n % 83,
                    n % 17,
                    n % 5
                )
                .repeat(4)
                .into_bytes()
            })
            .collect()
    }

    /// `samples` end to end, with each one's length.
    fn joined(samples: &[Vec<u8>]) -> (Vec<u8>, Vec<usize>) {
        (samples.concat(), samples.iter().map(Vec::len).collect())
    }

    /// A dictionary of at most `max_size` bytes trained from
    /// `records(count, seed)`.
    fn trained(count: usize, seed: usize, max_size: usize) -> io::Result<Dictionary> {
        let (bytes, sizes) = joined(&records(count, seed));
        Ok(Dictionary::new(train(&bytes, &sizes, max_size)?))
    }

    /// The ids a mismatch names; anything else fails the test, naming
    /// `what` and what the result held.
    fn mismatch<T: std::fmt::Debug>(result: Result<T, Failure>, what: &str) -> (Option<u32>, Given) {
        match result {
            Err(Failure::DictionaryMismatch { needed, given }) => (needed, given),
            other => panic!("{what}: expected a dictionary mismatch, and got {other:?}"),
        }
    }

    #[test]
    fn trains_a_dictionary_in_zstds_format() -> TestResult {
        let dictionary = trained(40, 1, 4096)?;
        assert!(dictionary.bytes.len() <= 4096, "{} bytes", dictionary.bytes.len());
        assert_eq!(&dictionary.bytes[..4], &[0x37, 0xA4, 0x30, 0xEC], "dictionary magic number");
        let id = dictionary.id.ok_or("a trained dictionary carries an id")?;
        assert_eq!(zstd::zstd_safe::get_dict_id_from_dict(&dictionary.bytes).map(|id| id.get()), Some(id), "libzstd reads the same id");
        Ok(())
    }

    #[test]
    fn training_needs_seven_samples_and_room_for_a_dictionary() -> TestResult {
        for count in [0, 1, 6] {
            let (bytes, sizes) = joined(&records(count, 2));
            match train(&bytes, &sizes, 4096) {
                Err(e) => assert_eq!(e.to_string(), "Src size is incorrect", "{count} samples"),
                Ok(dictionary) => panic!("{count} samples trained a dictionary of {} bytes", dictionary.len()),
            }
        }
        let (bytes, sizes) = joined(&records(7, 2));
        assert!(!train(&bytes, &sizes, 4096)?.is_empty(), "7 samples train a dictionary");
        match train(&bytes, &sizes, 255) {
            Err(e) => assert_eq!(e.to_string(), "Destination buffer is too small"),
            Ok(dictionary) => panic!("a 255-byte limit trained a dictionary of {} bytes", dictionary.len()),
        }
        Ok(())
    }

    #[test]
    fn a_frame_made_with_a_dictionary_names_it_and_expands_only_with_it() -> TestResult {
        let first = trained(40, 3, 8192)?;
        let other = trained(40, 4, 8192)?;
        let (Some(first_id), Some(other_id)) = (first.id, other.id) else {
            return Err("trained dictionaries carry ids".into());
        };
        assert_ne!(first_id, other_id);
        let data = records(12, 5).concat();
        let frame = packed_with(&data, LEVEL_3, Some(&first))?;
        assert_eq!(zstd::zstd_safe::get_dict_id_from_frame(&frame).map(|id| id.get()), Some(first_id), "the header names the dictionary");
        assert!(frame.len() < packed(&data, LEVEL_3)?.len(), "the dictionary makes the frame smaller");
        assert_eq!(unpacked_with(&frame, Some(&first))?, data);
        for size in [1, 5, frame.len()] {
            assert_eq!(unstreamed_with(&frame, size, Some(&first))?, data, "pieces of {size}");
        }
        let mut stream = Compression::new(LEVEL_3, Some(&first), None)?;
        for piece in data.chunks(100) {
            assert!(stream.feed(piece, &never)?);
        }
        let (streamed_frame, _, _) = stream.finish()?;
        assert_eq!(zstd::zstd_safe::get_dict_id_from_frame(&streamed_frame).map(|id| id.get()), Some(first_id), "a streamed frame names it too");
        assert_eq!(unstreamed_with(&streamed_frame, 3, Some(&first))?, data);

        let raw = Dictionary::new(text(100));
        for (given, told) in [(None, Given::Nothing), (Some(&other), Given::WithId(other_id)), (Some(&raw), Given::RawContent)] {
            for size in [1, 5, 17, frame.len()] {
                assert_eq!(mismatch(unstreamed_with(&frame, size, given), &format!("{told:?} in pieces of {size}")), (Some(first_id), told));
            }
            assert_eq!(mismatch(unpacked_with(&frame, given), &format!("{told:?} read as a file")), (Some(first_id), told));
        }

        let plain = packed(b"no dictionary here", LEVEL_3)?;
        assert_eq!(unpacked_with(&plain, Some(&first))?, b"no dictionary here", "a frame without a dictionary ignores one given");
        let both = [plain, frame].concat();
        assert_eq!(unstreamed_with(&both, 3, Some(&first))?, [b"no dictionary here".as_slice(), &data].concat());
        assert_eq!(mismatch(unstreamed_with(&both, 3, None), "the second frame"), (Some(first_id), Given::Nothing));
        Ok(())
    }

    #[test]
    fn names_the_dictionary_a_hand_built_frame_needs() {
        // Magic number; a single-segment header with a four-byte dictionary
        // id, 0x12345678, and a one-byte content size of 5; one last raw
        // block of 5 bytes; the bytes.
        let frame = [0x28, 0xB5, 0x2F, 0xFD, 0x23, 0x78, 0x56, 0x34, 0x12, 0x05, 0x29, 0x00, 0x00, b'h', b'e', b'l', b'l', b'o'];
        for size in 1..=frame.len() {
            assert_eq!(mismatch(unstreamed_with(&frame, size, None), &format!("pieces of {size}")), (Some(0x1234_5678), Given::Nothing));
        }
        assert_eq!(mismatch(unpacked(&frame), "read as a file"), (Some(0x1234_5678), Given::Nothing));
        let error = failed(Operation::Expand, Failure::DictionaryMismatch { needed: Some(0x1234_5678), given: Given::Nothing }, PIPED);
        assert_eq!((error.error_id.as_str(), error.category), ("ZstdDictionaryMismatch", ErrorCategory::InvalidArgument));
        assert_eq!(error.message, "Cannot expand the input: a frame in it needs dictionary 305419896, and no dictionary was given.");
    }

    #[test]
    fn checks_a_dictionary_in_zstds_format_before_it_is_used() -> TestResult {
        let good = trained(40, 6, 4096)?;
        let id = good.id.ok_or("a trained dictionary carries an id")?;
        let checked = Dictionary::checked(good.bytes.clone())?;
        assert_eq!(checked.id, Some(id));
        // The magic number and the id kept, and the entropy tables after
        // them overwritten.
        let mut broken = good.bytes.clone();
        for byte in &mut broken[8..200] {
            *byte = 0xFF;
        }
        match Dictionary::checked(broken) {
            Err(e) => assert_eq!(e.to_string(), "Dictionary is corrupted"),
            Ok(dictionary) => panic!("a broken dictionary was taken, with id {:?}", dictionary.id),
        }
        let raw = Dictionary::checked(text(100))?;
        assert_eq!(raw.id, None, "raw content is not checked");
        let error = dictionary_refused(&io::Error::other("Dictionary is corrupted"), "the bytes given to -Dictionary", None);
        assert_eq!((error.error_id.as_str(), error.category), ("ZstdDictionaryInvalid", ErrorCategory::InvalidData));
        assert_eq!(
            error.message,
            "Cannot use the bytes given to -Dictionary as a dictionary: it begins with zstd's dictionary magic number, and libzstd refused its tables (Dictionary is corrupted)."
        );
        Ok(())
    }

    #[test]
    fn uses_bytes_not_in_zstds_format_as_raw_content() -> TestResult {
        let content = text(3000);
        let raw = Dictionary::new(content.clone());
        assert_eq!(raw.id, None);
        let data = content[10_000..60_000].to_vec();
        let frame = packed_with(&data, LEVEL_3, Some(&raw))?;
        assert_eq!(zstd::zstd_safe::get_dict_id_from_frame(&frame), None, "raw content has no id for the header to name");
        assert!(frame.len() < packed(&data, LEVEL_3)?.len(), "the content makes the frame smaller");
        assert_eq!(unpacked_with(&frame, Some(&raw))?, data);
        assert_eq!(unstreamed_with(&frame, 7, Some(&raw))?, data);
        let refused = codec_error(unpacked(&frame), "a frame made with raw content, expanded without it");
        assert!(!refused.to_string().is_empty(), "libzstd's reason is empty");
        assert_eq!(Dictionary::new(vec![0x37, 0xA4, 0x30, 0xEC]).id, None, "the magic number alone is raw content");
        assert_eq!(Dictionary::new(vec![0x37, 0xA4, 0x30, 0xEC, 1, 0, 0, 0]).id, Some(1));
        Ok(())
    }

    #[test]
    fn defaults_to_level_3_on_the_calling_thread() -> TestResult {
        let settings = Settings::from_parameters(None, None)?;
        assert_eq!((settings.level, settings.threads), (3, 0));
        Ok(())
    }

    /// A frame built by hand from RFC 8878 whose header asks for a window
    /// of 2^`window_log` bytes and records no content size, and whose one
    /// last raw block holds `data`, at most 128 KiB.
    fn raw_frame(window_log: u32, data: &[u8]) -> Vec<u8> {
        let mut frame = vec![0x28, 0xB5, 0x2F, 0xFD, 0x00, ((window_log - 10) << 3) as u8];
        let block = (data.len() << 3) | 1;
        frame.extend_from_slice(&block.to_le_bytes()[..3]);
        frame.extend_from_slice(data);
        frame
    }

    /// `data` expanded as `unstreamed_with` does, with a budget.
    fn unstreamed_within(data: &[u8], size: usize, budget: Option<i64>) -> Result<Vec<u8>, Failure> {
        let mut expansion = Expansion::new(BufferSink::default(), None, budget)?;
        for piece in data.chunks(size.max(1)) {
            assert!(expansion.feed(piece, &never)?, "expansion stopped without being asked to");
        }
        expansion.finish().map(|sink| sink.bytes)
    }

    #[test]
    fn reads_the_window_a_frame_header_asks_for() -> TestResult {
        let frame = raw_frame(23, &text(100));
        assert_eq!(unpacked(&frame)?, text(100));
        assert_eq!(frame_start(&frame[..3]), FrameStart::Needs(5), "the magic number and the descriptor come first");
        assert_eq!(frame_start(&frame[..5]), FrameStart::Needs(6), "then the window descriptor the descriptor asks for");
        match frame_start(&frame[..6]) {
            FrameStart::Header { window, memory, content } => {
                assert_eq!(window, 1 << 23);
                assert!(memory > window, "the estimate of {memory} bytes covers the window");
                assert_eq!(content, None, "the header records no content size");
            }
            other => panic!("a whole header read as {other:?}"),
        }
        let recorded = packed(&text(100), LEVEL_3)?;
        match frame_start(&recorded[..FRAME_HEADER_MAX.min(recorded.len())]) {
            FrameStart::Header { content, .. } => assert_eq!(content, Some(text(100).len() as u64)),
            other => panic!("a header with its content size read as {other:?}"),
        }
        assert_eq!(frame_start(b"not zstd"), FrameStart::Unreadable);
        let skippable = [0x50, 0x2A, 0x4D, 0x18, 3, 0, 0, 0, 1, 2, 3];
        assert_eq!(frame_start(&skippable), FrameStart::Header { window: 0, memory: 0, content: Some(0) });
        Ok(())
    }

    #[test]
    fn refuses_a_window_over_the_limit_before_libzstd_takes_it() -> TestResult {
        let data = text(1000);
        let wide = raw_frame(28, &data);
        for size in [1, 3, 4096] {
            match unstreamed_within(&wide, size, None) {
                Err(Failure::WindowTooLarge { window, budget: None, besides, .. }) => assert_eq!((window, besides), (1 << 28, true), "fed {size} bytes at a time"),
                other => panic!("a 256 MiB window with no budget, fed {size} bytes at a time: {other:?}"),
            }
            match unstreamed_within(&wide, size, Some(64 << 20)) {
                Err(Failure::WindowTooLarge { window, memory, budget: Some(budget), .. }) => {
                    assert_eq!((window, budget), (1 << 28, 64 << 20));
                    assert!(memory > window, "the estimate of {memory} bytes covers the window");
                }
                other => panic!("a 256 MiB window within 64 MiB, fed {size} bytes at a time: {other:?}"),
            }
        }
        let narrow = raw_frame(20, &data);
        assert_eq!(unstreamed_within(&narrow, 3, None)?, data);
        assert_eq!(unstreamed_within(&narrow, 3, Some(8 << 20))?, data);
        assert!(
            matches!(unstreamed_within(&narrow, 3, Some(1 << 20)), Err(Failure::WindowTooLarge { window: 1_048_576, .. })),
            "a 1 MiB window takes more than 1 MiB to expand"
        );
        let two = [narrow.clone(), wide].concat();
        match unstreamed_within(&two, 7, Some(8 << 20)) {
            Err(Failure::WindowTooLarge { window, .. }) => assert_eq!(window, 1 << 28, "the second frame is checked as it begins"),
            other => panic!("two frames, the second too wide: {other:?}"),
        }
        let message = |failure: Failure| failed(Operation::Expand, failure, PIPED).message;
        assert_eq!(
            message(Failure::WindowTooLarge { window: 1 << 28, memory: 300, dictionary: 0, budget: None, besides: true }),
            "Cannot expand the input: a frame in it has a window of 268435456 bytes, more than the 134217728 bytes libzstd expands without -MaxMemory; -MaxMemory of 300 bytes or more lets it, with room besides for its result, which is held in memory twice."
        );
        assert_eq!(
            message(Failure::WindowTooLarge { window: 1 << 28, memory: 300, dictionary: 0, budget: Some(200), besides: false }),
            "Cannot expand the input: a frame in it has a window of 268435456 bytes, which takes 300 bytes to expand, more than -MaxMemory of 200 bytes."
        );
        assert_eq!(
            message(Failure::WindowTooLarge { window: 1 << 28, memory: 300, dictionary: 40, budget: Some(200), besides: false }),
            "Cannot expand the input: a frame in it has a window of 268435456 bytes, which takes 300 bytes to expand, 40 of them for the dictionary, more than -MaxMemory of 200 bytes."
        );
        assert_eq!(
            message(Failure::DictionaryTooLarge { needed: 300, dictionary: 40, budget: 200 }),
            "Cannot expand the input within -MaxMemory of 200 bytes: with its dictionary, expanding needs about 300 bytes before any frame, 40 of them for the dictionary."
        );
        Ok(())
    }

    #[test]
    fn compresses_a_stable_input_into_the_same_kind_of_frame() -> TestResult {
        let data = [text(40_000), noise(300_000, 5)].concat();
        let dictionary = trained(300, 0, 16_384)?;
        for settings in [LEVEL_3, Settings { level: 19, threads: 0 }, Settings { level: 3, threads: 4 }] {
            let mut sink = BufferSink::default();
            assert!(compress_stable(&data, &mut sink, settings, None, &never)?);
            let frame = sink.bytes;
            assert_eq!(content_size(&frame)?, Some(data.len() as u64), "level {} with {} workers", settings.level, settings.threads);
            assert_ne!(frame[4] & 0x04, 0, "Content_Checksum_flag");
            assert_eq!(unpacked(&frame)?, data);
            let mut sink = BufferSink::default();
            assert!(compress_stable(&data, &mut sink, settings, Some(&dictionary), &never)?);
            assert_eq!(unpacked_with(&sink.bytes, Some(&dictionary))?, data);
        }
        let mut sink = BufferSink::default();
        assert!(!compress_stable(&data, &mut sink, LEVEL_3, None, &always)?, "a stop before the first chunk");
        // libzstd holds back a stable input short of a block until it is
        // told to end, and a chunk boundary can fall anywhere.
        let whole = [text(20_000), noise(CHUNK, 3)].concat();
        for len in [0, 3, 100_000, 200_000, CHUNK, CHUNK + 1, whole.len()] {
            for settings in [LEVEL_3, Settings { level: 3, threads: 2 }] {
                let mut sink = BufferSink::default();
                assert!(compress_stable(&whole[..len], &mut sink, settings, None, &never)?);
                assert_eq!(content_size(&sink.bytes)?, Some(len as u64), "{len} bytes with {} workers", settings.threads);
                assert_eq!(unpacked(&sink.bytes)?, &whole[..len], "{len} bytes with {} workers", settings.threads);
            }
        }
        Ok(())
    }

    #[test]
    fn expands_frames_in_place_into_their_content_size() -> TestResult {
        let first = text(30_000);
        let second = noise(200_000, 9);
        let frames = [packed(&first, LEVEL_3)?, packed(&second, Settings { level: 19, threads: 0 })?].concat();
        let total = zstd_safe::find_decompressed_size(&frames).map_err(|e| e.to_string())?.ok_or("both frames record their content size")?;
        assert_eq!(total, (first.len() + second.len()) as u64);
        let mut output = vec![0u8; usize::try_from(total)?];
        assert!(expand_mapped(&frames, &mut output, None, &never)?);
        assert_eq!(output, [first.clone(), second].concat());
        assert!(!expand_mapped(&frames, &mut output, None, &always)?, "a stop before the first chunk");
        let cut = &frames[..frames.len() - 10];
        codec_error(expand_mapped(cut, &mut output, None, &never), "frames cut short");
        let mut short = vec![0u8; first.len()];
        codec_error(expand_mapped(&frames, &mut short, None, &never), "an output shorter than the content");
        let dictionary = trained(300, 0, 16_384)?;
        let named = packed_with(&first, LEVEL_3, Some(&dictionary))?;
        let mut room = vec![0u8; first.len()];
        let (needed, given) = mismatch(expand_mapped(&named, &mut room, None, &never), "a frame that needs a dictionary");
        assert_eq!((needed, given), (dictionary.id, Given::Nothing));
        assert!(expand_mapped(&named, &mut room, Some(&dictionary), &never)?);
        assert_eq!(room, first);
        Ok(())
    }

    #[test]
    fn fits_the_workers_to_the_budget() -> TestResult {
        let size = Some(1u64 << 30);
        let settings = Settings { level: 19, threads: 8 };
        let working = (1..=8).map(|workers| workers_estimate(19, size, workers, false).map(|share| share + OWN_SHARE)).collect::<io::Result<Vec<u64>>>()?;
        assert!(working.windows(2).all(|pair| pair[0] < pair[1]), "each worker adds to the estimate: {working:?}");
        let (fitted, estimate) = fit(settings, size, false, 0, None, None, PIPED)?;
        assert_eq!((fitted.threads, estimate), (8, None), "no budget, the workers asked for");
        let (fitted, estimate) = fit(settings, size, false, 0, None, Some(i64::try_from(working[7])?), PIPED)?;
        assert_eq!((fitted.threads, estimate), (8, Some(working[7])));
        let (fitted, estimate) = fit(settings, size, false, 0, None, Some(i64::try_from(working[2])?), PIPED)?;
        assert_eq!((fitted.threads, estimate), (3, Some(working[2])), "as many workers as fit");
        let refused = fit(settings, size, false, 0, None, Some(i64::try_from(working[0] - 1)?), PIPED).err().ok_or("expected a refusal")?;
        assert_eq!((refused.error_id.as_str(), refused.category), ("ZstdBudgetTooSmall", ErrorCategory::LimitsExceeded));
        assert_eq!(
            refused.message,
            format!("Cannot compress the input within -MaxMemory of {} bytes: at level 19 with one worker thread it needs about {} bytes.", working[0] - 1, working[0])
        );
        Ok(())
    }

    #[test]
    fn estimates_no_input_buffer_for_a_stable_input_on_the_calling_thread() -> TestResult {
        let size = Some(1u64 << 30);
        let buffered = calling_thread_estimate(19, size, false)?;
        let stable = calling_thread_estimate(19, size, true)?;
        let window = 1u64 << compression_parameters(19, size).windowLog;
        assert_eq!(buffered - stable, window + BLOCK_MAX, "libzstd's input buffer, a window and a block");
        let settings = Settings { level: 19, threads: 0 };
        let (fitted, estimate) = fit(settings, size, true, 0, None, Some(i64::try_from(stable + OWN_SHARE)?), PIPED)?;
        assert_eq!((fitted.threads, estimate), (0, Some(stable + OWN_SHARE)));
        let refused = fit(settings, size, false, 0, None, Some(i64::try_from(stable + OWN_SHARE)?), PIPED).err().ok_or("expected a refusal")?;
        assert_eq!(refused.error_id, "ZstdBudgetTooSmall");
        Ok(())
    }

    #[test]
    fn counts_a_page_table_for_every_2_mib_of_a_map_and_one_more() {
        assert_eq!(page_tables(0), 0);
        assert_eq!(page_tables(1), 2 * PAGE_TABLE);
        assert_eq!(page_tables(2 << 20), 2 * PAGE_TABLE);
        assert_eq!(page_tables((2 << 20) + 1), 3 * PAGE_TABLE);
        assert_eq!(page_tables(5 << 30), (2560 + 1) * PAGE_TABLE, "1/512 of 5 GiB, and one more");
    }

    #[test]
    fn maps_a_file_only_when_its_page_tables_fit_the_budget() -> TestResult {
        // At level 1 a gibibyte's page tables, 2 MiB, outweigh the input
        // buffer libzstd keeps for input it reads a chunk at a time.
        let size = 1u64 << 30;
        let settings = Settings { level: 1, threads: 0 };
        let mapped = calling_thread_estimate(1, Some(size), true)? + OWN_SHARE + page_tables(size);
        let chunked = calling_thread_estimate(1, Some(size), false)? + OWN_SHARE;
        assert!(chunked < mapped, "{chunked} read a chunk at a time against {mapped} mapped");
        let budget = i64::try_from(mapped - 1)?;
        match fitting(settings, Some(size), true, page_tables(size), 0, 0, budget)? {
            Fit::Beyond(working) => assert_eq!(working, mapped),
            Fit::Within(_, estimate) => return Err(format!("mapped, it fit at {estimate}").into()),
        }
        match fitting(settings, Some(size), false, 0, 0, 0, budget)? {
            Fit::Within(fitted, estimate) => assert_eq!((fitted.threads, estimate), (0, chunked)),
            Fit::Beyond(working) => return Err(format!("read a chunk at a time, it needs {working}").into()),
        }
        match fitting(settings, Some(size), true, page_tables(size), 0, 0, i64::try_from(mapped)?)? {
            Fit::Within(_, estimate) => assert_eq!(estimate, mapped),
            Fit::Beyond(working) => return Err(format!("mapped, it needs {working}").into()),
        }
        Ok(())
    }

    #[test]
    fn counts_the_page_tables_of_both_files_written_in_place() {
        let base = in_place_needs(1 << 20, 1 << 30, 0);
        assert_eq!(in_place_needs(1 << 20, (1 << 30) + (2 << 20), 0) - base, PAGE_TABLE, "4 KiB more for 2 MiB more output");
        assert_eq!(in_place_needs(1 << 20, 1 << 30, 5000) - base, 5000, "the dictionary's share");
        assert!(base > page_tables(1 << 20) + page_tables(1 << 30) + OWN_SHARE, "libzstd's context and a block besides");
    }

    #[test]
    fn counts_the_long_distance_tables_libzstd_turns_on_at_level_22() -> TestResult {
        use zstd_sys::ZSTD_cParameter::ZSTD_c_compressionLevel;
        assert!(long_distance(&compression_parameters(22, None)));
        assert!(long_distance(&compression_parameters(22, Some((64 << 20) + 1))));
        assert!(!long_distance(&compression_parameters(22, Some(64 << 20))), "a window of 2^26 bytes is enough for 64 MiB");
        assert!(!long_distance(&compression_parameters(19, None)));
        // SAFETY: the parameters are created here, handed only to libzstd,
        // and freed before the block ends.
        let unresolved = unsafe {
            let params = zstd_sys::ZSTD_createCCtxParams();
            assert!(!params.is_null());
            assert!(!is_error(zstd_sys::ZSTD_CCtxParams_setParameter(params, ZSTD_c_compressionLevel, 22)));
            let estimate = zstd_sys::ZSTD_estimateCStreamSize_usingCCtxParams(params);
            zstd_sys::ZSTD_freeCCtxParams(params);
            libzstd_size(estimate)?
        };
        let counted = calling_thread_estimate(22, None, false)?;
        assert!(counted >= unresolved + (64 << 20), "the 64 MiB table of hashes: {counted} against {unresolved}");
        // SAFETY: computes from its argument alone.
        let context = libzstd_size(unsafe { zstd_sys::ZSTD_estimateCCtxSize_usingCParams(compression_parameters(22, None)) })?;
        let job = 1u64 << 29;
        let bound = libzstd_size(zstd_safe::compress_bound(1 << 29))?;
        // A second worker adds a job to the round buffer, a job running,
        // with its context and 192 MiB of sequences, and four more jobs to
        // libzstd's table of jobs, each with an output buffer.
        let added = workers_estimate(22, None, 2, false)? - workers_estimate(22, None, 1, false)?;
        assert_eq!(added, job + context + (192 << 20) + 4 * bound + WORKER_SHARE);
        Ok(())
    }

    #[test]
    fn estimates_small_and_empty_inputs_on_the_calling_thread() -> TestResult {
        let small = Some(ONE_THREAD_UP_TO);
        assert_eq!(workers_estimate(3, small, 8, true)?, calling_thread_estimate(3, small, true)?, "libzstd ignores the workers");
        let empty = calling_thread_estimate(22, Some(0), true)?;
        assert!(empty < 1 << 20, "an empty input at level 22 is estimated at {empty} bytes");
        // The round buffer is allocated whole for any input past that: 24
        // workers and 3 more jobs of 4 MiB for a 1 MiB input at level 3.
        let round = workers_estimate(3, Some(1 << 20), 24, false)?;
        assert!(round >= 27 * (4 << 20), "{round}");
        Ok(())
    }

    #[test]
    fn fits_a_result_held_in_memory_twice() -> TestResult {
        let len = 1u64 << 20;
        let bound = libzstd_size(zstd_safe::compress_bound(1 << 20))?;
        let working = calling_thread_estimate(3, Some(len), true)? + OWN_SHARE;
        let budget = i64::try_from(working + 2 * bound)?;
        let (_, estimate) = fit(LEVEL_3, Some(len), true, bound, None, Some(budget), PIPED)?;
        assert_eq!(estimate, Some(working));
        let refused = fit(LEVEL_3, Some(len), true, bound, None, Some(budget - 1), PIPED).err().ok_or("expected a refusal")?;
        assert_eq!(
            refused.message,
            format!(
                "Cannot compress the input within -MaxMemory of {} bytes: at level 3 on the calling thread it needs about {budget} bytes, {} of them for its result of up to {bound} bytes, which is held in memory twice, as it is built and as the byte[] written.",
                budget - 1,
                2 * bound
            )
        );
        Ok(())
    }

    #[test]
    fn counts_a_dictionary_against_the_budget() -> TestResult {
        let dictionary = trained(300, 0, 16_384)?;
        let len = dictionary.bytes.len() as u64;
        assert_eq!((Dictionary::compression_share(None, 3), Dictionary::expansion_share(None)), (0, 0));
        let compressing = Dictionary::compression_share(Some(&dictionary), 3);
        let expanding = Dictionary::expansion_share(Some(&dictionary));
        assert!(compressing > 2 * len, "the command's copy, libzstd's, and its tables: {compressing}");
        assert!(expanding > 2 * len, "the command's copy and libzstd's: {expanding}");
        let size = Some(1u64 << 20);
        let without = calling_thread_estimate(3, size, true)? + OWN_SHARE;
        let (_, estimate) = fit(LEVEL_3, size, true, 0, Some(&dictionary), Some(i64::MAX), PIPED)?;
        assert_eq!(estimate, Some(without + compressing));
        let budget = i64::try_from(without + compressing - 1)?;
        let refused = fit(LEVEL_3, size, true, 0, Some(&dictionary), Some(budget), PIPED).err().ok_or("expected a refusal")?;
        assert_eq!(
            refused.message,
            format!("Cannot compress the input within -MaxMemory of {budget} bytes: at level 3 on the calling thread it needs about {} bytes, {compressing} of them for the dictionary.", without + compressing)
        );
        // SAFETY: computes from nothing outside it.
        let context = unsafe { zstd_sys::ZSTD_estimateDCtxSize() } as u64;
        let before = expanding + context + OWN_SHARE;
        match Expansion::new(BufferSink::default(), Some(&dictionary), Some(i64::try_from(before - 1)?)) {
            Err(Failure::DictionaryTooLarge { needed, dictionary: share, budget }) => assert_eq!((needed, share, budget), (before, expanding, i64::try_from(before - 1)?)),
            Err(other) => panic!("a dictionary over the budget: {other:?}"),
            Ok(_) => panic!("a dictionary over the budget was taken"),
        }
        let frame = packed_with(&records(12, 5).concat(), LEVEL_3, Some(&dictionary))?;
        let mut expansion = Expansion::new(BufferSink::default(), Some(&dictionary), Some(i64::try_from(before)?))?;
        match expansion.feed(&frame, &never) {
            Err(Failure::WindowTooLarge { dictionary: share, budget: Some(_), .. }) => assert_eq!(share, expanding, "the frame is counted with the dictionary"),
            other => panic!("a frame over what the dictionary leaves: {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn holds_a_result_within_its_limit() -> TestResult {
        let limit = Limit::left(10_000, 8_000);
        assert_eq!(limit, Limit { most: 1_000, budget: 10_000 });
        let mut sink = BufferSink::new(Some(limit));
        sink.write_all(&[1; 600])?;
        assert!(sink.bytes.capacity() <= 1_000, "it grows no further than the limit: {}", sink.bytes.capacity());
        sink.write_all(&[2; 400])?;
        let refused = sink.write_all(&[3]).err().ok_or("expected a refusal past the limit")?;
        assert_eq!(outgrown(&refused), Some(limit));
        assert!(matches!(write_failure(refused), Failure::ResultTooLarge { result: None, limit: outgrew } if outgrew == limit));
        assert_eq!(sink.bytes.len(), 1_000);
        let mut exact = BufferSink::default();
        exact.prepare(None, Some(12_345))?;
        assert!((12_345..24_690).contains(&exact.bytes.capacity()), "room for a known size: {}", exact.bytes.capacity());
        let error = failed(Operation::Expand, Failure::ResultTooLarge { result: Some(5_000), limit: Limit { most: 4_000, budget: 1 << 20 } }, PIPED);
        assert_eq!((error.error_id.as_str(), error.category), ("ZstdBudgetTooSmall", ErrorCategory::LimitsExceeded));
        assert_eq!(
            error.message,
            "Cannot expand the input within -MaxMemory of 1048576 bytes: its result of 5000 bytes, as its frames record, would be held in memory twice, as it is built and as the byte[] written, and 4000 bytes of result is the most that fits."
        );
        Ok(())
    }

    #[test]
    fn counts_a_result_held_in_memory_against_the_budget() -> TestResult {
        let data = text(20_000);
        let len = data.len() as u64;
        // What the budget must hold for `frame`: the work, and `result`
        // bytes of result twice.
        let room = |frame: &[u8], result: u64| -> Result<i64, Box<dyn std::error::Error>> {
            match frame_start(&frame[..FRAME_HEADER_MAX.min(frame.len())]) {
                FrameStart::Header { memory, .. } => Ok(i64::try_from(memory + OWN_SHARE + 2 * result)?),
                other => Err(format!("a header read as {other:?}").into()),
            }
        };
        let recorded = packed(&data, LEVEL_3)?;
        assert_eq!(unstreamed_within(&recorded, 4096, Some(room(&recorded, len)?))?, data);
        match unstreamed_within(&recorded, 4096, Some(room(&recorded, len)? - 2)) {
            Err(Failure::ResultTooLarge { result: Some(result), limit }) => assert_eq!((result, limit.most), (len, len - 1), "refused before libzstd sees the frame"),
            other => panic!("a result one byte over the budget: {other:?}"),
        }
        let unrecorded = streamed(&data, 1000, LEVEL_3)?;
        assert_eq!(unstreamed_within(&unrecorded, 4096, Some(room(&unrecorded, len)?))?, data);
        match unstreamed_within(&unrecorded, 4096, Some(room(&unrecorded, len - 1000)?)) {
            Err(Failure::ResultTooLarge { result: None, limit }) => assert_eq!(limit.most, len - 1000, "stopped as it outgrew the limit"),
            other => panic!("a result outgrowing the budget: {other:?}"),
        }
        let mut stream = Compression::new(LEVEL_3, None, Some(Limit { most: 100, budget: 1 << 20 }))?;
        assert!(stream.feed(&noise(10_000, 3), &never)?);
        match stream.finish() {
            Err(Failure::ResultTooLarge { result: None, limit }) => assert_eq!(limit.most, 100),
            other => panic!("a streamed frame past its limit: {:?}", other.map(|(frame, taken, _)| (frame.len(), taken))),
        }
        Ok(())
    }

    /// Space a test expands into.
    impl Space for Vec<u8> {
        fn make(len: u64) -> Result<Vec<u8>, String> {
            usize::try_from(len).map(|len| vec![0; len]).map_err(|e| e.to_string())
        }

        fn fill<T>(&mut self, fill: impl FnOnce(&mut [u8]) -> T) -> PsResult<T> {
            Ok(fill(self))
        }
    }

    /// Space that cannot be made, as when the host has no room for a
    /// byte[] that long.
    struct Unmakeable;

    impl Space for Unmakeable {
        fn make(len: u64) -> Result<Unmakeable, String> {
            Err(format!("no room for {len} bytes"))
        }

        fn fill<T>(&mut self, fill: impl FnOnce(&mut [u8]) -> T) -> PsResult<T> {
            Ok(fill(&mut []))
        }
    }

    /// How `expand_whole` took a record, in words, for a test's failure
    /// message.
    fn told<S>(whole: &PsResult<Result<Whole<S>, Failure>>) -> String {
        match whole {
            Ok(Ok(Whole::Expanded(_, total))) => format!("expanded whole, to {total} bytes"),
            Ok(Ok(Whole::Streamed(reason))) => format!("left to be streamed, {reason:?}"),
            Ok(Ok(Whole::Stopped)) => "stopped".to_string(),
            Ok(Err(failure)) => format!("refused, {failure:?}"),
            Err(e) => format!("failed, {}", e.message),
        }
    }

    /// What `expand_whole` expanded `bytes` into; anything else fails the
    /// test, naming `what`.
    fn taken_whole(bytes: &[u8], dictionary: Option<&Dictionary>, budget: Option<i64>, what: &str) -> Vec<u8> {
        match expand_whole::<Vec<u8>>(bytes, dictionary, budget, &never) {
            Ok(Ok(Whole::Expanded(space, total))) => {
                assert_eq!(space.len() as u64, total, "{what}: the space is as long as the frames record");
                space
            }
            other => panic!("{what}: expected it expanded whole, and it was {}", told(&other)),
        }
    }

    /// Why `expand_whole` left `bytes` to be fed to libzstd a chunk at a
    /// time; anything else fails the test, naming `what`.
    fn left_to_stream<S: Space>(bytes: &[u8], what: &str) -> Option<String> {
        match expand_whole::<S>(bytes, None, None, &never) {
            Ok(Ok(Whole::Streamed(reason))) => reason,
            other => panic!("{what}: expected it left to be streamed, and it was {}", told(&other)),
        }
    }

    /// `raw_frame`, with a header that records `recorded` bytes of content,
    /// in four bytes.
    fn raw_frame_recording(window_log: u32, recorded: u32, data: &[u8]) -> Vec<u8> {
        let mut frame = vec![0x28, 0xB5, 0x2F, 0xFD, 0x80, ((window_log - 10) << 3) as u8];
        frame.extend_from_slice(&recorded.to_le_bytes());
        let block = (data.len() << 3) | 1;
        frame.extend_from_slice(&block.to_le_bytes()[..3]);
        frame.extend_from_slice(data);
        frame
    }

    #[test]
    fn expands_a_record_whose_frames_record_their_sizes_straight_into_space_of_that_size() -> TestResult {
        let first = text(30_000);
        let second = noise(200_000, 9);
        let frames = [packed(&first, LEVEL_3)?, packed(b"", LEVEL_3)?, packed(&second, Settings { level: 19, threads: 0 })?].concat();
        assert_eq!(taken_whole(&frames, None, None, "three frames"), [first.clone(), second].concat());
        assert_eq!(taken_whole(&packed(b"", LEVEL_3)?, None, None, "an empty frame"), b"");
        let skippable = [0x50, 0x2A, 0x4D, 0x18, 3, 0, 0, 0, 1, 2, 3];
        assert_eq!(taken_whole(&[skippable.as_slice(), &packed(b"hello", LEVEL_3)?].concat(), None, None, "a skippable frame, then a frame"), b"hello");
        let by_hand = [0x28, 0xB5, 0x2F, 0xFD, 0x20, 0x05, 0x29, 0x00, 0x00, b'h', b'e', b'l', b'l', b'o'];
        assert_eq!(taken_whole(&by_hand, None, None, "a frame built by hand"), b"hello");
        let dictionary = trained(300, 0, 16_384)?;
        let named = packed_with(&first, LEVEL_3, Some(&dictionary))?;
        assert_eq!(taken_whole(&named, Some(&dictionary), None, "a frame made with a dictionary"), first);
        match expand_whole::<Vec<u8>>(&named, None, None, &never) {
            Ok(Err(Failure::DictionaryMismatch { needed, given: Given::Nothing })) => assert_eq!(needed, dictionary.id),
            other => panic!("a frame that needs a dictionary, without it: {}", told(&other)),
        }
        match expand_whole::<Vec<u8>>(&frames, None, None, &always) {
            Ok(Ok(Whole::Stopped)) => {}
            other => panic!("a stop before the first chunk: {}", told(&other)),
        }
        Ok(())
    }

    #[test]
    fn leaves_other_bytes_to_be_fed_a_chunk_at_a_time() -> TestResult {
        let data = text(20_000);
        let sized = packed(&data, LEVEL_3)?;
        let unrecorded = streamed(&data, 1000, LEVEL_3)?;
        assert_eq!(left_to_stream::<Vec<u8>>(b"", "no bytes"), None);
        assert_eq!(left_to_stream::<Vec<u8>>(&unrecorded, "a frame that records no size"), None);
        assert_eq!(left_to_stream::<Vec<u8>>(&[sized.clone(), unrecorded].concat(), "a frame that records its size, then one that does not"), None);
        let cases = [
            ("cut short", sized[..sized.len() - 10].to_vec()),
            ("its first six bytes", sized[..6].to_vec()),
            ("not zstd", noise(1000, 9)),
            ("three bytes after a frame", [sized.clone(), noise(3, 5)].concat()),
        ];
        for (what, bytes) in &cases {
            assert_eq!(
                left_to_stream::<Vec<u8>>(bytes, what).as_deref(),
                Some("libzstd cannot add up the content sizes its frames record (Could not get content size)"),
                "{what}"
            );
        }
        Ok(())
    }

    #[test]
    fn expands_a_chunk_at_a_time_when_the_space_cannot_be_made() -> TestResult {
        let data = [text(20_000), noise(300_000, 4)].concat();
        let sized = packed(&data, LEVEL_3)?;
        assert_eq!(left_to_stream::<Unmakeable>(&sized, "space that cannot be made"), Some(format!("a byte[] of {0} bytes could not be made (no room for {0} bytes)", data.len())));
        // The record then goes where the records after it go.
        assert_eq!(unstreamed(&sized, sized.len())?, data);
        Ok(())
    }

    #[test]
    fn takes_a_frame_of_any_window_whole_and_counts_its_result_once() -> TestResult {
        let data = text(1000);
        let len = data.len() as u64;
        let wide = raw_frame_recording(28, u32::try_from(data.len())?, &data);
        assert_eq!(content_size(&wide)?, Some(len));
        assert_eq!(taken_whole(&wide, None, None, "a window of 256 MiB, no budget"), data);
        assert!(matches!(unstreamed_within(&wide, 4096, None), Err(Failure::WindowTooLarge { window: 268_435_456, .. })), "a chunk at a time, the window is refused");
        // SAFETY: computes from nothing outside it.
        let context = unsafe { zstd_sys::ZSTD_estimateDCtxSize() } as u64;
        let needed = context + BLOCK_MAX + OWN_SHARE + len;
        assert_eq!(taken_whole(&wide, None, Some(i64::try_from(needed)?), "a budget that holds the result once"), data);
        match expand_whole::<Vec<u8>>(&wide, None, Some(i64::try_from(needed - 1)?), &never) {
            Ok(Err(Failure::HeldOnceTooLarge { needed: counted, result, dictionary: 0, budget })) => assert_eq!((counted, result, budget), (needed, len, i64::try_from(needed - 1)?)),
            other => panic!("a budget a byte short: {}", told(&other)),
        }
        let dictionary = trained(300, 0, 16_384)?;
        let share = Dictionary::expansion_share(Some(&dictionary));
        match expand_whole::<Vec<u8>>(&wide, Some(&dictionary), Some(i64::try_from(needed + share - 1)?), &never) {
            Ok(Err(Failure::HeldOnceTooLarge { needed: counted, dictionary: counted_share, .. })) => assert_eq!((counted, counted_share), (needed + share, share)),
            other => panic!("with a dictionary, a budget a byte short: {}", told(&other)),
        }
        let error = failed(Operation::Expand, Failure::HeldOnceTooLarge { needed: 5000, result: 700, dictionary: 0, budget: 4999 }, PIPED);
        assert_eq!((error.error_id.as_str(), error.category), ("ZstdBudgetTooSmall", ErrorCategory::LimitsExceeded));
        assert_eq!(
            error.message,
            "Cannot expand the input within -MaxMemory of 4999 bytes: it needs about 5000 bytes, 700 of them for its result, as its frames record, which is held in memory once, as the byte[] it is expanded into."
        );
        assert_eq!(
            failed(Operation::Expand, Failure::HeldOnceTooLarge { needed: 5000, result: 700, dictionary: 300, budget: 4999 }, PIPED).message,
            "Cannot expand the input within -MaxMemory of 4999 bytes: it needs about 5000 bytes, 300 of them for the dictionary and 700 for its result, as its frames record, which is held in memory once, as the byte[] it is expanded into."
        );
        Ok(())
    }

    #[test]
    fn goes_on_after_a_first_record_taken_whole() -> TestResult {
        let first = text(20_000);
        let second = noise(300_000, 6);
        let sized = packed(&first, LEVEL_3)?;
        let rest = streamed(&second, 4096, LEVEL_3)?;
        let (taken, len) = (sized.len() as u64, first.len() as u64);
        let mut space = taken_whole(&sized, None, None, "the first record");
        let mut expansion = go_on_from(&mut space, taken, len, None, None)?;
        assert!(expansion.feed(&rest, &never)?);
        assert_eq!(expansion.taken, taken + rest.len() as u64, "the bytes of both records");
        assert_eq!(expansion.finish()?.bytes, [first.clone(), second].concat());
        let mut expansion = go_on_from(&mut space, taken, len, None, None)?;
        assert!(expansion.feed(b"", &never)?);
        assert_eq!(expansion.finish()?.bytes, first, "a second record of no bytes leaves the data whole frames");
        // From here the result is held twice, as it is built and as the
        // byte[] written.
        let budget = i64::try_from(OWN_SHARE + 2 * len)?;
        assert!(go_on_from(&mut space, taken, len, None, Some(budget)).is_ok());
        match go_on_from(&mut space, taken, len, None, Some(budget - 2)) {
            Err(Failure::ResultTooLarge { result: Some(result), limit }) => assert_eq!((result, limit.most), (len, len - 1)),
            Err(other) => panic!("a result held twice over the budget: {other:?}"),
            Ok(_) => panic!("a result held twice over the budget was taken"),
        }
        Ok(())
    }

    /// `frame`, which `packed` wrote with a four-byte content size, with
    /// that size replaced by `recorded`.
    fn recording(frame: &[u8], recorded: u32) -> Vec<u8> {
        let mut changed = frame.to_vec();
        let descriptor = frame[4];
        assert_eq!(descriptor >> 6, 2, "a four-byte content size");
        let at = if descriptor & 0x20 != 0 { 5 } else { 6 };
        changed[at..at + 4].copy_from_slice(&recorded.to_le_bytes());
        changed
    }

    /// A frame built by hand from RFC 8878: a single-segment header whose
    /// one-byte content size is `recorded`, and one last raw block of the
    /// five bytes of hello.
    fn hello_recording(recorded: u8) -> Vec<u8> {
        vec![0x28, 0xB5, 0x2F, 0xFD, 0x20, recorded, 0x29, 0x00, 0x00, b'h', b'e', b'l', b'l', b'o']
    }

    /// Why `expand_whole` refused `bytes`, in libzstd's words or the
    /// command's; anything else fails the test, naming `what`.
    fn refused_whole(bytes: &[u8], what: &str) -> String {
        match expand_whole::<Vec<u8>>(bytes, None, None, &never) {
            Ok(Err(Failure::Codec(e))) => e.to_string(),
            other => panic!("{what}: expected it refused, and it was {}", told(&other)),
        }
    }

    #[test]
    fn refuses_a_frame_that_records_more_than_it_holds_as_a_size_mismatch() -> TestResult {
        let world = packed(b"world", LEVEL_3)?;
        let text_data = text(20_000);
        let text_len = u32::try_from(text_data.len())?;
        let text_frame = recording(&packed(&text_data, LEVEL_3)?, text_len + 1);
        // 2 MiB ends on a block boundary, so the frame ends with an empty
        // block, at which libzstd does not check the size.
        let noise_data = noise(2 << 20, 17);
        let noise_len = u32::try_from(noise_data.len())?;
        let noise_frame = recording(&packed(&noise_data, LEVEL_3)?, noise_len + 1);
        let cases = [
            ("a single-segment frame", hello_recording(10), 10, 5),
            ("a single-segment frame, then a frame", [hello_recording(10), world.clone()].concat(), 10, 5),
            ("a four-byte size", raw_frame_recording(20, 10, b"hello"), 10, 5),
            ("text", text_frame.clone(), text_len + 1, text_len),
            ("a frame that ends with an empty block", noise_frame.clone(), noise_len + 1, noise_len),
            ("that frame, then a frame", [noise_frame.clone(), world].concat(), noise_len + 1, noise_len),
        ];
        for (what, bytes, recorded, held) in &cases {
            assert_eq!(refused_whole(bytes, what), format!("a frame records a content size of {recorded} bytes and holds {held}"), "{what}");
        }
        // A chunk at a time, libzstd refuses the size itself, except at a
        // frame that ends with an empty block.
        assert_eq!(codec_error(unstreamed(&text_frame, CHUNK), "text a chunk at a time").to_string(), "Data corruption detected");
        assert_eq!(
            codec_error(unstreamed(&noise_frame, CHUNK), "an empty last block a chunk at a time").to_string(),
            format!("a frame records a content size of {} bytes and holds {noise_len}", noise_len + 1)
        );
        let error = failed(Operation::Expand, size_mismatch(10, 5), PIPED);
        assert_eq!((error.error_id.as_str(), error.category), ("ZstdInvalidData", ErrorCategory::InvalidData));
        assert_eq!(error.message, "Cannot expand the input: it is not valid zstd data (a frame records a content size of 10 bytes and holds 5).");
        Ok(())
    }

    #[test]
    fn refuses_a_frame_that_holds_more_than_it_records_as_the_destination_too_small() -> TestResult {
        let world = packed(b"world", LEVEL_3)?;
        let text_data = text(20_000);
        let text_len = u32::try_from(text_data.len())?;
        let text_frame = packed(&text_data, LEVEL_3)?;
        let noise_data = noise(2 << 20, 17);
        let noise_len = u32::try_from(noise_data.len())?;
        let noise_frame = packed(&noise_data, LEVEL_3)?;
        let cases = [
            ("a single-segment frame", hello_recording(3)),
            ("a single-segment frame, then a frame", [hello_recording(3), world.clone()].concat()),
            ("a four-byte size", raw_frame_recording(20, 3, b"hello")),
            ("text a byte short", recording(&text_frame, text_len - 1)),
            ("text half short", recording(&text_frame, text_len / 2)),
            ("a frame that ends with an empty block", recording(&noise_frame, noise_len - 1)),
            ("that frame, then a frame", [recording(&noise_frame, noise_len - 1), world].concat()),
            ("that frame, then one a byte long", [recording(&noise_frame, noise_len - 1), recording(&noise_frame, noise_len + 1)].concat()),
        ];
        for (what, bytes) in &cases {
            assert_eq!(refused_whole(bytes, what), "Destination buffer is too small", "{what}");
        }
        Ok(())
    }
}
