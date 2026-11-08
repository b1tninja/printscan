//! What the driver's validated PrintTicket kept of a job's explicit settings. A driver validating a ticket may
//! quietly replace an option it does not take: a job asking for two sides would come out one-sided, or a grey job
//! in colour. The spooler reads the validated ticket back and refuses such a job before anything is sent.
//!
//! Only what the job asked for is checked. A setting left to the queue (no duplex asked for, colour neither grey
//! nor colour) is the driver's to choose, and the queue's own preferences then apply.
//!
//! Application-managed colour ([`JobOptions::application_managed_colour`](printscan::print::JobOptions)) is checked
//! like the explicit settings: a driver that would colour-manage the document anyway refuses the job. The rendering
//! intent and the destination profile are hints: one the driver lacks or replaces is a warning.

use printscan::print::{Duplex, RenderingIntent};
use winprint::ticket::document::reader::ParsableXmlDocument as _;
use winprint::ticket::document::{NS_PSK, OwnedName, PrintFeature, PrintTicketDocument};
use winprint::ticket::{
    Collation, JobDuplex, PageColorManagement, PageDestinationColorProfile, PageIcmRenderingIntent, PageOrientation, PageOutputColor,
    PredefinedCollation, PredefinedDuplexType, PredefinedName, PredefinedPageColorManagement, PredefinedPageDestinationColorProfile,
    PredefinedPageIcmRenderingIntent, PredefinedPageOrientation, PredefinedPageOutputColor, PrintTicket, StaticFeaturePathOptionPack,
};

/// The options a validated ticket holds for the features a job can ask for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Chosen {
    pub duplex: Option<PredefinedDuplexType>,
    pub collation: Option<PredefinedCollation>,
    pub colour: Option<PredefinedPageOutputColor>,
    pub orientation: Option<PredefinedPageOrientation>,
    pub copies: Option<u32>,
    pub colour_management: Option<PredefinedPageColorManagement>,
    pub destination_profile: Option<PredefinedPageDestinationColorProfile>,
    pub intent: Option<PredefinedPageIcmRenderingIntent>,
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
            colour_management: pick(PageColorManagement::feature_path()).as_ref().and_then(PredefinedPageColorManagement::from_name),
            destination_profile: pick(PageDestinationColorProfile::feature_path())
                .as_ref()
                .and_then(PredefinedPageDestinationColorProfile::from_name),
            intent: pick(PageIcmRenderingIntent::feature_path()).as_ref().and_then(PredefinedPageIcmRenderingIntent::from_name),
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
    /// `None` (no colour management), asked only when the job's colours are application-managed and the driver
    /// offers it.
    pub colour_management: Option<PredefinedPageColorManagement>,
    /// `Application`, a hint beside [`Self::colour_management`] when the driver offers it.
    pub destination_profile: Option<PredefinedPageDestinationColorProfile>,
    /// A hint, asked only when the driver offers it.
    pub intent: Option<PredefinedPageIcmRenderingIntent>,
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
    if wanted.colour_management.is_some() && chosen.colour_management != wanted.colour_management {
        out.push("the document's colours unmanaged (application-managed colour)".to_owned());
    }
    out
}

/// The hints the driver replaced or left out of the validated ticket, as warnings.
pub(crate) fn replaced_hints(wanted: &Wanted, chosen: &Chosen) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(intent) = wanted.intent
        && chosen.intent != Some(intent)
    {
        let instead = chosen.intent.map_or("its default", intent_name);
        out.push(format!("the driver replaced the {} rendering intent with {instead}", intent_name(intent)));
    }
    if wanted.destination_profile.is_some() && chosen.destination_profile != wanted.destination_profile {
        out.push("the driver did not keep the application's destination profile (PageDestinationColorProfile)".to_owned());
    }
    out
}

