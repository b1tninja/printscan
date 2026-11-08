//! The bridge between printscan's blocking calls and ImageCaptureCore's main-thread callbacks.
//!
//! * **Calls go to the main queue.** [`on_main`] runs a block on the main dispatch queue; the
//!   block finds the main-thread state (the delegate, the browser) in a thread-local of the main
//!   thread and makes the Objective-C calls through `ffi`. Nothing Objective-C is ever sent between
//!   threads: a block carries plain data (a device id, a [`Command`]) and nothing else.
//! * **Answers come back through the [`Sink`].** The delegate (main thread) turns each callback
//!   into plain data and puts it in the sink: device-browser news into the device table, session
//!   callbacks into a queue per device. The sink is a mutex and a condition variable, so any thread
//!   can wait on it.
//! * **Waiting.** [`pump`] waits one slice for news. On the main thread it runs the main run loop
//!   for that slice, which runs the blocks and delivers the callbacks; on any other thread it waits
//!   on the condition variable while the main thread's own run loop does that work.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_image_capture_core::{ICDeviceBrowser, ICScannerDevice};

use super::delegate::Delegate;
use super::devices::DeviceTable;
use super::errors;
use super::ffi;
use super::model::DeviceInfo;
use super::session::{Command, Event, Port};

/// What the delegate has reported, shared by every thread.
#[derive(Default)]
pub(crate) struct Sink {
    state: Mutex<SinkState>,
    changed: Condvar,
}

#[derive(Default)]
struct SinkState {
    table: DeviceTable,
    /// The devices a session is being driven with, and the events not yet taken for each.
    sessions: HashMap<String, Vec<Event>>,
}

/// The most events kept for one device before the oldest are dropped: a session takes them every slice.
const MAX_QUEUED: usize = 1024;

impl Sink {
    fn with<T>(&self, change: impl FnOnce(&mut SinkState) -> T) -> T {
        let value = change(&mut self.state.lock().unwrap_or_else(PoisonError::into_inner));
        self.changed.notify_all();
        value
    }

    pub(crate) fn device_added(&self, device: DeviceInfo) {
        self.with(|state| state.table.add(device));
    }

    /// A device went away: out of the table, and a [`Event::Removed`] for its session if one runs.
    pub(crate) fn device_removed(&self, id: &str) {
        self.with(|state| {
            state.table.remove(id);
            if let Some(queue) = state.sessions.get_mut(id) {
                queue.push(Event::Removed);
            }
        });
    }

    pub(crate) fn local_enumerated(&self) {
        self.with(|state| state.table.local_enumerated());
    }

    fn browsing(&self, network: bool) {
        self.with(|state| state.table.started(Instant::now(), network));
    }

    /// An event for the session with device `id`; dropped when no session runs with it.
    pub(crate) fn push(&self, id: &str, event: Event) {
        self.with(|state| {
            if let Some(queue) = state.sessions.get_mut(id) {
                if queue.len() >= MAX_QUEUED {
                    queue.remove(0);
                }
                queue.push(event);
            }
        });
    }

    fn take(&self, id: &str) -> Vec<Event> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.sessions.get_mut(id).map(std::mem::take).unwrap_or_default()
    }

    /// Wait up to `slice` for any news.
    fn wait(&self, slice: Duration) {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = self.changed.wait_timeout(state, slice);
    }

    /// The device table, read under the lock.
    pub(crate) fn table<T>(&self, read: impl FnOnce(&DeviceTable) -> T) -> T {
        read(&self.state.lock().unwrap_or_else(PoisonError::into_inner).table)
    }
}

/// The process's sink.
pub(crate) fn sink() -> &'static Arc<Sink> {
    static SINK: OnceLock<Arc<Sink>> = OnceLock::new();
    SINK.get_or_init(Arc::default)
}

/// The Objective-C objects, which live on the main thread only.
struct MainState {
    /// The delegate of the browser and of every device; weak in both, so it is kept here.
    delegate: Retained<Delegate>,
    browser: Option<Retained<ICDeviceBrowser>>,
    /// The browse mask asked for so far (a network browse, once asked for, stays on).
    mask: usize,
}

impl MainState {
    fn new(mtm: MainThreadMarker) -> Self {
        Self { delegate: Delegate::new(mtm), browser: None, mask: 0 }
    }

    fn browse(&mut self, network: bool) {
        self.mask |= ffi::browse_mask(network);
        match &self.browser {
            Some(browser) => ffi::set_browser(browser, &self.delegate, self.mask),
            None => self.browser = Some(ffi::new_browser(&self.delegate, self.mask)),
        }
    }

    /// The scanner the browser lists with native id `id`.
    fn scanner(&self, id: &str) -> Option<Retained<ICScannerDevice>> {
        let browser = self.browser.as_ref()?;
        ffi::browser_devices(browser)
            .into_iter()
            .find(|device| ffi::device_id(device).as_deref() == Some(id))
            .and_then(|device| device.downcast::<ICScannerDevice>().ok())
    }
}

