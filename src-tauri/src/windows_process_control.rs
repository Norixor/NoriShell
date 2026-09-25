//! Windows process-tree ownership through a Job Object assigned at process creation.

use std::{
    ffi::{OsStr, OsString, c_void},
    fs::File,
    io::{self, Write},
    mem::{ManuallyDrop, size_of, size_of_val},
    os::windows::{ffi::OsStrExt, io::FromRawHandle},
    path::Path,
    ptr::{null, null_mut},
    thread,
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{
        CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, STILL_ACTIVE,
        SetHandleInformation, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Security::SECURITY_ATTRIBUTES,
    System::{
        Console::{COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole},
        JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
            QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
        },
        Pipes::CreatePipe,
        Threading::{
            CREATE_NEW_PROCESS_GROUP, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
            DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess,
            InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            PROC_THREAD_ATTRIBUTE_JOB_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
            PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess,
            UpdateProcThreadAttribute, WaitForSingleObject,
        },
    },
};

const ORDINARY_TERMINATION_EXIT_CODE: u32 = 0x4E56_5854;
const FORCED_TERMINATION_EXIT_CODE: u32 = 0x4E56_584B;

#[derive(Clone, Copy)]
enum JobBreakawayPolicy {
    Deny,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminationOutcome {
    AlreadyExited,
    Interrupted,
    Terminated,
    Killed,
    Unknown,
}

#[derive(Debug, Clone, Copy)]
pub struct TerminationPolicy {
    interrupt_wait: Duration,
    terminate_wait: Duration,
    kill_wait: Duration,
}

impl TerminationPolicy {
    #[must_use]
    pub const fn with_timeouts(
        interrupt_wait: Duration,
        terminate_wait: Duration,
        kill_wait: Duration,
    ) -> Self {
        Self {
            interrupt_wait,
            terminate_wait,
            kill_wait,
        }
    }
}

impl Default for TerminationPolicy {
    fn default() -> Self {
        Self::with_timeouts(
            Duration::from_secs(5),
            Duration::from_secs(5),
            Duration::from_secs(3),
        )
    }
}

/// Owns a Windows process tree whose root is placed in a Job Object by
/// `PROC_THREAD_ATTRIBUTE_JOB_LIST` before its first instruction can run.
pub struct ManagedJobProcess {
    job: OwnedHandle,
    root_process: OwnedHandle,
    process_id: u32,
    fully_stopped: bool,
}

pub struct ManagedStdio {
    pub stdin: File,
    pub stdout: File,
    pub stderr: File,
}

/// One ConPTY root process and its ordinary descendants, assigned to a private
/// Job Object before the root executes its first instruction. Programs can
/// explicitly launch detached children outside the Job.
pub struct ManagedConPtyProcess {
    process: ManagedJobProcess,
    input: File,
    output: File,
    pseudo_console: PseudoConsole,
}

impl ManagedJobProcess {
    /// Starts one exact executable without a shell and inherits the Core's
    /// environment. Standard handles are not explicitly inherited; ConPTY and
    /// redirected-I/O launchers use their own typed creation path.
    pub fn spawn(
        executable: &Path,
        arguments: &[OsString],
        current_directory: Option<&Path>,
    ) -> io::Result<Self> {
        let (process, stdio) =
            Self::spawn_internal(executable, arguments, current_directory, false, true)?;
        debug_assert!(stdio.is_none());
        Ok(process)
    }

    /// Starts one exact executable with three anonymous pipes. Only the child
    /// pipe ends are inheritable, and the explicit handle list prevents other
    /// Core handles from leaking into the managed process.
    pub fn spawn_with_stdio(
        executable: &Path,
        arguments: &[OsString],
        current_directory: Option<&Path>,
    ) -> io::Result<(Self, ManagedStdio)> {
        let (process, stdio) =
            Self::spawn_internal(executable, arguments, current_directory, true, true)?;
        let stdio = stdio.ok_or_else(|| io::Error::other("managed stdio was not created"))?;
        Ok((process, stdio))
    }

    /// Starts an exact executable with redirected standard I/O and an empty
    /// environment block. Plugin Host uses this path so Core environment
    /// variables cannot become an ambient plugin capability.
    pub fn spawn_with_stdio_empty_environment(
        executable: &Path,
        arguments: &[OsString],
        current_directory: Option<&Path>,
    ) -> io::Result<(Self, ManagedStdio)> {
        let (process, stdio) =
            Self::spawn_internal(executable, arguments, current_directory, true, false)?;
        let stdio = stdio.ok_or_else(|| io::Error::other("managed stdio was not created"))?;
        Ok((process, stdio))
    }

