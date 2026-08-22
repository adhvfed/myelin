use std::fs::File;
use std::os::fd::{AsRawFd, OwnedFd};
use std::process::{Command, Stdio};
use std::sync::Arc;

use nix::pty::{openpty, Winsize};

const MAX_TERMINAL_NAME_BYTES: usize = 64;
const DEFAULT_TERMINAL_COLUMNS: u16 = 80;
const DEFAULT_TERMINAL_ROWS: u16 = 24;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceTerminal {
    term: String,
    size: WorkspaceTerminalSize,
}

impl WorkspaceTerminal {
    pub fn new(
        term: &str,
        columns: u32,
        rows: u32,
        pixel_width: u32,
        pixel_height: u32,
    ) -> Result<Self, String> {
        if term.is_empty()
            || term.len() > MAX_TERMINAL_NAME_BYTES
            || !term
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err("terminal type must be a bounded portable name".into());
        }
        Ok(Self {
            term: term.into(),
            size: WorkspaceTerminalSize::new(columns, rows, pixel_width, pixel_height)?,
        })
    }

    pub fn resize(
        &mut self,
        columns: u32,
        rows: u32,
        pixel_width: u32,
        pixel_height: u32,
    ) -> Result<(), String> {
        self.size = WorkspaceTerminalSize::new(columns, rows, pixel_width, pixel_height)?;
        Ok(())
    }

    pub(crate) fn environment(&self) -> String {
        format!("TERM={}", self.term)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WorkspaceTerminalSize {
    columns: u16,
    rows: u16,
    pixel_width: u16,
    pixel_height: u16,
}

impl WorkspaceTerminalSize {
    fn new(columns: u32, rows: u32, pixel_width: u32, pixel_height: u32) -> Result<Self, String> {
        let columns = terminal_extent(columns, DEFAULT_TERMINAL_COLUMNS);
        let rows = terminal_extent(rows, DEFAULT_TERMINAL_ROWS);
        let pixel_width = u16::try_from(pixel_width).ok();
        let pixel_height = u16::try_from(pixel_height).ok();
        match (columns, rows, pixel_width, pixel_height) {
            (Some(columns), Some(rows), Some(pixel_width), Some(pixel_height)) => Ok(Self {
                columns,
                rows,
                pixel_width,
                pixel_height,
            }),
            _ => Err("terminal dimensions are outside the supported range".into()),
        }
    }

    fn winsize(self) -> Winsize {
        Winsize {
            ws_row: self.rows,
            ws_col: self.columns,
            ws_xpixel: self.pixel_width,
            ws_ypixel: self.pixel_height,
        }
    }
}

fn terminal_extent(requested: u32, default: u16) -> Option<u16> {
    if requested == 0 {
        Some(default)
    } else {
        u16::try_from(requested).ok()
    }
}

pub(crate) struct PreparedWorkspaceTerminal {
    master: OwnedFd,
    slave: OwnedFd,
}

impl PreparedWorkspaceTerminal {
    pub(crate) fn open(terminal: &WorkspaceTerminal) -> Result<Self, String> {
        let pair = openpty(&terminal.size.winsize(), None)
            .map_err(|error| format!("open workspace pseudo-terminal: {error}"))?;
        Ok(Self {
            master: pair.master,
            slave: pair.slave,
        })
    }

    pub(crate) fn wire_child(&self, command: &mut Command) -> Result<(), String> {
        let input = self
            .slave
            .try_clone()
            .map_err(|error| format!("clone workspace terminal input: {error}"))?;
        let output = self
            .slave
            .try_clone()
            .map_err(|error| format!("clone workspace terminal output: {error}"))?;
        let error = self
            .slave
            .try_clone()
            .map_err(|error| format!("clone workspace terminal error: {error}"))?;
        command
            .stdin(Stdio::from(input))
            .stdout(Stdio::from(output))
            .stderr(Stdio::from(error));
        Ok(())
    }

    pub(crate) fn finish(self) -> Result<WorkspaceTerminalIo, String> {
        drop(self.slave);
        let control = WorkspaceTerminalControl(Arc::new(File::from(self.master)));
        let input = control
            .0
            .try_clone()
            .map_err(|error| format!("clone workspace terminal input stream: {error}"))?;
        let output = control
            .0
            .try_clone()
            .map_err(|error| format!("clone workspace terminal output stream: {error}"))?;
        Ok(WorkspaceTerminalIo {
            input,
            output,
            control,
        })
    }
}

pub(crate) struct WorkspaceTerminalIo {
    pub(crate) input: File,
    pub(crate) output: File,
    pub(crate) control: WorkspaceTerminalControl,
}

#[derive(Clone)]
pub(crate) struct WorkspaceTerminalControl(Arc<File>);

impl WorkspaceTerminalControl {
    pub(crate) fn resize(
        &self,
        columns: u32,
        rows: u32,
        pixel_width: u32,
        pixel_height: u32,
    ) -> Result<(), String> {
        let size = WorkspaceTerminalSize::new(columns, rows, pixel_width, pixel_height)?.winsize();
        let result = unsafe { libc::ioctl(self.0.as_raw_fd(), libc::TIOCSWINSZ, &size) };
        if result == -1 {
            return Err(format!(
                "resize workspace pseudo-terminal: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_names_and_dimensions_are_bounded_before_opening_a_host_pty() {
        let terminal = WorkspaceTerminal::new("xterm-256color", 120, 40, 0, 0).unwrap();
        assert_eq!(terminal.environment(), "TERM=xterm-256color");

        for refused in [
            WorkspaceTerminal::new("", 120, 40, 0, 0),
            WorkspaceTerminal::new("xterm;touch-host", 120, 40, 0, 0),
            WorkspaceTerminal::new("xterm", u32::from(u16::MAX) + 1, 40, 0, 0),
            WorkspaceTerminal::new("xterm", 120, u32::from(u16::MAX) + 1, 0, 0),
        ] {
            assert!(refused.is_err());
        }
    }

    #[test]
    fn an_unsized_ssh_terminal_starts_with_a_usable_default_window() {
        let terminal = WorkspaceTerminal::new("xterm", 0, 0, 0, 0).unwrap();

        assert_eq!(terminal.size.winsize().ws_col, DEFAULT_TERMINAL_COLUMNS);
        assert_eq!(terminal.size.winsize().ws_row, DEFAULT_TERMINAL_ROWS);
    }

    #[test]
    fn a_prepared_terminal_can_be_resized_through_its_retained_control_fd() {
        let terminal = WorkspaceTerminal::new("xterm", 80, 24, 0, 0).unwrap();
        let io = PreparedWorkspaceTerminal::open(&terminal)
            .unwrap()
            .finish()
            .unwrap();
        io.control.resize(132, 50, 0, 0).unwrap();
    }
}
