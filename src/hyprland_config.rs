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
        format!("{}:{}", self.path.display(), self.line_number)
    }
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

fn inspect_monitor_rule_from(
    root_path: PathBuf,
    connector: &str,
    expected_rule: &str,
) -> MonitorRuleInspection {
    let mut parser = ConfigParser::default();
    parser.read_file(&root_path);

    let connector_rules = parser
        .monitor_rules
        .into_iter()
        .filter(|rule| rule.connector == connector)
        .collect::<Vec<_>>();
    let exact_match = connector_rules
        .iter()
        .find(|rule| rule.normalized_rule() == expected_rule)
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
    if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(config_home).join("hypr/hyprland.conf");
    }

    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/hypr/hyprland.conf")
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
                    .push(format!("Could not read {}: {error}", path.display()));
                return;
            }
        };
        self.files_read += 1;

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
                            display_path.display()
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
            including_file.display()
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
                including_file.display()
            ));
            Vec::new()
        }
        Err(error) => {
            warnings.push(format!(
                "Could not expand source glob '{}' from {}: {error}",
                value,
                including_file.display()
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
                    .map_err(|error| format!("could not read {}: {error}", base.display()))?;
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
}