    fn spawn_internal(
        executable: &Path,
        arguments: &[OsString],
        current_directory: Option<&Path>,
        redirect_stdio: bool,
        inherit_environment: bool,
    ) -> io::Result<(Self, Option<ManagedStdio>)> {
        validate_launch_inputs(executable, current_directory)?;

        let application_name = encode_null_terminated(executable.as_os_str())?;
        let mut command_line = windows_command_line(executable.as_os_str(), arguments)?;
        let current_directory = current_directory
            .map(|directory| encode_null_terminated(directory.as_os_str()))
            .transpose()?;

        let mut stdio_pipes = redirect_stdio.then(ChildStdioPipes::new).transpose()?;
        let job = create_kill_on_close_job(JobBreakawayPolicy::Deny)?;
        let attribute_count = if redirect_stdio { 2 } else { 1 };
        let mut attributes = ProcessAttributeList::new(attribute_count)?;
        let job_handles = [job.raw()];
        attributes.set_raw(
            PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
            job_handles.as_ptr().cast(),
            size_of_val(&job_handles),
        )?;
        let inherited_handles = stdio_pipes.as_ref().map(ChildStdioPipes::child_handles);
        if let Some(handles) = inherited_handles.as_ref() {
            attributes.set_raw(
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                size_of_val(handles),
            )?;
        }

        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = u32::try_from(size_of::<STARTUPINFOEXW>())
            .map_err(|_| io::Error::other("Windows startup structure is too large"))?;
        startup.lpAttributeList = attributes.raw();
        if let Some(pipes) = stdio_pipes.as_ref() {
            startup.StartupInfo.dwFlags |= STARTF_USESTDHANDLES;
            startup.StartupInfo.hStdInput = pipes.child_stdin.raw();
            startup.StartupInfo.hStdOutput = pipes.child_stdout.raw();
            startup.StartupInfo.hStdError = pipes.child_stderr.raw();
        }
        let mut process = PROCESS_INFORMATION::default();
        let directory_pointer = current_directory
            .as_ref()
            .map_or(null(), |value| value.as_ptr());
        let empty_environment = [0u16, 0u16];
        let environment_pointer = if inherit_environment {
            null()
        } else {
            empty_environment.as_ptr().cast()
        };
        let creation_flags = EXTENDED_STARTUPINFO_PRESENT
            | CREATE_NEW_PROCESS_GROUP
            | if inherit_environment {
                0
            } else {
                CREATE_UNICODE_ENVIRONMENT
            };

        // SAFETY: every pointer references a live, correctly sized buffer for
        // this call. lpApplicationName is explicit, so command-line whitespace
        // cannot redirect execution to a different executable.
        let created = unsafe {
            CreateProcessW(
                application_name.as_ptr(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                i32::from(redirect_stdio),
                creation_flags,
                environment_pointer,
                directory_pointer,
                (&raw const startup.StartupInfo).cast(),
                &raw mut process,
            )
        };
        if created == 0 {
            return Err(io::Error::last_os_error());
        }

        let managed = Self::from_created_process(job, process)?;
        let stdio = stdio_pipes.take().map(ChildStdioPipes::into_parent_stdio);
        Ok((managed, stdio))
    }

    fn from_created_process(job: OwnedHandle, process: PROCESS_INFORMATION) -> io::Result<Self> {
        let root_process = OwnedHandle::new(process.hProcess)?;
        let thread_handle = OwnedHandle::new(process.hThread)?;
        drop(thread_handle);
        let managed = Self {
            job,
            root_process,
            process_id: process.dwProcessId,
            fully_stopped: false,
        };
        if managed.active_process_count()? == 0 {
            return Err(io::Error::other(
                "new process was not assigned to its managed Job Object",
            ));
        }
        Ok(managed)
    }

    #[must_use]
    pub const fn process_id(&self) -> u32 {
        self.process_id
    }

    /// Waits for both the root process and every process in the private Job to
    /// stop, then returns the root exit code. A timeout is not success and does
    /// not release ownership of the still-running process tree.
    pub fn wait_for_exit(&mut self, timeout: Duration) -> io::Result<Option<u32>> {
        if !self.wait_until_stopped(timeout)? {
            return Ok(None);
        }
        let mut exit_code = u32::MAX;
        // SAFETY: the root process handle remains valid and `exit_code` is
        // writable for the duration of the call.
        if unsafe { GetExitCodeProcess(self.root_process.raw(), &raw mut exit_code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if exit_code == STILL_ACTIVE as u32 {
            return Err(io::Error::other(
                "stopped managed process still reports an active exit code",
            ));
        }
        Ok(Some(exit_code))
    }

    /// Runs an adapter-specific graceful action, then terminates the root, and
    /// finally kills the complete Job. Only an observed empty Job is success.
    pub fn terminate_with<F>(
        &mut self,
        policy: TerminationPolicy,
        graceful_interrupt: F,
    ) -> TerminationOutcome
    where
        F: FnOnce(u32) -> io::Result<()>,
    {
        match self.terminate_checked(policy, graceful_interrupt) {
            Ok(outcome) => outcome,
            Err(_) => TerminationOutcome::Unknown,
        }
    }

    pub fn terminate_checked<F>(
        &mut self,
        policy: TerminationPolicy,
        graceful_interrupt: F,
    ) -> io::Result<TerminationOutcome>
    where
        F: FnOnce(u32) -> io::Result<()>,
    {
        self.terminate_inner(policy, graceful_interrupt)
    }

    pub fn poll_root_exit(&self) -> io::Result<Option<u32>> {
        if !self.root_has_exited()? {
            return Ok(None);
        }
        let mut exit_code = u32::MAX;
        // SAFETY: the root process handle remains valid and `exit_code` is
        // writable for the duration of the call.
        if unsafe { GetExitCodeProcess(self.root_process.raw(), &raw mut exit_code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if exit_code == STILL_ACTIVE as u32 {
            return Err(io::Error::other(
                "exited managed process still reports an active exit code",
            ));
        }
        Ok(Some(exit_code))
    }

    fn terminate_inner<F>(
        &mut self,
        policy: TerminationPolicy,
        graceful_interrupt: F,
    ) -> io::Result<TerminationOutcome>
    where
        F: FnOnce(u32) -> io::Result<()>,
    {
        if self.wait_until_stopped(Duration::ZERO)? {
            return Ok(TerminationOutcome::AlreadyExited);
        }

        let _ = graceful_interrupt(self.process_id);
        if self.wait_until_stopped(policy.interrupt_wait)? {
            return Ok(TerminationOutcome::Interrupted);
        }

        // SAFETY: the process handle remains owned by self. Failure is not
        // treated as completion; the full Job is still queried and killed.
        unsafe {
            TerminateProcess(self.root_process.raw(), ORDINARY_TERMINATION_EXIT_CODE);
        }
        if self.wait_until_stopped(policy.terminate_wait)? {
            return Ok(TerminationOutcome::Terminated);
        }

        // SAFETY: the Job handle remains valid and owns all assigned processes.
        if unsafe { TerminateJobObject(self.job.raw(), FORCED_TERMINATION_EXIT_CODE) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if self.wait_until_stopped(policy.kill_wait)? {
            return Ok(TerminationOutcome::Killed);
        }
        Ok(TerminationOutcome::Unknown)
    }

    fn wait_until_stopped(&mut self, timeout: Duration) -> io::Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            let root_exited = self.root_has_exited()?;
            let active_processes = self.active_process_count()?;
            if root_exited && active_processes == 0 {
                self.fully_stopped = true;
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn root_has_exited(&self) -> io::Result<bool> {
        // SAFETY: the root process handle remains owned by self.
        match unsafe { WaitForSingleObject(self.root_process.raw(), 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(io::Error::last_os_error()),
        }
    }

    fn active_process_count(&self) -> io::Result<u32> {
        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: accounting is writable and its exact size is supplied.
        let queried = unsafe {
            QueryInformationJobObject(
                self.job.raw(),
                JobObjectBasicAccountingInformation,
                (&raw mut accounting).cast(),
                u32::try_from(size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>()).map_err(
                    |_| io::Error::other("Windows Job accounting structure is too large"),
                )?,
                null_mut(),
            )
        };
        if queried == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(accounting.ActiveProcesses)
        }
    }
}

impl ManagedConPtyProcess {
    /// Starts an exact executable inside a new ConPTY. The pseudo-console and
    /// Job Object attributes are installed on the same STARTUPINFOEX before
    /// CreateProcessW, so the root cannot escape between spawn and assignment.
    pub fn spawn(
        executable: &Path,
        arguments: &[OsString],
        current_directory: Option<&Path>,
        rows: u16,
        cols: u16,
    ) -> io::Result<Self> {
        validate_launch_inputs(executable, current_directory)?;
        let dimensions = conpty_dimensions(rows, cols)?;

        let (console_input, parent_input) = private_pipe()?;
        let (parent_output, console_output) = private_pipe()?;
        let pseudo_console =
            PseudoConsole::new(dimensions, console_input.raw(), console_output.raw())?;
        drop((console_input, console_output));

        let application_name = encode_null_terminated(executable.as_os_str())?;
        let mut command_line = windows_command_line(executable.as_os_str(), arguments)?;
        let current_directory = current_directory
            .map(|directory| encode_null_terminated(directory.as_os_str()))
            .transpose()?;

        // A terminal is user-controlled: tools such as Codex may explicitly
        // detach a daemon. Ordinary descendants still remain in this Job.
        let job = create_kill_on_close_job(JobBreakawayPolicy::Explicit)?;
        let mut attributes = ProcessAttributeList::new(2)?;
        let job_handles = [job.raw()];
        attributes.set_raw(
            PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
            job_handles.as_ptr().cast(),
            size_of_val(&job_handles),
        )?;
        attributes.set_raw(
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            pseudo_console.raw() as *const c_void,
            size_of::<HPCON>(),
        )?;

        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = u32::try_from(size_of::<STARTUPINFOEXW>())
            .map_err(|_| io::Error::other("Windows startup structure is too large"))?;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
        startup.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
        startup.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
        startup.lpAttributeList = attributes.raw();

        let directory_pointer = current_directory
            .as_ref()
            .map_or(null(), |value| value.as_ptr());
        let mut process = PROCESS_INFORMATION::default();
        // SAFETY: all buffers and attribute values remain live through this
        // call. No anonymous pipe handle is inherited by the child; ConPTY and
        // the Job Object are transferred only through STARTUPINFOEX.
        let created = unsafe {
            CreateProcessW(
                application_name.as_ptr(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT
                    | CREATE_NEW_PROCESS_GROUP
                    | CREATE_UNICODE_ENVIRONMENT,
                null(),
                directory_pointer,
                (&raw const startup.StartupInfo).cast(),
                &raw mut process,
            )
        };
        if created == 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(Self {
            process: ManagedJobProcess::from_created_process(job, process)?,
            input: parent_input.into_file(),
            output: parent_output.into_file(),
            pseudo_console,
        })
    }

    pub fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        self.pseudo_console.resize(conpty_dimensions(rows, cols)?)
    }

    #[must_use]
    pub const fn process_id(&self) -> u32 {
        self.process.process_id()
    }

    pub fn try_clone_reader(&self) -> io::Result<File> {
        self.output.try_clone()
    }

    pub fn try_clone_input_writer(&self) -> io::Result<File> {
        self.input.try_clone()
    }

    pub fn write_input(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.input.write_all(bytes)?;
        self.input.flush()
    }

    pub fn wait_for_exit(&mut self, timeout: Duration) -> io::Result<Option<u32>> {
        self.process.wait_for_exit(timeout)
    }

    pub fn poll_root_exit(&self) -> io::Result<Option<u32>> {
        self.process.poll_root_exit()
    }

    pub fn terminate(&mut self, policy: TerminationPolicy) -> TerminationOutcome {
        match self.terminate_checked(policy) {
            Ok(outcome) => outcome,
            Err(_) => TerminationOutcome::Unknown,
        }
    }

    pub fn terminate_checked(
        &mut self,
        policy: TerminationPolicy,
    ) -> io::Result<TerminationOutcome> {
        let mut input = self.input.try_clone();
        self.process.terminate_checked(policy, move |_| {
            let input = input.as_mut().map_err(|error| {
                io::Error::new(error.kind(), "ConPTY interrupt channel unavailable")
            })?;
            input.write_all(&[0x03])?;
            input.flush()
        })
    }

    /// Terminates the Job without writing to the ConPTY input pipe. This is
    /// required when another writer may be blocked by pipe backpressure: a
    /// second synchronous input write cannot be used as the cancellation path.
    pub fn terminate_without_input_checked(
        &mut self,
        policy: TerminationPolicy,
    ) -> io::Result<TerminationOutcome> {
        self.process.terminate_checked(policy, |_| Ok(()))
    }
}

fn validate_launch_inputs(executable: &Path, current_directory: Option<&Path>) -> io::Result<()> {
    if !executable.is_absolute() || !executable.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "managed executable must be an existing absolute file",
        ));
    }
    if let Some(directory) = current_directory
        && (!directory.is_absolute() || !directory.is_dir())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "managed working directory must be an existing absolute directory",
        ));
    }
    Ok(())
}

fn conpty_dimensions(rows: u16, cols: u16) -> io::Result<COORD> {
    if rows == 0 || cols == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ConPTY dimensions must be non-zero",
        ));
    }
    Ok(COORD {
        X: i16::try_from(cols)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "ConPTY width exceeds i16"))?,
        Y: i16::try_from(rows).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "ConPTY height exceeds i16")
        })?,
    })
}

