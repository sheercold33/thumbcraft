use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Watches the currently active folder for file additions, removals, renames, or modifications.
/// Uses Win32 `FindFirstChangeNotificationW` + `WaitForMultipleObjects` on Windows (0% idle CPU,
/// instant wake-up + ~300ms debounce) and wakes up `egui::Context` even when unfocused.
pub struct FolderWatcher {
    current_target: Option<(PathBuf, bool)>,
    dirty: Arc<AtomicBool>,
    stop_signal: Option<StopSignal>,
}

impl FolderWatcher {
    pub fn new() -> Self {
        Self {
            current_target: None,
            dirty: Arc::new(AtomicBool::new(false)),
            stop_signal: None,
        }
    }

    /// Ensures the watcher is watching `target` (`Some((dir, recursive))`) or stopped (`None`).
    pub fn set_target(&mut self, target: Option<(PathBuf, bool)>, ctx: Option<egui::Context>) {
        if self.current_target == target {
            return;
        }
        self.stop();
        self.dirty.store(false, Ordering::SeqCst);
        self.current_target = target.clone();

        let Some((dir, recursive)) = target else {
            return;
        };
        if !dir.is_dir() {
            return;
        }

        let dirty = self.dirty.clone();
        let stop = StopSignal::new();
        let stop_clone = stop.clone_for_thread();
        self.stop_signal = Some(stop);

        thread::spawn(move || {
            run_watch_loop(&dir, recursive, dirty, stop_clone, ctx);
        });
    }

    /// Returns `true` (and clears the flag) if a debounced folder change occurred.
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::SeqCst)
    }

    pub fn stop(&mut self) {
        if let Some(sig) = self.stop_signal.take() {
            sig.signal();
        }
        self.current_target = None;
    }
}

