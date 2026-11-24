use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const MAX_CONFIG_FILES_READ: usize = 256;
const MAX_CONFIG_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorRule {
    pub path: PathBuf,
    pub line_number: usize,
    pub raw: String,
    pub connector: String,
    pub mode: String,
    pub position: Option<String>,
    pub scale: Option<String>,
}

impl MonitorRule {
    pub fn normalized_rule(&self) -> String {
        match (&self.position, &self.scale) {
            (Some(position), Some(scale)) => {
                format!(
                    "monitor={},{},{},{}",
                    self.connector, self.mode, position, scale
                )
            }
            _ => format!("monitor={},{}", self.connector, self.mode),
        }
    }

    pub fn location(&self) -> String {
        format!("{}:{}", human_path(&self.path), self.line_number)
    }
}

pub fn human_path(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        if let Ok(relative) = path.strip_prefix(&home) {
            return Path::new("~").join(relative).display().to_string();
        }
    }
    path.display().to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorRuleInspection {
    pub root_path: PathBuf,
    pub expected_rule: String,
    pub files_read: usize,
    pub connector_rules: Vec<MonitorRule>,
    pub exact_match: Option<MonitorRule>,
    pub last_connector_rule: Option<MonitorRule>,
    pub read_warnings: Vec<String>,
}

impl MonitorRuleInspection {
    pub fn exact_match_is_effective(&self) -> bool {
        self.exact_match.as_ref().is_some_and(|exact| {
            self.last_connector_rule.as_ref().is_some_and(|last| {
                exact.path == last.path && exact.line_number == last.line_number
            })
        })
    }
}

pub fn inspect_monitor_rule(connector: &str, expected_rule: &str) -> MonitorRuleInspection {
    inspect_monitor_rule_from(default_hyprland_config_path(), connector, expected_rule)
}

pub fn inspect_connector_rules(connector: &str) -> MonitorRuleInspection {
    inspect_monitor_rule_from(
        default_hyprland_config_path(),
        connector,
        "__drmcru_no_expected_rule__",
    )
}

pub fn format_monitor_rule(connector: &str, mode: &str, position: &str, scale: &str) -> String {
    format_monitor_rule_for_path(
        &default_hyprland_config_path(),
        connector,
        mode,
        position,
        scale,
    )
}

fn format_monitor_rule_for_path(
    config_path: &Path,
    connector: &str,
    mode: &str,
    position: &str,
    scale: &str,
) -> String {
    if config_path
        .extension()
        .is_some_and(|extension| extension == "lua")
    {
        format!(
            "hl.monitor({{ output = {}, mode = {}, position = {}, scale = {} }})",
            lua_string(connector),
            lua_string(mode),
            lua_string(position),
            scale
        )
    } else {
        format!("monitor={connector},{mode},{position},{scale}")
    }
}

fn inspect_monitor_rule_from(
    root_path: PathBuf,
    connector: &str,
    expected_rule: &str,
) -> MonitorRuleInspection {
    let mut parser = ConfigParser::default();
    parser.read_file(&root_path);

    let normalized_expected = normalize_monitor_rule(expected_rule);

    let connector_rules = parser
        .monitor_rules
        .into_iter()
        .filter(|rule| rule.connector == connector)
        .collect::<Vec<_>>();
    let exact_match = connector_rules
        .iter()
        .rev()
        .find(|rule| {
            normalized_expected
                .as_ref()
                .is_some_and(|expected| rule.normalized_rule() == *expected)
        })
        .cloned();
    let last_connector_rule = connector_rules.last().cloned();

    MonitorRuleInspection {
        root_path,
        expected_rule: expected_rule.to_string(),
        files_read: parser.files_read,
        connector_rules,
        exact_match,
        last_connector_rule,
        read_warnings: parser.read_warnings,
    }
}

fn default_hyprland_config_path() -> PathBuf {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".config"))
        })
        .unwrap_or_else(|| PathBuf::from(".config"));
    let hypr = config_home.join("hypr");
    let lua = hypr.join("hyprland.lua");
    if lua.exists() {
        return lua;
    }

    hypr.join("hyprland.conf")
}