struct PseudoConsole(HPCON);

impl PseudoConsole {
    fn new(size: COORD, input: HANDLE, output: HANDLE) -> io::Result<Self> {
        let mut handle = 0;
        // SAFETY: both pipe handles remain valid for this call and `handle` is
        // writable storage for the created pseudo-console handle.
        let result = unsafe { CreatePseudoConsole(size, input, output, 0, &raw mut handle) };
        if result < 0 {
            Err(io::Error::from_raw_os_error(result))
        } else {
            Ok(Self(handle))
        }
    }

    const fn raw(&self) -> HPCON {
        self.0
    }

    fn resize(&self, size: COORD) -> io::Result<()> {
        // SAFETY: this object uniquely owns a live pseudo-console handle.
        let result = unsafe { ResizePseudoConsole(self.raw(), size) };
        if result < 0 {
            Err(io::Error::from_raw_os_error(result))
        } else {
            Ok(())
        }
    }
}

impl Drop for PseudoConsole {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the pseudo-console handle.
        unsafe { ClosePseudoConsole(self.raw()) };
    }
}

impl Drop for ManagedJobProcess {
    fn drop(&mut self) {
        if self.fully_stopped {
            return;
        }
        // KILL_ON_JOB_CLOSE remains the final guard if this explicit bounded
        // cleanup cannot obtain a trustworthy empty-Job observation.
        unsafe {
            TerminateJobObject(self.job.raw(), FORCED_TERMINATION_EXIT_CODE);
        }
        let _ = self.wait_until_stopped(Duration::from_secs(1));
    }
}