/// The Print Schema rendering intent for `intent` (`PageICMRenderingIntent`, the DEVMODE's `dmICMIntent`), and a note
/// on what it loses; `None` for [`RenderingIntent::Auto`] (the driver's choice) and an intent this backend does not
/// know.
pub(crate) fn icm_intent(intent: RenderingIntent) -> Option<(PredefinedPageIcmRenderingIntent, Option<&'static str>)> {
    match intent {
        RenderingIntent::Perceptual => Some((PredefinedPageIcmRenderingIntent::Photographs, None)),
        RenderingIntent::Relative => Some((PredefinedPageIcmRenderingIntent::RelativeColorimetric, None)),
        RenderingIntent::RelativeBpc => Some((
            PredefinedPageIcmRenderingIntent::RelativeColorimetric,
            Some("the Print Schema has no black point compensation: the rendering intent is relative colorimetric, and compensation is the driver's"),
        )),
        RenderingIntent::Saturation => Some((PredefinedPageIcmRenderingIntent::BusinessGraphics, None)),
        RenderingIntent::Absolute => Some((PredefinedPageIcmRenderingIntent::AbsoluteColorimetric, None)),
        // `Auto`, and any intent a newer printscan adds.
        _ => None,
    }
}

/// A rendering intent's name for people.
pub(crate) fn intent_name(intent: PredefinedPageIcmRenderingIntent) -> &'static str {
    match intent {
        PredefinedPageIcmRenderingIntent::AbsoluteColorimetric => "absolute colorimetric",
        PredefinedPageIcmRenderingIntent::RelativeColorimetric => "relative colorimetric",
        PredefinedPageIcmRenderingIntent::Photographs => "perceptual",
        PredefinedPageIcmRenderingIntent::BusinessGraphics => "saturation",
    }
}

/// What a job with application-managed colour sets in its ticket, from what the driver offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ApplicationColour {
    /// Whether to set `PageColorManagement` = `None`.
    pub unmanaged: bool,
    /// Whether to set `PageDestinationColorProfile` = `Application`.
    pub application_profile: bool,
}

