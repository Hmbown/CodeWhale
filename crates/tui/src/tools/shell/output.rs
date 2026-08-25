use std::collections::VecDeque;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use encoding_rs::{CoderResult, Decoder, Encoding};

const BOUNDED_OUTPUT_MAX_LINES: usize = 2_000;
const BOUNDED_OUTPUT_MAX_BYTES: usize = 50 * 1024;
const BOUNDED_OUTPUT_RETAIN_BYTES: usize = BOUNDED_OUTPUT_MAX_BYTES + 4;

/// Stateful UTF-8 decoder with a best-effort Windows ANSI-code-page fallback.
/// Decoder state keeps characters split across pipe reads intact. The ACP is
/// only a heuristic for native Windows programs; it cannot identify OEM code
/// pages or an arbitrary encoding selected independently by a child process.
pub(super) struct ShellOutputDecoder {
    pending_utf8: Vec<u8>,
    legacy_encoding: Option<&'static Encoding>,
    legacy_decoder: Option<Decoder>,
    finished: bool,
}

impl ShellOutputDecoder {
    fn new(legacy_encoding: Option<&'static Encoding>) -> Self {
        Self {
            pending_utf8: Vec::new(),
            legacy_encoding,
            legacy_decoder: None,
            finished: false,
        }
    }

    pub(super) fn decode(&mut self, bytes: &[u8], last: bool) -> String {
        if self.finished {
            return String::new();
        }
        if let Some(decoder) = self.legacy_decoder.as_mut() {
            let decoded = decode_chunk(decoder, bytes, last);
            self.finished = last;
            return decoded;
        }

        self.pending_utf8.extend_from_slice(bytes);
        match std::str::from_utf8(&self.pending_utf8) {
            Ok(valid) => {
                let decoded = valid.to_string();
                self.pending_utf8.clear();
                self.finished = last;
                decoded
            }
            Err(error) if error.error_len().is_none() && !last => {
                let valid_up_to = error.valid_up_to();
                let decoded = std::str::from_utf8(&self.pending_utf8[..valid_up_to])
                    .expect("Utf8Error::valid_up_to must delimit valid UTF-8")
                    .to_string();
                self.pending_utf8.drain(..valid_up_to);
                decoded
            }
            Err(_) => {
                let decoded = if let Some(encoding) = self.legacy_encoding {
                    let mut decoder = encoding.new_decoder_without_bom_handling();
                    let decoded = decode_chunk(&mut decoder, &self.pending_utf8, last);
                    self.legacy_decoder = Some(decoder);
                    decoded
                } else {
                    String::from_utf8_lossy(&self.pending_utf8).into_owned()
                };
                self.pending_utf8.clear();
                self.finished = last;
                decoded
            }
        }
    }
}

impl Default for ShellOutputDecoder {
    fn default() -> Self {
        Self::new(system_legacy_encoding())
    }
}

fn decode_chunk(decoder: &mut Decoder, mut bytes: &[u8], last: bool) -> String {
    let capacity = decoder
        .max_utf8_buffer_length(bytes.len())
        .unwrap_or_else(|| bytes.len().saturating_mul(4).saturating_add(16));
    let mut output = String::with_capacity(capacity);
    loop {
        let (result, read, _) = decoder.decode_to_string(bytes, &mut output, last);
        bytes = &bytes[read..];
        match result {
            CoderResult::InputEmpty => return output,
            CoderResult::OutputFull => output.reserve(capacity.max(16)),
        }
    }
}

pub(super) fn decode_shell_bytes(bytes: &[u8], last: bool) -> String {
    decode_shell_bytes_with_legacy(bytes, system_legacy_encoding(), last)
}

fn decode_shell_bytes_with_legacy(
    bytes: &[u8],
    legacy_encoding: Option<&'static Encoding>,
    last: bool,
) -> String {
    ShellOutputDecoder::new(legacy_encoding).decode(bytes, last)
}

#[cfg(windows)]
fn system_legacy_encoding() -> Option<&'static Encoding> {
    // SAFETY: GetACP takes no arguments and has no caller-owned lifetime.
    legacy_encoding_for_code_page(unsafe { windows::Win32::Globalization::GetACP() })
}

#[cfg(not(windows))]
fn system_legacy_encoding() -> Option<&'static Encoding> {
    None
}

// Only the Windows `system_legacy_encoding` calls this in production; the
// mapping table itself is exercised cross-platform by the unit tests.
#[cfg_attr(not(windows), allow(dead_code))]
fn legacy_encoding_for_code_page(code_page: u32) -> Option<&'static Encoding> {
    match code_page {
        874 => Some(encoding_rs::WINDOWS_874),
        932 => Some(encoding_rs::SHIFT_JIS),
        936 => Some(encoding_rs::GBK),
        949 => Some(encoding_rs::EUC_KR),
        950 => Some(encoding_rs::BIG5),
        1250 => Some(encoding_rs::WINDOWS_1250),
        1251 => Some(encoding_rs::WINDOWS_1251),
        1252 => Some(encoding_rs::WINDOWS_1252),
        1253 => Some(encoding_rs::WINDOWS_1253),
        1254 => Some(encoding_rs::WINDOWS_1254),
        1255 => Some(encoding_rs::WINDOWS_1255),
        1256 => Some(encoding_rs::WINDOWS_1256),
        1257 => Some(encoding_rs::WINDOWS_1257),
        1258 => Some(encoding_rs::WINDOWS_1258),
        _ => None,
    }
}

#[derive(Debug)]
pub(super) struct BoundedOutputSnapshot {
    pub(super) content: String,
    pub(super) total_bytes: usize,
    pub(super) retained_bytes: usize,
    pub(super) truncated: bool,
}

