use anyhow::{Result, bail};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::CompletedFrame;
use ratatui::prelude::*;
use std::io::{self, IsTerminal};

pub(super) fn inner_rect(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

pub(super) fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

pub(super) fn center_label(label: &str, width: u16) -> String {
    let inner_width = usize::from(width.saturating_sub(2));
    if label.len() >= inner_width {
        return label.to_string();
    }
    let left = (inner_width - label.len()) / 2;
    format!("{}{}", " ".repeat(left), label)
}

pub(super) fn row_style(selected: bool) -> Style {
    if selected {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default()
    }
}

pub(super) fn focus_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    }
}

pub(super) fn wrap_index(current: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    (current as isize + delta).rem_euclid(len as isize) as usize
}

pub(super) fn rect_contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x
        && x < rect.x.saturating_add(rect.width)
        && y >= rect.y
        && y < rect.y.saturating_add(rect.height)
}

pub(super) struct TerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalSession {
    pub(super) fn enter() -> Result<Self> {
        require_interactive_terminal(io::stdin().is_terminal(), io::stdout().is_terminal())?;
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
            return Err(error.into());
        }
        let terminal = match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = disable_raw_mode();
                let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
                return Err(error.into());
            }
        };
        Ok(Self { terminal })
    }

    pub(super) fn draw<F>(&mut self, f: F) -> io::Result<CompletedFrame<'_>>
    where
        F: FnOnce(&mut Frame<'_>),
    {
        self.terminal.draw(f)
    }
}

fn require_interactive_terminal(stdin_is_terminal: bool, stdout_is_terminal: bool) -> Result<()> {
    if !stdin_is_terminal || !stdout_is_terminal {
        bail!(
            "the drmcru TUI requires an interactive terminal; run `drmcru doctor` for noninteractive diagnostics"
        );
    }
    Ok(())
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noninteractive_sessions_get_an_actionable_error() {
        let error = require_interactive_terminal(false, true).unwrap_err();

        assert!(error.to_string().contains("interactive terminal"));
        assert!(error.to_string().contains("drmcru doctor"));
    }
}
