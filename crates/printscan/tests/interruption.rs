//! What to offer after a scan stops early: the actions, the remedy and what the next leg asks for. The cases are
//! the ones the desktop app's Scan interrupted dialog has always handled.

#![cfg(feature = "scan")]

use printscan::scan::{InterruptAction as A, InterruptKind as K, Interruption, Pass, Remedy, Source};

#[test]
fn pages_kept_with_more_wanted_offer_continue_for_the_rest() {
    let stop = Interruption::new(K::Jam, 2, Some(5)).with_source(Source::Feeder);
    assert_eq!(stop.primary(), Some(A::Continue));
    assert_eq!(stop.actions(), vec![A::Continue, A::Finish]);
    assert_eq!(stop.remaining(), Some(3));
    let resume = stop.resume();
    assert_eq!((resume.pages, resume.carried), (Some(3), 2));
    assert_eq!(resume.source, Some(Source::Feeder), "pages came in, so the feeder stays the source");
    assert_eq!(stop.remedy(), Some(Remedy::ClearJam));
}

#[test]
fn the_whole_feeder_has_no_count_and_asks_for_all_of_it_again() {
    let stop = Interruption::new(K::FeederEmpty, 2, None).with_source(Source::Feeder);
    assert_eq!(stop.remaining(), None);
    assert_eq!(stop.actions(), vec![A::Continue, A::Finish]);
    assert_eq!(stop.resume().pages, None);
    assert_eq!(stop.remedy(), Some(Remedy::LoadPaper));
}

#[test]
fn nothing_left_to_scan_offers_only_finish() {
    let stop = Interruption::new(K::Offline, 3, Some(3));
    assert_eq!(stop.primary(), None);
    assert_eq!(stop.actions(), vec![A::Finish]);
    assert_eq!(stop.resume().pages, None, "nothing is left to ask for");
}

#[test]
fn nothing_kept_offers_try_again_with_the_original_request() {
    let stop = Interruption::new(K::CoverOpen, 0, Some(3)).with_source(Source::Feeder);
    assert_eq!(stop.actions(), vec![A::TryAgain, A::Finish]);
    let resume = stop.resume();
    assert_eq!((resume.pages, resume.carried), (Some(3), 0), "start again with the original count");
    assert_eq!(resume.source, None, "nothing came in, so the source asked for stands (Auto stays Auto)");
}

#[test]
fn a_stop_before_the_source_was_known_keeps_the_one_asked_for() {
    assert_eq!(Interruption::new(K::Jam, 2, Some(5)).resume().source, None);
}

#[test]
fn fronts_of_a_manual_duplex_can_turn_to_the_reverse_sides_now() {
    let fronts = Interruption::new(K::FeederEmpty, 3, Some(5)).with_pass(Pass::Fronts).with_source(Source::Feeder);
    assert_eq!(fronts.actions(), vec![A::Continue, A::ReverseNow, A::Finish]);
    // No fronts yet: nothing to turn over.
    assert_eq!(Interruption::new(K::Jam, 0, Some(5)).with_pass(Pass::Fronts).actions(), vec![A::TryAgain, A::Finish]);
    // Every front in: turning over is still offered, beside Finish.
    assert_eq!(Interruption::new(K::Other, 5, Some(5)).with_pass(Pass::Fronts).actions(), vec![A::ReverseNow, A::Finish]);
    // The reverse-side pass never offers to turn over again.
    assert_eq!(Interruption::new(K::Jam, 2, Some(5)).with_pass(Pass::Backs).actions(), vec![A::Continue, A::Finish]);
}

#[test]
fn every_cause_but_a_cancel_or_an_unknown_names_something_to_put_right() {
    for kind in K::ALL.iter().copied() {
        let remedy = Remedy::for_kind(kind);
        assert_eq!(remedy.is_none(), matches!(kind, K::Cancelled | K::Other), "{kind:?}");
    }
    // Each remedy belongs to one cause.
    let named: Vec<Remedy> = K::ALL.iter().filter_map(|kind| Remedy::for_kind(*kind)).collect();
    assert_eq!(named.len(), Remedy::ALL.len());
    assert!(Remedy::ALL.iter().all(|remedy| named.contains(remedy)));
}