/// One decoded, arrival-ordered stream: complete output goes to disk while
/// memory retains only enough tail bytes for the 2,000-line/50KiB result bound.
pub(super) struct BoundedOutputAccumulator {
    tail: VecDeque<u8>,
    tail_newlines: usize,
    total_bytes: usize,
    total_newlines: usize,
    current_line_bytes: usize,
    last_line_bytes: usize,
    front_clipped: bool,
    last_byte: Option<u8>,
    decoder: ShellOutputDecoder,
    stream_finished: bool,
    stream_error: Option<String>,
    temp: Option<tempfile::NamedTempFile>,
    full_output_path: Option<PathBuf>,
    /// Why the on-disk spill file could not be created (disk full, descriptor
    /// exhaustion, unwritable temp dir). The stream still runs and the bounded
    /// tail is still delivered; only "Full output: <path>" is unavailable.
    spill_unavailable: Option<String>,
}

impl BoundedOutputAccumulator {
    /// Build an accumulator whose complete-output spill file lives in
    /// `spill_dir` (`None` = process temp dir). Never fails: when the spill
    /// file cannot be created (disk full, `EMFILE`, missing temp dir) the
    /// command still runs and the bounded tail is still returned — the spill
    /// is a convenience, not a precondition for executing `echo ok`. Tests
    /// pass a nonexistent dir to fault-inject the failure.
    pub(super) fn new_in(spill_dir: Option<&std::path::Path>) -> Self {
        Self::new_in_with_legacy(spill_dir, system_legacy_encoding())
    }

    fn new_in_with_legacy(
        spill_dir: Option<&std::path::Path>,
        legacy_encoding: Option<&'static Encoding>,
    ) -> Self {
        let mut builder = tempfile::Builder::new();
        builder.prefix("codewhale-bash-");
        let temp = match spill_dir {
            Some(dir) => builder.tempfile_in(dir),
            None => builder.tempfile(),
        };
        let (temp, spill_unavailable) = match temp {
            Ok(temp) => (Some(temp), None),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "shell output spill file unavailable; continuing with the in-memory tail only"
                );
                (None, Some(spill_unavailable_reason(&error)))
            }
        };
        Self {
            tail: VecDeque::with_capacity(BOUNDED_OUTPUT_RETAIN_BYTES),
            tail_newlines: 0,
            total_bytes: 0,
            total_newlines: 0,
            current_line_bytes: 0,
            last_line_bytes: 0,
            front_clipped: false,
            last_byte: None,
            decoder: ShellOutputDecoder::new(legacy_encoding),
            stream_finished: false,
            stream_error: None,
            temp,
            full_output_path: None,
            spill_unavailable,
        }
    }

    /// Why the complete output is not being persisted, if it is not.
    #[cfg(test)]
    pub(super) fn spill_unavailable(&self) -> Option<&str> {
        self.spill_unavailable.as_deref()
    }

    pub(super) fn append(&mut self, raw: &[u8]) -> io::Result<()> {
        if self.stream_finished {
            return Err(io::Error::other(
                "shell output arrived after the stream closed",
            ));
        }
        if let Some(temp) = self.temp.as_mut() {
            temp.write_all(raw)?;
        }
        let decoded = self.decoder.decode(raw, false);
        self.append_decoded(decoded.as_bytes());
        Ok(())
    }

    pub(super) fn finish(&mut self) -> io::Result<()> {
        if !self.stream_finished {
            let decoded = self.decoder.decode(&[], true);
            self.append_decoded(decoded.as_bytes());
            if let Some(temp) = self.temp.as_mut() {
                temp.flush()?;
            }
            self.stream_finished = true;
        }
        Ok(())
    }

    pub(super) fn record_error(&mut self, error: &io::Error) {
        self.stream_error = Some(error.to_string());
    }

    fn append_decoded(&mut self, bytes: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        for &byte in bytes {
            self.tail.push_back(byte);
            if byte == b'\n' {
                self.tail_newlines += 1;
                self.total_newlines += 1;
                self.last_line_bytes = self.current_line_bytes;
                self.current_line_bytes = 0;
            } else {
                self.current_line_bytes += 1;
            }
            self.last_byte = Some(byte);
        }
        while self.tail.len() > BOUNDED_OUTPUT_RETAIN_BYTES {
            self.pop_front();
            self.front_clipped = true;
        }
        while self.tail_lines() > BOUNDED_OUTPUT_MAX_LINES {
            while let Some(byte) = self.tail.pop_front() {
                if byte == b'\n' {
                    self.tail_newlines -= 1;
                    break;
                }
            }
            self.front_clipped = false;
        }
    }

    fn pop_front(&mut self) {
        if self.tail.pop_front() == Some(b'\n') {
            self.tail_newlines -= 1;
        }
    }

    fn tail_lines(&self) -> usize {
        self.tail_newlines + usize::from(self.tail.back().is_some_and(|byte| *byte != b'\n'))
    }

    fn total_lines(&self) -> usize {
        self.total_newlines + usize::from(self.last_byte.is_some_and(|byte| byte != b'\n'))
    }

    fn selected(&self) -> (Vec<u8>, bool) {
        let mut bytes = self.tail.iter().copied().collect::<Vec<_>>();
        let recent_line_bytes = if self.last_byte == Some(b'\n') {
            self.last_line_bytes
        } else {
            self.current_line_bytes
        };
        let partial_line = recent_line_bytes > BOUNDED_OUTPUT_MAX_BYTES;
        if partial_line {
            if bytes.last() == Some(&b'\n') {
                bytes.pop();
            }
            let floor = bytes.len().saturating_sub(BOUNDED_OUTPUT_MAX_BYTES);
            let start = (floor..bytes.len())
                .find(|index| std::str::from_utf8(&bytes[*index..]).is_ok())
                .unwrap_or(bytes.len());
            bytes.drain(..start);
        } else if self.front_clipped
            && let Some(newline) = bytes.iter().position(|byte| *byte == b'\n')
        {
            bytes.drain(..=newline);
        }
        (bytes, partial_line)
    }

    fn format_size(bytes: usize) -> String {
        if bytes < 1024 {
            format!("{bytes}B")
        } else if bytes < 1024 * 1024 {
            format!("{:.1}KB", bytes as f64 / 1024.0)
        } else {
            format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
        }
    }

    pub(super) fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub(super) fn snapshot(&mut self, finalize: bool) -> io::Result<BoundedOutputSnapshot> {
        if let Some(error) = self.stream_error.as_ref() {
            return Err(io::Error::other(error.clone()));
        }
        let (selected, partial_line) = self.selected();
        let retained_bytes = selected.len();
        let truncated = retained_bytes < self.total_bytes;
        let total_lines = self.total_lines();
        let kept_lines = selected.iter().filter(|byte| **byte == b'\n').count()
            + usize::from(selected.last().is_some_and(|byte| *byte != b'\n'));
        let mut content = String::from_utf8(selected).expect("stream decoder emits valid UTF-8");

        if finalize && self.stream_finished && self.full_output_path.is_none() {
            if truncated {
                if let Some(mut temp) = self.temp.take() {
                    temp.flush()?;
                    let (_, path) = temp.keep().map_err(|error| error.error)?;
                    self.full_output_path = Some(path);
                }
            } else {
                self.temp.take();
            }
        }
        if truncated && finalize && self.full_output_path.is_none() {
            let reason = self.spill_unavailable.as_deref().unwrap_or(
                "the output stream did not close cleanly, so the spill file was not kept",
            );
            content.push_str(&format!(
                "\n\n[Showing the last {} of {} lines ({} limit). Full output was not persisted: {reason}]",
                Self::format_size(retained_bytes),
                total_lines,
                Self::format_size(BOUNDED_OUTPUT_MAX_BYTES),
            ));
        } else if truncated
            && finalize
            && let Some(path) = self.full_output_path.as_ref()
        {
            if partial_line {
                content.push_str(&format!(
                    "\n\n[Showing last {} of line {} (line is {}). Full output: {}]",
                    Self::format_size(retained_bytes),
                    total_lines,
                    Self::format_size(self.current_line_bytes),
                    path.display()
                ));
            } else {
                let start = total_lines.saturating_sub(kept_lines) + 1;
                let limit = if self.front_clipped {
                    format!(" ({} limit)", Self::format_size(BOUNDED_OUTPUT_MAX_BYTES))
                } else {
                    String::new()
                };
                content.push_str(&format!(
                    "\n\n[Showing lines {start}-{total_lines} of {total_lines}{limit}. Full output: {}]",
                    path.display()
                ));
            }
        }
        Ok(BoundedOutputSnapshot {
            content,
            total_bytes: self.total_bytes,
            retained_bytes,
            truncated,
        })
    }

    #[cfg(test)]
    pub(super) fn retained_memory_bytes(&self) -> usize {
        self.tail.len()
    }

    #[cfg(test)]
    pub(super) fn full_output_path(&self) -> Option<&std::path::Path> {
        self.full_output_path.as_deref()
    }
}

