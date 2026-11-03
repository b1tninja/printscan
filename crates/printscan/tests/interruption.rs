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
