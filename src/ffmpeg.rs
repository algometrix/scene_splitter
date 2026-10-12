//! The one way this program runs ffmpeg and ffprobe to completion.
//!
//! Every run has a time limit and is stopped when the limit passes, so a
//! video on a drive that went away, or a file ffmpeg chokes on, cannot hang
//! a scan forever. Every failure is one case of [`FfmpegError`], so callers
//! say once what they were doing and leave the rest to the error.
//!
//! The player is the exception: its decoders run for as long as the video
//! plays, and it manages them itself.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, ErrorKind, Read};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// For questions answered from a file's headers alone, such as which streams
/// it holds. They take well under a second on a healthy drive. Half a minute
/// leaves room for a sleeping disk to spin up or a slow network share.
pub const QUICK_ANSWER_LIMIT: TimeLimit = TimeLimit::WholeRun(Duration::from_secs(30));

/// For work that grows with the length of the video and prints as it goes:
/// listing packets, decoding frames. No honest total can be named for it,
/// since a longer video simply takes longer. What can be named is how long
/// the program may stay silent. Output normally arrives many times a second.
/// The longest quiet moment is at the start, when a decoder works its way up
/// from the frame it can start on to the frame that was asked for, which in
/// a screen recording can be minutes of video.
pub const STEADY_OUTPUT_LIMIT: TimeLimit = TimeLimit::Silence(Duration::from_secs(120));

/// The arguments that choose what a scene file holds: the first video stream
/// and every audio stream, copied without re-encoding.
///
/// Splitting writes the scenes with these, and verification reads the
/// original and the scenes back with these. Both must select the same
/// packets, or a correct split would fail its check, so both use this one
/// list.
pub const COPY_SCENE_STREAMS: [&str; 6] = ["-map", "0:v:0", "-map", "0:a?", "-c", "copy"];

/// How much of what the program wrote about its failure is kept.
const STDERR_TAIL_BYTES: usize = 2000;
const PIPE_CHUNK_BYTES: usize = 64 * 1024;
/// How soon after its output ends the program is first asked whether it has
/// exited, and the longest pause between asking again.
const FIRST_EXIT_POLL: Duration = Duration::from_millis(1);
const SLOWEST_EXIT_POLL: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program {
    Ffmpeg,
    Ffprobe,
}

impl Program {
    /// The name to run when the program is on the PATH.
    pub fn command_name(self) -> &'static str {
        match self {
            Self::Ffmpeg => "ffmpeg",
            Self::Ffprobe => "ffprobe",
        }
    }

    pub fn file_name(self) -> String {
        format!("{}{}", self.command_name(), std::env::consts::EXE_SUFFIX)
    }
}

impl fmt::Display for Program {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.command_name())
    }
}

/// How long a run may take before it is stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeLimit {
    /// The whole run must finish within this long.
    WholeRun(Duration),
    /// The run may take as long as it needs while it keeps printing. It is
    /// stopped once it has printed nothing for this long.
    Silence(Duration),
}

impl TimeLimit {
    /// How much longer the run may go on. Zero means the limit has passed.
    fn remaining(self, started: Instant, last_output: Instant, now: Instant) -> Duration {
        let (counted_from, allowed) = match self {
            Self::WholeRun(allowed) => (started, allowed),
            Self::Silence(allowed) => (last_output, allowed),
        };
        (counted_from + allowed).saturating_duration_since(now)
    }
}

/// Why a line or a whole output could not be used, in a few plain words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotUnderstood(pub String);

/// Everything that can go wrong when running ffmpeg or ffprobe.
#[derive(Debug)]
pub enum FfmpegError {
    /// The program is missing or the system refused to run it.
    CouldNotStart { program: Program, source: io::Error },
    /// The program was stopped because its time limit passed.
    TimedOut { program: Program, limit: TimeLimit },
    /// The program ran and reported a failure.
    Failed {
        program: Program,
        status: ExitStatus,
        /// The end of what it wrote about the failure. Empty when it wrote
        /// nothing.
        stderr_tail: String,
    },
    /// The program finished, but what it printed is not what was asked for.
    OutputNotUnderstood { program: Program, detail: String },
    /// Reading from the program or waiting for it failed.
    LostContact { program: Program, source: io::Error },
}