/// Human-readable, actionable reason for a failed spill-file creation.
pub(super) fn spill_unavailable_reason(error: &io::Error) -> String {
    match resource_exhaustion_hint(error) {
        Some(hint) => format!("{error} ({hint})"),
        None => error.to_string(),
    }
}

/// When an I/O error looks like host resource exhaustion, name the likely
/// cause and the remedy. Returns `None` for ordinary errors.
pub(super) fn resource_exhaustion_hint(error: &io::Error) -> Option<&'static str> {
    use io::ErrorKind;
    match error.kind() {
        ErrorKind::StorageFull | ErrorKind::QuotaExceeded => {
            return Some("the disk holding the temp dir is full; free space and retry");
        }
        ErrorKind::OutOfMemory => {
            return Some("the host is out of memory; close heavy processes and retry");
        }
        _ => {}
    }
    let code = error.raw_os_error()?;
    // ENOSPC / EDQUOT / EMFILE / ENFILE / ENOMEM / EAGAIN — the codes fork(2),
    // pipe(2), and open(2) return when the machine is thrashing.
    #[cfg(unix)]
    {
        if code == libc::ENOSPC || code == libc::EDQUOT {
            return Some("the disk holding the temp dir is full; free space and retry");
        }
        if code == libc::EMFILE || code == libc::ENFILE {
            return Some(
                "the process or host has run out of file descriptors; close background jobs or raise `ulimit -n` and retry",
            );
        }
        if code == libc::ENOMEM {
            return Some("the host is out of memory; close heavy processes and retry");
        }
        if code == libc::EAGAIN {
            return Some(
                "the host refused to create a process, thread, or pipe (resource limit reached); close heavy processes and retry",
            );
        }
    }
    #[cfg(windows)]
    {
        // ERROR_DISK_FULL, ERROR_HANDLE_DISK_FULL, ERROR_NOT_ENOUGH_MEMORY, ERROR_TOO_MANY_OPEN_FILES
        if code == 112 || code == 39 {
            return Some("the disk holding the temp dir is full; free space and retry");
        }
        if code == 8 {
            return Some("the host is out of memory; close heavy processes and retry");
        }
        if code == 4 {
            return Some(
                "the process has run out of file handles; close background jobs and retry",
            );
        }
    }
    let _ = code;
    None
}

