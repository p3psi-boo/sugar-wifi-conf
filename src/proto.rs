// BLE payload framing must remain compatible with the existing client.

pub const CONCAT_TAG: &str = "%&%";
pub const END_TAG: &str = "&#&";

// For strict compatibility keep 20-byte chunking even if MTU is higher.
pub const NOTIFY_CHUNK_SIZE: usize = 20;

// Existing implementation sleeps ~200ms between notify chunks.
pub const NOTIFY_CHUNK_DELAY_MS: u64 = 200;

/// Default cap for un-terminated inbound payloads.
///
/// Client requests are small (key/ssid/password or key/uuid suffix). This cap is
/// purely a safety rail against unbounded growth if a client never sends `END_TAG`.
pub const DEFAULT_MAX_REASSEMBLY_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReassemblyError {
    /// Buffer would exceed `max_size` after appending the provided bytes.
    ///
    /// Policy: the buffer is cleared to recover quickly from missing terminators.
    Overflow { max_size: usize, attempted: usize },
}

/// Incremental buffer for reassembling chunked writes terminated by `END_TAG`.
///
/// Behavior:
/// - `push()` appends bytes, scans for one or more terminators, and returns each
///   complete message *without* the terminator.
/// - Any bytes after the last terminator are retained for the next `push()`.
/// - If the internal buffer would exceed `max_size`, it is cleared and an error
///   is returned.
#[derive(Debug, Clone)]
pub struct ReassemblyBuffer {
    buf: Vec<u8>,
    max_size: usize,
}

impl ReassemblyBuffer {
    pub fn new(max_size: usize) -> Self {
        Self {
            buf: Vec::new(),
            max_size,
        }
    }

    pub fn with_default_max() -> Self {
        Self::new(DEFAULT_MAX_REASSEMBLY_BYTES)
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Append bytes and return any fully framed messages (without `END_TAG`).
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, ReassemblyError> {
        if bytes.is_empty() {
            return Ok(Vec::new());
        }

        // Refuse to grow unbounded when the sender never terminates.
        let attempted = self.buf.len().saturating_add(bytes.len());
        if attempted > self.max_size {
            self.buf.clear();
            return Err(ReassemblyError::Overflow {
                max_size: self.max_size,
                attempted,
            });
        }

        self.buf.extend_from_slice(bytes);

        let end = END_TAG.as_bytes();
        debug_assert!(!end.is_empty());

        let mut out: Vec<Vec<u8>> = Vec::new();
        let mut start = 0usize;
        while start <= self.buf.len() {
            let Some(rel) = find_subslice(&self.buf[start..], end) else {
                break;
            };
            let pos = start + rel;
            out.push(self.buf[start..pos].to_vec());
            start = pos + end.len();
        }

        if start > 0 {
            // Retain trailing bytes after the last terminator.
            self.buf = self.buf.split_off(start);
        }

        Ok(out)
    }
}

impl Default for ReassemblyBuffer {
    fn default() -> Self {
        Self::with_default_max()
    }
}

/// Frame an outbound message for notify: append `END_TAG` and chunk into 20-byte pieces.
pub fn frame_notify_message(message: &[u8]) -> Vec<Vec<u8>> {
    let mut framed = Vec::with_capacity(message.len() + END_TAG.len());
    framed.extend_from_slice(message);
    framed.extend_from_slice(END_TAG.as_bytes());

    framed
        .chunks(NOTIFY_CHUNK_SIZE)
        .map(|c| c.to_vec())
        .collect()
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > haystack.len() {
        return None;
    }

    let first = needle[0];
    let max = haystack.len() - needle.len();
    let mut i = 0usize;
    while i <= max {
        if haystack[i] == first && &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminator_across_boundaries_reassembles() {
        let mut buf = ReassemblyBuffer::new(1024);

        // END_TAG is "&#&"; split it across two pushes.
        let out1 = buf.push(b"hello&").unwrap();
        assert!(out1.is_empty());
        assert_eq!(buf.len(), b"hello&".len());

        let out2 = buf.push(b"#&").unwrap();
        assert_eq!(out2, vec![b"hello".to_vec()]);
        assert!(buf.is_empty());
    }

    #[test]
    fn multiple_messages_in_one_buffer() {
        let mut buf = ReassemblyBuffer::new(1024);

        let out = buf.push(b"a&#&b&#&").unwrap();
        assert_eq!(out, vec![b"a".to_vec(), b"b".to_vec()]);
        assert!(buf.is_empty());

        let out2 = buf.push(b"c&#&d").unwrap();
        assert_eq!(out2, vec![b"c".to_vec()]);
        assert_eq!(buf.buf, b"d".to_vec());
    }

    #[test]
    fn oversized_input_clears_and_recovers() {
        let mut buf = ReassemblyBuffer::new(10);

        let err = buf.push(b"01234567890").unwrap_err();
        assert_eq!(
            err,
            ReassemblyError::Overflow {
                max_size: 10,
                attempted: 11
            }
        );
        assert!(buf.is_empty());

        // After overflow, the buffer should recover and parse a new message normally.
        let out = buf.push(b"ok&#&").unwrap();
        assert_eq!(out, vec![b"ok".to_vec()]);
        assert!(buf.is_empty());
    }

    #[test]
    fn frame_notify_message_chunks_and_terminates() {
        let chunks = frame_notify_message(b"x");
        assert_eq!(chunks.concat(), b"x&#&".to_vec());

        // Exactly 20 bytes payload should still get a terminator added.
        let msg = vec![b'a'; 20];
        let chunks = frame_notify_message(&msg);
        assert_eq!(chunks[0].len(), 20);
        assert_eq!(chunks.concat().len(), 23);
        assert!(chunks.concat().ends_with(END_TAG.as_bytes()));
    }
}