fn lua_string(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

#[derive(Default)]
struct ConfigParser {
    // Only active includes are guarded; source/dofile can load a file again.
    seen: BTreeSet<PathBuf>,
    required: BTreeSet<PathBuf>,
    files_read: usize,
    hit_file_limit: bool,
    monitor_rules: Vec<MonitorRule>,
    read_warnings: Vec<String>,
}

fn read_config_text(path: &Path) -> Result<String, String> {
    let meta = fs::metadata(path).map_err(|error| error.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".to_string());
    }
    if meta.len() > MAX_CONFIG_FILE_BYTES {
        return Err(format!("exceeds {MAX_CONFIG_FILE_BYTES} bytes"));
    }
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let open_meta = file.metadata().map_err(|error| error.to_string())?;
    if !open_meta.is_file() {
        return Err("not a regular file".to_string());
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_CONFIG_FILE_BYTES {
        return Err(format!("exceeds {MAX_CONFIG_FILE_BYTES} bytes"));
    }
    String::from_utf8(bytes).map_err(|error| format!("invalid UTF-8: {error}"))
}

impl ConfigParser {
    fn read_file(&mut self, path: &Path) {
        let display_path = path.to_path_buf();
        let seen_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if !self.seen.insert(seen_path.clone()) {
            return;
        }
        if self.files_read >= MAX_CONFIG_FILES_READ {
            if !self.hit_file_limit {
                self.hit_file_limit = true;
                self.read_warnings.push(format!(
                    "Stopped reading Hyprland config includes after {MAX_CONFIG_FILES_READ} files (skipped {})",
                    human_path(path)
                ));
            }
            self.seen.remove(&seen_path);
            return;
        }

        let contents = match read_config_text(path) {
            Ok(contents) => contents,
            Err(error) => {
                self.read_warnings
                    .push(format!("Could not read {}: {error}", human_path(path)));
                self.seen.remove(&seen_path);
                return;
            }
        };
        self.files_read += 1;

        if path.extension().is_some_and(|extension| extension == "lua") {
            for call in find_lua_calls(&contents) {
                match call.kind {
                    LuaCallKind::Monitor => {
                        if let Some(rule) =
                            parse_lua_monitor_rule(&display_path, &contents, call.start, call.end)
                        {
                            self.monitor_rules.push(rule);
                        }
                    }
                    LuaCallKind::DoFile => {
                        let Some(value) = parse_literal_lua_call_argument(call.raw(&contents))
                        else {
                            // Dynamic bootstrap expressions are common in Hyprland 0.56 and
                            // Omarchy. They cannot be resolved safely without executing Lua.
                            continue;
                        };
                        for source in
                            resolve_source_paths(&value, &display_path, &mut self.read_warnings)
                        {
                            self.read_file(&source);
                        }
                    }
                    LuaCallKind::Require => {
                        let Some(module) = parse_literal_lua_call_argument(call.raw(&contents))
                        else {
                            continue;
                        };
                        if let Some(source) = resolve_lua_require(&module, &display_path) {
                            let module_path =
                                fs::canonicalize(&source).unwrap_or_else(|_| source.clone());
                            if self.required.insert(module_path) {
                                self.read_file(&source);
                            }
                        }
                    }
                }
            }
            self.seen.remove(&seen_path);
            return;
        }

        for (index, raw_line) in contents.lines().enumerate() {
            let line_number = index + 1;
            let Some((key, value)) = parse_assignment(raw_line) else {
                continue;
            };

            match key {
                "monitor" => {
                    if let Some(rule) =
                        parse_monitor_rule(&display_path, line_number, raw_line, value)
                    {
                        self.monitor_rules.push(rule);
                    } else if value.contains('$') {
                        self.read_warnings.push(format!(
                            "Skipped dynamic monitor rule at {}:{line_number}",
                            human_path(&display_path)
                        ));
                    }
                }
                "source" => {
                    for source in
                        resolve_source_paths(value, &display_path, &mut self.read_warnings)
                    {
                        self.read_file(&source);
                    }
                }
                _ => {}
            }
        }
        self.seen.remove(&seen_path);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LuaCallKind {
    Monitor,
    DoFile,
    Require,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LuaCall {
    kind: LuaCallKind,
    start: usize,
    end: usize,
}

impl LuaCall {
    fn raw<'a>(&self, contents: &'a str) -> &'a str {
        &contents[self.start..=self.end]
    }
}

fn find_lua_calls(contents: &str) -> Vec<LuaCall> {
    let bytes = contents.as_bytes();
    let mut calls = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        if let Some(end) = skip_lua_comment_body(bytes, index) {
            index = end;
            continue;
        }
        if let Some((content_start, eq_count)) = lua_long_bracket_open(bytes, index) {
            index = find_lua_long_bracket_close(bytes, content_start, eq_count)
                .map(|close| close + eq_count + 2)
                .unwrap_or(bytes.len());
            continue;
        }
        if matches!(bytes[index], b'\'' | b'"') {
            index = skip_quoted_lua_string(bytes, index);
            continue;
        }

        let matched = [
            (b"hl.monitor".as_slice(), LuaCallKind::Monitor),
            (b"dofile".as_slice(), LuaCallKind::DoFile),
            (b"require".as_slice(), LuaCallKind::Require),
        ]
        .into_iter()
        .find(|(name, _)| {
            bytes[index..].starts_with(name)
                && (index == 0
                    || (!is_lua_identifier_byte(bytes[index - 1])
                        && !matches!(bytes[index - 1], b'.' | b':')))
                && bytes
                    .get(index + name.len())
                    .is_none_or(|byte| !is_lua_identifier_byte(*byte))
        });

        let Some((name, kind)) = matched else {
            index += 1;
            continue;
        };
        let open = skip_lua_whitespace_and_comments(bytes, index + name.len());
        let call_end = match bytes.get(open) {
            Some(&b'(') => match find_balanced_lua_delim(contents, open, b'(', b')') {
                Some(end) => Some(end),
                None => break,
            },
            Some(&b'{') if kind == LuaCallKind::Monitor => {
                match find_balanced_lua_delim(contents, open, b'{', b'}') {
                    Some(end) => Some(end),
                    None => break,
                }
            }
            Some(&(b'\'' | b'"')) if matches!(kind, LuaCallKind::DoFile | LuaCallKind::Require) => {
                match find_quoted_lua_string_end(bytes, open) {
                    Some(end) => Some(end),
                    None => break,
                }
            }
            Some(&b'[') if matches!(kind, LuaCallKind::DoFile | LuaCallKind::Require) => {
                if let Some((content_start, eq_count)) = lua_long_bracket_open(bytes, open) {
                    match find_lua_long_bracket_close(bytes, content_start, eq_count) {
                        Some(close) => Some(close + eq_count + 1),
                        None => break,
                    }
                } else {
                    None
                }
            }
            _ => None,
        };
        let Some(end) = call_end else {
            index += name.len();
            continue;
        };
        calls.push(LuaCall {
            kind,
            start: index,
            end,
        });
        index = end + 1;
    }

    calls
}

fn is_lua_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn find_bytes(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|relative| from + relative)
}

fn lua_long_bracket_open(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    if bytes.get(start) != Some(&b'[') {
        return None;
    }
    let mut index = start + 1;
    while bytes.get(index) == Some(&b'=') {
        index += 1;
    }
    if bytes.get(index) != Some(&b'[') {
        return None;
    }
    Some((index + 1, index - (start + 1)))
}

fn find_lua_long_bracket_close(
    bytes: &[u8],
    content_start: usize,
    eq_count: usize,
) -> Option<usize> {
    let mut close = Vec::with_capacity(eq_count + 2);
    close.push(b']');
    close.resize(1 + eq_count, b'=');
    close.push(b']');
    find_bytes(bytes, content_start, &close)
}

fn skip_lua_comment_body(bytes: &[u8], start: usize) -> Option<usize> {
    if !bytes.get(start..)?.starts_with(b"--") {
        return None;
    }
    if let Some((content_start, eq_count)) = lua_long_bracket_open(bytes, start + 2) {
        return Some(
            find_lua_long_bracket_close(bytes, content_start, eq_count)
                .map(|close| close + eq_count + 2)
                .unwrap_or(bytes.len()),
        );
    }
    Some(
        bytes[start + 2..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|relative| start + 2 + relative)
            .unwrap_or(bytes.len()),
    )
}

fn skip_lua_whitespace_and_comments(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
        } else if let Some(end) = skip_lua_comment_body(bytes, index) {
            index = end;
        } else {
            break;
        }
    }
    index
}