fn create_kill_on_close_job(breakaway: JobBreakawayPolicy) -> io::Result<OwnedHandle> {
    // SAFETY: null security attributes and name create one private Job Object.
    let job = OwnedHandle::new(unsafe { CreateJobObjectW(null(), null()) })?;
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if matches!(breakaway, JobBreakawayPolicy::Explicit) {
        limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_BREAKAWAY_OK;
    }
    // SAFETY: limits is initialized and the exact structure size is supplied.
    let configured = unsafe {
        SetInformationJobObject(
            job.raw(),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                .map_err(|_| io::Error::other("Windows Job limit structure is too large"))?,
        )
    };
    if configured == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(job)
    }
}

struct ChildStdioPipes {
    parent_stdin: OwnedHandle,
    parent_stdout: OwnedHandle,
    parent_stderr: OwnedHandle,
    child_stdin: OwnedHandle,
    child_stdout: OwnedHandle,
    child_stderr: OwnedHandle,
}

impl ChildStdioPipes {
    fn new() -> io::Result<Self> {
        let (child_stdin, parent_stdin) = inheritable_pipe()?;
        parent_stdin.clear_inherit()?;

        let (parent_stdout, child_stdout) = inheritable_pipe()?;
        parent_stdout.clear_inherit()?;

        let (parent_stderr, child_stderr) = inheritable_pipe()?;
        parent_stderr.clear_inherit()?;

        Ok(Self {
            parent_stdin,
            parent_stdout,
            parent_stderr,
            child_stdin,
            child_stdout,
            child_stderr,
        })
    }

