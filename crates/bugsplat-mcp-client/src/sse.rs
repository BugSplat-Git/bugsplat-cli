/// Splits a `text/event-stream` body, fed in arbitrary chunks, into each event's data.
/// Event names, ids and retry hints are dropped: an MCP stream only carries JSON-RPC
/// messages in `data`.
#[derive(Default)]
pub(crate) struct SseParser {
    buffer: Vec<u8>,
    data: Vec<String>,
    // The last line ended in CR; an LF that follows, even in the next chunk, belongs to it.
    after_cr: bool,
}

impl SseParser {
    /// The data of every event `chunk` completes. Lines end in CR, LF or CRLF.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        let mut start = 0;
        loop {
            if self.after_cr {
                match self.buffer.get(start) {
                    Some(b'\n') => start += 1,
                    Some(_) => {}
                    None => break,
                }
                self.after_cr = false;
            }
            let Some(length) = self.buffer[start..]
                .iter()
                .position(|byte| matches!(byte, b'\n' | b'\r'))
            else {
                break;
            };
            let end = start + length;
            let line = String::from_utf8_lossy(&self.buffer[start..end]).into_owned();
            self.after_cr = self.buffer[end] == b'\r';
            start = end + 1;

            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push(self.data.join("\n"));
                    self.data.clear();
                }
            } else if let Some(value) = line.strip_prefix("data:") {
                self.data
                    .push(value.strip_prefix(' ').unwrap_or(value).to_string());
            }
        }
        self.buffer.drain(..start);
        events
    }

    /// A last event the server did not end with a blank line before closing the stream.
    pub(crate) fn finish(mut self) -> Vec<String> {
        self.push(b"\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_events() {
        let mut parser = SseParser::default();
        assert_eq!(
            parser.push(b"event: message\ndata: {\"a\":1}\n\ndata: {\"b\":2}\n\n"),
            vec!["{\"a\":1}", "{\"b\":2}"]
        );
    }

    #[test]
    fn joins_an_event_split_across_chunks() {
        let mut parser = SseParser::default();
        assert!(parser.push(b"da").is_empty());
        assert!(parser.push(b"ta: {\"text\":\"caf\xc3").is_empty());
        assert!(parser.push(b"\xa9\"}\r\n").is_empty());
        assert_eq!(parser.push(b"\r\n"), vec!["{\"text\":\"café\"}"]);
    }

    #[test]
    fn joins_data_lines_and_skips_comments_and_other_fields() {
        let mut parser = SseParser::default();
        assert_eq!(
            parser.push(b": ping\nid: 7\nretry: 100\ndata:{\ndata: }\n\n"),
            vec!["{\n}"]
        );
    }

    #[test]
    fn finish_returns_an_event_without_its_blank_line() {
        let mut parser = SseParser::default();
        assert!(parser.push(b"data: {}").is_empty());
        assert_eq!(parser.finish(), vec!["{}"]);
    }

    #[test]
    fn bare_carriage_returns_end_lines_and_events() {
        let mut parser = SseParser::default();
        assert_eq!(parser.push(b"data: {\"a\":1}\r\r"), vec!["{\"a\":1}"]);
        assert_eq!(
            parser.push(b"data: {\"b\":2}\rdata: x\r\r"),
            vec!["{\"b\":2}\nx"]
        );
    }

    #[test]
    fn a_crlf_split_across_chunks_is_one_line_ending() {
        let mut parser = SseParser::default();
        assert!(parser.push(b"data: {}\r").is_empty());
        assert_eq!(parser.push(b"\n\r"), vec!["{}"]);
        assert!(parser.push(b"\ndata: 1\r\n").is_empty());
        assert_eq!(parser.push(b"\r\n"), vec!["1"]);
    }

    #[test]
    fn blank_lines_alone_are_not_events() {
        let mut parser = SseParser::default();
        assert!(parser.push(b"\n\n: comment\n\n").is_empty());
        assert!(parser.finish().is_empty());
    }
}