impl FfmpegError {
    /// Whether running the same command again could plausibly succeed.
    ///
    /// A program that ran and failed, or printed something odd, sometimes
    /// does better the second time. A missing program does not appear by
    /// itself, and a run that used up its time limit would only make the
    /// person wait that long again.
    pub fn is_worth_one_more_try(&self) -> bool {
        match self {
            Self::Failed { .. } | Self::OutputNotUnderstood { .. } | Self::LostContact { .. } => {
                true
            }
            Self::CouldNotStart { .. } | Self::TimedOut { .. } => false,
        }
    }
}

impl fmt::Display for FfmpegError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CouldNotStart { program, .. } => write!(
                formatter,
                "could not start {program}, is it installed and on the PATH?"
            ),
            Self::TimedOut {
                program,
                limit: TimeLimit::WholeRun(allowed),
            } => write!(
                formatter,
                "{program} did not finish within {} and was stopped",
                in_seconds(*allowed)
            ),
            Self::TimedOut {
                program,
                limit: TimeLimit::Silence(allowed),
            } => write!(
                formatter,
                "{program} did nothing for {} and was stopped",
                in_seconds(*allowed)
            ),
            Self::Failed {
                program,
                status,
                stderr_tail,
            } if stderr_tail.is_empty() => write!(formatter, "{program} failed ({status})"),
            Self::Failed {
                program,
                status,
                stderr_tail,
            } => write!(formatter, "{program} failed ({status}): {stderr_tail}"),
            Self::OutputNotUnderstood { program, detail } => {
                write!(
                    formatter,
                    "{program} printed something unexpected: {detail}"
                )
            }
            Self::LostContact { program, .. } => {
                write!(
                    formatter,
                    "lost contact with {program} while it was running"
                )
            }
        }
    }
}

impl std::error::Error for FfmpegError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CouldNotStart { source, .. } | Self::LostContact { source, .. } => Some(source),
            Self::TimedOut { .. } | Self::Failed { .. } | Self::OutputNotUnderstood { .. } => None,
        }
    }
}

fn in_seconds(duration: Duration) -> String {
    if duration.subsec_nanos() == 0 {
        format!("{} seconds", duration.as_secs())
    } else {
        format!("{:.1} seconds", duration.as_secs_f64())
    }
}

/// One run of ffmpeg or ffprobe, ready to be given its arguments.
///
/// The program never reads from the keyboard and, on Windows, never opens a
/// console window of its own.
#[derive(Debug, Clone)]
pub struct ToolCommand {
    program: Program,
    path: PathBuf,
    arguments: Vec<OsString>,
}

impl ToolCommand {
    /// A run of the copy of `program` at `path`. Most callers want
    /// `tools::command`, which knows where the programs are.
    pub fn at(program: Program, path: impl Into<PathBuf>) -> Self {
        Self {
            program,
            path: path.into(),
            arguments: Vec::new(),
        }
    }

    pub fn arg(mut self, argument: impl AsRef<OsStr>) -> Self {
        self.arguments.push(argument.as_ref().to_os_string());
        self
    }

    pub fn args<Arguments>(mut self, arguments: Arguments) -> Self
    where
        Arguments: IntoIterator,
        Arguments::Item: AsRef<OsStr>,
    {
        self.arguments.extend(
            arguments
                .into_iter()
                .map(|argument| argument.as_ref().to_os_string()),
        );
        self
    }

    /// Runs the program to its end and returns everything it printed.
    pub fn run(self, limit: TimeLimit) -> Result<Vec<u8>, FfmpegError> {
        let mut printed = Vec::new();
        self.supervise(limit, |chunk| {
            printed.extend_from_slice(chunk);
            Ok(())
        })?;
        Ok(printed)
    }

    /// Like [`Self::run`], for a program that prints text.
    pub fn run_for_text(self, limit: TimeLimit) -> Result<String, FfmpegError> {
        let program = self.program;
        let printed = self.run(limit)?;
        String::from_utf8(printed).map_err(|_| FfmpegError::OutputNotUnderstood {
            program,
            detail: "the output is not text".to_owned(),
        })
    }