/// Hard in-flight ceiling for one raw shell stream held in memory (#5472).
/// Past this the oldest bytes are dropped — counted, never silently lost — so
/// one chatty command (`cargo build -v`, `git log -p`) cannot grow the process
/// by its entire output. Deliberately far above every consumer of these bytes:
/// the 30 KB tool-result truncation (`shell_output::MAX_OUTPUT_SIZE`), the
/// 1,200-char job-panel tail and the 1 KiB completion tail all fit with three
/// orders of magnitude to spare. The only surface a clip can reach is the
/// durable completion artifact, which records the omission explicitly.
pub(super) const RAW_STREAM_MAX_BYTES: usize = 16 * 1024 * 1024;

/// Extra headroom before a front-drop, so the O(len) compaction runs once per
/// `cap / 4` bytes appended instead of once per chunk.
const RAW_STREAM_DROP_SLACK: usize = RAW_STREAM_MAX_BYTES / 4;

/// Tail retained once a job's output has been *delivered* — the foreground
/// result is already the tool result, or the completion evidence is already
/// written to its session artifact. Everything past this is dead weight for
/// the up-to-1 h the finished record stays listed (#5472 finding 1).
pub(super) const RAW_STREAM_SETTLED_TAIL_BYTES: usize = 64 * 1024;

/// One raw (undecoded) shell stream retained in memory for a live job.
///
/// Bounded two independent ways, which is the whole point of the type:
/// `append` enforces `cap` while the command runs, and `release_to_tail`
/// collapses the buffer the moment its bytes have been delivered. Both record
/// how many leading bytes were discarded so `total_len` — and therefore every
/// `stdout_len` / `byte_length` the model and the artifact see — stays honest.
pub(super) struct RawOutputBuffer {
    data: Vec<u8>,
    dropped: usize,
    cap: usize,
    closed: bool,
}

impl RawOutputBuffer {
    pub(super) fn new() -> Self {
        Self::with_cap(RAW_STREAM_MAX_BYTES)
    }

    pub(super) fn with_cap(cap: usize) -> Self {
        Self {
            data: Vec::new(),
            dropped: 0,
            cap: cap.max(1),
            closed: false,
        }
    }

    /// Append, returning `false` once nobody will ever read this stream again.
    ///
    /// The reader thread uses that as its exit condition, which is the only way
    /// out when a descendant has escaped the process group and holds the pipe
    /// write-end open: `read()` will never see EOF, so without this the thread
    /// runs — and retains its buffer — for the life of the process (#5472
    /// finding 2).
    pub(super) fn append(&mut self, bytes: &[u8]) -> bool {
        if self.closed {
            return false;
        }
        self.data.extend_from_slice(bytes);
        if self.data.len() > self.cap.saturating_add(RAW_STREAM_DROP_SLACK.min(self.cap)) {
            self.drop_front_to(self.cap);
        }
        true
    }

    /// Stop accepting reader bytes while preserving the exact retained cutoff.
    /// Memory is reduced separately by `release_to_tail`, only after delivery.
    pub(super) fn seal(&mut self) {
        self.closed = true;
    }

    /// Total bytes this stream has produced, including bytes no longer held.
    pub(super) fn total_len(&self) -> usize {
        self.dropped.saturating_add(self.data.len())
    }

    /// Leading bytes discarded by the in-flight cap or post-delivery release.
    pub(super) fn dropped(&self) -> usize {
        self.dropped
    }

    fn retained_start(&self) -> usize {
        self.dropped
    }

    pub(super) fn retained(&self) -> &[u8] {
        &self.data
    }

    pub(super) fn mark_closed(&mut self) {
        self.closed = true;
    }

    pub(super) fn is_closed(&self) -> bool {
        self.closed
    }

    /// Collapse to at most `keep` trailing bytes and give the allocation back.
    /// Called once a job is terminal *and* its output has been delivered.
    pub(super) fn release_to_tail(&mut self, keep: usize) {
        if self.data.len() <= keep {
            return;
        }
        self.drop_front_to(keep);
        self.data.shrink_to_fit();
    }

    fn drop_front_to(&mut self, keep: usize) {
        let raw_start = self.data.len().saturating_sub(keep);
        let start = stable_utf8_tail_start(&self.data, raw_start, false).unwrap_or(raw_start);
        self.data.drain(..start);
        self.dropped = self.dropped.saturating_add(start);
    }
}

impl Default for RawOutputBuffer {
    fn default() -> Self {
        Self::new()
    }
}

pub(super) type SharedRawOutput = Arc<Mutex<RawOutputBuffer>>;

pub(super) fn new_shared_raw_output() -> SharedRawOutput {
    Arc::new(Mutex::new(RawOutputBuffer::new()))
}

