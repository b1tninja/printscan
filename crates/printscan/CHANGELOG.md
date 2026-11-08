# Changelog

All notable changes to `printscan` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- `devices::scan_jobs::write_record` writes the journal durably as well as atomically. On Windows a rename refused
  with a sharing violation or access denied, because another process such as an indexer or an antivirus scanner
  has `scan-job.json` open for a moment, is retried with a short, bounded backoff (about 0.6 s in all) instead of
  failing the write. On Unix the folder is synced after the rename, so the rename itself survives a crash. On
  macOS the flush relies on std's `File::sync_all`, which uses `F_FULLFSYNC` there. The file's contents and format
  are unchanged.

## [0.1.0]

The first release.

Every public struct and enum is `#[non_exhaustive]`. Structs are built with `new`, `Default` and
the `with_*` setters, or by assigning fields to a value from one of those; a `match` on an enum
needs a `_` arm.
