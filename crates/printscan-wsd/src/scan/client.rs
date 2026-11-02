//! The operation-level client: one method per WS-Scan operation this backend uses, over any
//! [`Transport`].

use std::time::Duration;

use super::model::{self, CreatedJob, JobStatus, ScannerElements};
use super::ns::WSCN;
use super::ticket::Ticket;
use crate::service::ServiceKind;
use crate::xml::{self, esc};
use crate::{CancelToken, DeviceUrl, Error, NetworkLimits, Result, SoapClient, Transport, mtom};

/// A section of `GetScannerElements`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Section {
    Description,
    Configuration,
    Status,
}

impl Section {
    fn qname(self) -> &'static str {
        match self {
            Self::Description => "wscn:ScannerDescription",
            Self::Configuration => "wscn:ScannerConfiguration",
            Self::Status => "wscn:ScannerStatus",
        }
    }
}

/// The limits of a `RetrieveImage`: a page is far larger than any other reply (a 600 dpi colour
/// letter page is about 100 MB uncompressed), and the scanner may still be moving its head when it
/// answers.
pub fn image_limits() -> NetworkLimits {
    NetworkLimits { timeout: Duration::from_secs(180), max_response: 256 << 20, ..NetworkLimits::default() }
}

/// A client bound to one scanner's `ScannerService` endpoint.
pub struct Client<T: Transport> {
    soap: SoapClient<T>,
    image_limits: NetworkLimits,
}

impl<T: Transport> Client<T> {
    pub fn new(transport: T, url: DeviceUrl) -> Self {
        Self { soap: SoapClient::new(transport, url).with_reply_label("the scanner's reply"), image_limits: image_limits() }
    }

    /// Limits for every operation but `RetrieveImage`.
    #[must_use]
    pub fn with_limits(mut self, limits: NetworkLimits) -> Self {
        self.soap = self.soap.with_limits(limits);
        self
    }

    /// Limits for `RetrieveImage`.
    #[must_use]
    pub fn with_image_limits(mut self, limits: NetworkLimits) -> Self {
        self.image_limits = limits;
        self
    }

    /// Stop sending once `token` is set (an exchange already on the wire runs to its timeout).
    #[must_use]
    pub fn with_cancel(mut self, token: CancelToken) -> Self {
        self.soap = self.soap.with_cancel(token);
        self
    }

    /// A message-id source for tests that replay canned replies.
    #[must_use]
    pub fn with_id_source(mut self, ids: impl Fn() -> String + Send + Sync + 'static) -> Self {
        self.soap = self.soap.with_id_source(ids);
        self
    }

    pub fn url(&self) -> &DeviceUrl {
        self.soap.endpoint()
    }

    /// GetScannerElements for the given sections.
    pub fn get_scanner_elements(&self, sections: &[Section]) -> Result<ScannerElements> {
        let names: String = sections.iter().map(|s| format!("<wscn:Name>{}</wscn:Name>", s.qname())).collect();
        let body =
            format!("<wscn:GetScannerElementsRequest><wscn:RequestedElements>{names}</wscn:RequestedElements></wscn:GetScannerElementsRequest>");
        let call = ServiceKind::Scan.descriptor().call("GetScannerElements", body);
        self.soap.call_node(&call, |n| Ok(model::parse_scanner_elements(n)))
    }

    /// CreateScanJob with `ticket`.
    pub fn create_scan_job(&self, ticket: &Ticket) -> Result<CreatedJob> {
        let body = format!("<wscn:CreateScanJobRequest>{}</wscn:CreateScanJobRequest>", ticket.xml());
        let call = ServiceKind::Scan.descriptor().call("CreateScanJob", body);
        self.soap.call_node(&call, model::parse_created_job)
    }

    /// RetrieveImage: the next image of `job`, as the scanner sent it (an MTOM part or inline
    /// base64). The end of the job is the `ClientErrorNoImagesAvailable` fault.
    pub fn retrieve_image(&self, job: &CreatedJob, document_name: &str) -> Result<Vec<u8>> {
        let body = format!(
            "<wscn:RetrieveImageRequest><wscn:JobId>{}</wscn:JobId><wscn:JobToken>{}</wscn:JobToken>\
             <wscn:DocumentDescription><wscn:DocumentName>{}</wscn:DocumentName></wscn:DocumentDescription></wscn:RetrieveImageRequest>",
            job.id,
            esc(&job.token),
            esc(document_name)
        );
        let call = ServiceKind::Scan.descriptor().call("RetrieveImage", body);
        let soap = self.soap.at(self.soap.endpoint().clone()).with_limits(self.image_limits);
        soap.call_with_parts_node(&call, |response, parts| {
            let data = xml::child(response, WSCN, "ScanData").ok_or_else(|| Error::Protocol("RetrieveImageResponse without ScanData".into()))?;
            if let Some(include) = data.children().find(|c| c.is_element() && c.tag_name().name() == "Include") {
                let href = include.attribute("href").unwrap_or_default();
                return mtom::resolve_href(parts, href)
                    .map(<[u8]>::to_vec)
                    .ok_or_else(|| Error::Protocol(format!("RetrieveImageResponse names a part it does not carry ({href})")));
            }
            let text: String = data.children().filter(|c| c.is_text()).filter_map(|c| c.text()).collect();
            decode_base64(&text).ok_or_else(|| Error::Protocol("RetrieveImageResponse carries no image".into()))
        })
    }

    /// GetJobElements for the job's `JobStatus`. `Ok(None)` when the reply has no valid one; a
    /// scanner that no longer knows the job answers with a fault (`ClientErrorJobIdNotFound`, or
    /// `ClientErrorJobCancelled` for one cancelled at the device).
    pub fn get_job_status(&self, job: &CreatedJob) -> Result<Option<JobStatus>> {
        let body = format!(
            "<wscn:GetJobElementsRequest><wscn:JobId>{}</wscn:JobId><wscn:RequestedElements><wscn:Name>wscn:JobStatus</wscn:Name></wscn:RequestedElements></wscn:GetJobElementsRequest>",
            job.id
        );
        let call = ServiceKind::Scan.descriptor().call("GetJobElements", body);
        self.soap.call_node(&call, |n| Ok(model::parse_job_status(n)))
    }

    /// CancelJob.
    pub fn cancel_job(&self, job_id: u32) -> Result<()> {
        let body = format!("<wscn:CancelJobRequest><wscn:JobId>{job_id}</wscn:JobId></wscn:CancelJobRequest>");
        self.soap.call(&ServiceKind::Scan.descriptor().call("CancelJob", body), |_| Ok(()))
    }
}

/// Standard base64, whitespace ignored, padding optional. `None` for anything else or for nothing.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let digits: Vec<u8> = text.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    let digits = digits.strip_suffix(b"==").or_else(|| digits.strip_suffix(b"=")).unwrap_or(&digits);
    if digits.is_empty() || digits.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(digits.len() * 3 / 4);
    for chunk in digits.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= u32::from(value(c)?) << (18 - 6 * i);
        }
        let bytes = n.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_through_the_encoder() {
        for data in [&b"f"[..], b"fo", b"foo", b"foob", b"\x00\xff\x10\x80"] {
            let encoded = mtom::base64(data);
            assert_eq!(decode_base64(&encoded).as_deref(), Some(data), "{encoded}");
            assert_eq!(decode_base64(&format!(" {}\n", encoded.trim_end_matches('='))).as_deref(), Some(data));
        }
        assert_eq!(decode_base64(""), None);
        assert_eq!(decode_base64("a"), None);
        assert_eq!(decode_base64("ab*d"), None);
    }
}
