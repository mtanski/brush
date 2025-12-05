//! Command execution utilities.

pub use std::os::unix::process::CommandExt;
pub use std::os::unix::process::ExitStatusExt;

use command_fds::{CommandFdExt, FdMapping};

use crate::error;
use crate::openfiles;

/// Extension trait for injecting file descriptors into commands.
pub trait CommandFdInjectionExt {
    /// Injects the given open files as file descriptors into the command.
    ///
    /// # Arguments
    ///
    /// * `open_files` - A mapping of child file descriptors to open files.
    fn inject_fds(
        &mut self,
        open_files: impl Iterator<Item = (u32, openfiles::OpenFile)>,
    ) -> Result<(), error::Error>;
}

impl CommandFdInjectionExt for std::process::Command {
    fn inject_fds(
        &mut self,
        open_files: impl Iterator<Item = (u32, openfiles::OpenFile)>,
    ) -> Result<(), error::Error> {
        let fd_mappings = open_files
            .map(|(child_fd, open_file)| FdMapping {
                child_fd: i32::try_from(child_fd).unwrap(),
                parent_fd: open_file.into_owned_fd().unwrap(),
            })
            .collect();
        self.fd_mappings(fd_mappings)
            .map_err(|_e| error::ErrorKind::ChildCreationFailure)?;

        Ok(())
    }
}

/// Extension trait for arranging for commands to take the foreground.
pub trait CommandFgControlExt {
    /// Arranges for the command to take the foreground when it is executed.
    fn take_foreground(&mut self);
}

impl CommandFgControlExt for std::process::Command {
    fn take_foreground(&mut self) {
        // SAFETY:
        // This arranges for a provided function to run in the context of
        // the forked process before it exec's the target command. In general,
        // rust can't guarantee safety of code running in such a context.
        unsafe {
            self.pre_exec(setup_process_before_exec);
        }
    }
}

fn setup_process_before_exec() -> Result<(), std::io::Error> {
    use crate::sys;

    // Reset job control signals to default disposition
    // Parent (session leader) ignores these signals, but children should respond normally
    // This allows Ctrl-Z, Ctrl-C, etc. to work in child processes
    reset_job_control_signals()?;

    // NOTE: Don't call move_self_to_foreground() here!
    // The PARENT should manage foreground process group via tcsetpgrp() after getting child PID.
    // Having child call tcsetpgrp() in pre_exec is unreliable and can cause race conditions.
    // sys::terminal::move_self_to_foreground().map_err(std::io::Error::other)?;

    Ok(())
}

/// Reset job control signals to SIG_DFL (default disposition)
/// This undoes any SIG_IGN settings inherited from parent
fn reset_job_control_signals() -> Result<(), std::io::Error> {
    use nix::sys::signal::{signal, sigprocmask, SigHandler, SigSet, SigmaskHow, Signal};



    // These signals should have default disposition in child processes
    // even if the parent (session leader shell) ignores them
    // SIGPIPE: Critical for pipeline handling - without it, writing to closed pipes blocks forever
    let signals = [
        Signal::SIGINT,
        Signal::SIGQUIT,
        Signal::SIGTSTP,
        Signal::SIGTTOU,
        Signal::SIGTTIN,
        Signal::SIGPIPE,
    ];

    for sig in signals {
        unsafe {
            signal(sig, SigHandler::SigDfl).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to reset {:?} to SIG_DFL: {}", sig, e),
                )
            })?;
        }
    }

    // CRITICAL: Also unblock these signals!
    // Even with SIG_DFL handler, blocked signals won't be delivered
    let mut sigset = SigSet::empty();
    for sig in signals {
        sigset.add(sig);
    }
    sigprocmask(SigmaskHow::SIG_UNBLOCK, Some(&sigset), None).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Failed to unblock job control signals: {}", e),
        )
    })?;

    Ok(())
}
