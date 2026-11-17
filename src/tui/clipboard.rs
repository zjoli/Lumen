//! Write to the host terminal's clipboard, including over SSH, without reading it.
use base64::{engine::general_purpose::STANDARD, Engine};
use std::io::{self, IsTerminal, Write};

pub(super) fn copy(value: &str) -> io::Result<()> {
    let stdout = io::stdout();
    if !stdout.is_terminal() {
        return Err(io::Error::other("复制需要支持 OSC 52 的交互式终端。"));
    }
    write_clipboard(
        &mut stdout.lock(),
        value,
        std::env::var_os("TMUX").is_some(),
    )
}

fn write_clipboard(writer: &mut impl Write, value: &str, tmux: bool) -> io::Result<()> {
    let encoded = STANDARD.encode(value.as_bytes());
    if tmux {
        // tmux passthrough doubles the embedded ESC and ends with ST.
        write!(writer, "\x1bPtmux;\x1b\x1b]52;c;{encoded}\x07\x1b\\")?;
    } else {
        write!(writer, "\x1b]52;c;{encoded}\x07")?;
    }
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_payload_preserves_unicode_markdown_and_newlines() {
        let text = "数据库 e\u{301} 👩‍💻\n```rust\nlet x = 1;\n```\n";
        for tmux in [false, true] {
            let mut output = vec![];
            write_clipboard(&mut output, text, tmux).unwrap();
            let (prefix, suffix): (&[u8], &[u8]) = if tmux {
                (b"\x1bPtmux;\x1b\x1b]52;c;", b"\x07\x1b\\")
            } else {
                (b"\x1b]52;c;", b"\x07")
            };
            let payload = output
                .strip_prefix(prefix)
                .unwrap()
                .strip_suffix(suffix)
                .unwrap();
            assert_eq!(STANDARD.decode(payload).unwrap(), text.as_bytes());
        }
    }

    #[test]
    fn clipboard_write_and_flush_errors_are_reported() {
        struct Failed {
            flush: bool,
        }
        impl Write for Failed {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.flush {
                    Ok(bytes.len())
                } else {
                    Err(io::ErrorKind::BrokenPipe.into())
                }
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
        }
        for flush in [false, true] {
            assert!(write_clipboard(&mut Failed { flush }, "hello", false).is_err());
        }
    }
}