pub(super) fn take_delta_from_buffer(
    buffer: &SharedRawOutput,
    cursor: &mut usize,
) -> (Vec<u8>, usize, bool) {
    let guard = buffer.lock().unwrap_or_else(|e| e.into_inner());
    let total = guard.total_len();
    // The cursor is an absolute offset into the stream. Bytes the bound already
    // discarded can never be delivered as a delta, so skip forward over them
    // rather than re-sending the retained tail as if it were new.
    let retained_end = guard
        .retained_start()
        .saturating_add(guard.retained().len());
    let start_abs = (*cursor).max(guard.retained_start()).min(retained_end);
    let start = start_abs - guard.retained_start();
    let retained = guard.retained();
    // Clone only the unread portion (the delta), not the entire accumulated buffer.
    // Long-running processes can produce megabytes of output; cloning the full
    // buffer on every poll held the ShellManager mutex for O(total_bytes) time.
    let unread = &retained[start..];
    // A poll can land mid-character: the caller decodes this delta as UTF-8, so
    // handing back a truncated multibyte sequence renders it as replacement
    // glyphs and corrupts the next delta's leading byte too (the streaming-client
    // bug from #1675, in the shell preview path). Leave an incomplete trailing
    // sequence in the buffer for the next poll. Bytes that are genuinely invalid
    // rather than merely unfinished still pass through, so binary output cannot
    // stall the cursor, and the final result is read from the whole buffer.
    let closed = guard.is_closed();
    let consumed = if closed {
        unread.len()
    } else {
        match std::str::from_utf8(unread) {
            Ok(_) => unread.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => unread.len(),
        }
    };
    let delta = unread[..consumed].to_vec();
    *cursor = if closed { total } else { start_abs + consumed };
    (delta, total, closed)
}

/// Read only the tail of a byte buffer and return (total_len, tail_string).
///
/// Avoids cloning the full buffer when only a trailing excerpt is needed
/// (e.g. for the job-panel display). `max_tail_chars` is in Unicode scalar
/// values; we read at most `max_tail_chars * 4` bytes from the end to account
/// for multi-byte UTF-8 sequences.
pub(super) fn tail_from_buffer(buffer: &SharedRawOutput, max_tail_chars: usize) -> (usize, String) {
    let guard = buffer.lock().unwrap_or_else(|e| e.into_inner());
    // The reported length is the stream's total, not what is still held: a
    // released or clipped buffer must not make the model believe the command
    // printed less than it did.
    let total = guard.total_len();
    let retained = guard.retained();
    (
        total,
        tail_from_bytes(retained, max_tail_chars, guard.is_closed()),
    )
}

fn tail_from_bytes(bytes: &[u8], max_tail_chars: usize, last: bool) -> String {
    tail_from_bytes_with_legacy(bytes, max_tail_chars, last, system_legacy_encoding())
}

fn tail_from_bytes_with_legacy(
    bytes: &[u8],
    max_tail_chars: usize,
    last: bool,
    legacy_encoding: Option<&'static Encoding>,
) -> String {
    let raw_start = bytes
        .len()
        .saturating_sub(max_tail_chars.saturating_mul(4).saturating_add(16));
    let start = stable_utf8_tail_start(bytes, raw_start, last).unwrap_or(raw_start);
    let decoded = decode_shell_bytes_with_legacy(&bytes[start..], legacy_encoding, last);
    tail_text(&decoded, max_tail_chars)
}

fn stable_utf8_tail_start(bytes: &[u8], raw_start: usize, last: bool) -> Option<usize> {
    let mut aligned_start = raw_start;
    while aligned_start < bytes.len() && (bytes[aligned_start] & 0xC0) == 0x80 {
        aligned_start += 1;
    }
    if aligned_start != raw_start {
        if aligned_start == bytes.len() {
            return None;
        }
        let mut character_start = raw_start;
        let lower_bound = raw_start.saturating_sub(3);
        while character_start > lower_bound && (bytes[character_start] & 0xC0) == 0x80 {
            character_start -= 1;
        }
        if std::str::from_utf8(&bytes[character_start..aligned_start]).is_err() {
            return None;
        }
    }
    match std::str::from_utf8(&bytes[aligned_start..]) {
        Ok(_) => Some(aligned_start),
        Err(error) if !last && error.error_len().is_none() => Some(aligned_start),
        Err(_) => None,
    }
}

pub(super) fn tail_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let tail = text
        .chars()
        .rev()
        .take(max_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("...{tail}")
}

#[cfg(test)]
mod tests {
    use super::{
        BOUNDED_OUTPUT_MAX_BYTES, BOUNDED_OUTPUT_MAX_LINES, BoundedOutputAccumulator,
        RAW_STREAM_MAX_BYTES, RawOutputBuffer, SharedRawOutput, ShellOutputDecoder,
        decode_shell_bytes_with_legacy, legacy_encoding_for_code_page, tail_from_buffer,
        tail_from_bytes_with_legacy, take_delta_from_buffer,
    };
    use std::sync::{Arc, Mutex};

    fn raw(bytes: &[u8]) -> SharedRawOutput {
        let mut buffer = RawOutputBuffer::new();
        buffer.append(bytes);
        Arc::new(Mutex::new(buffer))
    }

    fn append(buffer: &SharedRawOutput, bytes: &[u8]) {
        buffer.lock().unwrap().append(bytes);
    }

    #[test]
    fn decoder_preserves_utf8_and_legacy_characters_split_across_reads() {
        let utf8 = "中文".as_bytes();
        let mut decoder = ShellOutputDecoder::new(None);
        let parts = [
            decoder.decode(&utf8[..1], false),
            decoder.decode(&utf8[1..4], false),
            decoder.decode(&utf8[4..], true),
        ];
        assert_eq!(parts.concat(), "中文");

        let (gbk, _, _) = encoding_rs::GBK.encode("中文");
        let mut decoder = ShellOutputDecoder::new(Some(encoding_rs::GBK));
        let parts = [
            decoder.decode(b"status: ", false),
            decoder.decode(&gbk[..1], false),
            decoder.decode(&gbk[1..3], false),
            decoder.decode(&gbk[3..], true),
        ];
        assert_eq!(parts.concat(), "status: 中文");
    }

