//! Copying text to the system clipboard.
//!
//! Prefers the desktop's clipboard tools, which keep the text available
//! after `tuimon` exits, and falls back to the OSC 52 escape sequence for
//! terminals that support it (useful over SSH).

use std::env;
use std::io::{self, Write};
use std::process::{Command, Stdio};

/// Clipboard tools tried in order: (program, arguments, display variable
/// the tool needs).
const TOOLS: [(&str, &[&str], &str); 3] = [
    ("wl-copy", &[], "WAYLAND_DISPLAY"),
    ("xclip", &["-selection", "clipboard"], "DISPLAY"),
    ("xsel", &["--clipboard", "--input"], "DISPLAY"),
];

/// How the text was copied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Tool(&'static str),
    /// Sent to the terminal, which may or may not support it.
    Osc52,
}

pub fn copy(text: &str) -> io::Result<Method> {
    for (program, args, display) in TOOLS {
        if env::var_os(display).is_some() && run(program, args, text).is_ok() {
            return Ok(Method::Tool(program));
        }
    }
    let mut stdout = io::stdout();
    write!(stdout, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    stdout.flush()?;
    Ok(Method::Osc52)
}

fn run(program: &str, args: &[&str], text: &str) -> io::Result<()> {
    // The tools fork a background process that serves the clipboard, so
    // keep it off the terminal.
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    } // Dropping stdin sends EOF.
    if child.wait()?.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{program} failed")))
    }
}

fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let bytes = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 0x3f) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"192.168.2.25"), "MTkyLjE2OC4yLjI1");
    }
}
