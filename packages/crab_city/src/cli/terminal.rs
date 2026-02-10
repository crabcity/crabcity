use anyhow::Result;
use nix::libc;

/// RAII guard that saves terminal settings and restores them on drop.
#[cfg(unix)]
pub struct TerminalGuard {
    original: Option<nix::sys::termios::Termios>,
}

#[cfg(unix)]
impl TerminalGuard {
    pub fn new() -> Self {
        use nix::sys::termios;
        let stdin = std::io::stdin();
        let original = termios::tcgetattr(&stdin).ok();
        Self { original }
    }

    pub fn enter_raw_mode(&self) {
        if let Some(ref original) = self.original {
            use nix::sys::termios;
            let stdin = std::io::stdin();
            let mut raw = original.clone();
            termios::cfmakeraw(&mut raw);
            let _ = termios::tcsetattr(&stdin, termios::SetArg::TCSANOW, &raw);
        }
    }
}

#[cfg(unix)]
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Some(ref original) = self.original {
            use nix::sys::termios;
            let stdin = std::io::stdin();
            let _ = termios::tcsetattr(&stdin, termios::SetArg::TCSANOW, original);
        }
    }
}

/// Get the current terminal size (rows, cols).
#[cfg(unix)]
pub fn get_terminal_size() -> Result<(u16, u16)> {
    let mut ws = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let ret = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) };
    if ret == -1 {
        anyhow::bail!("ioctl TIOCGWINSZ failed");
    }
    Ok((ws.ws_row, ws.ws_col))
}