    /// Runs the program and hands each line to `on_line` as it is printed,
    /// without the line ending. The run is stopped at the first line
    /// `on_line` does not understand.
    pub fn stream_lines(
        self,
        limit: TimeLimit,
        mut on_line: impl FnMut(&str) -> Result<(), NotUnderstood>,
    ) -> Result<(), FfmpegError> {
        let program = self.program;
        let mut lines = LineSplitter::default();
        self.supervise(limit, |chunk| lines.feed(chunk, &mut on_line))?;
        // Only a run that succeeded reaches this point, so text after the
        // last line break is a real last line and not one cut short by a
        // crash.
        lines
            .finish(&mut on_line)
            .map_err(|NotUnderstood(detail)| FfmpegError::OutputNotUnderstood { program, detail })
    }

    /// Runs the program and hands its output to `on_record` in pieces of
    /// exactly `record_bytes`, as they arrive. Bytes left over at the end
    /// that do not fill a record are dropped.
    pub fn stream_records(
        self,
        record_bytes: usize,
        limit: TimeLimit,
        mut on_record: impl FnMut(&[u8]),
    ) -> Result<(), FfmpegError> {
        let mut records = RecordSplitter::new(record_bytes);
        self.supervise(limit, |chunk| {
            records.feed(chunk, &mut on_record);
            Ok(())
        })
    }

    fn spawn(&self) -> Result<Child, FfmpegError> {
        let mut command = Command::new(&self.path);
        command
            .args(&self.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console_window(&mut command);
        command
            .spawn()
            .map_err(|source| FfmpegError::CouldNotStart {
                program: self.program,
                source,
            })
    }

    /// Runs the program to its end, passing on what it prints, and stops it
    /// when the time limit passes or `on_output` rejects what it printed.
    fn supervise(
        self,
        limit: TimeLimit,
        mut on_output: impl FnMut(&[u8]) -> Result<(), NotUnderstood>,
    ) -> Result<(), FfmpegError> {
        let program = self.program;
        let mut child = self.spawn()?;
        let (Some(mut stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            // Both pipes were asked for, so this cannot happen. If it ever
            // does, the program must not be left running unwatched.
            let _ = child.kill();
            let _ = child.wait();
            return Err(FfmpegError::LostContact {
                program,
                source: io::Error::other("the program was started without its output pipes"),
            });
        };

        let child = &Mutex::new(child);
        let clock = &OutputClock::starting_now();
        let (run_ended, run_ended_signal) = mpsc::channel::<()>();

        let (pumped, status, timed_out, stderr_tail) = std::thread::scope(|scope| {
            // Read on its own thread, or a program that writes a lot about
            // its problems would fill the pipe and wait forever.
            let stderr_reader = scope.spawn(move || read_tail(stderr, STDERR_TAIL_BYTES));
            let watchdog =
                scope.spawn(move || stop_when_overdue(child, limit, clock, &run_ended_signal));

            let pumped = pump(&mut stdout, clock, &mut on_output);
            if pumped.is_err() {
                // Nothing more can be learned from this run. Stopping a
                // program that already exited fails, which is fine.
                let _ = lock(child).kill();
            }
            let status = wait_for_exit(child);
            drop(run_ended);

            let timed_out = watchdog
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            let stderr_tail = stderr_reader
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            (pumped, status, timed_out, stderr_tail)
        });

        // A stopped program also fails and may cut its output short, so the
        // time limit is the cause to report when it passed.
        if timed_out {
            return Err(FfmpegError::TimedOut { program, limit });
        }
        match pumped {
            Ok(()) => {}
            Err(PumpFailure::Rejected(NotUnderstood(detail))) => {
                return Err(FfmpegError::OutputNotUnderstood { program, detail });
            }
            Err(PumpFailure::Unreadable(source)) => {
                return Err(FfmpegError::LostContact { program, source });
            }
        }
        let status = status.map_err(|source| FfmpegError::LostContact { program, source })?;
        if !status.success() {
            return Err(FfmpegError::Failed {
                program,
                status,
                stderr_tail,
            });
        }
        Ok(())
    }
}

/// Keeps a program started from the window from opening a console window of
/// its own on Windows. Does nothing elsewhere.
pub fn hide_console_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        /// The Windows process creation flag CREATE_NO_WINDOW.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

/// When the run began and when it last printed something.
struct OutputClock {
    started: Instant,
    last_output: Mutex<Instant>,
}

impl OutputClock {
    fn starting_now() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last_output: Mutex::new(now),
        }
    }

    fn output_arrived(&self) {
        *lock(&self.last_output) = Instant::now();
    }

    fn last_output(&self) -> Instant {
        *lock(&self.last_output)
    }
}

