//! Thin platform layer: PTY creation, window size and terminal raw mode.
//!
//! These are the only `unsafe` calls in the engine. Every declaration mirrors
//! the C signature of the platform; callers get a safe, small API. Constants and
//! `termios` layouts differ per operating system, so each one is defined next to
//! its target instead of being imported from a C library wrapper.

#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::os::fd::{FromRawFd, RawFd};

#[cfg(target_os = "linux")]
mod platform {
    pub const TIOCSCTTY: u64 = 0x540e;
    pub const TIOCGWINSZ: u64 = 0x5413;
    pub const TIOCSWINSZ: u64 = 0x5414;
    pub const TCSANOW: i32 = 0;
}

#[cfg(target_os = "macos")]
mod platform {
    pub const TIOCSCTTY: u64 = 0x2000_7461;
    pub const TIOCGWINSZ: u64 = 0x4008_7468;
    pub const TIOCSWINSZ: u64 = 0x8008_7467;
    pub const TCSANOW: i32 = 0;
}

/// `termios` as the C library lays it out on Linux (glibc/musl, x86_64/aarch64).
#[cfg(target_os = "linux")]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
struct Termios {
    c_iflag: u32,
    c_oflag: u32,
    c_cflag: u32,
    c_lflag: u32,
    c_line: u8,
    c_cc: [u8; 32],
    c_ispeed: u32,
    c_ospeed: u32,
}

/// `termios` on macOS: `tcflag_t` and `speed_t` are `unsigned long`.
#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
struct Termios {
    c_iflag: u64,
    c_oflag: u64,
    c_cflag: u64,
    c_lflag: u64,
    c_cc: [u8; 20],
    c_ispeed: u64,
    c_ospeed: u64,
}

#[cfg(unix)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct WinSize {
    rows: u16,
    cols: u16,
    xpixel: u16,
    ypixel: u16,
}

#[cfg(unix)]
unsafe extern "C" {
    fn posix_openpt(flags: i32) -> i32;
    fn grantpt(fd: i32) -> i32;
    fn unlockpt(fd: i32) -> i32;
    fn ptsname(fd: i32) -> *mut std::ffi::c_char;
    fn tcgetattr(fd: i32, termios: *mut Termios) -> i32;
    fn tcsetattr(fd: i32, action: i32, termios: *const Termios) -> i32;
    fn cfmakeraw(termios: *mut Termios);
    fn ioctl(fd: i32, request: u64, ...) -> i32;
    fn setsid() -> i32;
    fn isatty(fd: i32) -> i32;
}

#[cfg(unix)]
const O_RDWR: i32 = 2;