    #[test]
    fn complete_and_bounded_decoders_share_legacy_fallback() {
        let (gbk, _, _) = encoding_rs::GBK.encode("中文");
        assert_eq!(
            decode_shell_bytes_with_legacy(&gbk, Some(encoding_rs::GBK), true),
            "中文"
        );

        let mut output = BoundedOutputAccumulator::new_in_with_legacy(None, Some(encoding_rs::GBK));
        output.append(&gbk[..1]).expect("first byte");
        output.append(&gbk[1..]).expect("remaining bytes");
        output.finish().expect("finish");
        assert_eq!(output.snapshot(true).expect("snapshot").content, "中文");
    }

    #[test]
    fn raw_background_delta_uses_the_stateful_legacy_decoder() {
        let (gbk, _, _) = encoding_rs::GBK.encode("中文");
        let buffer = raw(&gbk[..1]);
        let mut cursor = 0;
        let mut decoder = ShellOutputDecoder::new(Some(encoding_rs::GBK));

        let (first, _, first_closed) = take_delta_from_buffer(&buffer, &mut cursor);
        assert!(!first_closed);
        assert!(decoder.decode(&first, false).is_empty());
        append(&buffer, &gbk[1..]);
        buffer.lock().unwrap().mark_closed();
        let (second, _, second_closed) = take_delta_from_buffer(&buffer, &mut cursor);

        assert!(second_closed);
        assert_eq!(decoder.decode(&second, second_closed), "中文");
    }

    #[test]
    fn process_status_cannot_finalize_before_late_reader_bytes_and_eof() {
        let buffer = raw(b"ready ");
        append(&buffer, &[0xE4]);
        let mut cursor = 0;
        let mut decoder = ShellOutputDecoder::new(None);

        let (first, _, closed) = take_delta_from_buffer(&buffer, &mut cursor);
        assert!(!closed);
        assert_eq!(decoder.decode(&first, closed), "ready ");

        // A process may already be terminal here, but only the reader owns EOF.
        let (before_late_bytes, _, closed) = take_delta_from_buffer(&buffer, &mut cursor);
        assert!(!closed);
        assert!(decoder.decode(&before_late_bytes, closed).is_empty());

        append(&buffer, &[0xB8, 0xAD]);
        let (late, _, closed) = take_delta_from_buffer(&buffer, &mut cursor);
        assert!(!closed);
        assert_eq!(decoder.decode(&late, closed), "中");

        buffer.lock().unwrap().mark_closed();
        let (eof, _, closed) = take_delta_from_buffer(&buffer, &mut cursor);
        assert!(closed);
        assert!(decoder.decode(&eof, closed).is_empty());
        assert!(decoder.decode(&[], true).is_empty(), "EOF flushes once");
    }

    #[test]
    fn tail_flushes_an_incomplete_sequence_only_after_reader_close() {
        let buffer = raw(b"ok \xE4");
        assert_eq!(tail_from_buffer(&buffer, 20).1, "ok ");

        buffer.lock().unwrap().mark_closed();
        let final_tail = tail_from_buffer(&buffer, 20).1;
        assert!(final_tail.starts_with("ok "));
        assert_ne!(final_tail, "ok ");
    }

    #[test]
    fn windows_ansi_code_page_mapping_is_explicit_and_bounded() {
        assert_eq!(
            legacy_encoding_for_code_page(874).map(|encoding| encoding.name()),
            Some(encoding_rs::WINDOWS_874.name())
        );
        assert_eq!(
            legacy_encoding_for_code_page(936).map(|encoding| encoding.name()),
            Some(encoding_rs::GBK.name())
        );
        assert_eq!(legacy_encoding_for_code_page(65001), None);
        assert_eq!(legacy_encoding_for_code_page(437), None);
    }

    #[test]
    fn legacy_tail_does_not_treat_cp1252_bytes_as_utf8_continuations() {
        let bytes = vec![0x80; 64];
        assert_eq!(
            tail_from_bytes_with_legacy(&bytes, 4, true, Some(encoding_rs::WINDOWS_1252)),
            "...€€€€"
        );
    }

    #[test]
    fn delta_holds_back_an_incomplete_trailing_utf8_sequence() {
        // "宽" is three bytes; deliver two of them, then the rest.
        let wide = "宽".as_bytes();
        let buffer = raw(b"ok ");
        append(&buffer, &wide[..2]);
        let mut cursor = 0usize;

        let (delta, total, _) = take_delta_from_buffer(&buffer, &mut cursor);
        assert_eq!(
            String::from_utf8(delta).expect("delta must be whole characters"),
            "ok "
        );
        assert_eq!(total, 5, "total still reports every buffered byte");
        assert_eq!(cursor, 3, "the split character stays unread");

        append(&buffer, &wide[2..]);
        let (delta, _, _) = take_delta_from_buffer(&buffer, &mut cursor);
        assert_eq!(
            String::from_utf8(delta).expect("delta must be whole characters"),
            "宽"
        );
    }

    #[test]
    fn delta_does_not_stall_on_genuinely_invalid_bytes() {
        // A lone 0xFF is never a valid start byte: passing it through keeps
        // binary output flowing instead of parking the cursor forever.
        let buffer = raw(&[b'a', 0xFF, b'b']);
        let mut cursor = 0usize;
        let (delta, total, _) = take_delta_from_buffer(&buffer, &mut cursor);
        assert_eq!(delta, vec![b'a', 0xFF, b'b']);
        assert_eq!(cursor, total);
    }

    // === #5472: in-memory retention bounds for the raw `Bash` streams ===

