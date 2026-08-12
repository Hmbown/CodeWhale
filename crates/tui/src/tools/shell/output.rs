use std::collections::VecDeque;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use encoding_rs::{CoderResult, Decoder, UTF_8};

const BOUNDED_OUTPUT_MAX_LINES: usize = 2_000;
const BOUNDED_OUTPUT_MAX_BYTES: usize = 50 * 1024;
const BOUNDED_OUTPUT_RETAIN_BYTES: usize = BOUNDED_OUTPUT_MAX_BYTES + 4;

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
    decoder: Decoder,
    stream_finished: bool,
    stream_error: Option<String>,
    temp: Option<tempfile::NamedTempFile>,
    full_output_path: Option<PathBuf>,
}

impl BoundedOutputAccumulator {
    pub(super) fn new() -> io::Result<Self> {
        Ok(Self {
            tail: VecDeque::with_capacity(BOUNDED_OUTPUT_RETAIN_BYTES),
            tail_newlines: 0,
            total_bytes: 0,
            total_newlines: 0,
            current_line_bytes: 0,
            last_line_bytes: 0,
            front_clipped: false,
            last_byte: None,
            decoder: UTF_8.new_decoder_without_bom_handling(),
            stream_finished: false,
            stream_error: None,
            temp: Some(
                tempfile::Builder::new()
                    .prefix("codewhale-bash-")
                    .tempfile()?,
            ),
            full_output_path: None,
        })
    }

    fn decode(&mut self, bytes: &[u8], last: bool) -> String {
        let capacity = self
            .decoder
            .max_utf8_buffer_length(bytes.len())
            .unwrap_or(bytes.len().saturating_mul(3).saturating_add(3));
        let mut decoded = String::with_capacity(capacity);
        let mut offset = 0;
        loop {
            let (result, read, _) =
                self.decoder
                    .decode_to_string(&bytes[offset..], &mut decoded, last);
            offset += read;
            if result == CoderResult::InputEmpty {
                return decoded;
            }
            decoded.reserve(capacity.max(4));
        }
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
        let decoded = self.decode(raw, false);
        self.append_decoded(decoded.as_bytes());
        Ok(())
    }

    pub(super) fn finish(&mut self) -> io::Result<()> {
        if !self.stream_finished {
            let decoded = self.decode(&[], true);
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
        if truncated
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

pub(super) fn take_delta_from_buffer(
    buffer: &Arc<Mutex<Vec<u8>>>,
    cursor: &mut usize,
) -> (Vec<u8>, usize) {
    let guard = buffer.lock().unwrap_or_else(|e| e.into_inner());
    let total = guard.len();
    let start = (*cursor).min(total);
    // Clone only the unread portion (the delta), not the entire accumulated buffer.
    // Long-running processes can produce megabytes of output; cloning the full
    // buffer on every poll held the ShellManager mutex for O(total_bytes) time.
    let unread = &guard[start..];
    // A poll can land mid-character: the caller decodes this delta as UTF-8, so
    // handing back a truncated multibyte sequence renders it as replacement
    // glyphs and corrupts the next delta's leading byte too (the streaming-client
    // bug from #1675, in the shell preview path). Leave an incomplete trailing
    // sequence in the buffer for the next poll. Bytes that are genuinely invalid
    // rather than merely unfinished still pass through, so binary output cannot
    // stall the cursor, and the final result is read from the whole buffer.
    let consumed = match std::str::from_utf8(unread) {
        Ok(_) => unread.len(),
        Err(error) if error.error_len().is_none() => error.valid_up_to(),
        Err(_) => unread.len(),
    };
    let delta = unread[..consumed].to_vec();
    *cursor = start + consumed;
    (delta, total)
}

/// Read only the tail of a byte buffer and return (total_len, tail_string).
///
/// Avoids cloning the full buffer when only a trailing excerpt is needed
/// (e.g. for the job-panel display). `max_tail_chars` is in Unicode scalar
/// values; we read at most `max_tail_chars * 4` bytes from the end to account
/// for multi-byte UTF-8 sequences.
pub(super) fn tail_from_buffer(
    buffer: &Arc<Mutex<Vec<u8>>>,
    max_tail_chars: usize,
) -> (usize, String) {
    let guard = buffer.lock().unwrap_or_else(|e| e.into_inner());
    let total = guard.len();
    // Over-estimate byte count (4 bytes per char worst case for UTF-8).
    let mut tail_start = total.saturating_sub(max_tail_chars.saturating_mul(4));
    // Snap forward to the next valid UTF-8 codepoint boundary so we don't
    // pass a slice beginning with continuation bytes (0x80-0xBF) to
    // from_utf8_lossy, which would emit a leading U+FFFD replacement char.
    while tail_start < total && (guard[tail_start] & 0xC0) == 0x80 {
        tail_start += 1;
    }
    let tail_str = String::from_utf8_lossy(&guard[tail_start..]).into_owned();
    (total, tail_text(&tail_str, max_tail_chars))
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
        take_delta_from_buffer,
    };
    use std::sync::{Arc, Mutex};

    #[test]
    fn delta_holds_back_an_incomplete_trailing_utf8_sequence() {
        // "宽" is three bytes; deliver two of them, then the rest.
        let wide = "宽".as_bytes();
        let buffer = Arc::new(Mutex::new(b"ok ".to_vec()));
        buffer.lock().unwrap().extend_from_slice(&wide[..2]);
        let mut cursor = 0usize;

        let (delta, total) = take_delta_from_buffer(&buffer, &mut cursor);
        assert_eq!(
            String::from_utf8(delta).expect("delta must be whole characters"),
            "ok "
        );
        assert_eq!(total, 5, "total still reports every buffered byte");
        assert_eq!(cursor, 3, "the split character stays unread");

        buffer.lock().unwrap().extend_from_slice(&wide[2..]);
        let (delta, _) = take_delta_from_buffer(&buffer, &mut cursor);
        assert_eq!(
            String::from_utf8(delta).expect("delta must be whole characters"),
            "宽"
        );
    }

    #[test]
    fn delta_does_not_stall_on_genuinely_invalid_bytes() {
        // A lone 0xFF is never a valid start byte: passing it through keeps
        // binary output flowing instead of parking the cursor forever.
        let buffer = Arc::new(Mutex::new(vec![b'a', 0xFF, b'b']));
        let mut cursor = 0usize;
        let (delta, total) = take_delta_from_buffer(&buffer, &mut cursor);
        assert_eq!(delta, vec![b'a', 0xFF, b'b']);
        assert_eq!(cursor, total);
    }

    #[test]
    fn bounded_output_keeps_last_two_thousand_complete_lines() {
        let source = (0..=BOUNDED_OUTPUT_MAX_LINES)
            .map(|index| format!("line-{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut output = BoundedOutputAccumulator::new().expect("accumulator");
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
        let mut output = BoundedOutputAccumulator::new().expect("accumulator");
        for chunk in raw.chunks(4_096) {
            output.append(chunk).expect("append");
            assert!(output.retained_memory_bytes() <= BOUNDED_OUTPUT_MAX_BYTES + 4);
        }
        output.finish().expect("finish");
        let snapshot = output.snapshot(true).expect("snapshot");
        assert!(snapshot.truncated);
        assert!(snapshot.retained_bytes <= BOUNDED_OUTPUT_MAX_BYTES);
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
        let mut output = BoundedOutputAccumulator::new().expect("accumulator");
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
}