thread_local! {
    /// Set on the main thread by the first block that runs there; never touched on another thread.
    static MAIN: RefCell<Option<MainState>> = const { RefCell::new(None) };
}

type Work = Box<dyn FnOnce(&mut MainState) + Send>;

/// Run `work` on the main queue with the main-thread state.
fn on_main(work: Work) {
    DispatchQueue::main().exec_async(move || {
        let Some(mtm) = MainThreadMarker::new() else {
            log::warn!("scan: a main-queue block ran off the main thread; ImageCaptureCore call skipped");
            return;
        };
        let pending = MAIN.with(|cell| match cell.try_borrow_mut() {
            Ok(mut slot) => {
                work(slot.get_or_insert_with(|| MainState::new(mtm)));
                None
            }
            // Another block is running further up this stack (a nested run loop); go again later.
            Err(_) => Some(work),
        });
        if let Some(work) = pending {
            on_main(work);
        }
    });
}

/// Wait up to `slice` for news: run the main run loop on the main thread, wait on the sink elsewhere.
pub(crate) fn pump(slice: Duration) {
    match MainThreadMarker::new() {
        Some(mtm) => ffi::run_main_loop(mtm, slice),
        None => sink().wait(slice),
    }
}

/// Make sure the browser runs (with network scanners when `network`).
pub(crate) fn browse(network: bool) {
    if sink().table(|table| table.is_browsing(network)) {
        return;
    }
    on_main(Box::new(move |state| {
        state.browse(network);
        sink().browsing(network);
    }));
}

/// A [`Port`] to one device, through the main queue and the sink.
pub(crate) struct MainPort {
    id: String,
}

impl MainPort {
    /// Start taking events for device `id`.
    pub(crate) fn new(id: &str) -> Self {
        sink().with(|state| state.sessions.insert(id.to_string(), Vec::new()));
        Self { id: id.to_string() }
    }
}

impl Drop for MainPort {
    fn drop(&mut self) {
        sink().with(|state| state.sessions.remove(&self.id));
    }
}

impl Port for MainPort {
    fn send(&mut self, command: Command) {
        let id = self.id.clone();
        on_main(Box::new(move |state| execute(state, &id, command)));
    }

    fn wait(&mut self, slice: Duration) -> Vec<Event> {
        let ready = sink().take(&self.id);
        if !ready.is_empty() {
            return ready;
        }
        pump(slice);
        sink().take(&self.id)
    }
}

/// Carry out `command` for device `id`, on the main thread.
fn execute(state: &mut MainState, id: &str, command: Command) {
    let answer = |event: Event| sink().push(id, event);
    let Some(scanner) = state.scanner(id) else {
        // A close for a device that went away has nothing left to do.
        let event = if command == Command::Close { Event::Closed(None) } else { Event::Refused(errors::no_such_device(id)) };
        return answer(event);
    };
    match command {
        Command::Open => {
            ffi::set_device_delegate(&scanner, &state.delegate);
            if ffi::has_open_session(&scanner) {
                answer(Event::Opened(None));
            } else {
                ffi::open_session(&scanner);
            }
        }
        Command::Units => answer(Event::Units(ffi::unit_types(&scanner))),
        Command::Select(kind) => {
            // A unit that is already selected gets no callback, so answer for it.
            if ffi::selected_kind(&scanner) == Some(kind) {
                answer(Event::Selected { unit: Some(kind), error: None });
            } else {
                ffi::select_unit(&scanner, kind);
            }
        }
        Command::Read => match ffi::read_unit(&scanner) {
            Some(info) => answer(Event::UnitRead(info)),
            None => answer(Event::Refused(printscan::scan::ScanError::Failed("the selected functional unit is of an unknown kind".into()))),
        },
        Command::Configure(setup) => {
            ffi::configure(&scanner, &setup);
            answer(Event::Configured);
        }
        Command::Scan(transfer) => {
            if let Err(error) = ffi::start_scan(&scanner, &transfer) {
                answer(Event::Refused(error));
            }
        }
        Command::Progress => answer(Event::Progress(ffi::progress(&scanner))),
        Command::Cancel => ffi::cancel_scan(&scanner),
        Command::Close => {
            if ffi::has_open_session(&scanner) {
                ffi::close_session(&scanner);
            } else {
                answer(Event::Closed(None));
            }
        }
    }
}

/// The devices the browser has listed once it settled, or the error when it never started.
pub(crate) fn devices(options: &super::IcaOptions) -> Result<Vec<DeviceInfo>, printscan::scan::ScanError> {
    browse(options.network);
    let asked = Instant::now();
    loop {
        let now = Instant::now();
        let (settled, started) = sink().table(|table| (table.settled(now, options), table.is_browsing(options.network)));
        if settled {
            return Ok(sink().table(DeviceTable::devices));
        }
        if !started && now.saturating_duration_since(asked) >= options.list_timeout {
            return Err(errors::timed_out("starting the device browser", options.list_timeout));
        }
        pump(super::session::SLICE);
    }
}

/// Tell the session with device `id` that the caller cancelled. Any thread.
pub(crate) fn cancel(id: &str) {
    sink().push(id, Event::CancelRequested);
}
