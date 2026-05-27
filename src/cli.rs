#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Tui,
    Doctor,
    Help,
    Version,
}

pub fn parse_env_args() -> Result<Command, String> {
    parse_args(std::env::args().skip(1))
}

fn parse_args<I, S>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_string())
        .collect::<Vec<_>>();

    match args.as_slice() {
        [] => Ok(Command::Tui),
        [arg] if matches!(arg.as_str(), "-h" | "--help" | "help") => Ok(Command::Help),
        [arg] if matches!(arg.as_str(), "-V" | "--version" | "version") => Ok(Command::Version),
        [arg] if matches!(arg.as_str(), "doctor" | "--doctor") => Ok(Command::Doctor),
        [arg] => Err(format!("unknown argument: {arg}")),
        _ => Err(format!("too many arguments: {}", args.join(" "))),
    }
}

pub fn help_text() -> String {
    [
        format!("drmcru {}", env!("CARGO_PKG_VERSION")),
        "Linux DRM/KMS custom resolution utility".to_string(),
        String::new(),
        "Usage:".to_string(),
        "  drmcru             Start the TUI".to_string(),
        "  drmcru doctor      Print noninteractive diagnostics".to_string(),
        "  drmcru --version   Print version".to_string(),
        "  drmcru --help      Show this help".to_string(),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_args_runs_tui() {
        assert_eq!(parse_args(Vec::<String>::new()), Ok(Command::Tui));
    }

    #[test]
    fn parses_supported_commands() {
        assert_eq!(parse_args(["doctor"]), Ok(Command::Doctor));
        assert_eq!(parse_args(["--doctor"]), Ok(Command::Doctor));
        assert_eq!(parse_args(["--help"]), Ok(Command::Help));
        assert_eq!(parse_args(["-h"]), Ok(Command::Help));
        assert_eq!(parse_args(["--version"]), Ok(Command::Version));
        assert_eq!(parse_args(["-V"]), Ok(Command::Version));
    }

    #[test]
    fn rejects_unknown_arguments() {
        assert!(parse_args(["--bad"]).is_err());
        assert!(parse_args(["doctor", "--extra"]).is_err());
    }
}
