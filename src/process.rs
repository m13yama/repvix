use crate::{Error, Result};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessSpec {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub capture: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessOutput {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub trait CommandRunner {
    fn run(&self, spec: &ProcessSpec) -> Result<ProcessOutput>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, spec: &ProcessSpec) -> Result<ProcessOutput> {
        let mut command = Command::new(&spec.program);
        command.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }

        if spec.capture {
            let output = command.output().map_err(|error| {
                Error::io(
                    format!("failed to execute {}", spec.program.to_string_lossy()),
                    error,
                )
            })?;
            Ok(ProcessOutput {
                success: output.status.success(),
                code: output.status.code(),
                stdout: output.stdout,
                stderr: output.stderr,
            })
        } else {
            let status = command
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()
                .map_err(|error| {
                    Error::io(
                        format!("failed to execute {}", spec.program.to_string_lossy()),
                        error,
                    )
                })?;
            Ok(ProcessOutput {
                success: status.success(),
                code: status.code(),
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        }
    }
}
