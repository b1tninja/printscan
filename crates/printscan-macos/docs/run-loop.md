# Blocking calls over ImageCaptureCore's callbacks

printscan's scanner contract is blocking: `Backend::acquire` returns when the scan is over, and
the app calls it from a worker thread, cancelling through a `CancelToken` and watching pages
through a `ProgressToken`. ImageCaptureCore is the opposite: every request (`requestOpenSession`,
`requestSelectFunctionalUnit:`, `requestScan`, ...) returns at once, and the outcome arrives later
as a delegate callback. Its headers say that, unless noted otherwise, **every delegate callback
is made on the main thread**. This page says how the backend joins the two.

**Status: compiled for macOS, not yet run against a scanner.** Everything marked *unverified*
below is an assumption read from the API, to be checked on hardware.

## The pieces

```text
 caller thread (any)                     main thread
 ───────────────────                     ───────────
 Backend::acquire
   session::drive(Acquire, MainPort)
     machine.start/event/tick → Command
     MainPort::send(Command) ───────────▶ main-queue block: execute(Command)
                                            ffi::… (requestScan, …)
                                          ImageCaptureCore
                                            delegate callback
                                              ffi::… copies arguments into plain data
     MainPort::wait(50 ms) ◀──── Sink ◀─────  sink().push(device id, Event)
       main thread?  run the main run loop for the slice
       otherwise     wait on the sink's condition variable
```

* **`session`** (plain Rust, every platform): `Command`s are the calls, `Event`s the callbacks,
  each as plain data. A `Machine` decides which command follows which event and when a deadline
  has passed. `Probe` opens a session, selects and reads every functional unit, and closes it;
  `Acquire` opens, picks the unit (the feeder first when `Source::Auto` may use it, to learn from
  `documentLoaded` whether paper is in it), writes the plan, scans, collects each page file,
  and closes. `drive` loops: wait one slice for events, feed them to the machine, send what it
  asks for, let it look at the clock. The tests drive the same machines with a scripted device.
* **`main_thread`**: the `Port` the machines run against on macOS. `send` puts a block on the main
  dispatch queue; the block finds the Objective-C state (the delegate and the device browser) in a
  thread-local of the main thread and calls `ffi`. A block carries only plain data (a device id, a
  `Command`), so no Objective-C object ever crosses a thread.
* **`delegate`**: one Objective-C class, the delegate of the browser and of every scanner device.
  Each callback copies its arguments into plain data and pushes them into the **sink**: a mutex
  and a condition variable. Browser news goes to the device table; session callbacks go to a
  queue for their device, and only while a session with that device runs.

## Who runs the main run loop

The callbacks are delivered by the main thread's run loop (and the blocks by the main queue,
which that run loop drains). Something must run it:

* **Called from the main thread** (a command-line tool, the example): `MainPort::wait` runs the
  main run loop itself for each 50 ms slice (`NSRunLoop::runMode:beforeDate:` in the default mode),
  which runs the queued blocks and delivers the callbacks on the spot.
* **Called from another thread** (an AppKit app, where `NSApplication` runs the main run loop):
  `wait` sleeps on the sink's condition variable, which every callback signals, while the main
  thread does the work. The app's UI stays responsive.
* **Called from another thread while the main thread is blocked** (a tool that scans on a worker
  and joins it from `main`): nothing would ever answer. Every step has a deadline
  (`IcaOptions::step_timeout`, 30 s; `scan_idle_timeout`, 3 minutes without a page or progress),
  and a missed one is an error, `ica-backend 1 (TIMED_OUT)`, whose message says the main thread
  must run its run loop. A listing whose browser never started fails the same way.

Calling from inside a main-queue block or an AppKit event handler works (the wait runs a nested
run loop) but holds up the app's UI for the length of the scan; call from a worker.

## Listing

The device browser starts on the first listing and keeps running for the life of the process, so
later listings answer from the device table at once. A first listing waits until the browser has
reported the local devices (`deviceBrowserDidEnumerateLocalDevices:`), at most
`IcaOptions::list_timeout`. With network scanners turned on it also gives them
`network_settle` (3 s) to announce themselves, since Bonjour has no "that was all".

## One session at a time

A process-wide lock serialises sessions: the selected functional unit and the transfer settings
belong to the device, so two callers must not interleave on one. A listing does not take the lock.

## Cancel

`CancelToken::request` (any thread) calls the canceller the backend registered, which pushes
`Event::CancelRequested` into the sink for the session's device. The machine sends `cancelScan`
while scanning, or closes the session if it has not started to scan; the scan's own completion
callback (with `ICReturnScanOperationCanceled`) then ends it, and the pages that arrived are kept.
A fixed page count from the feeder is enforced the same way: once that many pages arrived, the
backend cancels the rest. *Unverified*: whether a scanner has already pulled the next sheet by then.

## Pages

The transfer is file-based: ImageCaptureCore writes each page (TIFF unless the request names PNG,
JPEG or BMP) into a fresh temporary directory and calls `scannerDevice:didScanToURL:`. The
backend reads the file (it must be in that directory and at most 1 GiB), removes it, and builds
the `Page` with printscan's `bw::page_for`, which reads the resolution the file carries and, for a
black-and-white request scanned in grey, thresholds it. The directory is removed when the scan
ends. Memory-based band transfer (`ICScannerBandData`) is not used.

## Unverified on hardware

* That `scannerDevice:didScanToURL:` arrives once per page from a document feeder.
* That a request to select the unit that is already selected gets no callback (the backend answers
  for it itself), and that a session's readiness is signalled by `deviceDidBecomeReady:`,
  `scannerDeviceDidBecomeAvailable:` or a first `didSelectFunctionalUnit` (otherwise the backend
  goes on after `ready_grace`, 3 s).
* The origin of `scanArea`: the backend takes it to be the top left corner of the unit's area.
* Which return codes a real scanner sends for a jam, an empty feeder or an open cover
  (`ica::errors` classifies the codes by their names).
* Whether a sandboxed app needs `requestControlAuthorization` or entitlements beyond USB and network.
