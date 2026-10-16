//! Argument sanitization for trace spans.
//!
//! Implements security-conscious sanitization rules from component-model.md:
//! - Redacts fields matching sensitive patterns: *key*, *secret*, *password*, *token*, *credential*, *api_key*
//! - Scans string values for credential-shaped content (authorization headers,
//!   `key=value` secret forms, known token formats, high-entropy tokens)
//! - Truncates values longer than 1KB to prevent log flooding
//! - Caps the serialized arguments attribute of one span at 4KB
//!
//! Spans enter the append-only, replicated feed, so a missed secret is
//! permanent: every rule here prefers over-redaction to a miss.

use regex::{Captures, Regex};
use serde_json::{Map, Value};
use std::ops::Range;
use std::sync::OnceLock;

/// Maximum length for a single string value before truncation.
const MAX_VALUE_LENGTH: usize = 1024;

/// Maximum serialized size, in bytes, of the arguments attribute on one span.
pub const MAX_SPAN_ARGUMENTS_BYTES: usize = 4096;

/// Minimum alphanumeric length before a token is judged by the entropy check.
const MIN_RANDOM_TOKEN_ALNUM: usize = 20;

/// Character-class transition rate at or above which a token reads as random.
const RANDOM_TRANSITION_RATE: f64 = 0.25;

/// Lower transition rate accepted when Shannon entropy is also high.
const RANDOM_TRANSITION_RATE_WITH_ENTROPY: f64 = 0.2;

/// Shannon entropy (bits per character) paired with the lower transition rate.
const RANDOM_ENTROPY_BITS: f64 = 3.8;

/// Secret-bearing name fragments for `key=value` forms and `--flag value` pairs.
const SECRET_NAME: &str = r"(?:password|passwd|secret|token|api[_-]?key|apikey|access[_-]?key|private[_-]?key|credentials?|signature)";

/// Maximum nesting depth for recursive sanitization to prevent stack overflow.
const MAX_DEPTH: usize = 10;

/// Maximum array elements to process before truncating.
const MAX_ARRAY_SIZE: usize = 100;

/// Redaction marker for sensitive fields.
const REDACTED: &str = "[REDACTED]";

/// Sibling fields that name the program an argument list runs under, as in
/// `{"command": "mysql", "args": ["-phunter2"]}`.
const COMMAND_FIELDS: &[&str] = &["command", "cmd", "program", "executable"];

/// Patterns that indicate sensitive field names (case-insensitive matching).
/// NOTE: Keep aligned with egregore/src/telemetry.rs SENSITIVE_PATTERNS.
const SENSITIVE_PATTERNS: &[&str] = &[
    "key",
    "secret",
    "password",
    "token",
    "credential",
    "auth",
    "bearer",
    "session",
    "private",
    "cookie",
];

/// Sanitize tool arguments for safe inclusion in trace spans.
///
/// Applies the following transformations:
/// 1. Redacts any field whose name contains sensitive patterns
/// 2. Truncates string values longer than 1KB
/// 3. Recursively processes nested objects and arrays (with depth limit)
/// 4. Truncates arrays longer than 100 elements
///
/// # Arguments
/// * `arguments` - The raw JSON arguments from a tool call
///
/// # Returns
/// A sanitized JSON string suitable for span attributes
pub fn sanitize_arguments(arguments: &Value) -> String {
    let sanitized = sanitize_argument_value(arguments);
    serde_json::to_string(&sanitized).unwrap_or_else(|_| "{}".to_string())
}

/// Sanitize tool arguments, returning the sanitized JSON value.
///
/// Same rules as [`sanitize_arguments`]; use this when the result is
/// embedded in another JSON document.
pub fn sanitize_argument_value(arguments: &Value) -> Value {
    sanitize_value(arguments, None, 0, None)
}

/// Build the `arguments` attribute for a tool-call trace span.
///
/// Returns the sanitized arguments as structured JSON when their compact
/// serialization fits in [`MAX_SPAN_ARGUMENTS_BYTES`]. Larger arguments
/// become a string preview of the sanitized JSON text, ending in a
/// truncation marker and sized so that the attribute, serialized, still
/// fits the cap.
pub fn span_arguments_attribute(arguments: &Value) -> Value {
    let sanitized = sanitize_argument_value(arguments);
    let serialized = serde_json::to_string(&sanitized).unwrap_or_else(|_| "{}".to_string());
    if serialized.len() <= MAX_SPAN_ARGUMENTS_BYTES {
        return sanitized;
    }

    let marker = format!("... [truncated, {} bytes total]", serialized.len());
    // Two bytes for the quotes around the preview string.
    let budget = MAX_SPAN_ARGUMENTS_BYTES.saturating_sub(marker.len() + 2);
    let mut end = serialized.floor_char_boundary(budget);
    loop {
        let preview = Value::String(format!("{}{marker}", &serialized[..end]));
        let encoded_len = serde_json::to_string(&preview).map_or(usize::MAX, |s| s.len());
        if encoded_len <= MAX_SPAN_ARGUMENTS_BYTES || end == 0 {
            return preview;
        }
        // Escaping quotes and backslashes grew the preview: drop the overflow.
        let overflow = encoded_len - MAX_SPAN_ARGUMENTS_BYTES;
        end = serialized.floor_char_boundary(end.saturating_sub(overflow));
    }
}

/// Recursively sanitize a JSON value.
///
/// # Arguments
/// * `value` - The value to sanitize
/// * `field_name` - The field name if this value is part of an object (for sensitivity checking)
/// * `depth` - Current recursion depth (to prevent stack overflow on deeply nested structures)
/// * `command` - A sibling field naming the program an argument list runs under
fn sanitize_value(
    value: &Value,
    field_name: Option<&str>,
    depth: usize,
    command: Option<&str>,
) -> Value {
    // Prevent stack overflow on deeply nested structures
    if depth > MAX_DEPTH {
        return Value::String("[TRUNCATED: max depth exceeded]".to_string());
    }

    // Check if this field should be redacted based on its name
    if let Some(name) = field_name {
        if is_sensitive_field(name) {
            return Value::String(REDACTED.to_string());
        }
    }

    match value {
        Value::Object(map) => {
            let sibling_command = COMMAND_FIELDS
                .iter()
                .find_map(|field| map.get(*field).and_then(Value::as_str));
            let sanitized: Map<String, Value> = map
                .iter()
                .map(|(k, v)| {
                    let value = sanitize_value(v, Some(k), depth + 1, sibling_command);
                    (k.clone(), value)
                })
                .collect();
            Value::Object(sanitized)
        }
        Value::Array(arr) => {
            let total_len = arr.len();
            let truncated = total_len > MAX_ARRAY_SIZE;
            let items_to_process = if truncated { MAX_ARRAY_SIZE } else { total_len };
            let items = &arr[..items_to_process];

            // A secret-named flag element ("--password") redacts the element
            // after it, which holds its value in exec-style argument lists.
            // Short-flag passwords ("-p", "hunter2") depend on the command,
            // so the list is scanned as one command line.
            let flag_secrets = argv_short_flag_secrets(items, command);
            let mut sanitized: Vec<Value> = Vec::with_capacity(items_to_process + 1);
            let mut redact_next = false;
            for (v, secrets) in items.iter().zip(flag_secrets) {
                let is_flag_value = redact_next && !v.as_str().is_some_and(|s| s.starts_with('-'));
                redact_next = v.as_str().is_some_and(is_secret_flag);
                if is_flag_value {
                    sanitized.push(Value::String(REDACTED.to_string()));
                } else if let Some(s) = v
                    .as_str()
                    .filter(|_| !secrets.is_empty() && depth < MAX_DEPTH)
                {
                    sanitized.push(sanitize_string(&redact_ranges(s, &secrets), s.len()));
                } else {
                    sanitized.push(sanitize_value(v, None, depth + 1, None));
                }
            }

            if truncated {
                sanitized.push(Value::String(format!(
                    "[TRUNCATED: {} of {} items shown]",
                    items_to_process, total_len
                )));
            }
            Value::Array(sanitized)
        }
        Value::String(s) => sanitize_string(s, s.len()),
        // Numbers, booleans, and nulls pass through unchanged
        _ => value.clone(),
    }
}

