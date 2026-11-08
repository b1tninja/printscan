# Unsafe code audit: the ImageCaptureCore backend

The workspace sets `unsafe_code = "forbid"`. `printscan-macos` denies it instead (its `Cargo.toml`
says why), and only two modules carry `#![allow(unsafe_code)]`: `src/ica/ffi.rs` and
`src/ica/delegate.rs`. This page says why, where the unsafe code is, what each kind relies on, and
how to review it. Everything not listed here is safe Rust.

## Owner sign-off

**Pending.** The crate is experimental (`publish = false`), and CONTRIBUTING.md's rule that no
`unsafe` lives outside `printscan-windows`' audited modules has not yet been extended to it by the
owner. The sign-off should cover what this page describes and no more: the 25 blocks in
`src/ica/ffi.rs` and the class definition and one block in `src/ica/delegate.rs`, each with a
`// SAFETY:` comment.

## Why unsafe is needed

Scanners on macOS are reached through ImageCaptureCore, an Objective-C framework. The crate uses
the maintained [objc2](https://github.com/madsmtm/objc2) bindings rather than hand-written FFI:
`objc2-image-capture-core` declares the classes and protocols, `objc2-foundation` the Foundation
types, `dispatch2` the main queue. The header translator cannot prove the contracts of
ImageCaptureCore's methods, so it marks nearly all of them `unsafe fn`, even property getters with
no arguments; the extern constants (`ICErrorDomain`, `NSDefaultRunLoopMode`) are unsafe to read;
and a class that implements a protocol is declared with `define_class!`, whose conformances are
`unsafe impl`. There is no safe wrapper for ImageCaptureCore. Foundation's run loop, dates,
strings, URLs, errors, arrays and index sets are already safe in objc2-foundation and are used
without `unsafe`.

## Where it lives

| File | Unsafe | Role |
|------|--------|------|
| `src/ica/ffi.rs` | 25 blocks | Small safe functions, each wrapping the calls of one read or one request. |
| `src/ica/delegate.rs` | `define_class!` (1 class, 4 `unsafe impl`, 14 `#[unsafe(method(...))]`) and 1 block | The delegate ImageCaptureCore calls back into. |
| `src/ica/main_thread.rs` | none | The main-queue bridge and the event sink. Reaches Objective-C only through `ffi` and `delegate`. |
| `src/ica/backend.rs`, `session.rs`, `plan.rs`, `caps.rs`, `errors.rs`, `model.rs`, `devices.rs` | none | The backend, the state machines and the rules; the last six build and are tested on every platform. |

`[lints.rust] unsafe_code = "deny"` rejects unsafe anywhere else, and
`clippy::undocumented_unsafe_blocks = "deny"` rejects a block without a `// SAFETY:` comment.

## Categories and their safety arguments

**1. Extern constants (2).** `NSDefaultRunLoopMode` and `ICErrorDomain` are immutable `NSString`
constants exported by the frameworks the crate links; reading an extern static is sound once the
framework is loaded, which linking guarantees. `ICErrorDomain` is declared optional and read as such.

**2. Device reads (3).** `persistentIDString`, `UUIDString`, `type`, `name`, `transportType`,
`locationDescription`, `isRemote` on an `ICDevice` passed to a delegate method or retained from the
browser's `devices`. Getters with no arguments; strings come back retained and are copied into
Rust `String`s before the function returns.

**3. The device browser (3).** `init` on a fresh `ICDeviceBrowser::alloc()` (its designated
initialiser); `setDelegate:`, `browsedDeviceTypeMask`, `setBrowsedDeviceTypeMask:`, `isBrowsing`,
`start`, `stop` on that browser, with the mask changed only while it is stopped; `devices`. The
delegate is weak in the browser; the main-thread state keeps it for the life of the process.

**4. Session requests (5).** `setDelegate:`, `hasOpenSession`, `requestOpenSession`,
`requestCloseSession` and `cancelScan` on a retained `ICScannerDevice`. The delegate is set before
the first request, conforms to `ICDeviceDelegate` and `ICScannerDeviceDelegate`, and outlives the device's use of it.

**5. Functional units: list and selection (4).** `availableFunctionalUnitTypes` (an
`NSArray<NSNumber>`, at most 256 read), `selectedFunctionalUnit`, a unit's `type`, and
`requestSelectFunctionalUnit:` with a value of `ICScannerFunctionalUnitType` built from a known
`UnitKind`.

**6. Functional units: reads (4).** The getters of the selected unit (resolutions, bit depths,
measurement units, physical size, threshold, state, progress), and of a document feeder
(`supportedDocumentTypes`, `supportsDuplexScanning`, `documentLoaded`) or a flatbed
(`supportedDocumentTypes`) after a checked `downcast` (objc2's, which tests the class at run time).
Index sets are walked with `firstIndex`/`indexGreaterThanIndex:`, at most 256 values and only
upwards (`model::walk_index_set`); sizes are refused unless finite.

**7. Functional units: writes and the scan (4).** `setMeasurementUnit:`, `setResolution:`,
`setPixelDataType:`, `setBitDepth:`, `setUsesThresholdForBlackAndWhiteScanning:`,
`setThresholdForBlackAndWhiteScanning:`, `setScanArea:` on the selected unit, `setDocumentType:`
and `setDuplexScanningEnabled:` on a downcast feeder or flatbed; then `setTransferMode:`,
`setDownloadsDirectory:`, `setDocumentUTI:`, `setDocumentName:` and `requestScan` on the device.
Every value is of the type and enumeration the setter takes; the plan (`plan.rs`) takes resolutions,
depths and document types from the unit's own lists. The URL and strings are retained locals that
outlive the calls.

**8. The delegate class (`delegate.rs`).** `define_class!` declares `PrintscanIcaDelegate`, a
subclass of `NSObject` with no ivars and no `Drop`, main-thread-only because ImageCaptureCore calls
delegates on the main thread. Its `unsafe impl`s declare conformance to `NSObjectProtocol`,
`ICDeviceBrowserDelegate`, `ICDeviceDelegate` and `ICScannerDeviceDelegate`; each
`#[unsafe(method(...))]` must carry the selector and argument types of the protocol method, and
nullable `NSError` arguments are taken as `Option<&NSError>`. The required methods of each
protocol are all implemented. The one block sends `init` to a fresh allocation through
`msg_send![super(this), init]`. The methods themselves are safe Rust: they copy their arguments
through `ffi` into plain data and push it into the sink; they never touch the main-thread state,
so they cannot re-enter a running block.

## Threads

* Every `ffi` function runs on the main thread: in a block on the main dispatch queue, or in a
  delegate callback. The Objective-C objects live in a thread-local of the main thread
  (`main_thread::MAIN`) and are never sent to another thread; blocks carry plain data only
  (`dispatch2` requires them to be `Send`, which the compiler checks).
* The main-thread state is borrowed by one block at a time; a block that finds it borrowed (a
  nested run loop) puts itself back on the queue instead of borrowing twice.
* The waiting thread holds no lock while it runs the run loop or waits, so a callback can always
  push into the sink.

## How to review

```text
# every unsafe site in the crate (expect hits only in src/ica/ffi.rs and src/ica/delegate.rs)
rg -n "\bunsafe\b" crates/printscan-macos/src --glob '*.rs'

# the allow attributes (expect exactly two)
rg -n "allow\(unsafe_code\)" crates/printscan-macos

# the compiler and Clippy enforce both rules, for the macOS target
cargo clippy --target aarch64-apple-darwin -p printscan-macos --all-targets -- -D warnings
```

For each site, check that the receiver is a live, retained object; that every argument is of the
type and range the method takes; and that nothing borrowed from the call outlives it.

## Known limits

* **Not run on a Mac.** The Objective-C layer has been compiled and linted for
  `aarch64-apple-darwin` only; CI's macOS job also builds and runs the platform-independent tests
  there. No call has been made against a real ImageCaptureCore.
* **Framework-supplied data.** Getters are trusted to return what their declarations say; the
  mitigations are the bounds above (array and index-set lengths, finite sizes, page files of at
  most 1 GiB read only from the scan's own directory), not validation of the framework.
* **Not covered by tools.** Miri cannot run Objective-C; the tests cover the plain-Rust rules and
  the state machines with a scripted device.
