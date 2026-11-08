//! What the driver's validated PrintTicket kept of a job's explicit settings. A driver validating a ticket may
//! quietly replace an option it does not take: a job asking for two sides would come out one-sided, or a grey job
//! in colour. The spooler reads the validated ticket back and refuses such a job before anything is sent.
//!
//! Only what the job asked for is checked. A setting left to the queue (no duplex asked for, colour neither grey
//! nor colour) is the driver's to choose, and the queue's own preferences then apply.

use printscan::print::Duplex;
use winprint::ticket::document::reader::ParsableXmlDocument as _;
use winprint::ticket::document::{NS_PSK, OwnedName, PrintFeature, PrintTicketDocument};
use winprint::ticket::{
    Collation, JobDuplex, PageOrientation, PageOutputColor, PredefinedCollation, PredefinedDuplexType, PredefinedName, PredefinedPageOrientation,
    PredefinedPageOutputColor, PrintTicket, StaticFeaturePathOptionPack,
};

/// The options a validated ticket holds for the features a job can ask for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Chosen {
    pub duplex: Option<PredefinedDuplexType>,
    pub collation: Option<PredefinedCollation>,
    pub colour: Option<PredefinedPageOutputColor>,
    pub orientation: Option<PredefinedPageOrientation>,
    pub copies: Option<u32>,
}

impl Chosen {
    /// What `ticket` chose; every field `None` when it cannot be read.
    pub fn read(ticket: &PrintTicket) -> Self {
        let Ok(document) = PrintTicketDocument::parse_from_bytes(ticket.get_xml()) else {
            return Self::default();
        };
        let pick = |path: &[OwnedName]| option_at(&document.features, path).cloned();
        let copies = document
            .parameter_inits
            .iter()
            .find(|p| p.name.local_name == "JobCopiesAllDocuments" && p.name.namespace_ref() == Some(NS_PSK))
            .and_then(|p| p.value.integer())
            .and_then(|n| u32::try_from(n).ok());
        Self {
            duplex: pick(JobDuplex::feature_path()).as_ref().and_then(PredefinedDuplexType::from_name),
            collation: pick(Collation::feature_path()).as_ref().and_then(PredefinedCollation::from_name),
            colour: pick(PageOutputColor::feature_path()).as_ref().and_then(PredefinedPageOutputColor::from_name),
            orientation: pick(PageOrientation::feature_path()).as_ref().and_then(PredefinedPageOrientation::from_name),
            copies,
        }
    }

    /// Whether the job prints on both sides, as the ticket stands (the queue's default included).
    pub fn two_sided(&self) -> bool {
        matches!(self.duplex, Some(PredefinedDuplexType::TwoSidedLongEdge | PredefinedDuplexType::TwoSidedShortEdge))
    }
}

/// The name of the first option of the feature at `path` (names compared by namespace and local name, as the
/// Print Schema defines them; prefixes are the writer's own).
fn option_at<'a>(features: &'a [PrintFeature], path: &[OwnedName]) -> Option<&'a OwnedName> {
    let (first, rest) = path.split_first()?;
    let same = |name: &OwnedName| name.local_name == first.local_name && name.namespace == first.namespace;
    let feature = features.iter().find(|f| same(&f.name))?;
    if rest.is_empty() { feature.options.first()?.name.as_ref() } else { option_at(&feature.features, rest) }
}

/// What a job asks for explicitly, as the ticket's options.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Wanted {
    pub duplex: Option<PredefinedDuplexType>,
    /// Asked only when there are copies to collate and the ticket carries them.
    pub collation: Option<PredefinedCollation>,
    /// Grey (`Grayscale`; `Monochrome` is grey too) or `Color`.
    pub colour: Option<PredefinedPageOutputColor>,
    pub orientation: Option<PredefinedPageOrientation>,
    pub copies: u32,
}

/// The ticket option for `duplex`; `None` for a choice this backend does not know.
pub(crate) fn duplex_option(duplex: Duplex) -> Option<PredefinedDuplexType> {
    match duplex {
        Duplex::Off => Some(PredefinedDuplexType::OneSided),
        Duplex::LongEdge => Some(PredefinedDuplexType::TwoSidedLongEdge),
        Duplex::ShortEdge => Some(PredefinedDuplexType::TwoSidedShortEdge),
        _ => None,
    }
}

