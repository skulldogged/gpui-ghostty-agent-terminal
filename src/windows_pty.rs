use crate::{
    pty::{PtySize, send_or_shutdown},
    terminal_session::TerminalEvent,
};
use std::{
    alloc::{Layout, alloc, dealloc},
    ffi::{OsStr, OsString, c_void},
    io,
    os::windows::ffi::OsStrExt,
    ptr::null_mut,
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, INVALID_HANDLE_VALUE, S_OK,
        WAIT_FAILED, WAIT_OBJECT_0,
    },
    Security::SECURITY_ATTRIBUTES,
    System::{
        Console::{COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole},
        Pipes::CreatePipe,
        Threading::{
            CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
            EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, INFINITE,
            InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
            STARTUPINFOEXW, STARTUPINFOW, TerminateProcess, UpdateProcThreadAttribute,
            WaitForSingleObject,
        },
    },
};

unsafe extern "system" {
    fn ReadFile(
        file: HANDLE,
        buffer: *mut u8,
        bytes_to_read: u32,
        bytes_read: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;

    fn WriteFile(
        file: HANDLE,
        buffer: *const u8,
        bytes_to_write: u32,
        bytes_written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
}

pub struct PtySession {
    input: OwnedHandle,
    pseudoconsole: OwnedPseudoConsole,
    process: OwnedHandle,
    shutdown: Option<flume::Sender<()>>,
}

struct Pipe {
    read: OwnedHandle,
    write: OwnedHandle,
}

struct OwnedHandle(HANDLE);

struct OwnedPseudoConsole(HPCON);

// Each wrapper uniquely owns its handle and may move to the reader thread.
unsafe impl Send for OwnedHandle {}

impl PtySession {
    pub fn spawn(
        size: PtySize,
        events: flume::Sender<TerminalEvent>,
    ) -> Result<(Self, flume::Receiver<Vec<u8>>), String> {
        let input = Pipe::create()?;
        let output = Pipe::create()?;
        let mut pseudoconsole: HPCON = 0;
        let result = unsafe {
            CreatePseudoConsole(
                size.coord(),
                input.read.raw(),
                output.write.raw(),
                0,
                &mut pseudoconsole,
            )
        };
        if result != S_OK {
            return Err(last_error("create ConPTY"));
        }

        let pseudoconsole = OwnedPseudoConsole(pseudoconsole);
        let process = spawn_shell(pseudoconsole.raw())?;

        // ConPTY duplicated the host-facing ends. Keeping only the application
        // input/output ends avoids retaining an accidental EOF reference.
        drop(input.read);
        drop(output.write);

        let process_wait_handle = process.try_clone()?;
        let (output_tx, output_rx) = flume::bounded(256);
        let (shutdown_tx, shutdown_rx) = flume::bounded(1);
        let mut output_handle = output.read;
        let reader_events = events.clone();
        let reader_shutdown = shutdown_rx.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("terminal-conpty-reader".into())
            .spawn(move || {
                let mut buffer = [0_u8; 16 * 1024];
                loop {
                    match read_handle(output_handle.raw(), &mut buffer) {
                        Ok(0) => break,
                        Ok(read)
                            if !send_or_shutdown(
                                &output_tx,
                                &reader_shutdown,
                                buffer[..read].to_vec(),
                            ) =>
                        {
                            break;
                        }
                        Ok(_) if reader_events.try_send(TerminalEvent::Changed).is_err() => {}
                        Ok(_) => {}
                        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => break,
                        Err(error) => {
                            let _ = send_or_shutdown(
                                &reader_events,
                                &reader_shutdown,
                                TerminalEvent::Failed(format!("ConPTY read stopped: {error}")),
                            );
                            break;
                        }
                    }
                }
                let _ = output_handle.close();
            })
        {
            unsafe { TerminateProcess(process.raw(), 0) };
            return Err(format!("spawn ConPTY reader thread: {error}"));
        }

        if let Err(error) = std::thread::Builder::new()
            .name("terminal-conpty-process-waiter".into())
            .spawn(move || {
                let wait_result =
                    unsafe { WaitForSingleObject(process_wait_handle.raw(), INFINITE) };
                match wait_result {
                    WAIT_OBJECT_0 => {
                        let _ = send_or_shutdown(&events, &shutdown_rx, TerminalEvent::Exited);
                    }
                    WAIT_FAILED => {
                        let _ = send_or_shutdown(
                            &events,
                            &shutdown_rx,
                            TerminalEvent::Failed(format!(
                                "wait for ConPTY process: {}",
                                io::Error::last_os_error()
                            )),
                        );
                    }
                    unexpected => {
                        let _ = send_or_shutdown(
                            &events,
                            &shutdown_rx,
                            TerminalEvent::Failed(format!(
                                "wait for ConPTY process returned unexpected status {unexpected}"
                            )),
                        );
                    }
                }
            })
        {
            unsafe { TerminateProcess(process.raw(), 0) };
            return Err(format!("spawn ConPTY process waiter thread: {error}"));
        }

        Ok((
            Self {
                input: input.write,
                pseudoconsole,
                process,
                shutdown: Some(shutdown_tx),
            },
            output_rx,
        ))
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        write_handle(self.input.raw(), bytes).map_err(|error| format!("write ConPTY: {error}"))
    }

    pub fn resize(&mut self, size: PtySize) -> Result<(), String> {
        let result = unsafe { ResizePseudoConsole(self.pseudoconsole.raw(), size.coord()) };
        if result == S_OK {
            Ok(())
        } else {
            Err(last_error("resize ConPTY"))
        }
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        // Disconnect every shutdown receiver before closing ConPTY. Any
        // worker blocked while publishing output or a lifecycle event then
        // cancels its send and releases its pipe handle.
        drop(self.shutdown.take());
        let _ = self.input.close();
        unsafe {
            TerminateProcess(self.process.raw(), 0);
        }
    }
}

impl PtySize {
    fn coord(self) -> COORD {
        COORD {
            X: self.cols as i16,
            Y: self.rows as i16,
        }
    }
}

impl Pipe {
    fn create() -> Result<Self, String> {
        let mut read = INVALID_HANDLE_VALUE;
        let mut write = INVALID_HANDLE_VALUE;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: 0,
        };
        if unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) } == 0 {
            return Err(last_error("create ConPTY pipe"));
        }
        Ok(Self {
            read: OwnedHandle(read),
            write: OwnedHandle(write),
        })
    }
}