/// Scan a string value for credentials, then truncate it to the value cap.
/// `original_len` is the length reported in the truncation marker.
fn sanitize_string(s: &str, original_len: usize) -> Value {
    // Scan the whole value before truncating. A secret straddling the cut
    // would otherwise leave a prefix too short to recognize, and redactions
    // can pull text from anywhere in the value into the kept prefix, so no
    // shorter scan window is safe. Every pattern is linear time, so the scan
    // stays linear in the value's length.
    let scanned = redact_credential_values(s);
    if scanned.len() > MAX_VALUE_LENGTH {
        let end = scanned.floor_char_boundary(MAX_VALUE_LENGTH);
        Value::String(format!(
            "{}... [truncated, {original_len} bytes total]",
            &scanned[..end]
        ))
    } else {
        Value::String(scanned)
    }
}

/// Maximum length for tool result content before truncation.
/// Tool results can be quite large; truncate to prevent log flooding.
const MAX_TOOL_RESULT_LENGTH: usize = 8192;

/// Patterns that indicate sensitive data in tool output (all lowercase for matching).
const SENSITIVE_VALUE_PATTERNS: &[&str] = &[
    "password=",
    "password:",
    "token=",
    "token:",
    "secret=",
    "secret:",
    "api_key=",
    "apikey=",
    "bearer ",
    "authorization:",
    "-----begin",
    "-----end",
];

/// Sanitize tool result content for safe inclusion in logs and LLM context.
///
/// Applies the following transformations:
/// 1. Truncates content longer than 8KB
/// 2. Redacts lines containing sensitive patterns
/// 3. Redacts entire PEM blocks (private keys, certificates)
///
/// # Arguments
/// * `content` - The raw tool result content
///
/// # Returns
/// A sanitized string safe for logging
pub fn sanitize_tool_result(content: &str) -> String {
    let lower = content.to_lowercase();

    // Check for PEM-encoded content (private keys, certificates) - redact entire content
    if lower.contains("-----begin") && lower.contains("-----end") {
        return "[REDACTED: PEM-encoded sensitive content]".to_string();
    }

    // Check for other sensitive patterns and redact affected lines
    let mut result = content.to_string();
    let mut needs_check = true;

    while needs_check {
        needs_check = false;
        let current_lower = result.to_lowercase();

        for pattern in SENSITIVE_VALUE_PATTERNS {
            if current_lower.contains(pattern) {
                result = result
                    .lines()
                    .map(|line| {
                        if line.to_lowercase().contains(pattern) {
                            "[REDACTED: sensitive content]"
                        } else {
                            line
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                needs_check = true; // Recheck in case redaction revealed more patterns
                break;
            }
        }
    }

    // Then truncate if too long
    if result.len() > MAX_TOOL_RESULT_LENGTH {
        let mut end = MAX_TOOL_RESULT_LENGTH;
        while end > 0 && !result.is_char_boundary(end) {
            end -= 1;
        }
        format!(
            "{}... [truncated, {} bytes total]",
            &result[..end],
            content.len()
        )
    } else {
        result
    }
}

/// Check if a field name matches any sensitive patterns.
///
/// Uses case-insensitive substring matching.
fn is_sensitive_field(name: &str) -> bool {
    let lower = name.to_lowercase();
    SENSITIVE_PATTERNS
        .iter()
        .any(|pattern| lower.contains(pattern))
}

/// Redact credential-shaped content inside a string value.
///
/// Field-name redaction misses a secret carried in an innocent field, such
/// as `"command": "curl -H 'Authorization: Bearer …'"`. This scans the text
/// itself, in order, for:
/// 1. PEM private key blocks, including one cut off before its END line
/// 2. Known token formats (GitHub, GitLab, Slack, OpenAI/Anthropic, Stripe,
///    AWS access key ids, Google API keys, npm, JWTs)
/// 3. `Authorization:` / `Proxy-Authorization:` header values
/// 4. `Bearer <token>`
/// 5. Passwords in URL userinfo (`scheme://user:password@host`)
/// 6. `key`, `sig` and `code` query parameters
/// 7. Short-flag passwords of known commands (see [`short_flag_rules`])
/// 8. `name=value` / `name: value` forms whose name carries a secret word
/// 9. `--flag value` pairs whose flag carries a secret word
/// 10. High-entropy tokens (see [`randomness`])
///
/// Over-redaction is deliberate: hex digests, UUIDs and long identifiers
/// mixing case and digits may read as random and be redacted.
pub fn redact_credential_values(text: &str) -> String {
    let mut out = text.to_string();
    for (pattern, replacement) in fixed_credential_patterns() {
        out = pattern.replace_all(&out, *replacement).into_owned();
    }
    out = redact_ranges(&out, &short_flag_secret_ranges(&out));
    out = secret_assignment_pattern()
        .replace_all(&out, redact_keeping_quotes)
        .into_owned();
    out = secret_flag_pattern()
        .replace_all(&out, |caps: &Captures| {
            // `--password-stdin` is a switch; the word after it is no value.
            if caps[1].trim_end().to_ascii_lowercase().ends_with("-stdin") {
                caps[0].to_string()
            } else {
                redact_keeping_quotes(caps)
            }
        })
        .into_owned();
    redact_random_tokens(&out)
}

/// Patterns whose replacement is a fixed template.
fn fixed_credential_patterns() -> &'static [(Regex, &'static str)] {
    static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            (
                r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY[A-Z0-9 ]*-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY[A-Z0-9 ]*-----|\z)",
                REDACTED,
            ),
            (
                r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|glpat-[A-Za-z0-9_-]{20,}|xox[abposr]-[A-Za-z0-9-]{10,}|sk-[A-Za-z0-9_-]{20,}|[sr]k_(?:live|test)_[A-Za-z0-9]{16,}|A(?:KIA|SIA)[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{35}|npm_[A-Za-z0-9]{36}|eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]*)",
                REDACTED,
            ),
            (
                r#"(?i)\b((?:proxy-)?authorization["']?\s*[:=]\s*["']?)[^\r\n"']+"#,
                "${1}[REDACTED]",
            ),
            (r"(?i)\b(bearer\s+)[A-Za-z0-9._~+/=-]{8,}", "${1}[REDACTED]"),
            (
                r#"(?i)\b([a-z][a-z0-9+.-]*://[^\s/?#@:"']*:)[^\s/?#@"']+@"#,
                "${1}[REDACTED]@",
            ),
            (r#"(?i)([?&](?:key|sig|code)=)[^&#\s"']+"#, "${1}[REDACTED]"),
        ]
        .into_iter()
        .map(|(pattern, replacement)| (Regex::new(pattern).unwrap(), replacement))
        .collect()
    })
}