#[test]
fn the_rules_have_wire_strings_the_bindings_list() {
    assert_eq!(A::Continue.as_str(), "continue");
    assert_eq!("reverse_now".parse::<A>().ok(), Some(A::ReverseNow));
    assert_eq!(Pass::parse("backs"), Some(Pass::Backs));
    assert_eq!(Remedy::CheckPaperOrWait.to_string(), "check_paper_or_wait");
    let contract = printscan::scan::contract_json();
    for key in [
        "\"passes\":[\"direct\",\"fronts\",\"backs\"]",
        "\"interrupt_actions\":[\"try_again\",\"continue\",\"reverse_now\",\"finish\"]",
        "\"remedies\":[\"close_cover\",",
    ] {
        assert!(contract.contains(key), "{key} in {contract}");
    }
}

#[cfg(feature = "serde")]
#[test]
fn a_stop_and_its_resume_read_back() {
    let stop = Interruption::new(K::BusyOrFeederEmpty, 4, Some(9)).with_pass(Pass::Backs).with_source(Source::Feeder);
    let text = serde_json::to_string(&stop).unwrap();
    assert_eq!(text, r#"{"kind":"busy_or_feeder_empty","received":4,"expected":9,"pass":"backs","source":"feeder"}"#);
    assert_eq!(serde_json::from_str::<Interruption>(&text).unwrap(), stop);
    let resume = serde_json::to_string(&stop.resume()).unwrap();
    assert_eq!(resume, r#"{"pages":5,"carried":4,"source":"feeder"}"#);
}

/// Merge by the rule an app follows while backs arrive, on plain numbers.
fn merged(mut doc: Vec<u32>, first: usize, fronts: usize, backs: &[u32], reversed: bool) -> Vec<u32> {
    for (index, back) in backs.iter().enumerate() {
        let at = printscan::scan::back_position(first, fronts, index, reversed).expect("a front is left");
        doc.insert(at, *back);
    }
    doc
}

#[test]
fn each_back_lands_after_its_front_in_a_flipped_stack_or_in_scan_order() {
    // Fronts 1 2 3 4; the stack was flipped as a whole: the backs of 4, 3, 2, 1 arrive.
    assert_eq!(merged(vec![1, 2, 3, 4], 0, 4, &[14, 13, 12, 11], true), vec![1, 11, 2, 12, 3, 13, 4, 14]);
    // Not flipped: the backs arrive in front order and each shifts the rest down.
    assert_eq!(merged(vec![1, 2, 3, 4], 0, 4, &[11, 12, 13, 14], false), vec![1, 11, 2, 12, 3, 13, 4, 14]);
    // Fewer backs than fronts: they belong to the LAST fronts.
    assert_eq!(merged(vec![1, 2, 3, 4], 0, 4, &[14, 13], true), vec![1, 2, 3, 13, 4, 14]);
    assert_eq!(merged(vec![1, 2, 3, 4], 0, 4, &[11, 12], false), vec![1, 11, 2, 12, 3, 4]);
    assert_eq!(merged(vec![1, 2], 0, 2, &[], true), vec![1, 2]);
    // Fronts that start part-way into the document.
    assert_eq!(merged(vec![7, 8, 1, 2, 9], 2, 2, &[12, 11], true), vec![7, 8, 1, 11, 2, 12, 9]);
    assert_eq!(printscan::scan::back_position(0, 2, 2, true), None, "no front is left for a third back");
    assert_eq!(printscan::scan::back_position(0, 0, 0, true), None);
    assert_eq!(printscan::scan::back_position(usize::MAX, 2, 0, true), Some(usize::MAX), "saturates");
}

#[test]
fn the_backs_pass_asks_for_the_backs_still_missing_and_never_for_none() {
    assert_eq!(printscan::scan::backs_to_request(3, 1), 2);
    assert_eq!(printscan::scan::backs_to_request(3, 3), 1);
    assert_eq!(printscan::scan::backs_to_request(0, 0), 1, "never asks the scanner for zero pages");
}

#[cfg(feature = "devices")]
#[test]
fn placing_backs_in_a_document_agrees_with_the_page_files_a_journal_writes() {
    use printscan::devices::scan_jobs::duplex_page_number;
    for sheets in 1..=6_u32 {
        let n = sheets as usize;
        let fronts: Vec<usize> = (0..n).collect();
        let backs: Vec<usize> = (n..2 * n).collect();
        let doc = merged(fronts.iter().map(|&i| i as u32).collect(), 0, n, &backs.iter().map(|&i| i as u32).collect::<Vec<_>>(), true);
        for (position, index) in doc.iter().enumerate() {
            assert_eq!(duplex_page_number(sheets, *index as usize), position + 1, "{sheets} sheets, scan index {index}");
        }
    }
}