/// The settings the driver did not keep, as phrases for an error ("two-sided on the long edge"); empty when it kept
/// them all. A setting the ticket does not show at all counts as not kept, except one-sided, grey and portrait: a
/// printer with no two-sided, colour or orientation feature prints that way anyway.
pub(crate) fn dropped(wanted: &Wanted, chosen: &Chosen) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(duplex) = wanted.duplex
        && chosen.duplex != Some(duplex)
        && !(duplex == PredefinedDuplexType::OneSided && chosen.duplex.is_none())
    {
        out.push(
            match duplex {
                PredefinedDuplexType::OneSided => "one-sided",
                PredefinedDuplexType::TwoSidedLongEdge => "two-sided on the long edge",
                PredefinedDuplexType::TwoSidedShortEdge => "two-sided on the short edge",
            }
            .to_owned(),
        );
    }
    if let Some(collation) = wanted.collation
        && chosen.collation != Some(collation)
    {
        out.push(if collation == PredefinedCollation::Collated { "collated copies" } else { "uncollated copies" }.to_owned());
    }
    match wanted.colour {
        Some(PredefinedPageOutputColor::Color) if chosen.colour != Some(PredefinedPageOutputColor::Color) => out.push("colour".to_owned()),
        Some(PredefinedPageOutputColor::Grayscale | PredefinedPageOutputColor::Monochrome)
            if chosen.colour == Some(PredefinedPageOutputColor::Color) =>
        {
            out.push("grey".to_owned());
        }
        _ => {}
    }
    if let Some(orientation) = wanted.orientation
        && chosen.orientation != Some(orientation)
        && !(orientation == PredefinedPageOrientation::Portrait && chosen.orientation.is_none())
    {
        out.push(format!("{orientation:?} orientation").to_lowercase());
    }
    if wanted.copies > 1 && chosen.copies != Some(wanted.copies) {
        out.push(format!("{} copies", wanted.copies));
    }
    // One copy asked (also when the job draws its copies itself): a ticket without a count prints one, but one that
    // came back with more would multiply every copy.
    if wanted.copies <= 1 && chosen.copies.is_some_and(|copies| copies > 1) {
        out.push("one copy".to_owned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A validated ticket as drivers write it: their own prefixes, one option per feature, copies as a parameter.
    const TICKET: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<psf:PrintTicket xmlns:psf="http://schemas.microsoft.com/windows/2003/08/printing/printschemaframework"
    xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema"
    xmlns:ns0000="http://schemas.microsoft.com/windows/2003/08/printing/printschemakeywords" version="1">
  <psf:Feature name="ns0000:JobDuplexAllDocumentsContiguously"><psf:Option name="ns0000:TwoSidedLongEdge"/></psf:Feature>
  <psf:Feature name="ns0000:DocumentCollate"><psf:Option name="ns0000:Collated"/></psf:Feature>
  <psf:Feature name="ns0000:PageOutputColor"><psf:Option name="ns0000:Monochrome"/></psf:Feature>
  <psf:Feature name="ns0000:PageOrientation"><psf:Option name="ns0000:Landscape"/></psf:Feature>
  <psf:ParameterInit name="ns0000:JobCopiesAllDocuments"><psf:Value xsi:type="xsd:integer">3</psf:Value></psf:ParameterInit>
</psf:PrintTicket>"#;

    #[test]
    fn a_validated_ticket_is_read_whatever_prefixes_the_driver_wrote() {
        let chosen = Chosen::read(&PrintTicket::from_xml(TICKET));
        assert_eq!(
            chosen,
            Chosen {
                duplex: Some(PredefinedDuplexType::TwoSidedLongEdge),
                collation: Some(PredefinedCollation::Collated),
                colour: Some(PredefinedPageOutputColor::Monochrome),
                orientation: Some(PredefinedPageOrientation::Landscape),
                copies: Some(3),
            }
        );
        assert!(chosen.two_sided());
        assert_eq!(Chosen::read(&PrintTicket::from_xml("not a ticket")), Chosen::default());
    }

    #[test]
    fn what_the_driver_kept_passes_and_what_it_replaced_is_named() {
        let chosen = Chosen::read(&PrintTicket::from_xml(TICKET));
        let kept = Wanted {
            duplex: Some(PredefinedDuplexType::TwoSidedLongEdge),
            collation: Some(PredefinedCollation::Collated),
            colour: Some(PredefinedPageOutputColor::Grayscale),
            orientation: Some(PredefinedPageOrientation::Landscape),
            copies: 3,
        };
        assert!(dropped(&kept, &chosen).is_empty(), "Monochrome is grey");
        let replaced = Wanted {
            duplex: Some(PredefinedDuplexType::TwoSidedShortEdge),
            colour: Some(PredefinedPageOutputColor::Color),
            orientation: Some(PredefinedPageOrientation::Portrait),
            copies: 2,
            ..kept
        };
        assert_eq!(dropped(&replaced, &chosen), ["two-sided on the short edge", "colour", "portrait orientation", "2 copies"]);
        // Nothing asked, nothing checked; one copy needs no count.
        assert!(dropped(&Wanted { copies: 1, ..Wanted::default() }, &Chosen::default()).is_empty());
        // One copy asked, as when the job draws its copies itself: a count of one stands, more is refused.
        let one = Wanted { copies: 1, ..Wanted::default() };
        assert!(dropped(&one, &Chosen { copies: Some(1), ..Chosen::default() }).is_empty());
        assert_eq!(dropped(&one, &Chosen { copies: Some(3), ..Chosen::default() }), ["one copy"]);
        // A feature missing from the ticket was not kept, unless the printer does that anyway.
        let missing = |wanted: Wanted| dropped(&Wanted { copies: 1, ..wanted }, &Chosen::default());
        assert_eq!(missing(Wanted { duplex: Some(PredefinedDuplexType::TwoSidedLongEdge), ..Wanted::default() }), ["two-sided on the long edge"]);
        assert_eq!(missing(Wanted { colour: Some(PredefinedPageOutputColor::Color), ..Wanted::default() }), ["colour"]);
        assert!(missing(Wanted { duplex: Some(PredefinedDuplexType::OneSided), ..Wanted::default() }).is_empty());
        assert!(missing(Wanted { colour: Some(PredefinedPageOutputColor::Grayscale), ..Wanted::default() }).is_empty());
    }
}