    fn child_handles(&self) -> [HANDLE; 3] {
        [
            self.child_stdin.raw(),
            self.child_stdout.raw(),
            self.child_stderr.raw(),
        ]
    }

    fn into_parent_stdio(self) -> ManagedStdio {
        let Self {
            parent_stdin,
            parent_stdout,
            parent_stderr,
            child_stdin,
            child_stdout,
            child_stderr,
        } = self;
        drop((child_stdin, child_stdout, child_stderr));
        ManagedStdio {
            stdin: parent_stdin.into_file(),
            stdout: parent_stdout.into_file(),
            stderr: parent_stderr.into_file(),
        }
    }
}

fn inheritable_pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut read_handle = null_mut();
    let mut write_handle = null_mut();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>())
            .map_err(|_| io::Error::other("Windows security attributes are too large"))?,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    // SAFETY: output pointers are valid and attributes remains live for the
    // call. Both returned handles are owned by the caller on success.
    if unsafe {
        CreatePipe(
            &raw mut read_handle,
            &raw mut write_handle,
            &raw const attributes,
            0,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let read = match OwnedHandle::new(read_handle) {
        Ok(handle) => handle,
        Err(error) => {
            if !write_handle.is_null() {
                // SAFETY: CreatePipe succeeded and transferred this handle.
                unsafe {
                    CloseHandle(write_handle);
                }
            }
            return Err(error);
        }
    };
    let write = OwnedHandle::new(write_handle)?;
    Ok((read, write))
}

fn private_pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let (read, write) = inheritable_pipe()?;
    read.clear_inherit()?;
    write.clear_inherit()?;
    Ok((read, write))
}

struct OwnedHandle(HANDLE);

// SAFETY: this wrapper has unique ownership, never exposes a mutable alias,
// and closes the handle exactly once. Process, Job Object, and pipe handles
// are process-scoped Win32 kernel handles and are not thread-affine.
unsafe impl Send for OwnedHandle {}

impl OwnedHandle {
    fn new(handle: HANDLE) -> io::Result<Self> {
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }

    const fn raw(&self) -> HANDLE {
        self.0
    }

