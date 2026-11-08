//! The two CUPS operations this backend sends to the scheduler itself, and what it reads from their
//! replies.
//!
//! `CUPS-Get-Default` and `CUPS-Get-Printers` are CUPS extensions, in the operation-id range
//! RFC 8011 reserves for vendor extensions (0x4000 to 0x7fff). They are posted to the scheduler's
//! root (`/`) and carry no `printer-uri`: the operation group is `attributes-charset`,
//! `attributes-natural-language`, `requesting-user-name`, `requested-attributes` and, for
//! `CUPS-Get-Printers`, a `limit`. Each printer (or class) in the reply is one printer group.

use printscan::print::backends::PrinterState;
use printscan_ipp::codec::{Attribute, Group, GroupTag, Message, Value};
use printscan_ipp::model::{self, PrinterAttrs};

use super::queue::is_valid_queue_name;

/// `CUPS-Get-Default`: the scheduler's default destination.
pub const CUPS_GET_DEFAULT: u16 = 0x4001;

/// `CUPS-Get-Printers`: every printer and class the scheduler has.
pub const CUPS_GET_PRINTERS: u16 = 0x4002;

/// The attributes asked for of each queue.
pub const QUEUE_ATTRIBUTES: &[&str] = &[
    "printer-name",
    "printer-info",
    "printer-location",
    "printer-make-and-model",
    "printer-type",
    "printer-state",
    "printer-state-reasons",
    "printer-is-accepting-jobs",
    "sides-supported",
];

/// Bits of CUPS's `printer-type` this backend reads.
pub mod printer_type {
    /// The destination is a class (a pool of printers), reached at `/classes/<name>`.
    pub const CLASS: i32 = 0x0001;
    /// The destination is shared from another scheduler.
    pub const REMOTE: i32 = 0x0002;
}

/// One queue (a printer or a class) as the scheduler lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Queue {
    /// The queue name: the native printer id of this backend.
    pub name: String,
    /// `printer-info`: the description a person gave the queue.
    pub info: Option<String>,
    /// `printer-location`.
    pub location: Option<String>,
    /// `printer-make-and-model`.
    pub make_and_model: Option<String>,
    /// `printer-type` as the scheduler sent it (CUPS's bit field), when it sent one.
    pub printer_type: Option<i32>,
    /// A class rather than a printer.
    pub is_class: bool,
    /// Shared from another scheduler.
    pub is_remote: bool,
    /// The scheduler's default destination (from `CUPS-Get-Default`).
    pub default: bool,
    pub state: Option<PrinterState>,
    /// `printer-state-reasons` without `none`.
    pub reasons: Vec<String>,
    /// `printer-is-accepting-jobs`.
    pub accepting_jobs: Option<bool>,
    /// Whether `sides-supported` lists a two-sided mode; `None` when it was not sent.
    pub two_sided: Option<bool>,
}

impl Queue {
    /// A queue named `name` about which nothing else is known.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            info: None,
            location: None,
            make_and_model: None,
            printer_type: None,
            is_class: false,
            is_remote: false,
            default: false,
            state: None,
            reasons: Vec::new(),
            accepting_jobs: None,
            two_sided: None,
        }
    }

    /// What a person sees: `printer-info` when it is not empty, else the queue name.
    pub fn display_name(&self) -> &str {
        self.info.as_deref().filter(|i| !i.trim().is_empty()).unwrap_or(&self.name)
    }

    /// Read one printer group. `None` when it names no queue, or a name this backend would refuse
    /// as a native id (see [`crate::queue`]).
    pub fn from_group(group: &Group) -> Option<Self> {
        let attrs = PrinterAttrs::from_group(group);
        let name = attrs.name.clone().filter(|n| is_valid_queue_name(n))?;
        let printer_type = group.first_i32("printer-type");
        let has = |bit: i32| printer_type.is_some_and(|t| t & bit != 0);
        let mut queue = Self::new(name);
        queue.info = attrs.info.clone();
        queue.location = group.first_str("printer-location").map(str::to_owned);
        queue.make_and_model = attrs.make_and_model.clone();
        queue.printer_type = printer_type;
        queue.is_class = has(printer_type::CLASS);
        queue.is_remote = has(printer_type::REMOTE);
        queue.state = model::neutral_printer_state(&attrs);
        queue.reasons = attrs.state_reasons.iter().filter(|r| r.as_str() != "none").cloned().collect();
        queue.accepting_jobs = attrs.accepting_jobs;
        queue.two_sided = attrs.sides.as_ref().map(|s| s.iter().any(|k| k.starts_with("two-sided")));
        Some(queue)
    }
}