impl Drop for FolderWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    const INVALID_HANDLE_VALUE: *mut c_void = (-1isize) as *mut c_void;
    const WAIT_OBJECT_0: u32 = 0x0000_0000;
    const WAIT_TIMEOUT: u32 = 0x0000_0102;
    const INFINITE: u32 = 0xFFFF_FFFF;

    const FILE_NOTIFY_CHANGE_FILE_NAME: u32 = 0x0000_0001;
    const FILE_NOTIFY_CHANGE_DIR_NAME: u32 = 0x0000_0002;
    const FILE_NOTIFY_CHANGE_SIZE: u32 = 0x0000_0008;
    const FILE_NOTIFY_CHANGE_LAST_WRITE: u32 = 0x0000_0010;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateEventW(
            lp_event_attributes: *mut c_void,
            b_manual_reset: i32,
            b_initial_state: i32,
            lp_name: *const u16,
        ) -> *mut c_void;
        fn SetEvent(h_event: *mut c_void) -> i32;
        fn CloseHandle(h_object: *mut c_void) -> i32;
        fn FindFirstChangeNotificationW(
            lp_path_name: *const u16,
            b_watch_subtree: i32,
            dw_notify_filter: u32,
        ) -> *mut c_void;
        fn FindNextChangeNotification(h_change_handle: *mut c_void) -> i32;
        fn FindCloseChangeNotification(h_change_handle: *mut c_void) -> i32;
        fn WaitForMultipleObjects(
            n_count: u32,
            lp_handles: *const *mut c_void,
            b_wait_all: i32,
            dw_milliseconds: u32,
        ) -> u32;
    }

    pub struct StopSignal {
        // Shared atomic flag + Win32 manual-reset event handle stored as usize for Send+Sync
        stopped: Arc<AtomicBool>,
        event_handle: usize,
    }

    pub struct ThreadStopHandle {
        stopped: Arc<AtomicBool>,
        event_handle: usize,
    }

    impl StopSignal {
        pub fn new() -> Self {
            let h = unsafe { CreateEventW(std::ptr::null_mut(), 1, 0, std::ptr::null()) };
            Self {
                stopped: Arc::new(AtomicBool::new(false)),
                event_handle: h as usize,
            }
        }

        pub fn clone_for_thread(&self) -> ThreadStopHandle {
            ThreadStopHandle {
                stopped: self.stopped.clone(),
                event_handle: self.event_handle,
            }
        }

        pub fn signal(self) {
            self.stopped.store(true, Ordering::SeqCst);
            if self.event_handle != 0 {
                unsafe {
                    SetEvent(self.event_handle as *mut c_void);
                }
            }
            // Note: Thread closes the event handle when exiting if spawned; if never spawned, close here.
        }
    }

    impl ThreadStopHandle {
        fn is_stopped(&self) -> bool {
            self.stopped.load(Ordering::SeqCst)
        }

        fn raw_handle(&self) -> *mut c_void {
            self.event_handle as *mut c_void
        }
    }

    impl Drop for ThreadStopHandle {
        fn drop(&mut self) {
            if self.event_handle != 0 {
                unsafe {
                    CloseHandle(self.event_handle as *mut c_void);
                }
            }
        }
    }

    pub fn run_watch_loop(
        dir: &Path,
        recursive: bool,
        dirty: Arc<AtomicBool>,
        stop: ThreadStopHandle,
        ctx: Option<egui::Context>,
    ) {
        let wide_dir: Vec<u16> = dir
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME
            | FILE_NOTIFY_CHANGE_DIR_NAME
            | FILE_NOTIFY_CHANGE_SIZE
            | FILE_NOTIFY_CHANGE_LAST_WRITE;

        let change_handle = unsafe {
            FindFirstChangeNotificationW(wide_dir.as_ptr(), i32::from(recursive), filter)
        };
        if change_handle.is_null() || change_handle == INVALID_HANDLE_VALUE {
            return;
        }

        let stop_handle = stop.raw_handle();
        let handles = [stop_handle, change_handle];

        loop {
            if stop.is_stopped() {
                break;
            }
            let res = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
            if res == WAIT_OBJECT_0 || stop.is_stopped() {
                break;
            }
            if res != WAIT_OBJECT_0 + 1 {
                break;
            }

            if unsafe { FindNextChangeNotification(change_handle) } == 0 {
                break;
            }

            // Debounce burst notifications (e.g., Photoshop/CorelDRAW creating -> writing -> renaming a file)
            let burst_start = Instant::now();
            loop {
                if stop.is_stopped() {
                    break;
                }
                let elapsed = burst_start.elapsed();
                if elapsed >= Duration::from_millis(1200) {
                    break;
                }
                let wait_ms = 280u32;
                let dres = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, wait_ms) };
                if dres == WAIT_OBJECT_0 || stop.is_stopped() {
                    break;
                } else if dres == WAIT_OBJECT_0 + 1 {
                    if unsafe { FindNextChangeNotification(change_handle) } == 0 {
                        break;
                    }
                    continue;
                } else if dres == WAIT_TIMEOUT {
                    break;
                } else {
                    break;
                }
            }

            if stop.is_stopped() {
                break;
            }
            dirty.store(true, Ordering::SeqCst);
            if let Some(ref egui_ctx) = ctx {
                egui_ctx.request_repaint();
            }
        }

        unsafe {
            FindCloseChangeNotification(change_handle);
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub struct StopSignal {
        stopped: Arc<AtomicBool>,
    }

    pub struct ThreadStopHandle {
        stopped: Arc<AtomicBool>,
    }

    impl StopSignal {
        pub fn new() -> Self {
            Self {
                stopped: Arc::new(AtomicBool::new(false)),
            }
        }

        pub fn clone_for_thread(&self) -> ThreadStopHandle {
            ThreadStopHandle {
                stopped: self.stopped.clone(),
            }
        }

        pub fn signal(self) {
            self.stopped.store(true, Ordering::SeqCst);
        }
    }

    pub fn run_watch_loop(
        dir: &Path,
        _recursive: bool,
        dirty: Arc<AtomicBool>,
        stop: ThreadStopHandle,
        ctx: Option<egui::Context>,
    ) {
        let mut last_count = count_entries(dir);
        while !stop.stopped.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(1000));
            if stop.stopped.load(Ordering::SeqCst) {
                break;
            }
            let now = count_entries(dir);
            if now != last_count {
                last_count = now;
                dirty.store(true, Ordering::SeqCst);
                if let Some(ref egui_ctx) = ctx {
                    egui_ctx.request_repaint();
                }
            }
        }
    }

    fn count_entries(dir: &Path) -> usize {
        std::fs::read_dir(dir).map(|rd| rd.count()).unwrap_or(0)
    }
}

use platform::{run_watch_loop, StopSignal};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_watcher_detects_new_file_automatically() {
        let tmp = std::env::temp_dir().join(format!(
            "thumbcraft_watch_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let mut watcher = FolderWatcher::new();
        watcher.set_target(Some((tmp.clone(), false)), None);
        // Give watcher thread a brief moment to register FindFirstChangeNotificationW
        thread::sleep(Duration::from_millis(80));
        assert!(!watcher.take_dirty());

        // Create a brand new file inside the watched folder
        std::fs::write(tmp.join("new_art.png"), b"fake").unwrap();

        let deadline = Instant::now() + Duration::from_secs(4);
        let mut detected = false;
        while Instant::now() < deadline {
            if watcher.take_dirty() {
                detected = true;
                break;
            }
            thread::sleep(Duration::from_millis(30));
        }

        watcher.stop();
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(detected, "FolderWatcher did not detect newly created file!");
    }
}