/// `name=value` or `name: value` where the name carries a secret word.
/// Group 1 is the name and separator, group 2 the value.
fn secret_assignment_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(&format!(
            r#"(?i)([a-z0-9_.-]*{SECRET_NAME}[a-z0-9_.-]*["']?\s*[:=]\s*)("[^"]*"|'[^']*'|[^\s"'&;,]+)"#
        ))
        .unwrap()
    })
}

/// `--flag value` where the flag starts a word and carries a secret word, and
/// the value is not itself a flag. Group 1 is the flag (with the character
/// before it) and whitespace, group 2 the value.
fn secret_flag_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(&format!(
            r#"(?i)((?:^|[\s"'])--?[a-z0-9_-]*{SECRET_NAME}[a-z0-9_-]*\s+)("[^"]*"|'[^']*'|[^\s"'-][^\s"']*)"#
        ))
        .unwrap()
    })
}

/// Whether a whole argument is a secret-named flag taking a value, such as
/// `--password` (but not the `--password-stdin` switch).
fn is_secret_flag(arg: &str) -> bool {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN
        .get_or_init(|| {
            Regex::new(&format!(r"(?i)^--?[a-z0-9_-]*{SECRET_NAME}[a-z0-9_-]*$")).unwrap()
        })
        .is_match(arg)
        && !arg.to_ascii_lowercase().ends_with("-stdin")
}

/// What a short flag's value holds.
#[derive(Clone, Copy)]
enum FlagSecret {
    /// The whole value is a password.
    Password,
    /// `user:password`; only the part after the first `:` is secret.
    UserPassword,
}

/// Commands whose short flags carry passwords, where the same flag means
/// something else to other tools (`-p` is a port to `ssh` and `docker run`).
///
/// Each rule is a regex whose group 1 holds one invocation's arguments, up
/// to a shell separator outside quotes; a regex run over those arguments
/// whose group 1 is a flag value; and what that value holds. The command may
/// appear anywhere, so `docker exec db mysql …` and `sh -c 'mysql …'` match.
fn short_flag_rules() -> &'static [(Regex, Regex, FlagSecret)] {
    static RULES: OnceLock<Vec<(Regex, Regex, FlagSecret)>> = OnceLock::new();
    RULES.get_or_init(|| {
        let value = r#"("[^"]*"|'[^']*'|\S+)"#;
        let args = r#"(\s(?:"[^"]*"|'[^']*'|[^;|&\n])*)"#;
        [
            // MySQL and MariaDB clients: `-pPASSWORD`, attached only, since
            // `-p name` prompts for the password and names a database.
            (
                format!(r"\b(?:mysql\w*|mariadb[\w-]*){args}"),
                format!(r"\s-p{value}"),
                FlagSecret::Password,
            ),
            // Registry logins: `-p PASSWORD`, `-pPASSWORD`, `-p=PASSWORD`.
            (
                format!(
                    r"\b(?:docker|podman|nerdctl|buildah|skopeo)(?:\s+-[\w-]+(?:=\S+|\s+[^-\s]\S*)?)*\s+login{args}"
                ),
                format!(r"\s-p(?:=|[ \t]+)?{value}"),
                FlagSecret::Password,
            ),
            // sshpass: only its own options, before the command it wraps,
            // whose `-p` may be a port.
            (
                r#"\bsshpass((?:\s+(?:-[fdP][ \t]*(?:"[^"]*"|'[^']*'|\S+)|-e\S*|-[hvV]+))*\s+-p[ \t]*(?:"[^"]*"|'[^']*'|\S+))"#
                    .to_string(),
                format!(r"\s-p[ \t]*{value}"),
                FlagSecret::Password,
            ),
            // curl: `-u`/`--user` and `-U`/`--proxy-user` take
            // `user:password`, also at the end of a short-flag cluster.
            (
                format!(r"\bcurl{args}"),
                format!(r"\s(?:-[a-zA-Z]*[uU][ \t]*|--(?:proxy-)?user(?:=|[ \t]+)){value}"),
                FlagSecret::UserPassword,
            ),
        ]
        .into_iter()
        .map(|(command, flag, secret)| {
            (
                Regex::new(&command).unwrap(),
                Regex::new(&flag).unwrap(),
                secret,
            )
        })
        .collect()
    })
}

/// Sorted, disjoint byte ranges of short-flag passwords in `text`.
fn short_flag_secret_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    for (command, flag, secret) in short_flag_rules() {
        for invocation in command.captures_iter(text) {
            let args = invocation.get(1).unwrap();
            for caps in flag.captures_iter(args.as_str()) {
                let value = caps.get(1).unwrap().range();
                let value =
                    without_quotes(text, args.start() + value.start..args.start() + value.end);
                let start = match secret {
                    FlagSecret::Password => value.start,
                    FlagSecret::UserPassword => match text[value.clone()].find(':') {
                        Some(colon) => value.start + colon + 1,
                        None => continue,
                    },
                };
                if start < value.end {
                    ranges.push(start..value.end);
                }
            }
        }
    }
    merge_ranges(ranges)
}

