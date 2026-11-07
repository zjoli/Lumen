use super::ProviderError;
const LIMIT: usize = 1024 * 1024;
#[derive(Debug)]
pub(crate) struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}
#[derive(Default)]
pub(crate) struct SseParser {
    line: Vec<u8>,
    data: Vec<String>,
    event: Option<String>,
    size: usize,
    skip_lf: bool,
    first_line: bool,
}
impl SseParser {
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, ProviderError> {
        let mut out = vec![];
        for &byte in chunk {
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            self.size += 1;
            if self.size > LIMIT {
                return Err(ProviderError::protocol("SSE event exceeds size limit"));
            }
            if byte == b'\r' || byte == b'\n' {
                if byte == b'\r' {
                    self.skip_lf = true;
                }
                self.line(&mut out)?;
            } else {
                self.line.push(byte);
            }
        }
        Ok(out)
    }
    fn line(&mut self, out: &mut Vec<SseEvent>) -> Result<(), ProviderError> {
        let bytes = std::mem::take(&mut self.line);
        let mut line = std::str::from_utf8(&bytes)
            .map_err(|_| ProviderError::protocol("invalid UTF-8 in SSE"))?;
        if !self.first_line {
            self.first_line = true;
            line = line.trim_start_matches('\u{feff}');
        }
        if line.is_empty() {
            if !self.data.is_empty() {
                out.push(SseEvent {
                    event: self.event.take(),
                    data: self.data.join("\n"),
                });
            }
            self.data.clear();
            self.event = None;
            self.size = 0;
            return Ok(());
        }
        if line.starts_with(':') {
            return Ok(());
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => self.data.push(value.into()),
            "event" => self.event = Some(value.into()),
            _ => (),
        };
        Ok(())
    }
    pub fn finish(&self) -> Result<(), ProviderError> {
        if self.line.is_empty() && self.data.is_empty() {
            Ok(())
        } else {
            Err(ProviderError::protocol("truncated SSE event"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arbitrary_boundaries_utf8_crlf_multiline_bom_and_heartbeats() {
        let wire="\u{feff}: heartbeat\r\nevent: sample\r\ndata: 中文\r\ndata: line2\r\n\r\n: ping\n\ndata: last\n\n";
        for size in 1..=wire.len() {
            let mut p = SseParser::default();
            let mut out = vec![];
            for chunk in wire.as_bytes().chunks(size) {
                out.extend(p.push(chunk).unwrap());
            }
            p.finish().unwrap();
            assert_eq!(out.len(), 2);
            assert_eq!(out[0].event.as_deref(), Some("sample"));
            assert_eq!(out[0].data, "中文\nline2");
            assert_eq!(out[1].data, "last");
        }
    }
    #[test]
    fn malformed_truncated_and_oversized_events_are_rejected_before_decode() {
        let mut p = SseParser::default();
        p.push(b"data: unfinished").unwrap();
        assert!(p.finish().is_err());
        let mut p = SseParser::default();
        assert!(p
            .push(&[b'd', b'a', b't', b'a', b':', 0xff, b'\n'])
            .is_err());
        let mut p = SseParser::default();
        assert!(p.push(&vec![b'x'; LIMIT + 1]).is_err());
    }
}