/// What to set for application-managed colour, given the options the driver lists for `PageColorManagement` and
/// `PageDestinationColorProfile` (`None` for one that is not a Print Schema keyword); warnings go to `warnings`.
/// `Err` with the reason when the job is to be refused: the driver colour-manages and cannot be told not to.
///
/// A driver with no `PageColorManagement` at all has no setting to tell (the in-box class drivers, which convert
/// nothing): the job prints with a warning, as grey prints on a printer with no colour setting. One that lists the
/// feature without `None` would convert the colours a second time, which is not what was asked.
pub(crate) fn application_colour(
    management: &[Option<PredefinedPageColorManagement>],
    profiles: &[Option<PredefinedPageDestinationColorProfile>],
    warnings: &mut Vec<String>,
) -> Result<ApplicationColour, String> {
    let unmanaged = management.contains(&Some(PredefinedPageColorManagement::None));
    if !management.is_empty() && !unmanaged {
        return Err(
            "its driver manages colour and has no setting to turn that off, so application-managed colour would be converted twice".to_owned()
        );
    }
    if management.is_empty() {
        warnings.push(
            "the driver has no colour-management setting (PageColorManagement); whether it converts the document's colours is up to it".to_owned(),
        );
    }
    let application_profile = profiles.contains(&Some(PredefinedPageDestinationColorProfile::Application));
    if !profiles.is_empty() && !application_profile {
        warnings.push("the driver does not take the destination profile as the application's (PageDestinationColorProfile)".to_owned());
    }
    Ok(ApplicationColour { unmanaged, application_profile })
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

    /// A validated ticket with the colour features as a driver writes them: the destination profile's option with
    /// its scored property, a vendor feature beside them, and copies as a parameter.
    const COLOUR_TICKET: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<psf:PrintTicket xmlns:psf="http://schemas.microsoft.com/windows/2003/08/printing/printschemaframework"
    xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema"
    xmlns:psk="http://schemas.microsoft.com/windows/2003/08/printing/printschemakeywords"
    xmlns:ns0001="http://schemas.example.com/vendor/printing" version="1">
  <psf:Feature name="psk:PageColorManagement"><psf:Option name="psk:None"/></psf:Feature>
  <psf:Feature name="psk:PageDestinationColorProfile"><psf:Option name="psk:Application"><psf:ScoredProperty name="psk:DestinationColorProfileURI"><psf:ParameterRef name="psk:PageDestinationColorProfileURI"/></psf:ScoredProperty></psf:Option></psf:Feature>
  <psf:Feature name="psk:PageICMRenderingIntent"><psf:Option name="psk:Photographs"/></psf:Feature>
  <psf:Feature name="ns0001:VendorColour"><psf:Option name="ns0001:Vivid"/></psf:Feature>
  <psf:ParameterInit name="psk:JobCopiesAllDocuments"><psf:Value xsi:type="xsd:integer">1</psf:Value></psf:ParameterInit>
</psf:PrintTicket>"#;

    /// A driver replacing what was asked: its own colour management, its own profile and another intent.
    const MANAGED_TICKET: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<psf:PrintTicket xmlns:psf="http://schemas.microsoft.com/windows/2003/08/printing/printschemaframework"
    xmlns:ns0000="http://schemas.microsoft.com/windows/2003/08/printing/printschemakeywords" version="1">
  <psf:Feature name="ns0000:PageColorManagement"><psf:Option name="ns0000:Driver"/></psf:Feature>
  <psf:Feature name="ns0000:PageDestinationColorProfile"><psf:Option name="ns0000:DriverConfiguration"/></psf:Feature>
  <psf:Feature name="ns0000:PageICMRenderingIntent"><psf:Option name="ns0000:BusinessGraphics"/></psf:Feature>
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
                ..Chosen::default()
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
            ..Wanted::default()
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
        // A feature missing from the ticket was not kept, unless the printer does that anyway.
        let missing = |wanted: Wanted| dropped(&Wanted { copies: 1, ..wanted }, &Chosen::default());
        assert_eq!(missing(Wanted { duplex: Some(PredefinedDuplexType::TwoSidedLongEdge), ..Wanted::default() }), ["two-sided on the long edge"]);
        assert_eq!(missing(Wanted { colour: Some(PredefinedPageOutputColor::Color), ..Wanted::default() }), ["colour"]);
        assert!(missing(Wanted { duplex: Some(PredefinedDuplexType::OneSided), ..Wanted::default() }).is_empty());
        assert!(missing(Wanted { colour: Some(PredefinedPageOutputColor::Grayscale), ..Wanted::default() }).is_empty());
    }

    #[test]
    fn the_colour_features_are_read_from_the_validated_ticket() {
        let chosen = Chosen::read(&PrintTicket::from_xml(COLOUR_TICKET));
        assert_eq!(chosen.colour_management, Some(PredefinedPageColorManagement::None));
        assert_eq!(chosen.destination_profile, Some(PredefinedPageDestinationColorProfile::Application));
        assert_eq!(chosen.intent, Some(PredefinedPageIcmRenderingIntent::Photographs));
        assert_eq!(chosen.copies, Some(1));
        // A ticket without them chose none.
        let plain = Chosen::read(&PrintTicket::from_xml(TICKET));
        assert_eq!((plain.colour_management, plain.destination_profile, plain.intent), (None, None, None));
    }

    #[test]
    fn application_managed_colour_the_driver_did_not_keep_refuses_the_job() {
        let wanted = Wanted {
            copies: 1,
            colour_management: Some(PredefinedPageColorManagement::None),
            destination_profile: Some(PredefinedPageDestinationColorProfile::Application),
            intent: Some(PredefinedPageIcmRenderingIntent::Photographs),
            ..Wanted::default()
        };
        let kept = Chosen::read(&PrintTicket::from_xml(COLOUR_TICKET));
        assert!(dropped(&wanted, &kept).is_empty());
        assert!(replaced_hints(&wanted, &kept).is_empty());

        let replaced = Chosen::read(&PrintTicket::from_xml(MANAGED_TICKET));
        assert_eq!(dropped(&wanted, &replaced), ["the document's colours unmanaged (application-managed colour)"]);
        assert_eq!(
            replaced_hints(&wanted, &replaced),
            [
                "the driver replaced the perceptual rendering intent with saturation",
                "the driver did not keep the application's destination profile (PageDestinationColorProfile)",
            ]
        );
        // Asked of a driver that offered it, then missing from its ticket: not kept.
        assert_eq!(dropped(&wanted, &Chosen::default()).len(), 1);
        assert_eq!(replaced_hints(&wanted, &Chosen::default())[0], "the driver replaced the perceptual rendering intent with its default");
        // Not asked (colour left to the driver): whatever the driver chose passes.
        let unasked = Wanted { copies: 1, ..Wanted::default() };
        assert!(dropped(&unasked, &replaced).is_empty() && replaced_hints(&unasked, &replaced).is_empty());
    }

    #[test]
    fn rendering_intents_map_to_the_print_schema() {
        let intent = |i| icm_intent(i).map(|(intent, _)| intent);
        assert_eq!(intent(RenderingIntent::Perceptual), Some(PredefinedPageIcmRenderingIntent::Photographs));
        assert_eq!(intent(RenderingIntent::Relative), Some(PredefinedPageIcmRenderingIntent::RelativeColorimetric));
        assert_eq!(intent(RenderingIntent::RelativeBpc), Some(PredefinedPageIcmRenderingIntent::RelativeColorimetric));
        assert_eq!(intent(RenderingIntent::Saturation), Some(PredefinedPageIcmRenderingIntent::BusinessGraphics));
        assert_eq!(intent(RenderingIntent::Absolute), Some(PredefinedPageIcmRenderingIntent::AbsoluteColorimetric));
        assert_eq!(icm_intent(RenderingIntent::Auto), None);
        // Only black point compensation is lost on the way.
        let noted: Vec<_> = RenderingIntent::ALL.into_iter().filter(|&i| icm_intent(i).is_some_and(|(_, note)| note.is_some())).collect();
        assert_eq!(noted, [RenderingIntent::RelativeBpc]);
    }

    #[test]
    fn application_colour_follows_what_the_driver_offers() {
        use PredefinedPageColorManagement as Cm;
        use PredefinedPageDestinationColorProfile as Profile;
        let run = |management: &[Option<Cm>], profiles: &[Option<Profile>]| {
            let mut warnings = Vec::new();
            application_colour(management, profiles, &mut warnings).map(|set| (set, warnings.len()))
        };
        let both = ApplicationColour { unmanaged: true, application_profile: true };
        let alone = ApplicationColour { application_profile: false, ..both };
        // Everything offered: both set, nothing to warn about.
        assert_eq!(run(&[Some(Cm::System), Some(Cm::None), None], &[Some(Profile::DriverConfiguration), Some(Profile::Application)]), Ok((both, 0)));
        // No destination profile feature: the colour management alone, quietly.
        assert_eq!(run(&[Some(Cm::None)], &[]), Ok((alone, 0)));
        // A destination profile feature without Application: a warning.
        assert_eq!(run(&[Some(Cm::None)], &[Some(Profile::DriverConfiguration)]), Ok((alone, 1)));
        // No colour-management feature (the in-box class drivers): printed, with a warning.
        assert_eq!(run(&[], &[]), Ok((ApplicationColour { unmanaged: false, application_profile: false }, 1)));
        // A driver that manages colour and cannot be told not to: refused.
        assert!(run(&[Some(Cm::Driver), Some(Cm::System)], &[Some(Profile::Application)]).is_err());
        assert!(run(&[None], &[]).is_err(), "a vendor's own option is not None");
    }
}
