//! The process-wide registry with two backends registered through the free functions.
//! (One scenario per file: the global registry cannot be reset within a process.)

mod common;

use common::escl_like::{AdfState, ESCL_ID, EsclLike};
use common::request;
use common::sane_like::{SANE_ID, SaneLike};
use printscan::scan::{RegisterError, Source};

#[test]
fn free_functions_aggregate_and_route_across_registered_backends() {
    printscan::scan::register(Box::new(SaneLike::new(1))).expect("sane");
    printscan::scan::register(Box::new(EsclLike::new(AdfState::Loaded, 2))).expect("escl");
    assert!(printscan::scan::supported());
    assert_eq!(printscan::scan::backend_names(), ["sane", "escl"]);

    // The same name again is an error, not a panic, and changes nothing.
    assert_eq!(printscan::scan::register(Box::new(SaneLike::new(5))), Err(RegisterError::DuplicateName { name: "sane" }));
    assert_eq!(printscan::scan::backend_names(), ["sane", "escl"]);

    let list = printscan::scan::scanner_list().expect("list");
    let found: Vec<&str> = list.items.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(found, [format!("sane:{SANE_ID}"), format!("escl:{ESCL_ID}")]);
    assert_eq!(printscan::scan::scanners().expect("scanners").items.len(), 2);
    assert_eq!(printscan::scan::scanner_details(&format!("escl:{ESCL_ID}")).expect("details").id, format!("escl:{ESCL_ID}"));
    assert!(printscan::scan::status(&format!("sane:{SANE_ID}")).expect("status").is_none());
    assert!(printscan::scan::status(&format!("escl:{ESCL_ID}")).expect("status").is_some());
    assert_eq!(printscan::scan::acquire(&request(&format!("escl:{ESCL_ID}"), Source::Feeder, 2)).expect("acquire").pages.len(), 2);
    assert!(printscan::scan::diagnostic_dump("").expect("dump").contains("== escl =="));
}