    fn clear_inherit(&self) -> io::Result<()> {
        // SAFETY: this owned handle remains valid for the call.
        if unsafe { SetHandleInformation(self.raw(), HANDLE_FLAG_INHERIT, 0) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn into_file(self) -> File {
        let this = ManuallyDrop::new(self);
        // SAFETY: ownership of this unique Win32 handle moves into File.
        unsafe { File::from_raw_handle(this.0) }
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns one non-null Win32 handle.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct ProcessAttributeList {
    storage: Vec<usize>,
    initialized: bool,
}

impl ProcessAttributeList {
    fn new(attribute_count: u32) -> io::Result<Self> {
        let mut bytes = 0_usize;
        // The first call is documented to size the allocation and fail with
        // ERROR_INSUFFICIENT_BUFFER.
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), attribute_count, 0, &raw mut bytes);
        }
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let words = bytes.div_ceil(size_of::<usize>());
        let mut list = Self {
            storage: vec![0; words],
            initialized: false,
        };
        // SAFETY: storage is pointer-aligned and at least the requested size.
        if unsafe {
            InitializeProcThreadAttributeList(list.raw(), attribute_count, 0, &raw mut bytes)
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        list.initialized = true;
        Ok(list)
    }

    fn set_raw(&mut self, attribute: usize, value: *const c_void, size: usize) -> io::Result<()> {
        // SAFETY: value lives through CreateProcessW and size matches its array.
        let updated = unsafe {
            UpdateProcThreadAttribute(self.raw(), 0, attribute, value, size, null_mut(), null())
        };
        if updated == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn raw(&mut self) -> *mut c_void {
        self.storage.as_mut_ptr().cast()
    }
}

impl Drop for ProcessAttributeList {
    fn drop(&mut self) {
        if !self.initialized {
            return;
        }
        // SAFETY: storage was initialized as one attribute list above.
        unsafe {
            DeleteProcThreadAttributeList(self.raw());
        }
    }
}

fn encode_null_terminated(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut encoded = value.encode_wide().collect::<Vec<_>>();
    if encoded.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows process value contains an embedded NUL",
        ));
    }
    encoded.push(0);
    Ok(encoded)
}

fn windows_command_line(executable: &OsStr, arguments: &[OsString]) -> io::Result<Vec<u16>> {
    let mut command_line = Vec::new();
    append_quoted_argument(&mut command_line, executable)?;
    for argument in arguments {
        command_line.push(u16::from(b' '));
        append_quoted_argument(&mut command_line, argument)?;
    }
    command_line.push(0);
    Ok(command_line)
}