impl OwnedHandle {
    fn raw(&self) -> HANDLE {
        self.0
    }

    fn close(&mut self) -> Result<(), String> {
        if self.0 == INVALID_HANDLE_VALUE || self.0.is_null() {
            return Ok(());
        }
        let handle = self.0;
        self.0 = INVALID_HANDLE_VALUE;
        if unsafe { CloseHandle(handle) } == 0 {
            Err(last_error("close Windows handle"))
        } else {
            Ok(())
        }
    }

    fn try_clone(&self) -> Result<Self, String> {
        let current_process = unsafe { GetCurrentProcess() };
        let mut duplicate = INVALID_HANDLE_VALUE;
        if unsafe {
            DuplicateHandle(
                current_process,
                self.raw(),
                current_process,
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            Err(last_error("duplicate Windows handle"))
        } else {
            Ok(Self(duplicate))
        }
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

impl OwnedPseudoConsole {
    fn raw(&self) -> HPCON {
        self.0
    }
}

impl Drop for OwnedPseudoConsole {
    fn drop(&mut self) {
        unsafe { ClosePseudoConsole(self.0) };
    }
}

fn spawn_shell(pseudoconsole: HPCON) -> Result<OwnedHandle, String> {
    let shell = shell();
    let mut arguments = Vec::new();
    if shell
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| {
            name.eq_ignore_ascii_case("pwsh.exe") || name.eq_ignore_ascii_case("powershell.exe")
        })
    {
        arguments.push(OsString::from("-NoLogo"));
    }
    let mut command_line = command_line(shell.as_os_str(), &arguments)?;
    let application = nul_terminated(shell.as_os_str(), "shell executable")?;
    let environment = environment_block()?;

    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
    startup.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
    startup.StartupInfo.hStdError = INVALID_HANDLE_VALUE;

    let mut attributes_size = 0;
    unsafe {
        InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut attributes_size);
    }
    let layout = Layout::from_size_align(attributes_size, 8)
        .map_err(|error| format!("layout ConPTY process attributes: {error}"))?;
    let attributes = unsafe { alloc(layout) as LPPROC_THREAD_ATTRIBUTE_LIST };
    if attributes.is_null() {
        return Err("allocate ConPTY process attributes".into());
    }

    let initialized =
        unsafe { InitializeProcThreadAttributeList(attributes, 1, 0, &mut attributes_size) != 0 };
    if !initialized {
        unsafe { dealloc(attributes.cast(), layout) };
        return Err(last_error("initialize ConPTY process attributes"));
    }
    let attached = unsafe {
        UpdateProcThreadAttribute(
            attributes,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            pseudoconsole as *const c_void,
            std::mem::size_of::<HPCON>(),
            null_mut(),
            null_mut(),
        ) != 0
    };
    if !attached {
        unsafe {
            DeleteProcThreadAttributeList(attributes);
            dealloc(attributes.cast(), layout);
        }
        return Err(last_error("attach ConPTY process attribute"));
    }
    startup.lpAttributeList = attributes;

    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            null_mut(),
            null_mut(),
            0,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
            environment.as_ptr().cast(),
            null_mut(),
            (&mut startup as *mut STARTUPINFOEXW).cast::<STARTUPINFOW>(),
            &mut process,
        ) != 0
    };
    unsafe {
        DeleteProcThreadAttributeList(attributes);
        dealloc(attributes.cast(), layout);
    }
    if !created {
        return Err(last_error("spawn ConPTY shell"));
    }

    let thread = OwnedHandle(process.hThread);
    let process = OwnedHandle(process.hProcess);
    drop(thread);
    Ok(process)
}

