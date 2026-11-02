# Security

printscan parses replies from devices on the local network, so a device that sends hostile data
is in scope: a panic, unbounded memory or time, a connection to an address the policy refuses, or
a print job sent without consent is a vulnerability.

## Reporting

Report it privately through GitHub's
[private vulnerability reporting](https://github.com/b1tninja/printscan/security/advisories/new),
not in a public issue. Include the crate and version, and a reply or file that shows the problem if
you have one.

## Supported versions

Fixes go into the latest published version of each crate.
