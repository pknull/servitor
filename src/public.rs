//! Rules for the text Servitor publishes about its own tools.
//!
//! Tool and provider names reach the feed in span names, denial details and
//! the manifest; tool descriptions reach the manifest. Names a server or
//! agent advertises are accepted only as bounded identifiers, and
//! descriptions are published as bounded single-line text.

/// Longest name accepted as a public identifier, in bytes.
const MAX_NAME_LEN: usize = 64;

/// Longest description published, in characters.
const MAX_DESCRIPTION_CHARS: usize = 200;

/// Whether `name` may be published as a tool, provider or agent name: 1 to 64
/// ASCII letters, digits, `_`, `.` or `-`, starting with a letter or digit.
pub fn is_public_name(name: &str) -> bool {
    name.len() <= MAX_NAME_LEN
        && name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// A description as the manifest publishes it: whitespace runs collapse to
/// one space, control and invisible formatting characters are dropped, and
/// the text stops at 200 characters.
pub fn public_description(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control() && !is_invisible_format(*c))
        .take(MAX_DESCRIPTION_CHARS)
        .collect()
}

/// Zero-width and bidirectional formatting characters, which can make
/// published text read differently from its bytes.
fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_names_are_bounded_identifiers() {
        for name in ["execute", "read.file-v2", "a", "Docker_ps", &"x".repeat(64)] {
            assert!(is_public_name(name), "{name:?}");
        }
        for name in [
            "",
            "_hidden",
            "bad name",
            "line\nbreak",
            "tab\t",
            "ünï",
            &"x".repeat(65),
        ] {
            assert!(!is_public_name(name), "{name:?}");
        }
    }

    #[test]
    fn public_descriptions_are_single_line_and_bounded() {
        assert_eq!(
            public_description("  Run a\n\tcommand\r\n now  "),
            "Run a command now"
        );
        assert_eq!(
            public_description("safe\u{202E}txt\u{200B}\u{7}"),
            "safetxt"
        );
        assert_eq!(public_description(&"é".repeat(500)).chars().count(), 200);
    }
}