    #[test]
    fn raw_buffer_caps_in_flight_bytes_and_keeps_the_total_honest() {
        let mut buffer = RawOutputBuffer::with_cap(1_024);
        // 4 MiB through a 1 KiB cap: the analogue of `cargo build -v` through
        // the 16 MiB production ceiling.
        for _ in 0..1_024 {
            buffer.append(&[b'x'; 4_096]);
        }
        let produced = 1_024 * 4_096;
        assert_eq!(
            buffer.total_len(),
            produced,
            "the stream's length must survive the bound"
        );
        assert_eq!(buffer.dropped(), produced - buffer.retained().len());
        assert!(
            buffer.retained().len() <= 1_024 + 1_024 / 4,
            "retained {} exceeded cap + slack",
            buffer.retained().len()
        );
    }

    #[test]
    fn raw_buffer_release_collapses_to_a_tail_and_reports_the_omission() {
        let mut buffer = RawOutputBuffer::new();
        buffer.append(&[b'y'; 200_000]);
        assert_eq!(buffer.dropped(), 0, "200 KB is under the in-flight ceiling");

        buffer.release_to_tail(1_000);
        assert_eq!(buffer.retained().len(), 1_000);
        assert_eq!(buffer.dropped(), 199_000);
        assert_eq!(
            buffer.total_len(),
            200_000,
            "releasing memory must not rewrite how much the command printed"
        );
    }

    #[test]
    fn raw_buffer_never_retains_a_split_character() {
        let mut buffer = RawOutputBuffer::with_cap(8);
        // Each "宽" is 3 bytes, so a byte-exact tail would land mid-character.
        for _ in 0..64 {
            buffer.append("宽".as_bytes());
        }
        assert!(
            std::str::from_utf8(buffer.retained()).is_ok(),
            "front-drop must snap off continuation bytes"
        );

        let mut released = RawOutputBuffer::new();
        for _ in 0..64 {
            released.append("宽".as_bytes());
        }
        released.release_to_tail(10);
        assert!(std::str::from_utf8(released.retained()).is_ok());
    }

    #[test]
    fn delta_skips_bytes_the_bound_already_discarded() {
        // A consumer that stops reading while output keeps arriving must be
        // moved forward, not handed the retained tail as if it were new bytes.
        let buffer = Arc::new(Mutex::new(RawOutputBuffer::with_cap(16)));
        append(&buffer, b"first-chunk-that-will-be-dropped-entirely");
        let mut cursor = 0usize;
        let (delta, total, _) = take_delta_from_buffer(&buffer, &mut cursor);
        let dropped = buffer.lock().unwrap().dropped();
        assert!(dropped > 0, "the cap must have clipped the front");
        assert_eq!(cursor, total, "cursor lands at the stream's true position");
        assert_eq!(
            delta.len(),
            total - dropped,
            "only bytes still held can be delivered"
        );

        append(&buffer, b"tail");
        let (delta, _, _) = take_delta_from_buffer(&buffer, &mut cursor);
        assert_eq!(
            delta,
            b"tail".to_vec(),
            "subsequent deltas continue from the corrected cursor"
        );
    }

    #[test]
    fn tail_reports_the_stream_total_not_the_retained_length() {
        let buffer = Arc::new(Mutex::new(RawOutputBuffer::new()));
        append(&buffer, b"abcdefghij");
        buffer.lock().unwrap().release_to_tail(4);
        let (total, tail) = tail_from_buffer(&buffer, 100);
        assert_eq!(total, 10, "stdout_len must not shrink when memory is freed");
        assert_eq!(tail, "ghij");
    }

    #[test]
    fn sealing_a_stream_preserves_the_cutoff_and_stops_the_reader() {
        let mut buffer = RawOutputBuffer::new();
        assert!(buffer.append(&[b'a'; 5_000]), "a live stream keeps reading");
        buffer.seal();

        assert!(buffer.is_closed(), "detach is a terminal consumer cutoff");
        assert_eq!(buffer.retained(), &[b'a'; 5_000]);
        assert_eq!(buffer.dropped(), 0, "sealing does not rewrite the cutoff");
        assert_eq!(
            buffer.total_len(),
            5_000,
            "the stream's length survives the seal"
        );
        assert!(
            !buffer.append(&[b'b'; 100]),
            "a sealed stream tells the reader thread to exit"
        );
        assert_eq!(buffer.retained(), &[b'a'; 5_000]);
        assert_eq!(buffer.dropped(), 0, "rejected bytes are past the cutoff");
        assert_eq!(
            buffer.total_len(),
            5_000,
            "the public total freezes at the deliverable cutoff"
        );

        let buffer = Arc::new(Mutex::new(buffer));
        let mut cursor = 5_000;
        let (delta, total, closed) = take_delta_from_buffer(&buffer, &mut cursor);
        assert!(closed);
        assert!(
            delta.is_empty(),
            "rejected trailing bytes must not resend the tail"
        );
        assert_eq!(cursor, total);

        let mut buffer = buffer.lock().unwrap();
        buffer.release_to_tail(1_000);
        assert_eq!(buffer.retained(), &[b'a'; 1_000]);
        assert_eq!(buffer.dropped(), 4_000);
        assert_eq!(buffer.total_len(), 5_000);
    }

    #[test]
    fn raw_stream_ceiling_clears_every_downstream_bound() {
        // The clip must be unreachable by the model-visible surfaces: the 30 KB
        // result truncation, the 1,200-char job tail, the 1 KiB completion tail.
        const { assert!(RAW_STREAM_MAX_BYTES > 30_000 * 100) };
        const { assert!(super::RAW_STREAM_SETTLED_TAIL_BYTES > 30_000) };
    }