/// Short-flag password ranges for each element of an exec-style argument
/// list, found by scanning the elements joined as one command line after
/// `command`, the program a sibling field names.
///
/// Inside an element, whitespace, quotes and shell separators are literal,
/// so the joined line carries one `_` per byte of each; that keeps every
/// element one token and every byte offset unchanged.
fn argv_short_flag_secrets(items: &[Value], command: Option<&str>) -> Vec<Vec<Range<usize>>> {
    let mut line = command.unwrap_or_default().to_string();
    let mut spans = Vec::with_capacity(items.len());
    for item in items {
        line.push(' ');
        let start = line.len();
        match item.as_str().filter(|s| !s.is_empty()) {
            Some(s) => {
                for c in s.chars() {
                    if c.is_whitespace() || matches!(c, '"' | '\'' | ';' | '|' | '&') {
                        line.extend(std::iter::repeat_n('_', c.len_utf8()));
                    } else {
                        line.push(c);
                    }
                }
                spans.push(Some(start..line.len()));
            }
            None => {
                line.push('_');
                spans.push(None);
            }
        }
    }

    let secrets = short_flag_secret_ranges(&line);
    spans
        .into_iter()
        .map(|span| {
            let Some(span) = span else {
                return Vec::new();
            };
            secrets
                .iter()
                .filter_map(|secret| {
                    let start = secret.start.max(span.start);
                    let end = secret.end.min(span.end);
                    (start < end).then(|| start - span.start..end - span.start)
                })
                .collect()
        })
        .collect()
}

/// `range` without its surrounding quotes, when it is a quoted string.
fn without_quotes(text: &str, range: Range<usize>) -> Range<usize> {
    let bytes = &text.as_bytes()[range.clone()];
    match (bytes.first(), bytes.last()) {
        (Some(&first @ (b'"' | b'\'')), Some(&last)) if bytes.len() >= 2 && first == last => {
            range.start + 1..range.end - 1
        }
        _ => range,
    }
}