/// A panic elsewhere while the lock was held does not make the value behind
/// it wrong: it is a process handle or a point in time.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

enum PumpFailure {
    Rejected(NotUnderstood),
    Unreadable(io::Error),
}

/// Passes on everything the program prints until it closes its output.
fn pump(
    stdout: &mut impl Read,
    clock: &OutputClock,
    on_output: &mut impl FnMut(&[u8]) -> Result<(), NotUnderstood>,
) -> Result<(), PumpFailure> {
    let mut chunk = vec![0u8; PIPE_CHUNK_BYTES];
    loop {
        let received = match stdout.read(&mut chunk) {
            Ok(0) => return Ok(()),
            Ok(received) => received,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(PumpFailure::Unreadable(error)),
        };
        clock.output_arrived();
        on_output(&chunk[..received]).map_err(PumpFailure::Rejected)?;
    }
}

/// Waits until the run ends or its time limit passes, and stops the program
/// in the second case. Returns whether it had to.
fn stop_when_overdue(
    child: &Mutex<Child>,
    limit: TimeLimit,
    clock: &OutputClock,
    run_ended: &Receiver<()>,
) -> bool {
    loop {
        let remaining = limit.remaining(clock.started, clock.last_output(), Instant::now());
        if remaining.is_zero() {
            let mut child = lock(child);
            // The program may have finished in this same instant, which is
            // not a timeout.
            if matches!(child.try_wait(), Ok(Some(_))) {
                return false;
            }
            // Stopping it closes its pipes, which ends the reading on the
            // other threads. This fails only when it has already exited.
            let _ = child.kill();
            return true;
        }
        match run_ended.recv_timeout(remaining) {
            // Either the limit has passed or newer output has moved it.
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
}

/// Waits for a program that has closed its output to exit.
///
/// It asks again and again instead of blocking, because a blocking wait
/// would hold the process handle and the watchdog could not stop a program
/// that closes its output and then hangs.
fn wait_for_exit(child: &Mutex<Child>) -> io::Result<ExitStatus> {
    let mut pause = FIRST_EXIT_POLL;
    loop {
        if let Some(status) = lock(child).try_wait()? {
            return Ok(status);
        }
        std::thread::sleep(pause);
        pause = (pause * 2).min(SLOWEST_EXIT_POLL);
    }
}

/// Reads everything and returns the end of it as text. An unreadable pipe
/// ends the reading: what was read so far is still the best description of
/// the failure there is.
fn read_tail(mut stderr: impl Read, tail_bytes: usize) -> String {
    let mut kept = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stderr.read(&mut chunk) {
            Ok(0) => break,
            Ok(received) => {
                kept.extend_from_slice(&chunk[..received]);
                keep_only_the_tail(&mut kept, tail_bytes);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&kept).trim().to_owned()
}

fn keep_only_the_tail(bytes: &mut Vec<u8>, tail_bytes: usize) {
    if bytes.len() > tail_bytes {
        bytes.drain(..bytes.len() - tail_bytes);
    }
}

/// Cuts a stream of bytes into lines, however the pipe happened to chunk it.
#[derive(Default)]
struct LineSplitter {
    /// The start of a line whose end has not arrived yet.
    unfinished: Vec<u8>,
}

impl LineSplitter {
    fn feed(
        &mut self,
        chunk: &[u8],
        on_line: &mut impl FnMut(&str) -> Result<(), NotUnderstood>,
    ) -> Result<(), NotUnderstood> {
        let mut rest = chunk;
        while let Some(line_break) = rest.iter().position(|&byte| byte == b'\n') {
            let (line_end, after) = rest.split_at(line_break);
            if self.unfinished.is_empty() {
                on_line(as_line(line_end)?)?;
            } else {
                self.unfinished.extend_from_slice(line_end);
                on_line(as_line(&self.unfinished)?)?;
                self.unfinished.clear();
            }
            rest = &after[1..];
        }
        self.unfinished.extend_from_slice(rest);
        Ok(())
    }

    /// Hands over a last line that did not end in a line break.
    fn finish(
        self,
        on_line: &mut impl FnMut(&str) -> Result<(), NotUnderstood>,
    ) -> Result<(), NotUnderstood> {
        if self.unfinished.is_empty() {
            return Ok(());
        }
        on_line(as_line(&self.unfinished)?)
    }
}

fn as_line(bytes: &[u8]) -> Result<&str, NotUnderstood> {
    let line =
        std::str::from_utf8(bytes).map_err(|_| NotUnderstood("a line is not text".to_owned()))?;
    Ok(line.strip_suffix('\r').unwrap_or(line))
}

/// Cuts a stream of bytes into records of one fixed size, however the pipe
/// happened to chunk it.
struct RecordSplitter {
    record_bytes: usize,
    /// The start of a record whose end has not arrived yet.
    unfinished: Vec<u8>,
}

impl RecordSplitter {
    fn new(record_bytes: usize) -> Self {
        assert!(record_bytes > 0, "a record must hold at least one byte");
        Self {
            record_bytes,
            unfinished: Vec::with_capacity(record_bytes),
        }
    }

    fn feed(&mut self, chunk: &[u8], on_record: &mut impl FnMut(&[u8])) {
        let mut rest = chunk;
        if !self.unfinished.is_empty() {
            let missing = self.record_bytes - self.unfinished.len();
            let (completing, after) = rest.split_at(missing.min(rest.len()));
            self.unfinished.extend_from_slice(completing);
            rest = after;
            if self.unfinished.len() < self.record_bytes {
                return;
            }
            on_record(&self.unfinished);
            self.unfinished.clear();
        }
        let mut records = rest.chunks_exact(self.record_bytes);
        for record in &mut records {
            on_record(record);
        }
        self.unfinished.extend_from_slice(records.remainder());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_of(chunks: &[&[u8]]) -> Result<Vec<String>, NotUnderstood> {
        let mut lines = Vec::new();
        let mut collect = |line: &str| {
            lines.push(line.to_owned());
            Ok(())
        };
        let mut splitter = LineSplitter::default();
        for chunk in chunks {
            splitter.feed(chunk, &mut collect)?;
        }
        splitter.finish(&mut collect)?;
        Ok(lines)
    }

    fn records_of(record_bytes: usize, chunks: &[&[u8]]) -> Vec<Vec<u8>> {
        let mut records = Vec::new();
        let mut splitter = RecordSplitter::new(record_bytes);
        for chunk in chunks {
            splitter.feed(chunk, &mut |record: &[u8]| records.push(record.to_vec()));
        }
        records
    }

    #[test]
    fn lines_are_split_at_line_breaks() {
        assert_eq!(lines_of(&[b"one\ntwo\n"]).unwrap(), vec!["one", "two"]);
    }

    #[test]
    fn a_line_spread_over_several_chunks_arrives_whole() {
        assert_eq!(
            lines_of(&[b"on", b"e\ntw", b"", b"o\n"]).unwrap(),
            vec!["one", "two"]
        );
    }

    #[test]
    fn a_last_line_without_a_line_break_still_arrives() {
        assert_eq!(lines_of(&[b"one\ntwo"]).unwrap(), vec!["one", "two"]);
    }

    #[test]
    fn windows_line_endings_are_removed() {
        assert_eq!(lines_of(&[b"one\r\ntwo\r\n"]).unwrap(), vec!["one", "two"]);
    }

    #[test]
    fn empty_lines_are_passed_on() {
        assert_eq!(
            lines_of(&[b"one\n\ntwo\n"]).unwrap(),
            vec!["one", "", "two"]
        );
    }

    #[test]
    fn a_line_that_is_not_text_is_rejected() {
        assert!(lines_of(&[b"one\n\xff\xfe\n"]).is_err());
    }

    #[test]
    fn a_rejected_line_stops_the_splitting() {
        let mut seen = 0;
        let mut reject_the_second = |_: &str| {
            seen += 1;
            if seen == 2 {
                Err(NotUnderstood("no".to_owned()))
            } else {
                Ok(())
            }
        };
        let outcome = LineSplitter::default().feed(b"a\nb\nc\n", &mut reject_the_second);
        assert_eq!(outcome, Err(NotUnderstood("no".to_owned())));
        assert_eq!(seen, 2);
    }

    #[test]
    fn records_are_cut_at_their_size() {
        assert_eq!(
            records_of(2, &[b"aabbcc"]),
            vec![b"aa".to_vec(), b"bb".to_vec(), b"cc".to_vec()]
        );
    }

    #[test]
    fn a_record_spread_over_several_chunks_arrives_whole() {
        assert_eq!(
            records_of(4, &[b"a", b"a", b"aab", b"bbbcc", b"cc"]),
            vec![b"aaaa".to_vec(), b"bbbb".to_vec(), b"cccc".to_vec()]
        );
    }

    #[test]
    fn bytes_that_do_not_fill_a_record_are_dropped() {
        assert_eq!(records_of(4, &[b"aaaabb"]), vec![b"aaaa".to_vec()]);
        assert!(records_of(4, &[b"aa"]).is_empty());
    }

    #[test]
    fn only_the_end_of_a_long_complaint_is_kept() {
        let mut bytes = b"0123456789".to_vec();
        keep_only_the_tail(&mut bytes, 4);
        assert_eq!(bytes, b"6789");

        let mut short = b"01".to_vec();
        keep_only_the_tail(&mut short, 4);
        assert_eq!(short, b"01");
    }

    #[test]
    fn the_tail_of_what_was_written_is_returned_as_trimmed_text() {
        let written: &[u8] = b"first line\nlast line\n";
        assert_eq!(read_tail(written, 10), "last line");
        assert_eq!(read_tail(written, 1000), "first line\nlast line");
    }

    const LIMIT: Duration = Duration::from_secs(10);

    #[test]
    fn a_whole_run_limit_counts_from_the_start_whatever_was_printed() {
        let started = Instant::now();
        let last_output = started + Duration::from_secs(9);
        let limit = TimeLimit::WholeRun(LIMIT);

        let before = started + Duration::from_secs(4);
        assert_eq!(
            limit.remaining(started, last_output, before),
            Duration::from_secs(6)
        );
        let after = started + Duration::from_secs(11);
        assert!(limit.remaining(started, last_output, after).is_zero());
    }

    #[test]
    fn a_silence_limit_counts_from_the_last_output() {
        let started = Instant::now();
        let last_output = started + Duration::from_secs(9);
        let limit = TimeLimit::Silence(LIMIT);

        // Eleven seconds in, but only two since the program last printed.
        let still_printing = started + Duration::from_secs(11);
        assert_eq!(
            limit.remaining(started, last_output, still_printing),
            Duration::from_secs(8)
        );
        let gone_quiet = last_output + LIMIT;
        assert!(limit.remaining(started, last_output, gone_quiet).is_zero());
    }

    #[test]
    fn a_program_that_does_not_exist_could_not_start() {
        let missing = ToolCommand::at(Program::Ffmpeg, "no_such_folder/no_such_program");
        let error = missing.run(QUICK_ANSWER_LIMIT).unwrap_err();

        assert!(
            matches!(
                error,
                FfmpegError::CouldNotStart {
                    program: Program::Ffmpeg,
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(!error.is_worth_one_more_try());
        assert_eq!(
            error.to_string(),
            "could not start ffmpeg, is it installed and on the PATH?"
        );
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn each_time_limit_is_named_in_its_own_words() {
        let whole_run = FfmpegError::TimedOut {
            program: Program::Ffprobe,
            limit: TimeLimit::WholeRun(Duration::from_secs(30)),
        };
        assert_eq!(
            whole_run.to_string(),
            "ffprobe did not finish within 30 seconds and was stopped"
        );

        let silence = FfmpegError::TimedOut {
            program: Program::Ffmpeg,
            limit: TimeLimit::Silence(Duration::from_millis(1500)),
        };
        assert_eq!(
            silence.to_string(),
            "ffmpeg did nothing for 1.5 seconds and was stopped"
        );
        assert!(!silence.is_worth_one_more_try());
    }

    #[test]
    fn output_that_was_not_understood_says_what_was_wrong() {
        let error = FfmpegError::OutputNotUnderstood {
            program: Program::Ffprobe,
            detail: "the output is not text".to_owned(),
        };
        assert_eq!(
            error.to_string(),
            "ffprobe printed something unexpected: the output is not text"
        );
        assert!(error.is_worth_one_more_try());
    }

    /// A run of the ffmpeg on the PATH, for the tests that need a real one.
    fn ffmpeg() -> ToolCommand {
        ToolCommand::at(Program::Ffmpeg, Program::Ffmpeg.command_name())
    }

    /// Plays a test pattern at its real speed for a minute, printing
    /// nothing: a stand-in for a program that hangs.
    fn ffmpeg_busy_for_a_minute() -> ToolCommand {
        ffmpeg()
            .args(["-nostdin", "-v", "error", "-re"])
            .args(["-f", "lavfi", "-i", "testsrc=duration=60:size=64x36:rate=5"])
            .args(["-f", "null", "-"])
    }

    const TEST_LIMIT: Duration = Duration::from_millis(400);
    /// Far less than the minute the stand-in would run if nothing stopped it.
    const STOPPED_WELL_WITHIN: Duration = Duration::from_secs(20);

    #[test]
    #[ignore = "needs ffmpeg on the PATH"]
    fn a_run_that_outlasts_its_limit_is_stopped() {
        let limit = TimeLimit::WholeRun(TEST_LIMIT);
        let started = Instant::now();
        let error = ffmpeg_busy_for_a_minute().run(limit).unwrap_err();

        assert!(
            matches!(error, FfmpegError::TimedOut { limit: reported, .. } if reported == limit),
            "{error:?}"
        );
        assert!(started.elapsed() < STOPPED_WELL_WITHIN);
    }

    #[test]
    #[ignore = "needs ffmpeg on the PATH"]
    fn a_run_that_goes_silent_is_stopped() {
        let limit = TimeLimit::Silence(TEST_LIMIT);
        let started = Instant::now();
        let error = ffmpeg_busy_for_a_minute()
            .stream_lines(limit, |_line| Ok(()))
            .unwrap_err();

        assert!(
            matches!(error, FfmpegError::TimedOut { limit: reported, .. } if reported == limit),
            "{error:?}"
        );
        assert!(started.elapsed() < STOPPED_WELL_WITHIN);
    }

    #[test]
    #[ignore = "needs ffmpeg on the PATH"]
    fn a_failed_run_carries_what_the_program_wrote_about_it() {
        let error = ffmpeg()
            .args(["-nostdin", "-v", "error", "-i", "no_such_video.mp4"])
            .args(["-f", "null", "-"])
            .run(QUICK_ANSWER_LIMIT)
            .unwrap_err();

        let FfmpegError::Failed { stderr_tail, .. } = &error else {
            panic!("{error:?}");
        };
        assert!(stderr_tail.contains("no_such_video.mp4"), "{stderr_tail}");
        assert!(error.is_worth_one_more_try());
    }

    #[test]
    #[ignore = "needs ffmpeg on the PATH"]
    fn a_run_returns_what_the_program_printed() {
        let printed = ffmpeg()
            .arg("-version")
            .run_for_text(QUICK_ANSWER_LIMIT)
            .unwrap();
        assert!(printed.starts_with("ffmpeg version"), "{printed}");
    }

    #[test]
    #[ignore = "needs ffmpeg on the PATH"]
    fn records_arrive_one_per_decoded_frame() {
        const FRAME_BYTES: usize = 16 * 16;
        const FRAMES: usize = 7;
        let mut frames = 0;
        ffmpeg()
            .args(["-nostdin", "-v", "error"])
            .args(["-f", "lavfi", "-i", "testsrc=size=16x16:rate=5"])
            .args(["-frames:v", &FRAMES.to_string()])
            .args(["-pix_fmt", "gray", "-f", "rawvideo", "-"])
            .stream_records(FRAME_BYTES, STEADY_OUTPUT_LIMIT, |frame| {
                assert_eq!(frame.len(), FRAME_BYTES);
                frames += 1;
            })
            .unwrap();
        assert_eq!(frames, FRAMES);
    }

    #[test]
    #[ignore = "needs ffmpeg on the PATH"]
    fn a_line_the_caller_rejects_stops_the_run() {
        let started = Instant::now();
        // Lists a checksum per frame for a minute at real speed, unless
        // stopped.
        let error = ffmpeg()
            .args(["-nostdin", "-v", "error", "-re"])
            .args(["-f", "lavfi", "-i", "testsrc=duration=60:size=64x36:rate=5"])
            .args(["-f", "framecrc", "-"])
            .stream_lines(STEADY_OUTPUT_LIMIT, |_line| {
                Err(NotUnderstood("not wanted".to_owned()))
            })
            .unwrap_err();

        assert!(
            matches!(&error, FfmpegError::OutputNotUnderstood { detail, .. } if detail == "not wanted"),
            "{error:?}"
        );
        assert!(started.elapsed() < STOPPED_WELL_WITHIN);
    }
}
