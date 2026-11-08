//! Bounded XML reading: printscan-wsd's size, encoding and depth checks, then a parse with a node
//! cap and no DTD. Elements are matched by namespace and local name; prefixes never matter.

use roxmltree::{Document, Node, ParsingOptions};

use crate::error::{Error, Result};

/// Parse a reply, refusing anything over printscan-wsd's caps (4 MiB, 40 levels, 100,000 nodes) and any DTD.
pub(crate) fn parse(bytes: &[u8]) -> Result<Document<'_>> {
    let text = printscan_wsd::xml::checked_text(bytes).map_err(|e| match e {
        printscan_wsd::Error::TooLarge(what, n) => Error::TooLarge(what, n),
        other => Error::Xml(other.to_string()),
    })?;
    let options = ParsingOptions { allow_dtd: false, nodes_limit: printscan_wsd::xml::MAX_NODES, ..ParsingOptions::default() };
    Document::parse_with_options(text, options).map_err(|e| Error::Xml(e.to_string()))
}

/// Is `node` the element `{ns}local`?
pub(crate) fn is(node: Node<'_, '_>, ns: &str, local: &str) -> bool {
    node.is_element() && node.tag_name().name() == local && node.tag_name().namespace() == Some(ns)
}

/// The first child element `{ns}local`.
pub(crate) fn child<'a, 'i>(node: Node<'a, 'i>, ns: &str, local: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|c| is(*c, ns, local))
}

/// The child elements `{ns}local`, at most `max` of them.
pub(crate) fn children<'a, 'i>(node: Node<'a, 'i>, ns: &'static str, local: &'static str, max: usize) -> impl Iterator<Item = Node<'a, 'i>> {
    node.children().filter(move |c| is(*c, ns, local)).take(max)
}

/// The direct text of an element, trimmed and cut at printscan-wsd's text cap.
pub(crate) fn text(node: Node<'_, '_>) -> String {
    let mut out = String::new();
    for c in node.children().filter(|c| c.is_text()) {
        out.push_str(c.text().unwrap_or(""));
        if out.len() > printscan_wsd::xml::MAX_TEXT * 4 {
            break;
        }
    }
    out.trim().chars().take(printscan_wsd::xml::MAX_TEXT).collect()
}

/// The trimmed text of the first child `{ns}local`, if it has any.
pub(crate) fn child_text(node: Node<'_, '_>, ns: &str, local: &str) -> Option<String> {
    child(node, ns, local).map(text).filter(|t| !t.is_empty())
}

/// The first child `{ns}local` as a whole number.
pub(crate) fn child_u32(node: Node<'_, '_>, ns: &str, local: &str) -> Option<u32> {
    child_text(node, ns, local)?.parse().ok()
}

/// The first child `{ns}local` as a signed number.
pub(crate) fn child_i32(node: Node<'_, '_>, ns: &str, local: &str) -> Option<i32> {
    child_text(node, ns, local)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_documents_are_errors() {
        let bomb = "<?xml version=\"1.0\"?><!DOCTYPE l [<!ENTITY a \"aaaa\">]><l>&a;</l>";
        assert!(matches!(parse(bomb.as_bytes()), Err(Error::Xml(_))));
        assert!(matches!(parse(&[0xff, 0xfe]), Err(Error::Xml(_))));
        assert!(parse("<a>".repeat(100).as_bytes()).is_err());
        assert!(matches!(parse(&vec![b' '; 5 << 20]), Err(Error::TooLarge(..))));
    }
}