    #[test]
    fn bounded_output_keeps_last_two_thousand_complete_lines() {
        let source = (0..=BOUNDED_OUTPUT_MAX_LINES)
            .map(|index| format!("line-{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut output = BoundedOutputAccumulator::new_in(None);
        output.append(source.as_bytes()).expect("append");
        output.finish().expect("finish");
        let snapshot = output.snapshot(true).expect("snapshot");
        assert!(snapshot.truncated);
        assert!(snapshot.content.starts_with("line-1\n"));
        assert!(snapshot.content.contains("Showing lines 2-2001 of 2001"));
    }

    #[test]
    fn bounded_output_streams_raw_full_output_and_bounds_decoded_tail() {
        let raw = vec![0xFF; 2 * 1024 * 1024];
        let mut output = BoundedOutputAccumulator::new_in(None);
        for chunk in raw.chunks(4_096) {
            output.append(chunk).expect("append");
            assert!(output.retained_memory_bytes() <= BOUNDED_OUTPUT_MAX_BYTES + 4);
        }
        output.finish().expect("finish");
        let snapshot = output.snapshot(true).expect("snapshot");
        assert!(snapshot.truncated);
        assert!(snapshot.retained_bytes <= BOUNDED_OUTPUT_MAX_BYTES);
        // 0xFF is invalid UTF-8, so a UTF-8 decode is lossy and leaves the
        // replacement character behind. Windows decodes shell output through
        // the ANSI code page instead (#5602): under CP1252 every 0xFF is a
        // valid 'ÿ' and nothing is replaced, while under a DBCS page it still
        // is. Demanding UTF-8's answer on Windows asserted the mojibake that
        // #5602 exists to remove, so assert the property both decodes owe and
        // keep the strict check where the decode is unambiguously UTF-8.
        assert!(!snapshot.content.is_empty());
        #[cfg(not(windows))]
        assert!(snapshot.content.contains('\u{FFFD}'));
        let path = output
            .full_output_path()
            .expect("full output")
            .to_path_buf();
        assert_eq!(std::fs::read(&path).expect("read full output"), raw);
        drop(output);
        std::fs::remove_file(path).expect("remove full output");
    }

    #[test]
    fn bounded_output_huge_terminal_line_matches_upstream_notice() {
        let mut source = vec![b'x'; BOUNDED_OUTPUT_MAX_BYTES + 1_024];
        source.push(b'\n');
        let mut output = BoundedOutputAccumulator::new_in(None);
        output.append(&source).expect("append");
        output.finish().expect("finish");
        let snapshot = output.snapshot(true).expect("snapshot");
        assert!(snapshot.content.contains("Showing last 50.0KB of line 1"));
        assert!(snapshot.content.contains("line is 0B"));
        let path = output
            .full_output_path()
            .expect("full output")
            .to_path_buf();
        drop(output);
        std::fs::remove_file(path).expect("remove full output");
    }

    #[test]
    fn spill_failure_is_soft_and_names_the_reason() {
        // A missing spill dir simulates a full or broken temp volume: the
        // stream still runs, the tail is still delivered, and the notice says
        // why "Full output: <path>" is absent instead of failing the command.
        let missing =
            std::env::temp_dir().join(format!("codewhale-missing-spill-{}", std::process::id()));
        let mut output = BoundedOutputAccumulator::new_in(Some(&missing));
        let reason = output.spill_unavailable().expect("spill unavailable");
        assert!(!reason.is_empty(), "reason must name the io error");

        output.append(b"ok\n").expect("append works without spill");
        output.finish().expect("finish works without spill");
        let short = output.snapshot(true).expect("snapshot");
        assert_eq!(short.content, "ok\n");
        assert!(!short.truncated);
        assert!(output.full_output_path().is_none());

        let source = (0..=BOUNDED_OUTPUT_MAX_LINES)
            .map(|index| format!("line-{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut output = BoundedOutputAccumulator::new_in(Some(&missing));
        output.append(source.as_bytes()).expect("append");
        output.finish().expect("finish");
        let snapshot = output.snapshot(true).expect("snapshot");
        assert!(snapshot.truncated);
        assert!(snapshot.content.starts_with("line-1\n"));
        assert!(
            snapshot.content.contains("Full output was not persisted:"),
            "{}",
            snapshot.content
        );
        assert!(!snapshot.content.contains("Full output: "));
        assert!(output.full_output_path().is_none());
    }

    #[test]
    fn resource_exhaustion_hint_names_disk_descriptors_and_memory() {
        use std::io::{Error, ErrorKind};
        assert!(
            super::resource_exhaustion_hint(&Error::from(ErrorKind::StorageFull))
                .expect("storage full")
                .contains("disk")
        );
        assert!(
            super::resource_exhaustion_hint(&Error::from(ErrorKind::OutOfMemory))
                .expect("oom")
                .contains("memory")
        );
        #[cfg(unix)]
        {
            assert!(
                super::resource_exhaustion_hint(&Error::from_raw_os_error(libc::ENOSPC))
                    .expect("enospc")
                    .contains("disk")
            );
            assert!(
                super::resource_exhaustion_hint(&Error::from_raw_os_error(libc::EMFILE))
                    .expect("emfile")
                    .contains("file descriptors")
            );
            assert!(
                super::resource_exhaustion_hint(&Error::from_raw_os_error(libc::EAGAIN))
                    .expect("eagain")
                    .contains("retry")
            );
        }
        assert!(super::resource_exhaustion_hint(&Error::from(ErrorKind::NotFound)).is_none());
        assert!(
            super::resource_exhaustion_hint(&Error::from(ErrorKind::PermissionDenied)).is_none()
        );
    }
}