/// The request message of a scheduler operation: the operation group with charset, language, the
/// user and `requested-attributes` ([`QUEUE_ATTRIBUTES`]), then `extra`.
pub fn request(version: (u8, u8), code: u16, request_id: u32, user: &str, extra: Vec<Attribute>) -> Message {
    let mut group = Group::new(GroupTag::Operation)
        .with(Attribute::new("attributes-charset", Value::Charset("utf-8".into())))
        .with(Attribute::new("attributes-natural-language", Value::NaturalLanguage("en".into())))
        .with(Attribute::name_value("requesting-user-name", user))
        .with(Attribute::keywords("requested-attributes", QUEUE_ATTRIBUTES));
    group.attributes.extend(extra);
    Message::new(version.0, version.1, code, request_id).with_group(group)
}

/// The queues of a reply, in the scheduler's order, at most `limit`; groups that name no valid
/// queue are skipped.
pub fn queues(reply: &Message, limit: usize) -> Vec<Queue> {
    reply.groups_of(GroupTag::Printer).filter_map(Queue::from_group).take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn printer(name: &str, kind: i32) -> Group {
        Group::new(GroupTag::Printer)
            .with(Attribute::name_value("printer-name", name))
            .with(Attribute::text("printer-info", "Office laser"))
            .with(Attribute::text("printer-location", "Room 2"))
            .with(Attribute::enumeration("printer-type", kind))
            .with(Attribute::enumeration("printer-state", 5))
            .with(Attribute::keywords("printer-state-reasons", &["none", "media-empty-warning"]))
            .with(Attribute::boolean("printer-is-accepting-jobs", false))
            .with(Attribute::keywords("sides-supported", &["one-sided", "two-sided-long-edge"]))
    }

    #[test]
    fn a_printer_group_reads_as_a_queue() {
        let queue = Queue::from_group(&printer("Office", 0x0001 | 0x0004)).unwrap();
        assert_eq!(queue.name, "Office");
        assert_eq!(queue.display_name(), "Office laser");
        assert_eq!(queue.location.as_deref(), Some("Room 2"));
        assert!(queue.is_class && !queue.is_remote && !queue.default);
        assert_eq!(queue.state, Some(PrinterState::Stopped));
        assert_eq!(queue.reasons, ["media-empty-warning"]);
        assert_eq!(queue.accepting_jobs, Some(false));
        assert_eq!(queue.two_sided, Some(true));
        assert_eq!(Queue::new("x").display_name(), "x");
    }

    #[test]
    fn groups_without_a_usable_name_are_skipped_and_the_count_is_capped() {
        let mut reply = Message::new(2, 0, 0, 1).with_group(Group::new(GroupTag::Operation));
        for name in ["a", "bad/name", "b", "c"] {
            reply = reply.with_group(printer(name, 0));
        }
        reply = reply.with_group(Group::new(GroupTag::Printer).with(Attribute::text("printer-info", "no name")));
        let names: Vec<String> = queues(&reply, 10).into_iter().map(|q| q.name).collect();
        assert_eq!(names, ["a", "b", "c"]);
        assert_eq!(queues(&reply, 2).len(), 2);
    }

    #[test]
    fn requests_carry_no_printer_uri() {
        let message = request((2, 0), CUPS_GET_PRINTERS, 7, "printscan", vec![Attribute::integer("limit", 5)]);
        assert_eq!((message.major, message.minor, message.code, message.request_id), (2, 0, 0x4002, 7));
        let group = message.group(GroupTag::Operation).unwrap();
        let names: Vec<&str> = group.attributes.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["attributes-charset", "attributes-natural-language", "requesting-user-name", "requested-attributes", "limit"]);
        assert_eq!(group.first_str("requesting-user-name"), Some("printscan"));
        // It encodes, so the codec accepts the vendor operation id.
        assert!(message.encode().is_ok());
    }
}