fn shell() -> std::path::PathBuf {
    for candidate in [
        std::env::var_os("PROGRAMFILES")
            .map(std::path::PathBuf::from)
            .map(|root| root.join("PowerShell\\7\\pwsh.exe")),
        Some(std::path::PathBuf::from(
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
        )),
        std::env::var_os("COMSPEC").map(std::path::PathBuf::from),
        Some(std::path::PathBuf::from("C:\\Windows\\System32\\cmd.exe")),
    ]
    .into_iter()
    .flatten()
    {
        if candidate.is_file() {
            return candidate;
        }
    }
    std::path::PathBuf::from("cmd.exe")
}

fn command_line(executable: &OsStr, arguments: &[OsString]) -> Result<Vec<u16>, String> {
    let mut output = Vec::new();
    push_argument(&mut output, executable)?;
    for argument in arguments {
        output.push(u16::from(b' '));
        push_argument(&mut output, argument)?;
    }
    output.push(0);
    Ok(output)
}

fn push_argument(output: &mut Vec<u16>, argument: &OsStr) -> Result<(), String> {
    let argument: Vec<u16> = argument.encode_wide().collect();
    if argument.contains(&0) {
        return Err("ConPTY command argument contains NUL".into());
    }
    let quoted = argument.is_empty()
        || argument
            .iter()
            .any(|value| matches!(*value, 0x09 | 0x20 | 0x22));
    if !quoted {
        output.extend(argument);
        return Ok(());
    }

    output.push(u16::from(b'"'));
    let mut backslashes = 0;
    for value in argument {
        if value == u16::from(b'\\') {
            backslashes += 1;
        } else {
            if value == u16::from(b'"') {
                output.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2 + 1));
            } else {
                output.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
            }
            output.push(value);
            backslashes = 0;
        }
    }
    output.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2));
    output.push(u16::from(b'"'));
    Ok(())
}

fn environment_block() -> Result<Vec<u16>, String> {
    let mut environment: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    for (key, value) in [
        (OsString::from("TERM"), OsString::from("xterm-256color")),
        (OsString::from("COLORTERM"), OsString::from("truecolor")),
    ] {
        environment.retain(|(existing, _)| !existing.eq_ignore_ascii_case(&key));
        environment.push((key, value));
    }
    environment.sort_by_cached_key(|(key, _)| key.to_string_lossy().to_ascii_lowercase());

    let mut output = Vec::new();
    for (key, value) in environment {
        append_environment_value(&mut output, &key)?;
        output.push(u16::from(b'='));
        append_environment_value(&mut output, &value)?;
        output.push(0);
    }
    output.push(0);
    Ok(output)
}

fn append_environment_value(output: &mut Vec<u16>, value: &OsStr) -> Result<(), String> {
    let value: Vec<u16> = value.encode_wide().collect();
    if value.contains(&0) {
        return Err("ConPTY environment contains NUL".into());
    }
    output.extend(value);
    Ok(())
}

fn nul_terminated(value: &OsStr, field: &str) -> Result<Vec<u16>, String> {
    let mut value: Vec<u16> = value.encode_wide().collect();
    if value.is_empty() || value.contains(&0) {
        return Err(format!("invalid ConPTY {field}"));
    }
    value.push(0);
    Ok(value)
}

fn read_handle(handle: HANDLE, buffer: &mut [u8]) -> io::Result<usize> {
    let mut read = 0;
    let result = unsafe {
        ReadFile(
            handle,
            buffer.as_mut_ptr(),
            buffer.len().min(u32::MAX as usize) as u32,
            &mut read,
            null_mut(),
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(read as usize)
    }
}

fn write_handle(handle: HANDLE, bytes: &[u8]) -> io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        let mut written = 0;
        let result = unsafe {
            WriteFile(
                handle,
                bytes[offset..].as_ptr(),
                bytes.len().saturating_sub(offset).min(u32::MAX as usize) as u32,
                &mut written,
                null_mut(),
            )
        };
        if result == 0 {
            return Err(io::Error::last_os_error());
        }
        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "ConPTY input write made no progress",
            ));
        }
        offset += written as usize;
    }
    Ok(())
}

fn last_error(operation: &str) -> String {
    format!("{operation}: {}", io::Error::last_os_error())
}