fn append_quoted_argument(command_line: &mut Vec<u16>, argument: &OsStr) -> io::Result<()> {
    let encoded = argument.encode_wide().collect::<Vec<_>>();
    if encoded.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows process argument contains an embedded NUL",
        ));
    }
    let quote = encoded.is_empty()
        || encoded.iter().any(|value| {
            matches!(*value, value if value == u16::from(b' ') || value == u16::from(b'\t') || value == u16::from(b'"'))
        });
    if !quote {
        command_line.extend(encoded);
        return Ok(());
    }

    command_line.push(u16::from(b'"'));
    let mut backslashes = 0_usize;
    for value in encoded {
        if value == u16::from(b'\\') {
            backslashes += 1;
        } else if value == u16::from(b'"') {
            command_line.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2 + 1));
            command_line.push(value);
            backslashes = 0;
        } else {
            command_line.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
            backslashes = 0;
            command_line.push(value);
        }
    }
    command_line.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2));
    command_line.push(u16::from(b'"'));
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        io::{Read, Write},
        process::Command,
        time::Duration,
    };

    use super::{
        JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobBreakawayPolicy,
        JobObjectExtendedLimitInformation, ManagedConPtyProcess, ManagedJobProcess, ManagedStdio,
        QueryInformationJobObject, TerminationOutcome, TerminationPolicy, create_kill_on_close_job,
    };

    const FAST_POLICY: TerminationPolicy = TerminationPolicy::with_timeouts(
        Duration::from_millis(100),
        Duration::from_millis(500),
        Duration::from_secs(2),
    );

    #[test]
    fn only_terminal_jobs_allow_explicit_breakaway() {
        for (policy, expected_flags) in [
            (JobBreakawayPolicy::Deny, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE),
            (
                JobBreakawayPolicy::Explicit,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK,
            ),
        ] {
            let job = create_kill_on_close_job(policy).expect("create managed Job");
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            let queried = unsafe {
                QueryInformationJobObject(
                    job.raw(),
                    JobObjectExtendedLimitInformation,
                    (&raw mut limits).cast(),
                    u32::try_from(std::mem::size_of_val(&limits)).expect("limit size"),
                    std::ptr::null_mut(),
                )
            };
            assert_ne!(queried, 0, "query Job limits");
            assert_eq!(limits.BasicLimitInformation.LimitFlags, expected_flags);
        }
    }

    #[test]
    fn command_line_quoting_preserves_spaces_quotes_and_trailing_backslashes() {
        let line = super::windows_command_line(
            std::ffi::OsStr::new(r"C:\Program Files\NoriShell\terminal.exe"),
            &[
                OsString::from("plain"),
                OsString::from("two words"),
                OsString::from(r#"quoted\"value"#),
                OsString::from(r"trailing\"),
            ],
        )
        .expect("valid command line");
        let line = String::from_utf16(&line[..line.len() - 1]).expect("UTF-16 command line");
        assert_eq!(
            line,
            "\"C:\\Program Files\\NoriShell\\terminal.exe\" plain \"two words\" \"quoted\\\\\\\"value\" trailing\\"
        );
    }

    #[test]
    fn root_exit_does_not_hide_a_live_descendant_and_job_kill_reaps_the_tree() {
        let executable = current_test_executable();
        let args = test_arguments("windows_job_parent_spawns_leaf_then_exits");
        let mut process = ManagedJobProcess::spawn(&executable, &args, None)
            .expect("spawn process in Job Object at creation");
        wait_for_active_processes(&process, 2);
        wait_for_root_exit(&process);
        assert_eq!(process.active_process_count().expect("active count"), 1);

        assert_eq!(
            process.terminate_with(FAST_POLICY, |_| Ok(())),
            TerminationOutcome::Killed
        );
        assert_eq!(process.active_process_count().expect("empty Job"), 0);
    }

    #[test]
    fn ordinary_termination_confirms_the_single_process_job_is_empty() {
        let executable = current_test_executable();
        let args = test_arguments("windows_job_leaf_waits");
        let mut process = ManagedJobProcess::spawn(&executable, &args, None)
            .expect("spawn process in Job Object at creation");
        wait_for_active_processes(&process, 1);

        assert_eq!(
            process.terminate_with(FAST_POLICY, |_| Ok(())),
            TerminationOutcome::Terminated
        );
        assert_eq!(process.active_process_count().expect("empty Job"), 0);
    }

    #[test]
    fn completed_job_reports_already_exited() {
        let executable = current_test_executable();
        let args = test_arguments("windows_job_child_exits_immediately");
        let mut process = ManagedJobProcess::spawn(&executable, &args, None)
            .expect("spawn process in Job Object at creation");
        assert!(
            process
                .wait_until_stopped(Duration::from_secs(3))
                .expect("wait for empty Job")
        );
        assert_eq!(
            process.terminate_with(FAST_POLICY, |_| Ok(())),
            TerminationOutcome::AlreadyExited
        );
    }

    #[test]
    fn redirected_stdio_round_trips_without_leaking_the_job() {
        let executable = current_test_executable();
        let args = test_arguments("windows_job_stdio_round_trip");
        let (mut process, stdio) = ManagedJobProcess::spawn_with_stdio(&executable, &args, None)
            .expect("spawn redirected process in Job Object at creation");
        let ManagedStdio {
            mut stdin,
            mut stdout,
            mut stderr,
        } = stdio;
        stdin
            .write_all(b"norishell-stdio-nonce\n")
            .expect("write child stdin");
        drop(stdin);

        let mut stdout_bytes = Vec::new();
        stdout
            .read_to_end(&mut stdout_bytes)
            .expect("read child stdout");
        let mut stderr_bytes = Vec::new();
        stderr
            .read_to_end(&mut stderr_bytes)
            .expect("read child stderr");
        assert!(String::from_utf8_lossy(&stdout_bytes).contains("norishell-stdio-reply"));
        assert!(String::from_utf8_lossy(&stderr_bytes).contains("redacted-diagnostic"));
        assert!(
            process
                .wait_until_stopped(Duration::from_secs(3))
                .expect("wait for redirected process")
        );
        assert_eq!(process.active_process_count().expect("empty Job"), 0);
    }

    #[test]
    fn conpty_resize_is_observed_by_a_child_owned_from_process_creation() {
        let executable = current_test_executable();
        let args = test_arguments("windows_conpty_child_reports_dimensions");
        let mut process = ManagedConPtyProcess::spawn(&executable, &args, None, 24, 80)
            .expect("spawn ConPTY child in Job Object at creation");
        let mut reader = process.try_clone_reader().expect("clone ConPTY reader");
        let (sender, receiver) = std::sync::mpsc::channel();
        let reader_thread = std::thread::spawn(move || {
            let mut output = Vec::new();
            let result = reader.read_to_end(&mut output).map(|_| output);
            let _ = sender.send(result);
        });

        process.resize(40, 100).expect("resize live ConPTY");
        assert_eq!(
            process
                .wait_for_exit(Duration::from_secs(5))
                .expect("wait for ConPTY process"),
            Some(0)
        );
        drop(process);
        let output = receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("bounded ConPTY output")
            .expect("read ConPTY output");
        reader_thread.join().expect("join ConPTY reader");
        let output = String::from_utf8_lossy(&output);
        assert!(output.contains("norishell-conpty-size:100x40"), "{output}");
    }

    #[test]
    fn conpty_child_can_explicitly_break_away_from_its_job() {
        let executable = current_test_executable();
        let args = test_arguments("windows_conpty_child_explicitly_breaks_away");
        let mut process = ManagedConPtyProcess::spawn(&executable, &args, None, 24, 80)
            .expect("spawn ConPTY child in terminal Job");
        let mut reader = process.try_clone_reader().expect("clone ConPTY reader");
        let (sender, receiver) = std::sync::mpsc::channel();
        let reader_thread = std::thread::spawn(move || {
            let mut output = Vec::new();
            let result = reader.read_to_end(&mut output).map(|_| output);
            let _ = sender.send(result);
        });
        assert_eq!(
            process
                .wait_for_exit(Duration::from_secs(5))
                .expect("wait for breakaway probe"),
            Some(0)
        );
        drop(process);
        receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("bounded ConPTY output")
            .expect("read ConPTY output");
        reader_thread.join().expect("join ConPTY reader");
    }

    #[test]
    fn conpty_dimensions_reject_zero_and_i16_overflow() {
        assert!(super::conpty_dimensions(0, 80).is_err());
        assert!(super::conpty_dimensions(24, 0).is_err());
        assert!(super::conpty_dimensions(24, u16::MAX).is_err());
    }

    #[test]
    #[ignore = "helper process invoked by the managed Job Object tests"]
    fn windows_job_leaf_waits() {
        std::thread::sleep(Duration::from_secs(30));
    }

    #[test]
    #[ignore = "helper process invoked by the managed Job Object tests"]
    fn windows_job_parent_spawns_leaf_then_exits() {
        let executable = current_test_executable();
        let mut child = Command::new(executable)
            .args(test_arguments("windows_job_leaf_waits"))
            .spawn()
            .expect("spawn inherited Job descendant");
        std::thread::sleep(Duration::from_millis(500));
        assert!(child.try_wait().expect("probe leaf").is_none());
    }

    #[test]
    #[ignore = "helper process invoked by the managed Job Object tests"]
    fn windows_job_child_exits_immediately() {}

    #[test]
    #[ignore = "helper process invoked by the managed Job Object tests"]
    fn windows_job_stdio_round_trip() {
        let mut input = String::new();
        std::io::stdin()
            .read_to_string(&mut input)
            .expect("read managed stdin");
        assert_eq!(input, "norishell-stdio-nonce\n");
        println!("norishell-stdio-reply");
        eprintln!("redacted-diagnostic");
    }

    #[test]
    #[ignore = "helper process invoked by the managed ConPTY test"]
    fn windows_conpty_child_reports_dimensions() {
        use windows_sys::Win32::System::Console::{
            CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo, GetStdHandle, STD_OUTPUT_HANDLE,
        };

        std::thread::sleep(Duration::from_millis(250));
        let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
        assert_ne!(
            unsafe { GetConsoleScreenBufferInfo(output, &raw mut info) },
            0,
            "read ConPTY screen buffer"
        );
        let width = info.srWindow.Right - info.srWindow.Left + 1;
        let height = info.srWindow.Bottom - info.srWindow.Top + 1;
        println!("norishell-conpty-size:{width}x{height}");
    }

    #[test]
    #[ignore = "helper process invoked by the managed ConPTY breakaway test"]
    fn windows_conpty_child_explicitly_breaks_away() {
        use std::os::windows::{io::AsRawHandle, process::CommandExt};
        use windows_sys::Win32::System::{
            JobObjects::IsProcessInJob, Threading::CREATE_BREAKAWAY_FROM_JOB,
        };

        let executable = current_test_executable();
        let mut child = Command::new(executable)
            .args(test_arguments("windows_job_leaf_waits"))
            .creation_flags(CREATE_BREAKAWAY_FROM_JOB)
            .spawn()
            .expect("spawn child outside terminal Job");
        let mut in_job = 0;
        let queried =
            unsafe { IsProcessInJob(child.as_raw_handle(), std::ptr::null_mut(), &raw mut in_job) };
        child.kill().expect("terminate breakaway probe");
        child.wait().expect("reap breakaway probe");
        assert_ne!(queried, 0, "query child Job membership");
        assert_eq!(in_job, 0, "explicit breakaway child remained in a Job");
    }

    fn test_arguments(name: &str) -> Vec<OsString> {
        vec![
            OsString::from("--ignored"),
            OsString::from("--exact"),
            OsString::from(format!("windows_process_control::tests::{name}")),
            OsString::from("--nocapture"),
        ]
    }

    fn current_test_executable() -> std::path::PathBuf {
        let executable = std::env::current_exe().expect("current test executable");
        assert!(
            executable.is_absolute(),
            "current test executable is not absolute: {executable:?}"
        );
        let metadata = std::fs::metadata(&executable).unwrap_or_else(|error| {
            panic!("current test executable is not readable: {executable:?}: {error}")
        });
        assert!(
            metadata.is_file(),
            "current test executable is not a file: {executable:?}"
        );
        executable
    }

    fn wait_for_active_processes(process: &ManagedJobProcess, expected: u32) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if process
                .active_process_count()
                .expect("active process count")
                == expected
            {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Job did not reach {expected} active processes"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_for_root_exit(process: &ManagedJobProcess) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if process.root_has_exited().expect("root process state") {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Job root process did not exit"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
