use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

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
    seen: BTreeSet<PathBuf>,
    files_read: usize,
    monitor_rules: Vec<MonitorRule>,
    read_warnings: Vec<String>,
}

impl ConfigParser {
    fn read_file(&mut self, path: &Path) {
        let display_path = path.to_path_buf();
        let seen_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if !self.seen.insert(seen_path) {
            return;
        }

        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) => {
                self.read_warnings
                    .push(format!("Could not read {}: {error}", human_path(path)));
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
                            self.read_file(&source);
                        }
                    }
                }
            }
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
        if bytes[index..].starts_with(b"--[[") {
            index = find_bytes(bytes, index + 4, b"]]")
                .map(|end| end + 2)
                .unwrap_or(bytes.len());
            continue;
        }
        if bytes[index..].starts_with(b"--") {
            index = bytes[index..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|relative| index + relative + 1)
                .unwrap_or(bytes.len());
            continue;
        }
        if bytes[index..].starts_with(b"[[") {
            index = find_bytes(bytes, index + 2, b"]]")
                .map(|end| end + 2)
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
                && (index == 0 || !is_lua_identifier_byte(bytes[index - 1]))
                && bytes
                    .get(index + name.len())
                    .is_none_or(|byte| !is_lua_identifier_byte(*byte))
        });

        let Some((name, kind)) = matched else {
            index += 1;
            continue;
        };
        let mut open = index + name.len();
        while bytes.get(open).is_some_and(u8::is_ascii_whitespace) {
            open += 1;
        }
        if bytes.get(open) != Some(&b'(') {
            index += name.len();
            continue;
        }
        let Some(end) = find_balanced_lua_call(contents, open) else {
            break;
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
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|relative| from + relative)
}

fn skip_quoted_lua_string(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut index = start + 1;
    let mut escaped = false;
    while index < bytes.len() {
        if escaped {
            escaped = false;
        } else if bytes[index] == b'\\' {
            escaped = true;
        } else if bytes[index] == quote {
            return index + 1;
        }
        index += 1;
    }
    bytes.len()
}

fn parse_lua_monitor_rule(
    path: &Path,
    contents: &str,
    start: usize,
    end: usize,
) -> Option<MonitorRule> {
    let raw = &contents[start..=end];
    let fields = parse_lua_table_fields(raw);
    let connector = fields.iter().find(|(key, _)| key == "output")?;
    let mode = fields
        .iter()
        .find(|(key, _)| key == "mode")
        .map(|(_, value)| value.clone())
        .or_else(|| {
            fields
                .iter()
                .any(|(key, value)| key == "disabled" && value == "true")
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
            .find(|(key, _)| key == "position")
            .map(|(_, value)| value.clone()),
        scale: fields
            .iter()
            .find(|(key, _)| key == "scale")
            .map(|(_, value)| value.clone()),
    })
}

fn find_balanced_lua_call(contents: &str, open: usize) -> Option<usize> {
    let bytes = contents.as_bytes();
    let mut depth = 0usize;
    let mut index = open;
    while index < bytes.len() {
        if bytes[index..].starts_with(b"--[[") {
            index = find_bytes(bytes, index + 4, b"]]")? + 2;
            continue;
        }
        if bytes[index..].starts_with(b"--") {
            index = bytes[index..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|relative| index + relative + 1)
                .unwrap_or(bytes.len());
            continue;
        }
        if bytes[index..].starts_with(b"[[") {
            index = find_bytes(bytes, index + 2, b"]]")? + 2;
            continue;
        }
        if matches!(bytes[index], b'\'' | b'"') {
            index = skip_quoted_lua_string(bytes, index);
            continue;
        }
        if bytes[index] == b'(' {
            depth += 1;
        } else if bytes[index] == b')' {
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
    let table = without_comments
        .find('{')
        .and_then(|start| {
            without_comments
                .rfind('}')
                .filter(|end| *end > start)
                .map(|end| &without_comments[start + 1..end])
        })
        .unwrap_or(&without_comments);

    table
        .split([',', ';', '\n'])
        .filter_map(|part| {
            let (key, value) = part.split_once('=')?;
            let key = key.trim().to_string();
            let value = value
                .trim()
                .trim_matches(|character| matches!(character, '"' | '\'' | '}' | ')'))
                .trim()
                .to_string();
            matches!(
                key.as_str(),
                "output" | "mode" | "position" | "scale" | "disabled"
            )
            .then_some((key, value))
        })
        .collect()
}

fn strip_lua_comments(contents: &str) -> String {
    let bytes = contents.as_bytes();
    let mut cleaned = String::with_capacity(contents.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index..].starts_with(b"--[[") {
            let end = find_bytes(bytes, index + 4, b"]]")
                .map(|end| end + 2)
                .unwrap_or(bytes.len());
            cleaned.extend(
                contents[index..end]
                    .chars()
                    .filter(|character| *character == '\n'),
            );
            index = end;
            continue;
        }
        if bytes[index..].starts_with(b"--") {
            let end = bytes[index..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|relative| index + relative)
                .unwrap_or(bytes.len());
            index = end;
            continue;
        }
        if matches!(bytes[index], b'\'' | b'"') {
            let end = skip_quoted_lua_string(bytes, index);
            cleaned.push_str(&contents[index..end]);
            index = end;
            continue;
        }
        if bytes[index..].starts_with(b"[[") {
            let end = find_bytes(bytes, index + 2, b"]]")
                .map(|end| end + 2)
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
    let open = raw.find('(')?;
    let value = raw.get(open + 1..raw.len().checked_sub(1)?)?.trim();
    let quote = *value.as_bytes().first()?;
    if !matches!(quote, b'\'' | b'"') || value.as_bytes().last() != Some(&quote) {
        return None;
    }

    let mut result = String::new();
    let mut escaped = false;
    for character in value[1..value.len() - 1].chars() {
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

fn resolve_lua_require(module: &str, including_file: &Path) -> Option<PathBuf> {
    if module.is_empty() || module.contains(['/', '\\']) {
        return None;
    }

    let module_path = PathBuf::from(module.replace('.', "/"));
    let parent = including_file.parent().unwrap_or_else(|| Path::new("."));
    let mut roots = vec![parent.to_path_buf()];
    if parent.file_name().is_some_and(|name| name == "hypr") {
        if let Some(config_root) = parent.parent() {
            roots.push(config_root.to_path_buf());
        }
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
    if let Some((key, value)) = parse_assignment(rule) {
        if key == "monitor" {
            return parse_monitor_rule(Path::new("<expected>"), 1, rule, value)
                .map(|parsed| parsed.normalized_rule());
        }
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
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest).display().to_string();
        }
    }
    if let Some(rest) = value.strip_prefix("$HOME/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest).display().to_string();
        }
    }
    if let Some(rest) = value.strip_prefix("$XDG_CONFIG_HOME/") {
        if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME") {
            return PathBuf::from(config_home).join(rest).display().to_string();
        }
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
}