/// Opens a pseudo-terminal pair. Returns `(master, slave, slave_path)`.
#[cfg(unix)]
pub fn open_pty() -> io::Result<(File, File, String)> {
    // SAFETY: `posix_openpt` returns a new file descriptor or -1.
    let fd = unsafe { posix_openpt(O_RDWR) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the descriptor is valid and owned by us from here on.
    let master = unsafe { File::from_raw_fd(fd) };

    // SAFETY: plain ioctl-style calls on a descriptor we own.
    if unsafe { grantpt(fd) } != 0 || unsafe { unlockpt(fd) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `ptsname` returns a pointer to a static string for this fd.
    let name = unsafe { ptsname(fd) };
    if name.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the pointer is NUL-terminated and valid for the call.
    let path = unsafe { std::ffi::CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned();
    let slave = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)?;
    Ok((master, slave, path))
}

/// Size of the terminal behind `fd`, in `(columns, rows)`.
#[cfg(unix)]
pub fn window_size(fd: RawFd) -> io::Result<(usize, usize)> {
    let mut size = WinSize::default();
    // SAFETY: `TIOCGWINSZ` writes a `struct winsize` into the passed pointer.
    let result = unsafe { ioctl(fd, platform::TIOCGWINSZ, std::ptr::addr_of_mut!(size)) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if size.cols == 0 || size.rows == 0 {
        return Err(io::Error::other("terminal reported a zero size"));
    }
    Ok((usize::from(size.cols), usize::from(size.rows)))
}

/// Resizes the terminal behind `fd`.
#[cfg(unix)]
pub fn set_size(fd: RawFd, cols: usize, rows: usize) -> io::Result<()> {
    let size = WinSize {
        rows: rows.min(usize::from(u16::MAX)) as u16,
        cols: cols.min(usize::from(u16::MAX)) as u16,
        xpixel: 0,
        ypixel: 0,
    };
    // SAFETY: `TIOCSWINSZ` reads a `struct winsize` from the passed pointer.
    let result = unsafe { ioctl(fd, platform::TIOCSWINSZ, std::ptr::addr_of!(size)) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Whether `fd` refers to a terminal.
#[cfg(unix)]
pub fn is_tty(fd: RawFd) -> bool {
    // SAFETY: `isatty` only inspects the descriptor.
    unsafe { isatty(fd) != 0 }
}

/// Starts a new session. Used in the child before `exec`.
#[cfg(unix)]
pub fn new_session() -> i32 {
    // SAFETY: no memory is involved; the call only changes process attributes.
    unsafe { setsid() }
}

/// Makes the terminal behind `fd` the controlling terminal of the session.
#[cfg(unix)]
pub fn take_control_tty(fd: RawFd) -> i32 {
    // SAFETY: `TIOCSCTTY` takes an int argument; zero means "this terminal".
    unsafe { ioctl(fd, platform::TIOCSCTTY, 0i32) }
}

// ------------------------------------------------------------------ windows
// Raw mode and screen size through the console API. The pseudo-terminal
// creation lives in `crate::pty`; here we only prepare the host terminal.

#[cfg(windows)]
mod windows_glue {
    use std::io;

    pub type StdHandle = *mut core::ffi::c_void;

    pub const STD_INPUT_HANDLE: u32 = u32::MAX - 10;
    pub const STD_OUTPUT_HANDLE: u32 = u32::MAX - 11;

    // Console mode flags we clear for raw input.
    const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
    const ENABLE_LINE_INPUT: u32 = 0x0002;
    const ENABLE_ECHO_INPUT: u32 = 0x0004;
    // Flag we set for ANSI output.
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Coord {
        x: i16,
        y: i16,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SmallRect {
        left: i16,
        top: i16,
        right: i16,
        bottom: i16,
    }

    #[repr(C)]
    struct ConsoleScreenBufferInfo {
        size: Coord,
        cursor: Coord,
        attributes: u16,
        window: SmallRect,
        maximum_window: Coord,
    }

    unsafe extern "system" {
        fn GetStdHandle(kind: u32) -> StdHandle;
        fn GetConsoleMode(handle: StdHandle, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: StdHandle, mode: u32) -> i32;
        fn GetConsoleScreenBufferInfo(handle: StdHandle, info: *mut ConsoleScreenBufferInfo)
        -> i32;
    }

    pub fn std_handle(kind: u32) -> io::Result<StdHandle> {
        // SAFETY: plain handle lookup; an invalid console returns NULL.
        let handle = unsafe { GetStdHandle(kind) };
        if handle.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "console handle is not available",
            ));
        }
        Ok(handle)
    }

    /// Size of the console output buffer, in `(columns, rows)`.
    pub fn window_size() -> io::Result<(usize, usize)> {
        let handle = std_handle(STD_OUTPUT_HANDLE)?;
        let mut info = ConsoleScreenBufferInfo {
            size: Coord::default(),
            cursor: Coord::default(),
            attributes: 0,
            window: SmallRect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            maximum_window: Coord::default(),
        };
        // SAFETY: `info` is a caller-owned struct of the expected layout.
        if unsafe { GetConsoleScreenBufferInfo(handle, &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let cols = info.size.x.max(1);
        let rows = info.size.y.max(1);
        Ok((
            usize::try_from(cols).unwrap_or(1),
            usize::try_from(rows).unwrap_or(1),
        ))
    }

    /// Puts the console into raw input mode and enables VT output. The
    /// previous modes are restored on drop through `original_input` and
    /// `original_output`.
    #[derive(Debug)]
    pub struct RawModes {
        input: StdHandle,
        output: StdHandle,
        original_input: u32,
        original_output: u32,
    }

    impl RawModes {
        pub fn enable() -> io::Result<Self> {
            let input = std_handle(STD_INPUT_HANDLE)?;
            let output = std_handle(STD_OUTPUT_HANDLE)?;
            let mut original_input = 0u32;
            let mut original_output = 0u32;
            // SAFETY: caller-owned console modes.
            if unsafe { GetConsoleMode(input, &mut original_input) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if unsafe { GetConsoleMode(output, &mut original_output) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let raw_input =
                original_input & !(ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT);
            let vt_output = original_output | ENABLE_VIRTUAL_TERMINAL_PROCESSING;
            if unsafe { SetConsoleMode(input, raw_input) } == 0
                || unsafe { SetConsoleMode(output, vt_output) } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                input,
                output,
                original_input,
                original_output,
            })
        }
    }

    impl Drop for RawModes {
        fn drop(&mut self) {
            // SAFETY: restores the modes saved by `enable`.
            unsafe {
                SetConsoleMode(self.input, self.original_input);
                SetConsoleMode(self.output, self.original_output);
            }
        }
    }

    // -- pseudo console -----------------------------------------------------

    // The aliases keep the names the Windows API documents; renaming them
    // would only distance the code from its source of truth.
    #[allow(clippy::upper_case_acronyms)]
    pub type HRESULT = i32;
    pub type Bool = i32;

    pub const S_OK: HRESULT = 0;
    pub const WAIT_OBJECT_0: u32 = 0;
    pub const ERROR_INSUFFICIENT_BUFFER: i32 = 122;
    pub const EXTENDED_STARTUPINFO_PRESENT: u32 = 0x0008_0000;
    pub const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x0002_0016;
    pub const INFINITE: u32 = 0xFFFF_FFFF;

    #[allow(clippy::upper_case_acronyms)]
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct HPCON(*mut core::ffi::c_void);

    #[repr(C)]
    struct StartupInfoW {
        size: u32,
        reserved: *mut u16,
        desktop: *mut u16,
        title: *mut u16,
        x: u32,
        y: u32,
        x_size: u32,
        y_size: u32,
        x_count_chars: u32,
        y_count_chars: u32,
        fill_attribute: u32,
        flags: u32,
        show_window: u16,
        reserved2: u16,
        reserved3: *mut u8,
        std_input: StdHandle,
        std_output: StdHandle,
        std_error: StdHandle,
    }

    #[repr(C)]
    struct StartupInfoExW {
        info: StartupInfoW,
        attribute_list: *mut core::ffi::c_void,
    }

    #[repr(C)]
    struct ProcessInformation {
        process: StdHandle,
        thread: StdHandle,
        process_id: u32,
        thread_id: u32,
    }

    unsafe extern "system" {
        fn CreatePipe(
            read: *mut StdHandle,
            write: *mut StdHandle,
            attributes: *const core::ffi::c_void,
            size: u32,
        ) -> Bool;
        fn CreatePseudoConsole(
            size: Coord,
            input: StdHandle,
            output: StdHandle,
            flags: u32,
            hpc: *mut HPCON,
        ) -> HRESULT;
        fn ResizePseudoConsole(hpc: HPCON, size: Coord) -> HRESULT;
        fn ClosePseudoConsole(hpc: HPCON);
        fn InitializeProcThreadAttributeList(
            list: *mut core::ffi::c_void,
            count: u32,
            flags: u32,
            size: *mut usize,
        ) -> Bool;
        fn DeleteProcThreadAttributeList(list: *mut core::ffi::c_void);
        fn UpdateProcThreadAttribute(
            list: *mut core::ffi::c_void,
            flags: u32,
            attribute: usize,
            value: *mut core::ffi::c_void,
            value_size: usize,
            previous: *mut core::ffi::c_void,
            ret: *mut *mut core::ffi::c_void,
        ) -> Bool;
        fn CreateProcessW(
            application: *const u16,
            command_line: *mut u16,
            process_attributes: *const core::ffi::c_void,
            thread_attributes: *const core::ffi::c_void,
            inherit: Bool,
            flags: u32,
            environment: *const core::ffi::c_void,
            current_directory: *const u16,
            startup: *const StartupInfoW,
            information: *mut ProcessInformation,
        ) -> Bool;
        fn WaitForSingleObject(handle: StdHandle, milliseconds: u32) -> u32;
        fn GetExitCodeProcess(handle: StdHandle, code: *mut u32) -> Bool;
        fn TerminateProcess(handle: StdHandle, code: u32) -> Bool;
        fn CloseHandle(handle: StdHandle) -> Bool;
    }

    /// An open pseudo console. Closes itself on drop.
    pub struct Conpty {
        hpc: HPCON,
        // The console keeps using the ends it was opened with until a client
        // attaches, so they stay alive with it instead of being closed early.
        input_read: StdHandle,
        output_write: StdHandle,
    }

    impl std::fmt::Debug for Conpty {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Conpty").finish_non_exhaustive()
        }
    }

    // SAFETY: the raw pseudo-console handle is an opaque kernel object; we only
    // ever pass it back to its own API. `Conpty` is used from the main thread
    // and its handles are not shared.
    unsafe impl Send for Conpty {}

    impl Conpty {
        /// Creates a pseudo console of `cols` by `rows` wired to a fresh pipe
        /// pair. Returns the console plus the pipes the host keeps: the input
        /// write end and the output read end.
        pub fn open(cols: usize, rows: usize) -> io::Result<(Self, std::fs::File, std::fs::File)> {
            let (input_read, input_write) = open_pipe()?;
            let (output_read, output_write) = open_pipe()?;
            let size = Coord {
                x: i16::try_from(cols).unwrap_or(i16::MAX),
                y: i16::try_from(rows).unwrap_or(i16::MAX),
            };
            let mut hpc = HPCON::default();
            // SAFETY: all handles were just created and are valid; the struct
            // receives the new handle.
            let status =
                unsafe { CreatePseudoConsole(size, input_read, output_write, 0, &mut hpc) };
            if status != S_OK {
                return Err(io::Error::other(format!(
                    "CreatePseudoConsole failed with HRESULT {status:#x}"
                )));
            }
            // The console may not have taken its ends over until a client
            // attaches, so they live and die with it instead of being closed
            // here: closing them early leaves the child writing to nothing.
            Ok((
                Self {
                    hpc,
                    input_read,
                    output_write,
                },
                file_from(input_write),
                file_from(output_read),
            ))
        }

        /// Tells the console its new size.
        pub fn resize(&self, cols: usize, rows: usize) -> io::Result<()> {
            let size = Coord {
                x: i16::try_from(cols).unwrap_or(i16::MAX),
                y: i16::try_from(rows).unwrap_or(i16::MAX),
            };
            // SAFETY: the handle is owned and valid.
            let status = unsafe { ResizePseudoConsole(self.hpc, size) };
            if status != S_OK {
                return Err(io::Error::other(format!(
                    "ResizePseudoConsole failed with HRESULT {status:#x}"
                )));
            }
            Ok(())
        }

        /// Spawns a child attached to this console. Returns the process handle.
        pub fn spawn(&self, command_line: &[u16]) -> io::Result<StdHandle> {
            let mut list_size = 0usize;
            // SAFETY: a NULL list only asks for the required size.
            let measured = unsafe {
                InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut list_size)
            };
            if measured == 0 {
                // The documented answer to the measuring call is exactly
                // ERROR_INSUFFICIENT_BUFFER, with the size already written.
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER) {
                    return Err(error);
                }
            }
            if list_size == 0 {
                return Err(io::Error::other(
                    "InitializeProcThreadAttributeList reported no size",
                ));
            }
            // The attribute list is a chain of pointer-sized nodes, so its
            // backing store is aligned like one instead of like bytes.
            let mut storage = vec![0usize; list_size.div_ceil(std::mem::size_of::<usize>())];
            let list = storage.as_mut_ptr().cast::<core::ffi::c_void>();
            // SAFETY: the buffer is sized and aligned for one attribute.
            if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut list_size) } == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: this attribute takes the pseudo-console handle itself as
            // the value, not a pointer to it. Passing the address of a local
            // hands the child an invalid handle, and it dies in its loader with
            // STATUS_DLL_INIT_FAILED (0xc0000142) before running anything.
            let hpc = self.hpc;
            if unsafe {
                UpdateProcThreadAttribute(
                    list,
                    0,
                    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                    hpc.0,
                    std::mem::size_of::<HPCON>(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            } == 0
            {
                let error = io::Error::last_os_error();
                // SAFETY: the list is initialized and no longer used.
                unsafe { DeleteProcThreadAttributeList(list) };
                return Err(error);
            }

            let mut command_line = command_line.to_vec();
            let startup = StartupInfoExW {
                info: zeroed_startup_info(std::mem::size_of::<StartupInfoExW>() as u32),
                attribute_list: list,
            };
            let mut information = ProcessInformation {
                process: std::ptr::null_mut(),
                thread: std::ptr::null_mut(),
                process_id: 0,
                thread_id: 0,
            };
            // SAFETY: every pointer is valid or null; the wide command line is
            // mutable as the API may write; the startup struct is extended, so
            // the flag is set and the `size` field already says `StartupInfoExW`.
            // The pseudo-console attribute gives the client its standard handles,
            // so no handle inheritance is needed here.
            let spawned = unsafe {
                CreateProcessW(
                    std::ptr::null(),
                    command_line.as_mut_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    EXTENDED_STARTUPINFO_PRESENT,
                    std::ptr::null(),
                    std::ptr::null(),
                    &startup.info,
                    &mut information,
                )
            };
            if spawned == 0 {
                let error = io::Error::last_os_error();
                // SAFETY: the list is initialized and no longer used.
                unsafe { DeleteProcThreadAttributeList(list) };
                return Err(error);
            }
            // SAFETY: the list was initialized by this call and is not used
            // again; the thread handle is owned and closed here.
            unsafe {
                DeleteProcThreadAttributeList(list);
                CloseHandle(information.thread);
            }
            Ok(information.process)
        }

        /// Whether the child exited. Returns its code when it did.
        // The handle is an opaque kernel object that only its own API
        // touches; `not_unsafe_ptr_arg_deref` cannot see the SAFETY contract
        // documented above each call.
        #[allow(clippy::not_unsafe_ptr_arg_deref)]
        pub fn try_wait(&self, process: StdHandle) -> io::Result<Option<u32>> {
            // SAFETY: the handle is owned and valid.
            if unsafe { WaitForSingleObject(process, 0) } != WAIT_OBJECT_0 {
                return Ok(None);
            }
            let mut code = 0u32;
            // SAFETY: the handle is owned and valid.
            if unsafe { GetExitCodeProcess(process, &mut code) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Some(code))
        }

        /// Terminates the child.
        #[allow(clippy::not_unsafe_ptr_arg_deref)]
        pub fn kill(&self, process: StdHandle) {
            // SAFETY: the handle is owned; TerminateProcess is best effort and
            // the wait guarantees the handle is signalled before reuse.
            unsafe {
                TerminateProcess(process, 1);
                WaitForSingleObject(process, INFINITE);
                CloseHandle(process);
            }
        }
    }

    impl Drop for Conpty {
        fn drop(&mut self) {
            // SAFETY: the console and the ends it was opened with are owned
            // exactly once, and the closed console no longer uses them.
            unsafe {
                ClosePseudoConsole(self.hpc);
                CloseHandle(self.input_read);
                CloseHandle(self.output_write);
            }
        }
    }

    fn open_pipe() -> io::Result<(StdHandle, StdHandle)> {
        let mut read = std::ptr::null_mut();
        let mut write = std::ptr::null_mut();
        // `CreatePseudoConsole` duplicates these ends into the console host with
        // inheritance enabled, so they stay private to this process here.
        // SAFETY: two caller-owned handle slots; NULL attributes mean the
        // default, non-inheritable handles.
        if unsafe { CreatePipe(&mut read, &mut write, std::ptr::null(), 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((read, write))
    }

    fn file_from(handle: StdHandle) -> std::fs::File {
        use std::os::windows::io::FromRawHandle;
        // SAFETY: the handle was created by CreatePipe and is owned from here.
        unsafe { std::fs::File::from_raw_handle(handle) }
    }

    fn zeroed_startup_info(size: u32) -> StartupInfoW {
        StartupInfoW {
            size,
            reserved: std::ptr::null_mut(),
            desktop: std::ptr::null_mut(),
            title: std::ptr::null_mut(),
            x: 0,
            y: 0,
            x_size: 0,
            y_size: 0,
            x_count_chars: 0,
            y_count_chars: 0,
            fill_attribute: 0,
            flags: 0,
            show_window: 0,
            reserved2: 0,
            reserved3: std::ptr::null_mut(),
            std_input: std::ptr::null_mut(),
            std_output: std::ptr::null_mut(),
            std_error: std::ptr::null_mut(),
        }
    }
}

#[cfg(windows)]
pub use windows_glue::{Conpty, RawModes as RawMode, window_size};

/// Restores the original terminal state when dropped.
#[cfg(unix)]
#[derive(Debug)]
pub struct RawMode {
    fd: RawFd,
    original: Termios,
}

#[cfg(unix)]
impl RawMode {
    /// Puts the terminal into raw mode: no echo, no line buffering, no signal
    /// generation. The previous state is restored on drop.
    pub fn enable(fd: RawFd) -> io::Result<Self> {
        if !is_tty(fd) {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "not a terminal",
            ));
        }
        // SAFETY: `termios` is plain old data; zeroing is a valid initial state.
        let mut original: Termios = unsafe { std::mem::zeroed() };
        // SAFETY: the descriptor is a terminal and the pointer is valid.
        if unsafe { tcgetattr(fd, std::ptr::addr_of_mut!(original)) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut raw = original;
        // SAFETY: `cfmakeraw` mutates a caller-owned struct in place.
        unsafe { cfmakeraw(std::ptr::addr_of_mut!(raw)) };
        // SAFETY: same as above for `tcsetattr`.
        if unsafe { tcsetattr(fd, platform::TCSANOW, std::ptr::addr_of!(raw)) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { fd, original })
    }
}

#[cfg(unix)]
impl Drop for RawMode {
    fn drop(&mut self) {
        // SAFETY: restores a snapshot previously returned by `tcgetattr`.
        unsafe {
            tcsetattr(
                self.fd,
                platform::TCSANOW,
                std::ptr::addr_of!(self.original),
            );
        }
    }
}