fn find_quoted_lua_string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let quote = *bytes.get(start)?;
    if !matches!(quote, b'\'' | b'"') {
        return None;
    }
    let mut index = start + 1;
    let mut escaped = false;
    while index < bytes.len() {
        if escaped {
            escaped = false;
        } else if bytes[index] == b'\\' {
            escaped = true;
        } else if bytes[index] == quote {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn skip_quoted_lua_string(bytes: &[u8], start: usize) -> usize {
    find_quoted_lua_string_end(bytes, start).map_or(bytes.len(), |end| end + 1)
}

fn parse_lua_monitor_rule(
    path: &Path,
    contents: &str,
    start: usize,
    end: usize,
) -> Option<MonitorRule> {
    let raw = &contents[start..=end];
    let fields = parse_lua_table_fields(raw);
    let connector = fields.iter().rfind(|(key, _)| key == "output")?;
    let mode = fields
        .iter()
        .rfind(|(key, _)| key == "mode")
        .map(|(_, value)| value.clone())
        .or_else(|| {
            fields
                .iter()
                .rfind(|(key, _)| key == "disabled")
                .is_some_and(|(_, value)| value == "true")
                .then(|| "disabled".to_string())
        })?;
    Some(MonitorRule {
        path: path.to_path_buf(),
        line_number: contents[..start]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1,
        raw: raw.to_string(),
        connector: connector.1.clone(),
        mode,
        position: fields
            .iter()
            .rfind(|(key, _)| key == "position")
            .map(|(_, value)| value.clone()),
        scale: fields
            .iter()
            .rfind(|(key, _)| key == "scale")
            .map(|(_, value)| value.clone()),
    })
}

fn find_balanced_lua_delim(
    contents: &str,
    open: usize,
    open_byte: u8,
    close_byte: u8,
) -> Option<usize> {
    let bytes = contents.as_bytes();
    let mut depth = 0usize;
    let mut index = open;
    while index < bytes.len() {
        if let Some(end) = skip_lua_comment_body(bytes, index) {
            index = end;
            continue;
        }
        if let Some((content_start, eq_count)) = lua_long_bracket_open(bytes, index) {
            index = find_lua_long_bracket_close(bytes, content_start, eq_count)? + eq_count + 2;
            continue;
        }
        if matches!(bytes[index], b'\'' | b'"') {
            index = find_quoted_lua_string_end(bytes, index)? + 1;
            continue;
        }
        if bytes[index] == open_byte {
            depth += 1;
        } else if bytes[index] == close_byte {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

fn parse_lua_table_fields(raw: &str) -> Vec<(String, String)> {
    let without_comments = strip_lua_comments(raw);
    let mut rest = without_comments.trim();
    if let Some(after_name) = rest.strip_prefix("hl.monitor") {
        rest = after_name.trim();
    }
    if rest.starts_with('(') {
        let Some(close) = find_balanced_lua_delim(rest, 0, b'(', b')') else {
            return Vec::new();
        };
        if close + 1 != rest.len() {
            return Vec::new();
        }
        rest = rest[1..close].trim();
    }
    if !rest.starts_with('{') {
        return Vec::new();
    }
    let Some(close) = find_balanced_lua_delim(rest, 0, b'{', b'}') else {
        return Vec::new();
    };
    if close + 1 != rest.len() {
        return Vec::new();
    }
    let table_body = &rest[1..close];

    split_lua_table_entries(table_body)
        .into_iter()
        .filter_map(|entry| {
            let (key, value) = split_lua_table_entry(entry)?;
            let parsed = match key.as_str() {
                "output" | "mode" | "position" => parse_lua_string_literal(value)?,
                "scale" => parse_lua_scale_literal(value)?,
                "disabled" if matches!(value, "true" | "false") => value.to_string(),
                _ => return None,
            };
            Some((key, parsed))
        })
        .collect()
}

fn split_lua_table_entries(table_body: &str) -> Vec<&str> {
    let bytes = table_body.as_bytes();
    let mut entries = Vec::new();
    let mut start = 0usize;
    let mut index = 0usize;
    let mut paren_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut bracket_depth = 0usize;

    while index < bytes.len() {
        if let Some((content_start, eq_count)) = lua_long_bracket_open(bytes, index) {
            index = find_lua_long_bracket_close(bytes, content_start, eq_count)
                .map(|close| close + eq_count + 2)
                .unwrap_or(bytes.len());
            continue;
        }
        if matches!(bytes[index], b'\'' | b'"') {
            index = skip_quoted_lua_string(bytes, index);
            continue;
        }
        match bytes[index] {
            b'(' => paren_depth += 1,
            b')' => paren_depth = paren_depth.saturating_sub(1),
            b'{' => brace_depth += 1,
            b'}' => brace_depth = brace_depth.saturating_sub(1),
            b'[' => bracket_depth += 1,
            b']' => bracket_depth = bracket_depth.saturating_sub(1),
            b',' | b';' if paren_depth == 0 && brace_depth == 0 && bracket_depth == 0 => {
                entries.push(&table_body[start..index]);
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }

    if start <= table_body.len() {
        entries.push(&table_body[start..]);
    }
    entries
}

fn split_lua_table_entry(entry: &str) -> Option<(String, &str)> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    if entry.starts_with('[') {
        let close = find_balanced_lua_delim(entry, 0, b'[', b']')?;
        let key = parse_lua_string_literal(entry[1..close].trim())?;
        let rest = entry[close + 1..].trim_start();
        let after_eq = rest.strip_prefix('=')?;
        if after_eq.starts_with('=') {
            return None;
        }
        return Some((key, after_eq.trim()));
    }

    let (raw_key, raw_value) = entry.split_once('=')?;
    if raw_value.starts_with('=') {
        return None;
    }
    let key = raw_key.trim();
    let first = *key.as_bytes().first()?;
    if !(first.is_ascii_alphabetic() || first == b'_') || !key.bytes().all(is_lua_identifier_byte) {
        return None;
    }
    Some((key.to_string(), raw_value.trim()))
}

fn parse_lua_scale_literal(value: &str) -> Option<String> {
    if let Some(text) = parse_lua_string_literal(value) {
        return (!text.is_empty()).then_some(text);
    }
    let value = value.trim();
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
        || value.bytes().filter(|byte| *byte == b'.').count() > 1
    {
        return None;
    }
    let numeric = value.parse::<f64>().ok()?;
    (numeric.is_finite() && numeric > 0.0).then(|| value.to_string())
}

fn parse_lua_string_literal(value: &str) -> Option<String> {
    let value = value.trim();
    let bytes = value.as_bytes();
    if let Some((content_start, eq_count)) = lua_long_bracket_open(bytes, 0) {
        let close = find_lua_long_bracket_close(bytes, content_start, eq_count)?;
        if close + eq_count + 2 != bytes.len() {
            return None;
        }
        let mut content = &value[content_start..close];
        if let Some(rest) = content.strip_prefix("\r\n") {
            content = rest;
        } else if let Some(rest) = content.strip_prefix('\n') {
            content = rest;
        }
        return Some(content.to_string());
    }

    let end = find_quoted_lua_string_end(bytes, 0)?;
    if end + 1 != bytes.len() {
        return None;
    }

    let mut result = String::new();
    let mut escaped = false;
    for character in value[1..end].chars() {
        if escaped {
            result.push(match character {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            result.push(character);
        }
    }
    if escaped {
        result.push('\\');
    }
    Some(result)
}

fn strip_lua_comments(contents: &str) -> String {
    let bytes = contents.as_bytes();
    let mut cleaned = String::with_capacity(contents.len());
    let mut index = 0;

    while index < bytes.len() {
        if let Some(end) = skip_lua_comment_body(bytes, index) {
            cleaned.extend(
                contents[index..end]
                    .chars()
                    .filter(|character| *character == '\n'),
            );
            index = end;
            continue;
        }
        if matches!(bytes[index], b'\'' | b'"') {
            let end = skip_quoted_lua_string(bytes, index);
            cleaned.push_str(&contents[index..end]);
            index = end;
            continue;
        }
        if let Some((content_start, eq_count)) = lua_long_bracket_open(bytes, index) {
            let end = find_lua_long_bracket_close(bytes, content_start, eq_count)
                .map(|close| close + eq_count + 2)
                .unwrap_or(bytes.len());
            cleaned.push_str(&contents[index..end]);
            index = end;
            continue;
        }

        let Some(character) = contents[index..].chars().next() else {
            break;
        };
        cleaned.push(character);
        index += character.len_utf8();
    }

    cleaned
}

fn parse_literal_lua_call_argument(raw: &str) -> Option<String> {
    let cleaned = strip_lua_comments(raw);
    let trimmed = cleaned.trim();
    let name_len = trimmed
        .bytes()
        .take_while(|byte| is_lua_identifier_byte(*byte))
        .count();
    let mut arg = trimmed.get(name_len..)?.trim();
    if arg.starts_with('(') {
        let close = find_balanced_lua_delim(arg, 0, b'(', b')')?;
        if close + 1 != arg.len() {
            return None;
        }
        arg = arg[1..close].trim();
    }
    parse_lua_string_literal(arg)
}

fn resolve_lua_require(module: &str, including_file: &Path) -> Option<PathBuf> {
    if module.is_empty() || module.contains(['/', '\\']) {
        return None;
    }

    let module_path = PathBuf::from(module.replace('.', "/"));
    let parent = including_file.parent().unwrap_or_else(|| Path::new("."));
    let mut roots = vec![parent.to_path_buf()];
    if parent.file_name().is_some_and(|name| name == "hypr")
        && let Some(config_root) = parent.parent()
    {
        roots.push(config_root.to_path_buf());
    }

    for root in roots {
        let file = root.join(&module_path).with_extension("lua");
        if file.is_file() {
            return Some(file);
        }
        let init = root.join(&module_path).join("init.lua");
        if init.is_file() {
            return Some(init);
        }
    }
    None
}

fn normalize_monitor_rule(rule: &str) -> Option<String> {
    let rule = rule.trim();
    if let Some((key, value)) = parse_assignment(rule)
        && key == "monitor"
    {
        return parse_monitor_rule(Path::new("<expected>"), 1, rule, value)
            .map(|parsed| parsed.normalized_rule());
    }

    find_lua_calls(rule)
        .into_iter()
        .find(|call| call.kind == LuaCallKind::Monitor)
        .and_then(|call| {
            parse_lua_monitor_rule(Path::new("<expected>"), rule, call.start, call.end)
        })
        .map(|parsed| parsed.normalized_rule())
}

fn parse_assignment(line: &str) -> Option<(&str, &str)> {
    let line = strip_comment(line).trim();
    let (key, value) = line.split_once('=')?;
    Some((key.trim(), value.trim()))
}

fn strip_comment(line: &str) -> &str {
    line.split_once('#')
        .map(|(before, _)| before)
        .unwrap_or(line)
}

fn parse_monitor_rule(
    path: &Path,
    line_number: usize,
    raw_line: &str,
    value: &str,
) -> Option<MonitorRule> {
    if value.contains('$') {
        return None;
    }

    let parts = value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() < 2 {
        return None;
    }

    Some(MonitorRule {
        path: path.to_path_buf(),
        line_number,
        raw: raw_line.trim().to_string(),
        connector: parts[0].to_string(),
        mode: parts[1].to_string(),
        position: parts.get(2).map(|value| (*value).to_string()),
        scale: parts.get(3).map(|value| (*value).to_string()),
    })
}

fn resolve_source_paths(
    value: &str,
    including_file: &Path,
    warnings: &mut Vec<String>,
) -> Vec<PathBuf> {
    let expanded = expand_config_path(value);
    let path = PathBuf::from(expanded);
    let path = if path.is_absolute() {
        path
    } else {
        including_file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    };

    if path_contains_unsupported_glob(&path) {
        warnings.push(format!(
            "Skipped unsupported glob source '{}' from {}",
            value,
            human_path(including_file)
        ));
        return Vec::new();
    }

    if !path_contains_simple_glob(&path) {
        return vec![path];
    }

    match expand_simple_glob_path(&path) {
        Ok(paths) if !paths.is_empty() => paths,
        Ok(_) => {
            warnings.push(format!(
                "Source glob '{}' from {} matched no files",
                value,
                human_path(including_file)
            ));
            Vec::new()
        }
        Err(error) => {
            warnings.push(format!(
                "Could not expand source glob '{}' from {}: {error}",
                value,
                human_path(including_file)
            ));
            Vec::new()
        }
    }
}

fn expand_config_path(value: &str) -> String {
    let value = value.trim_matches('"').trim_matches('\'');
    if let Some(rest) = value.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest).display().to_string();
    }
    if let Some(rest) = value.strip_prefix("$HOME/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest).display().to_string();
    }
    if let Some(rest) = value.strip_prefix("$XDG_CONFIG_HOME/")
        && let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME")
    {
        return PathBuf::from(config_home).join(rest).display().to_string();
    }
    value.to_string()
}

fn path_contains_simple_glob(path: &Path) -> bool {
    path.to_string_lossy()
        .chars()
        .any(|character| matches!(character, '*' | '?'))
}

fn path_contains_unsupported_glob(path: &Path) -> bool {
    path.to_string_lossy()
        .chars()
        .any(|character| matches!(character, '[' | ']'))
}

fn expand_simple_glob_path(pattern: &Path) -> Result<Vec<PathBuf>, String> {
    let mut bases = Vec::new();
    let mut components = Vec::new();

    for component in pattern.components() {
        match component {
            Component::Prefix(prefix) => bases.push(PathBuf::from(prefix.as_os_str())),
            Component::RootDir => bases.push(PathBuf::from("/")),
            Component::CurDir => components.push(OsString::from(".")),
            Component::ParentDir => components.push(OsString::from("..")),
            Component::Normal(value) => components.push(value.to_os_string()),
        }
    }

    if bases.is_empty() {
        bases.push(PathBuf::new());
    }

    for component in components {
        let component_text = component.to_string_lossy();
        if component_text.contains('*') || component_text.contains('?') {
            let mut next = Vec::new();
            for base in &bases {
                let entries = fs::read_dir(base)
                    .map_err(|error| format!("could not read {}: {error}", human_path(base)))?;
                for entry in entries.filter_map(Result::ok) {
                    let file_name = entry.file_name();
                    let file_name = file_name.to_string_lossy();
                    if wildcard_match(&component_text, &file_name) {
                        next.push(entry.path());
                    }
                }
            }
            next.sort();
            bases = next;
        } else {
            bases = bases
                .into_iter()
                .map(|base| base.join(&component))
                .collect();
        }
    }

    bases.sort();
    Ok(bases)
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let value = value.chars().collect::<Vec<_>>();
    let mut pattern_index = 0;
    let mut value_index = 0;
    let mut star_index = None;
    let mut star_value_index = 0;

    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == '?' || pattern[pattern_index] == value[value_index])
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == '*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_value_index = value_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_value_index += 1;
            value_index = star_value_index;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == '*' {
        pattern_index += 1;
    }

    pattern_index == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_test_dir() -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "drmcru-hypr-config-test-{}-{now}",
            std::process::id()
        ))
    }

    #[test]
    fn repeated_source_and_dofile_preserve_load_order_but_require_is_cached() {
        for extension in ["conf", "lua"] {
            let dir = unique_test_dir();
            fs::create_dir_all(&dir).unwrap();
            let root = dir.join(format!("hyprland.{extension}"));
            let source = dir.join(format!("monitors.{extension}"));
            let rule = "monitor=DP-1,1920x1080@60,auto,1";
            if extension == "conf" {
                fs::write(&source, format!("{rule}\nsource=hyprland.conf\n")).unwrap();
                fs::write(&root, "source=monitors.conf\nmonitor=DP-1,1920x1080@144,auto,1\nsource=monitors.conf\n").unwrap();
            } else {
                fs::write(&source, "hl.monitor({output='DP-1', mode='1920x1080@60', position='auto', scale=1})\ndofile('hyprland.lua')\n").unwrap();
                fs::write(&root, "dofile('monitors.lua')\nhl.monitor({output='DP-1', mode='1920x1080@144', position='auto', scale=1})\ndofile('monitors.lua')\n").unwrap();
            }
            let report = inspect_monitor_rule_from(root.clone(), "DP-1", rule);
            assert_eq!(report.connector_rules.len(), 3);
            assert!(report.exact_match_is_effective());
            if extension == "lua" {
                fs::write(
                    &source,
                    "hl.monitor({output='DP-1', mode='1920x1080@60', position='auto', scale=1})\n",
                )
                .unwrap();
                fs::write(&root, "require('monitors')\nhl.monitor({output='DP-1', mode='1920x1080@144', position='auto', scale=1})\nrequire('monitors')\n").unwrap();
                let report = inspect_monitor_rule_from(root, "DP-1", rule);
                assert_eq!(report.connector_rules.len(), 2);
                assert!(!report.exact_match_is_effective());
            }
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn literal_lua_arguments_reject_concatenation_and_accept_comments() {
        assert_eq!(
            parse_literal_lua_call_argument("dofile('monitors.lua' -- comment\n)"),
            Some("monitors.lua".to_string())
        );
        assert_eq!(
            parse_literal_lua_call_argument("dofile('monitors' .. '.lua')"),
            None
        );
        assert_eq!(
            parse_literal_lua_call_argument("require('hypr.' .. module .. '.lua')"),
            None
        );
    }

    #[test]
    fn parses_literal_monitor_rule() {
        let rule = parse_monitor_rule(
            Path::new("/tmp/hyprland.conf"),
            7,
            "monitor = DP-1,1920x1080@239.76,auto,1 # comment",
            "DP-1,1920x1080@239.76,auto,1",
        )
        .unwrap();

        assert_eq!(rule.connector, "DP-1");
        assert_eq!(rule.mode, "1920x1080@239.76");
        assert_eq!(rule.position.as_deref(), Some("auto"));
        assert_eq!(rule.scale.as_deref(), Some("1"));
        assert_eq!(
            rule.normalized_rule(),
            "monitor=DP-1,1920x1080@239.76,auto,1"
        );
    }

    #[test]
    fn parses_lua_monitor_rules_and_literal_dofile() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();
        let root = dir.join("hyprland.lua");
        let sourced = dir.join("monitors.lua");
        fs::write(
            &root,
            "dofile('monitors.lua') -- literal include\n\
             hl.monitor({ output = 'DP-1', mode = '1920x1080@240',\n\
                 position = 'auto', scale = 1.25 })\n",
        )
        .unwrap();
        fs::write(
            &sourced,
            "hl.monitor({ output = 'HDMI-A-1', mode = 'preferred', position = 'auto', scale = 1 })\n",
        )
        .unwrap();

        let report =
            inspect_monitor_rule_from(root, "DP-1", "monitor=DP-1,1920x1080@240,auto,1.25");

        assert_eq!(report.files_read, 2);
        assert!(report.read_warnings.is_empty());
        assert_eq!(report.connector_rules.len(), 1);
        assert!(report.exact_match_is_effective());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn follows_local_lua_require_and_ignores_dynamic_bootstrap() {
        let dir = unique_test_dir();
        let hypr = dir.join("hypr");
        fs::create_dir_all(&hypr).unwrap();
        let root = hypr.join("hyprland.lua");
        let monitors = hypr.join("monitors.lua");
        fs::write(
            &root,
            "dofile((os.getenv(\"OMARCHY_PATH\") or \"/usr/share/omarchy\") .. \"/default/hypr/bootstrap.lua\")\n\
             require(\"hypr.monitors\")\n",
        )
        .unwrap();
        fs::write(
            &monitors,
            "hl.monitor({ output = \"DP-1\", mode = \"1920x1080@239.76\", position = \"0x0\", scale = 1 })\n",
        )
        .unwrap();

        let report = inspect_monitor_rule_from(root, "DP-1", "monitor=DP-1,1920x1080@239.76,0x0,1");

        assert_eq!(report.files_read, 2);
        assert!(report.read_warnings.is_empty());
        assert_eq!(report.connector_rules.len(), 1);
        assert!(report.exact_match_is_effective());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lua_calls_preserve_include_order_and_skip_comments() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();
        let root = dir.join("hyprland.lua");
        let sourced = dir.join("monitors.lua");
        fs::write(
            &root,
            "-- hl.monitor({ output = \"DP-1\", mode = \"800x600@60\" })\n\
             hl.monitor({ output = \"DP-1\", mode = \"1920x1080@60\", position = \"auto\", scale = 1 })\n\
             dofile(\"monitors.lua\")\n\
             hl.monitor({ output = \"DP-1\", mode = \"1920x1080@144\", position = \"auto\", scale = 1 })\n",
        )
        .unwrap();
        fs::write(
            &sourced,
            "hl.monitor({ output = \"DP-1\", mode = \"1920x1080@120\", position = \"auto\", scale = 1 })\n",
        )
        .unwrap();

        let report = inspect_monitor_rule_from(root, "DP-1", "monitor=DP-1,1920x1080@144,auto,1");

        assert_eq!(report.connector_rules.len(), 3);
        assert_eq!(report.connector_rules[0].mode, "1920x1080@60");
        assert_eq!(report.connector_rules[1].mode, "1920x1080@120");
        assert_eq!(report.connector_rules[2].mode, "1920x1080@144");
        assert!(report.exact_match_is_effective());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lua_expected_rule_matches_canonical_monitor_values() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();
        let root = dir.join("hyprland.lua");
        fs::write(
            &root,
            "hl.monitor({ output = \"DP-1\", mode = \"1920x1080@144\", position = \"0x0\", scale = 1.25 })\n",
        )
        .unwrap();
        let expected = format_monitor_rule_for_path(&root, "DP-1", "1920x1080@144", "0x0", "1.25");

        let report = inspect_monitor_rule_from(root, "DP-1", &expected);

        assert!(report.exact_match_is_effective());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_lua_disabled_monitor_rule() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();
        let root = dir.join("hyprland.lua");
        fs::write(
            &root,
            "hl.monitor({ output = \"eDP-1\", disabled = true })\n",
        )
        .unwrap();

        let report = inspect_monitor_rule_from(root, "eDP-1", "monitor=eDP-1,disabled");

        assert_eq!(report.connector_rules.len(), 1);
        assert_eq!(report.connector_rules[0].mode, "disabled");
        assert!(report.exact_match_is_effective());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lua_monitor_fields_ignore_comments_and_accept_table_whitespace() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();
        let root = dir.join("hyprland.lua");
        fs::write(
            &root,
            "hl.monitor( { output = \"DP-1\"; -- mode = \"800x600@60\"\n\
             --[[ disabled = true; ]]\n\
             mode = \"1920x1080@144\"; position = \"auto\"; scale = 1 } )\n",
        )
        .unwrap();

        let report = inspect_monitor_rule_from(root, "DP-1", "monitor=DP-1,1920x1080@144,auto,1");

        assert_eq!(report.connector_rules.len(), 1);
        assert_eq!(report.connector_rules[0].mode, "1920x1080@144");
        assert!(report.exact_match_is_effective());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn follows_sources_and_detects_later_override() {
        let dir = unique_test_dir();
        fs::create_dir_all(dir.join("conf.d")).unwrap();
        let root = dir.join("hyprland.conf");
        let sourced = dir.join("conf.d/monitors.conf");
        fs::write(
            &root,
            "source = conf.d/monitors.conf\nmonitor=DP-1,1920x1080@239.76,auto,1\n",
        )
        .unwrap();
        fs::write(&sourced, "monitor=DP-1,1280x1080@239.76,auto,1\n").unwrap();

        let report =
            inspect_monitor_rule_from(root, "DP-1", "monitor=DP-1,1920x1080@239.76,auto,1");

        assert_eq!(report.files_read, 2);
        assert_eq!(report.connector_rules.len(), 2);
        assert!(report.exact_match.is_some());
        assert!(report.exact_match_is_effective());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn expands_simple_source_globs_in_sorted_order() {
        let dir = unique_test_dir();
        fs::create_dir_all(dir.join("conf.d")).unwrap();
        let root = dir.join("hyprland.conf");
        fs::write(&root, "source = conf.d/*.conf\n").unwrap();
        fs::write(
            dir.join("conf.d/10-base.conf"),
            "monitor=DP-1,1280x1080@239.76,auto,1\n",
        )
        .unwrap();
        fs::write(
            dir.join("conf.d/20-monitor.conf"),
            "monitor=DP-1,1920x1080@239.76,auto,1\n",
        )
        .unwrap();

        let report =
            inspect_monitor_rule_from(root, "DP-1", "monitor=DP-1,1920x1080@239.76,auto,1");

        assert_eq!(report.files_read, 3);
        assert!(report.read_warnings.is_empty());
        assert_eq!(report.connector_rules.len(), 2);
        assert!(report.exact_match_is_effective());
        assert_eq!(
            report
                .last_connector_rule
                .as_ref()
                .map(MonitorRule::normalized_rule),
            Some("monitor=DP-1,1920x1080@239.76,auto,1".to_string())
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn wildcard_match_supports_star_and_question_mark() {
        assert!(wildcard_match("*.conf", "monitors.conf"));
        assert!(wildcard_match("monitor-?.conf", "monitor-1.conf"));
        assert!(!wildcard_match("monitor-?.conf", "monitor-10.conf"));
    }

    #[test]
    fn reports_exact_match_as_ineffective_when_later_rule_wins() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();
        let root = dir.join("hyprland.conf");
        fs::write(
            &root,
            "monitor=DP-1,1920x1080@239.76,auto,1\nmonitor=DP-1,1280x1080@239.76,auto,1\n",
        )
        .unwrap();

        let report =
            inspect_monitor_rule_from(root, "DP-1", "monitor=DP-1,1920x1080@239.76,auto,1");

        assert!(report.exact_match.is_some());
        assert!(!report.exact_match_is_effective());
        assert_eq!(
            report
                .last_connector_rule
                .as_ref()
                .map(MonitorRule::normalized_rule),
            Some("monitor=DP-1,1280x1080@239.76,auto,1".to_string())
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn uses_last_duplicate_exact_rule_for_effective_match() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();
        let root = dir.join("hyprland.conf");
        fs::write(
            &root,
            "monitor=DP-1,1920x1080@60,auto,1\n\
             monitor=DP-1,1280x720@60,auto,1\n\
             monitor=DP-1,1920x1080@60,auto,1\n",
        )
        .unwrap();

        let report = inspect_monitor_rule_from(root, "DP-1", "monitor=DP-1,1920x1080@60,auto,1");

        assert!(report.exact_match_is_effective());
        assert_eq!(
            report.exact_match.as_ref().map(|rule| rule.line_number),
            Some(3)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn supports_parenthesis_free_lua_calls_and_quoted_commas() {
        let dir = unique_test_dir();
        let hypr = dir.join("hypr");
        fs::create_dir_all(&hypr).unwrap();
        let root = hypr.join("hyprland.lua");
        let extra = hypr.join("extra.lua");
        let shared = hypr.join("shared.lua");
        fs::write(
            &root,
            "dofile 'extra.lua'\n\
             require [[hypr.shared]]\n\
             hl.monitor {\n\
                 output = \"desc:Dell, Inc. U2723QE\",\n\
                 mode = \"3840x2160@60\",\n\
                 position = \"0x0\",\n\
                 [\"scale\"] = \"auto\",\n\
                 reserved = { mode = \"800x600@60\", top = 30 },\n\
             }\n",
        )
        .unwrap();
        fs::write(
            &extra,
            "hl.monitor { output = \"DP-1\", mode = \"1920x1080@120\", position = \"auto\", scale = 1 }\n",
        )
        .unwrap();
        fs::write(
            &shared,
            "hl.monitor { output = \"DP-1\", mode = \"1920x1080@144\", position = \"auto\", scale = 1.25 }\n",
        )
        .unwrap();

        let dp1 =
            inspect_monitor_rule_from(root.clone(), "DP-1", "monitor=DP-1,1920x1080@144,auto,1.25");
        assert_eq!(dp1.files_read, 3);
        assert!(dp1.read_warnings.is_empty());
        assert_eq!(dp1.connector_rules.len(), 2);
        assert!(dp1.exact_match_is_effective());

        let dell = inspect_monitor_rule_from(
            root,
            "desc:Dell, Inc. U2723QE",
            "monitor=desc:Dell, Inc. U2723QE,3840x2160@60,0x0,auto",
        );
        assert_eq!(dell.connector_rules.len(), 1);
        assert_eq!(dell.connector_rules[0].connector, "desc:Dell, Inc. U2723QE");
        assert_eq!(dell.connector_rules[0].mode, "3840x2160@60");
        assert_eq!(dell.connector_rules[0].scale.as_deref(), Some("auto"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn skips_dynamic_lua_identifiers_and_method_shadowed_calls() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();
        let root = dir.join("hyprland.lua");
        fs::write(
            &root,
            "local custom = { require = function() end }\n\
             custom.require('nonexistent')\n\
             hl.monitor({ output = dynamic_connector, mode = '1920x1080@60' })\n\
             hl.monitor({ output = 'DP-1', mode = dynamic_mode })\n\
             hl.monitor({ output = 'DP-1', mode = '1920x1080@60', position = dyn_pos, scale = dyn_scale })\n",
        )
        .unwrap();

        let dynamic = inspect_monitor_rule_from(
            root.clone(),
            "dynamic_connector",
            "monitor=dynamic_connector,1920x1080@60",
        );
        assert!(dynamic.connector_rules.is_empty());
        assert!(dynamic.read_warnings.is_empty());

        let dp1 = inspect_monitor_rule_from(root, "DP-1", "monitor=DP-1,1920x1080@60,0x0,1");
        assert_eq!(dp1.connector_rules.len(), 1);
        assert_eq!(dp1.connector_rules[0].mode, "1920x1080@60");
        assert_eq!(dp1.connector_rules[0].position, None);
        assert_eq!(dp1.connector_rules[0].scale, None);
        assert!(!dp1.exact_match_is_effective());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bounds_diamond_includes_and_rejects_non_regular_files() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();
        // Build a 12-level diamond DAG (2^12 = 4096 paths without a file-read cap).
        for level in 0..12 {
            let file = dir.join(format!("l{level}.conf"));
            let next = format!("l{}.conf", level + 1);
            fs::write(&file, format!("source = {next}\nsource = {next}\n")).unwrap();
        }
        fs::write(dir.join("l12.conf"), "monitor=DP-1,1920x1080@60,auto,1\n").unwrap();

        let report = inspect_monitor_rule_from(
            dir.join("l0.conf"),
            "DP-1",
            "monitor=DP-1,1920x1080@60,auto,1",
        );
        assert_eq!(report.files_read, MAX_CONFIG_FILES_READ);
        assert_eq!(report.read_warnings.len(), 1);
        assert!(report.read_warnings[0].contains("Stopped reading"));

        let fifo = dir.join("pipe.conf");
        let mkfifo = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo should run");
        assert!(mkfifo.success());
        let fifo_report = inspect_monitor_rule_from(fifo, "DP-1", "monitor=DP-1,1920x1080@60");
        assert_eq!(fifo_report.files_read, 0);
        assert!(
            fifo_report
                .read_warnings
                .iter()
                .any(|warning| warning.contains("not a regular file"))
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