/// Sort ranges and merge any that overlap or touch.
fn merge_ranges(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// Replace each of the sorted, disjoint `ranges` of `text` with the marker.
/// A range that splits a character widens to cover it.
fn redact_ranges(text: &str, ranges: &[Range<usize>]) -> String {
    if ranges.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for range in ranges {
        let start = text.floor_char_boundary(range.start).max(last);
        let end = text.ceil_char_boundary(range.end).max(start);
        out.push_str(&text[last..start]);
        out.push_str(REDACTED);
        last = end;
    }
    out.push_str(&text[last..]);
    out
}

/// Replace capture group 2 with the redaction marker, keeping its quotes.
fn redact_keeping_quotes(caps: &Captures) -> String {
    let value = &caps[2];
    let quote = match value.chars().next() {
        Some(q @ ('"' | '\'')) => q.to_string(),
        _ => String::new(),
    };
    format!("{}{quote}{REDACTED}{quote}", &caps[1])
}

/// Redact tokens that read as random, and random segments of `/` paths.
///
/// A candidate is a maximal run of base64/URL-safe characters. A strongly
/// random run is redacted whole. Otherwise a run with `/` has its random
/// segments redacted, keeping the path around them; a run with no random
/// segment is redacted whole if it is weakly random (base64 whose `/`
/// splits it into segments too short to judge alone).
fn redact_random_tokens(text: &str) -> String {
    static TOKEN: OnceLock<Regex> = OnceLock::new();
    let token = TOKEN.get_or_init(|| Regex::new(r"[A-Za-z0-9+/=_-]+").unwrap());

    let mut ranges = Vec::new();
    for run in token.find_iter(text) {
        let whole = randomness(run.as_str());
        if whole == Randomness::Strong {
            ranges.push(run.range());
            continue;
        }
        let mut segment_hits = Vec::new();
        if run.as_str().contains('/') {
            let mut start = run.start();
            for segment in run.as_str().split('/') {
                if randomness(segment) != Randomness::None {
                    segment_hits.push(start..start + segment.len());
                }
                start += segment.len() + 1;
            }
        }
        if !segment_hits.is_empty() {
            ranges.extend(segment_hits);
        } else if whole == Randomness::Weak {
            ranges.push(run.range());
        }
    }
    redact_ranges(text, &ranges)
}

/// How strongly a token reads as randomly generated.
#[derive(Debug, PartialEq)]
enum Randomness {
    None,
    /// Judged random only with help from its Shannon entropy.
    Weak,
    /// Judged random by its class transition rate alone.
    Strong,
}

/// Judge whether a token reads as randomly generated rather than human-written.
///
/// Requires at least [`MIN_RANDOM_TOKEN_ALNUM`] alphanumeric characters with
/// both letters and digits. Randomness is judged by how often adjacent
/// characters switch class (lowercase, uppercase, digit), ignoring an
/// uppercase-to-lowercase switch, which starts every capitalized word.
/// Random base64 or hex switches on roughly half its characters; paths,
/// package names and CamelCase identifiers rarely do. Shannon entropy rescues
/// single-case random tokens, which switch class less often.
fn randomness(token: &str) -> Randomness {
    let alnum: Vec<u8> = token.bytes().filter(u8::is_ascii_alphanumeric).collect();
    if alnum.len() < MIN_RANDOM_TOKEN_ALNUM
        || !alnum.iter().any(u8::is_ascii_digit)
        || !alnum.iter().any(u8::is_ascii_alphabetic)
    {
        return Randomness::None;
    }

    let rate = class_transition_rate(token);
    if rate >= RANDOM_TRANSITION_RATE {
        Randomness::Strong
    } else if rate >= RANDOM_TRANSITION_RATE_WITH_ENTROPY
        && shannon_entropy(&alnum) >= RANDOM_ENTROPY_BITS
    {
        Randomness::Weak
    } else {
        Randomness::None
    }
}

/// Fraction of adjacent alphanumeric pairs that switch character class,
/// not counting uppercase-to-lowercase switches.
fn class_transition_rate(token: &str) -> f64 {
    #[derive(PartialEq)]
    enum Class {
        Lower,
        Upper,
        Digit,
    }
    let class = |b: u8| match b {
        b'a'..=b'z' => Some(Class::Lower),
        b'A'..=b'Z' => Some(Class::Upper),
        b'0'..=b'9' => Some(Class::Digit),
        _ => None,
    };

    let (mut pairs, mut switches) = (0usize, 0usize);
    for window in token.as_bytes().windows(2) {
        if let (Some(a), Some(b)) = (class(window[0]), class(window[1])) {
            pairs += 1;
            if a != b && !(a == Class::Upper && b == Class::Lower) {
                switches += 1;
            }
        }
    }
    if pairs == 0 {
        0.0
    } else {
        switches as f64 / pairs as f64
    }
}

/// Shannon entropy of a byte string, in bits per byte.
fn shannon_entropy(bytes: &[u8]) -> f64 {
    let mut counts = [0usize; 256];
    for &b in bytes {
        counts[b as usize] += 1;
    }
    let len = bytes.len() as f64;
    counts
        .iter()
        .filter(|&&count| count > 0)
        .map(|&count| {
            let p = count as f64 / len;
            -p * p.log2()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sanitize_redacts_api_key_field() {
        let args = json!({
            "command": "curl",
            "api_key": "sk-secret-12345"
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["command"], "curl");
        assert_eq!(parsed["api_key"], "[REDACTED]");
    }

    #[test]
    fn sanitize_redacts_password_field() {
        let args = json!({
            "username": "admin",
            "password": "hunter2"
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["username"], "admin");
        assert_eq!(parsed["password"], "[REDACTED]");
    }

    #[test]
    fn sanitize_redacts_token_field() {
        let args = json!({
            "auth_token": "bearer-xyz",
            "access_token": "oauth-abc"
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["auth_token"], "[REDACTED]");
        assert_eq!(parsed["access_token"], "[REDACTED]");
    }

    #[test]
    fn sanitize_redacts_secret_field() {
        let args = json!({
            "client_secret": "shh",
            "secret_key": "double-secret"
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["client_secret"], "[REDACTED]");
        assert_eq!(parsed["secret_key"], "[REDACTED]");
    }

    #[test]
    fn sanitize_redacts_credential_field() {
        let args = json!({
            "credentials": "user:pass",
            "db_credential": "postgres://..."
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["credentials"], "[REDACTED]");
        assert_eq!(parsed["db_credential"], "[REDACTED]");
    }

    #[test]
    fn sanitize_redacts_key_variations() {
        let args = json!({
            "apikey": "key1",
            "API_KEY": "key2",
            "openai_key": "key3",
            "private_key": "key4"
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["apikey"], "[REDACTED]");
        assert_eq!(parsed["API_KEY"], "[REDACTED]");
        assert_eq!(parsed["openai_key"], "[REDACTED]");
        assert_eq!(parsed["private_key"], "[REDACTED]");
    }

    #[test]
    fn sanitize_truncates_long_strings() {
        let long_content = "x".repeat(2000);
        let args = json!({
            "content": long_content
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        let content = parsed["content"].as_str().unwrap();
        assert!(content.contains("[truncated, 2000 bytes total]"));
        assert!(content.len() < 1200); // Should be around 1024 + marker
    }

    #[test]
    fn sanitize_handles_nested_objects() {
        let args = json!({
            "config": {
                "url": "https://example.com",
                "api_key": "secret-nested"
            }
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["config"]["url"], "https://example.com");
        assert_eq!(parsed["config"]["api_key"], "[REDACTED]");
    }

    #[test]
    fn sanitize_handles_arrays() {
        let args = json!({
            "items": ["safe", "also-safe"],
            "tokens": ["should-not-be-redacted"]
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        // Array values themselves are not redacted - only the field "tokens" would be
        assert_eq!(parsed["tokens"], "[REDACTED]");
        assert_eq!(parsed["items"][0], "safe");
    }

    #[test]
    fn sanitize_preserves_numbers_and_booleans() {
        let args = json!({
            "count": 42,
            "enabled": true,
            "ratio": 1.5
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["count"], 42);
        assert_eq!(parsed["enabled"], true);
        assert_eq!(parsed["ratio"], 1.5);
    }

    #[test]
    fn sanitize_handles_null() {
        let args = json!({
            "optional": null
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert!(parsed["optional"].is_null());
    }

    #[test]
    fn sanitize_handles_empty_object() {
        let args = json!({});
        let result = sanitize_arguments(&args);
        assert_eq!(result, "{}");
    }

    #[test]
    fn sanitize_case_insensitive_matching() {
        let args = json!({
            "API_KEY": "upper",
            "Api_Key": "mixed",
            "api_key": "lower"
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["API_KEY"], "[REDACTED]");
        assert_eq!(parsed["Api_Key"], "[REDACTED]");
        assert_eq!(parsed["api_key"], "[REDACTED]");
    }

    #[test]
    fn sanitize_truncates_utf8_safely() {
        // Each emoji is 4 bytes, so 500 emojis = 2000 bytes (over 1KB limit)
        let emoji_string = "🎉".repeat(500);
        let args = json!({
            "content": emoji_string
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        let content = parsed["content"].as_str().unwrap();
        assert!(content.contains("[truncated,"));
        // Verify the string is valid UTF-8 by attempting to use it
        let _ = content.to_string();
    }

    #[test]
    fn sanitize_truncates_deeply_nested_structures() {
        // Create a deeply nested structure (> MAX_DEPTH levels)
        let mut nested = json!({ "value": "deep" });
        for _ in 0..15 {
            nested = json!({ "nested": nested });
        }

        let result = sanitize_arguments(&nested);
        assert!(result.contains("[TRUNCATED: max depth exceeded]"));
    }

    #[test]
    fn sanitize_truncates_large_arrays() {
        // Create an array with > MAX_ARRAY_SIZE elements
        let large_array: Vec<i32> = (0..150).collect();
        let args = json!({
            "items": large_array
        });

        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        let items = parsed["items"].as_array().unwrap();
        // Should have 100 elements + 1 truncation marker
        assert_eq!(items.len(), 101);
        assert!(items
            .last()
            .unwrap()
            .as_str()
            .unwrap()
            .contains("[TRUNCATED:"));
        assert!(items
            .last()
            .unwrap()
            .as_str()
            .unwrap()
            .contains("100 of 150"));
    }

    #[test]
    fn sanitize_redacts_cookie_field() {
        let args = json!({
            "session_cookie": "abc123",
            "auth_cookie": "xyz789"
        });
        let result = sanitize_arguments(&args);
        let parsed: Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["session_cookie"], "[REDACTED]");
        assert_eq!(parsed["auth_cookie"], "[REDACTED]");
    }

    // Tests for value-level credential scanning

    /// Sanitize `{"command": text}` and return the sanitized command string.
    fn scan_command(text: &str) -> String {
        let sanitized = sanitize_argument_value(&json!({ "command": text }));
        sanitized["command"].as_str().unwrap().to_string()
    }

    fn assert_redacted(text: &str, secret: &str) {
        let out = scan_command(text);
        assert!(
            !out.contains(secret),
            "secret {secret:?} survived value scan: {out:?}"
        );
        assert!(out.contains(REDACTED), "no redaction marker in {out:?}");
    }

    #[test]
    fn value_scan_redacts_authorization_headers_in_strings() {
        assert_redacted(
            "curl -H 'Authorization: Bearer abc123def456' https://api.example.com/v1",
            "abc123def456",
        );
        assert_redacted(
            "curl -H \"Authorization: token gho0wn3dVal\" https://api.example.com",
            "gho0wn3dVal",
        );
        assert_redacted("Authorization: Basic dXNlcjpodW50ZXIy", "dXNlcjpodW50ZXIy");
        assert_redacted(
            "Proxy-Authorization: Basic cHJveHk6cGFzcw==",
            "cHJveHk6cGFzcw",
        );
        assert_redacted(
            r#"{"headers": {"Authorization": "Bearer abc123def456"}}"#,
            "abc123def456",
        );

        let out =
            scan_command("curl -H 'Authorization: Bearer abc123def456' https://api.example.com/v1");
        assert!(out.starts_with("curl -H 'Authorization: "));
        assert!(out.ends_with("' https://api.example.com/v1"));
    }

    #[test]
    fn value_scan_redacts_bare_bearer_tokens() {
        assert_redacted(
            "wget --header=Bearer:x; echo Bearer abc123def456",
            "abc123def456",
        );
        assert_redacted(
            "bearer eyJzdWIiOiIxMjM0NTY3ODkwIn0",
            "eyJzdWIiOiIxMjM0NTY3ODkwIn0",
        );
    }

    #[test]
    fn value_scan_redacts_key_value_secret_forms() {
        assert_redacted("PGPASSWORD=hunter2 psql -h db.internal app", "hunter2");
        assert_redacted("mysql --password=hunter2 -h db.internal", "hunter2");
        assert_redacted("export AWS_SECRET_ACCESS_KEY=wJalrXUtn", "wJalrXUtn");
        assert_redacted("GITHUB_TOKEN=plainvalue make release", "plainvalue");
        assert_redacted("client_secret: s3cr3tvalue", "s3cr3tvalue");
        assert_redacted("api-key = 'quoted secret value'", "quoted secret value");
        assert_redacted(r#"echo '{"password": "hunter2"}' | jq ."#, "hunter2");
        assert_redacted(
            "curl 'https://maps.example.com/api?q=x&key=plainvalue'",
            "plainvalue",
        );
        assert_redacted(
            "curl https://api.example.com/v1?api_key=plainvalue",
            "plainvalue",
        );

        let out = scan_command("PGPASSWORD=hunter2 psql -h db.internal app");
        assert_eq!(out, "PGPASSWORD=[REDACTED] psql -h db.internal app");
    }

    #[test]
    fn value_scan_redacts_space_separated_secret_flags() {
        assert_redacted("mysql --password hunter2 -h db.internal", "hunter2");
        assert_redacted("tool --api-key plainvalue --verbose", "plainvalue");
        assert_redacted("vault login --token \"quoted value\"", "quoted value");

        // A following flag is not a value.
        let out = scan_command("tool --no-password --verbose");
        assert_eq!(out, "tool --no-password --verbose");
    }

    #[test]
    fn value_scan_redacts_short_flag_passwords_for_known_commands() {
        let cases = [
            (
                "curl -u admin:hunter2 https://api.example.com",
                "curl -u admin:[REDACTED] https://api.example.com",
            ),
            (
                "curl --user admin:hunter2 https://api.example.com",
                "curl --user admin:[REDACTED] https://api.example.com",
            ),
            (
                "curl --user=admin:hunter2 https://api.example.com",
                "curl --user=admin:[REDACTED] https://api.example.com",
            ),
            (
                "curl -uadmin:hunter2 https://api.example.com",
                "curl -uadmin:[REDACTED] https://api.example.com",
            ),
            (
                "curl -fsSu 'admin:hunter 2' https://api.example.com",
                "curl -fsSu 'admin:[REDACTED]' https://api.example.com",
            ),
            (
                "curl -U proxy:hunter2 -x http://proxy:3128 https://example.com",
                "curl -U proxy:[REDACTED] -x http://proxy:3128 https://example.com",
            ),
            (
                "curl --proxy-user proxy:hunter2 https://example.com",
                "curl --proxy-user proxy:[REDACTED] https://example.com",
            ),
            (
                "mysql -h db.internal -uroot -phunter2 app",
                "mysql -h db.internal -uroot -p[REDACTED] app",
            ),
            (
                "mysqldump -uroot -p'hunter 2' app > app.sql",
                "mysqldump -uroot -p'[REDACTED]' app > app.sql",
            ),
            (
                "docker exec db mysql -uroot -phunter2 -e 'select 1; select 2'",
                "docker exec db mysql -uroot -p[REDACTED] -e 'select 1; select 2'",
            ),
            (
                "ssh db.internal 'mysqladmin -phunter2 status'",
                "ssh db.internal 'mysqladmin -p[REDACTED] status'",
            ),
            (
                "sshpass -p hunter2 ssh -p 22 deploy@host",
                "sshpass -p [REDACTED] ssh -p 22 deploy@host",
            ),
            (
                "sshpass -phunter2 scp -P 2222 file host:/tmp",
                "sshpass -p[REDACTED] scp -P 2222 file host:/tmp",
            ),
            (
                "sshpass -v -p 'hunter 2' ssh host",
                "sshpass -v -p '[REDACTED]' ssh host",
            ),
            (
                "docker login -u deploy -p hunter2 ghcr.io",
                "docker login -u deploy -p [REDACTED] ghcr.io",
            ),
            (
                "docker login registry.example.com -phunter2",
                "docker login registry.example.com -p[REDACTED]",
            ),
            (
                "docker --config /tmp/cfg login -p=hunter2 ghcr.io",
                "docker --config /tmp/cfg login -p=[REDACTED] ghcr.io",
            ),
            (
                "podman login -u deploy -p hunter2 quay.io",
                "podman login -u deploy -p [REDACTED] quay.io",
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(scan_command(text), expected, "input: {text}");
        }
    }

    #[test]
    fn value_scan_keeps_short_flags_that_are_not_passwords() {
        let commands = [
            // -p is a port, a prompt, or a different tool's flag here.
            "ssh -p 22 deploy@host",
            "scp -P 2222 file host:/tmp",
            "docker run -d --name web -p 8080:80 nginx:1.27",
            "docker run --rm -p 127.0.0.1:5432:5432 postgres:16",
            "mysql -h db.internal -P 3306 -u app -p appdb",
            "mkdir -p /srv/data && ls -pa /srv",
            // No password given: curl and docker prompt or read stdin.
            "curl -u admin https://api.example.com",
            "curl --user-agent 'probe:1.0' https://example.com",
            "docker login --username deploy --password-stdin ghcr.io",
            "cat token.txt | docker login -u deploy --password-stdin ghcr.io",
        ];
        for command in commands {
            assert_eq!(scan_command(command), command, "ordinary command altered");
        }
    }

    #[test]
    fn value_scan_redacts_short_flag_passwords_in_argument_arrays() {
        let cases = [
            (
                json!({ "argv": ["mysql", "-uroot", "-phunter2", "app"] }),
                json!({ "argv": ["mysql", "-uroot", "-p[REDACTED]", "app"] }),
            ),
            (
                json!({ "argv": ["sshpass", "-p", "hunter 2", "ssh", "-p", "22", "host"] }),
                json!({ "argv": ["sshpass", "-p", REDACTED, "ssh", "-p", "22", "host"] }),
            ),
            (
                json!({ "argv": ["curl", "-u", "admin:hunter2", "https://example.com"] }),
                json!({ "argv": ["curl", "-u", "admin:[REDACTED]", "https://example.com"] }),
            ),
            (
                json!({ "command": "docker", "args": ["login", "-u", "me", "-p", "hunter2"] }),
                json!({ "command": "docker", "args": ["login", "-u", "me", "-p", REDACTED] }),
            ),
            (
                json!({ "command": "/usr/bin/mysql", "args": ["-phunter2"] }),
                json!({ "command": "/usr/bin/mysql", "args": ["-p[REDACTED]"] }),
            ),
            (
                json!({ "argv": ["docker", "login", "--password-stdin", "ghcr.io"] }),
                json!({ "argv": ["docker", "login", "--password-stdin", "ghcr.io"] }),
            ),
            (
                json!({ "argv": ["ssh", "-p", "22", "host"] }),
                json!({ "argv": ["ssh", "-p", "22", "host"] }),
            ),
        ];
        for (args, expected) in cases {
            assert_eq!(sanitize_argument_value(&args), expected, "input: {args}");
        }
    }

    #[test]
    fn value_scan_maps_argument_array_secrets_across_multibyte_whitespace() {
        // A multibyte space inside an element must not shift the offsets
        // mapped back from the joined command line.
        let args = json!({ "argv": ["sshpass", "-p", "hun\u{3000}ter\u{a0}2", "ssh", "host"] });
        assert_eq!(
            sanitize_argument_value(&args),
            json!({ "argv": ["sshpass", "-p", REDACTED, "ssh", "host"] })
        );
        let args = json!({ "argv": ["mysql", "-e", "select\u{3000}1", "-phunter2"] });
        assert_eq!(
            sanitize_argument_value(&args),
            json!({ "argv": ["mysql", "-e", "select\u{3000}1", "-p[REDACTED]"] })
        );
    }

    #[test]
    fn argument_array_elements_past_max_depth_are_truncated() {
        // Elements with a short-flag secret obey the depth limit like the rest.
        let mut nested = json!(["mysql", "-phunter2"]);
        for _ in 0..MAX_DEPTH {
            nested = json!({ "n": nested });
        }
        let mut out = &sanitize_argument_value(&nested);
        for _ in 0..MAX_DEPTH {
            out = &out["n"];
        }
        let marker = "[TRUNCATED: max depth exceeded]";
        assert_eq!(out, &json!([marker, marker]));
    }

    #[test]
    fn value_scan_redacts_secret_flag_value_in_argument_array() {
        let args = json!({
            "command": "mysql",
            "args": ["--password", "hunter2", "-h", "db.internal"]
        });
        let sanitized = sanitize_argument_value(&args);
        assert_eq!(
            sanitized["args"],
            json!(["--password", REDACTED, "-h", "db.internal"])
        );
    }

    #[test]
    fn value_scan_redacts_strings_inside_arrays() {
        let args = json!({ "args": ["-H", "Authorization: Bearer abc123def456"] });
        let out = sanitize_arguments(&args);
        assert!(!out.contains("abc123def456"), "{out}");
    }

    #[test]
    fn value_scan_redacts_url_userinfo_passwords() {
        assert_redacted(
            "git clone https://deploy:hunter2@github.com/org/repo.git",
            "hunter2",
        );
        assert_redacted("psql postgres://admin:hunter2@db:5432/app", "hunter2");

        let out = scan_command("git clone https://deploy:hunter2@github.com/org/repo.git");
        assert!(out.contains("github.com/org/repo.git"), "{out}");
    }

    #[test]
    fn value_scan_redacts_known_token_formats() {
        let tokens = [
            "ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789",
            "github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyz",
            "glpat-abcdefghij0123456789",
            "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789",
            "sk-proj-abcdefghijklmnopqrstuvwxyz",
            "xoxb-123456789012-1234567890123-abcdefghij",
            "AKIAIOSFODNN7EXAMPLE",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
        ];
        for token in tokens {
            assert_redacted(&format!("deploy --with {token} now"), token);
        }
    }

    #[test]
    fn value_scan_redacts_high_entropy_tokens() {
        let tokens = [
            // Mixed-case alphanumeric
            "q8Zr3Lp0Xv7Nk2Wm5Ty9Bc4D",
            // Base64 with '/' and '+'
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "Zm9vYmFyYmF6+cXV4/Y29yZ2U9Z3JhdWx0eQ==",
            // Hex-encoded
            "a3f1c9e8b7d6052f4e3a1b9c8d7e6f50",
            // URL-safe base64
            "Ks9-Tq2_Lm8Np4Rv7Wx1Yz3Bc6Df0Gh5",
        ];
        for token in tokens {
            assert_redacted(&format!("deploy --release {token}"), token);
        }

        // Secrets embedded in a URL path segment.
        let webhook = "curl -X POST https://hooks.example.com/services/T0AB12CD3/B0EF45GH6/q8Zr3Lp0Xv7Nk2Wm5Ty9Bc4D";
        assert_redacted(webhook, "q8Zr3Lp0Xv7Nk2Wm5Ty9Bc4D");
        assert!(scan_command(webhook).starts_with("curl -X POST https://hooks.example."));

        // A random path segment is redacted alone; the path around it stays.
        assert_eq!(
            scan_command(
                "cat /srv/backups/servitor/nightly/IhvyGFJ7qXTi9OlPaHx166oJHCtEdwGy/manifest"
            ),
            "cat /srv/backups/servitor/nightly/[REDACTED]/manifest"
        );
    }

    #[test]
    fn value_scan_redacts_pem_private_keys() {
        let key = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----";
        assert_redacted(
            &format!("printf '%s' '{key}' > id"),
            "b3BlbnNzaC1rZXktdjEAAAAA",
        );

        // A key cut off before its END line is still redacted.
        let open = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEAtruncated";
        assert_redacted(open, "MIIEowIBAAKCAQEAtruncated");
    }

    #[test]
    fn value_scan_leaves_ordinary_commands_untouched() {
        let commands = [
            "ls -la /var/log",
            "git status --short",
            "git log --oneline -5",
            "git clone git@github.com:pknull/servitor.git",
            "git commit --author=\"Jane Doe <jane@example.com>\" -m 'fix: tidy'",
            "docker ps --format '{{.Names}}'",
            "docker run --rm -v /var/lib/docker/volumes:/data alpine:3.20 ls",
            "kubectl get pods -n kube-system --field-selector=status.phase=Running",
            "cargo test --all-features -- --test-threads=1",
            "grep -rn token src/ --include=*.rs",
            "systemctl restart prometheus-node-exporter.service",
            "cat /home/user/Code/Thallus/servitor/src/agent/direct.rs",
            "journalctl -u egregore --since 2026-10-15T19:00:00Z",
            "tar czf egregore-2.0.1-x86_64-unknown-linux-gnu.tar.gz dist/",
            "curl -fsSL https://example.com/install.sh -o /tmp/install.sh",
            "ffmpeg -i input_1920x1080_60fps.mp4 -c:v libx264 output_720p.mp4",
            "python3 -m pip install --upgrade setuptools==75.1.0",
            "echo the session is about to expire",
            "echo my-secret-recipe for cake",
            // CamelCase identifiers with digits switch class at word starts only.
            "cargo test --lib Http2ConnectionManagerFactory",
            "rg parseHTTP2ResponseHeaders src/ Base64EncodedPayloadReader",
        ];
        for command in commands {
            assert_eq!(scan_command(command), command, "ordinary command altered");
        }
    }

    #[test]
    fn value_scan_runs_before_truncation() {
        // The token straddles the 1KB truncation point; scanning a truncated
        // copy would see only a short, low-confidence prefix and let it through.
        let token = "q8Zr3Lp0Xv7Nk2Wm5Ty9Bc4D";
        let text = format!("{}{token}", "a ".repeat(506));
        assert!(text.len() > MAX_VALUE_LENGTH);
        let out = scan_command(&text);
        assert!(!out.contains(&token[..12]), "token prefix leaked: {out}");
    }

    #[test]
    fn value_scan_covers_text_shifted_into_kept_prefix_by_redaction() {
        // Redacting a ~4.6KB private key shrinks the value by more than 4KB,
        // pulling text from deep in the string into the kept 1KB. A token
        // cut by a bounded scan window (the old one ended at byte 5120)
        // leaves a prefix too short to recognize, which would leak.
        let key = format!(
            "-----BEGIN RSA PRIVATE KEY-----\n{}\n-----END RSA PRIVATE KEY-----",
            "A".repeat(4600)
        );
        let token = "ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789";
        let old_window_edge = 5120;
        let filler = " ".repeat(old_window_edge - 12 - key.len() - 1);
        let text = format!("{key} {filler}{token} tail");
        assert_eq!(&text[old_window_edge - 12..old_window_edge], &token[..12]);

        let out = scan_command(&text);
        assert!(!out.contains(&token[..12]), "token fragment leaked: {out}");
        assert!(!out.contains("AAAAAAAA"), "key material leaked: {out}");
    }

    #[test]
    fn value_scan_keeps_whole_value_when_redaction_shrinks_it_under_the_cap() {
        // The whole value is scanned, so a long value that redaction brings
        // under the cap loses nothing and carries no truncation marker.
        let key = format!(
            "-----BEGIN RSA PRIVATE KEY-----\n{}\n-----END RSA PRIVATE KEY-----",
            "A".repeat(6000)
        );
        let out = scan_command(&format!("{key} > id_rsa"));
        assert_eq!(out, format!("{REDACTED} > id_rsa"));
    }

    #[test]
    fn value_scan_truncates_long_redacted_value_on_char_boundary() {
        // A token beyond the kept prefix is redacted, then the result is cut
        // at the cap on a char boundary and marked with the original length.
        let text = format!(
            "{}{}",
            "🎉".repeat(300),
            " ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789"
        );
        let out = scan_command(&text);
        assert!(!out.contains("ghp_"), "{out}");
        assert!(
            out.ends_with(&format!("... [truncated, {} bytes total]", text.len())),
            "{out}"
        );
        assert_eq!(out.matches('🎉').count(), MAX_VALUE_LENGTH / 4);
    }

    // Tests for the span arguments attribute

    #[test]
    fn span_arguments_within_cap_stay_structured() {
        let args = json!({ "command": "ls -la", "api_key": "sk-secret" });
        let attribute = span_arguments_attribute(&args);
        assert_eq!(attribute, sanitize_argument_value(&args));
        assert_eq!(attribute["api_key"], REDACTED);
    }

    #[test]
    fn span_arguments_over_cap_are_truncated() {
        let mut args = Map::new();
        for i in 0..50 {
            args.insert(format!("field_{i}"), Value::String("x".repeat(1000)));
        }
        let attribute = span_arguments_attribute(&Value::Object(args));
        let serialized = serde_json::to_string(&attribute).unwrap();
        assert!(
            serialized.len() <= MAX_SPAN_ARGUMENTS_BYTES,
            "{}",
            serialized.len()
        );
        assert!(attribute.as_str().unwrap().contains("[truncated,"));
    }

    #[test]
    fn span_arguments_cap_accounts_for_json_escaping() {
        // Every '"' in the value grows when the preview is itself JSON-encoded.
        let mut args = Map::new();
        for i in 0..50 {
            args.insert(format!("field_{i}"), Value::String("\"\\".repeat(450)));
        }
        let attribute = span_arguments_attribute(&Value::Object(args));
        let serialized = serde_json::to_string(&attribute).unwrap();
        assert!(
            serialized.len() <= MAX_SPAN_ARGUMENTS_BYTES,
            "{}",
            serialized.len()
        );
    }

    #[test]
    fn span_arguments_cap_keeps_multibyte_text_valid() {
        let args = json!({ "content": "🎉".repeat(5000) });
        let attribute = span_arguments_attribute(&Value::Array(vec![args; 6]));
        let preview = attribute
            .as_str()
            .expect("over-cap arguments become a preview");
        assert!(preview.contains("🎉"));
        let serialized = serde_json::to_string(&attribute).unwrap();
        assert!(
            serialized.len() <= MAX_SPAN_ARGUMENTS_BYTES,
            "{}",
            serialized.len()
        );
    }

    // Tests for sanitize_tool_result

    #[test]
    fn tool_result_redacts_password() {
        let content = "Config loaded:\nuser=admin\npassword=secret123\nhost=localhost";
        let result = sanitize_tool_result(content);
        assert!(result.contains("[REDACTED: sensitive content]"));
        assert!(!result.contains("secret123"));
        assert!(result.contains("user=admin"));
    }

    #[test]
    fn tool_result_redacts_token() {
        let content = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9";
        let result = sanitize_tool_result(content);
        assert!(result.contains("[REDACTED: sensitive content]"));
        assert!(!result.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"));
    }

    #[test]
    fn tool_result_redacts_private_key() {
        let content =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----";
        let result = sanitize_tool_result(content);
        assert!(result.contains("[REDACTED: PEM-encoded sensitive content]"));
        assert!(!result.contains("MIIEowIBAAKCAQEA"));
    }

    #[test]
    fn tool_result_truncates_long_content() {
        let long_content = "x".repeat(10000);
        let result = sanitize_tool_result(&long_content);
        assert!(result.contains("[truncated, 10000 bytes total]"));
        assert!(result.len() < 9000);
    }

    #[test]
    fn tool_result_preserves_safe_content() {
        let content = "Operation completed successfully.\nFiles processed: 42\nStatus: OK";
        let result = sanitize_tool_result(content);
        assert_eq!(result, content);
    }
}
