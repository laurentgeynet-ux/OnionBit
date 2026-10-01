# Security Policy

OnionBit is anonymity software — security bugs can deanonymize users. Please report
vulnerabilities responsibly.

## Reporting a vulnerability

**Do not open a public issue.** Use GitHub's
[private vulnerability reporting](https://github.com/laurentgeynet-ux/OnionBit/security/advisories/new)
(“Report a vulnerability” on the Security tab), or email
**laurent.geynet@gmail.com**.

Please include:
- Affected component and version/commit
- Reproduction steps or a proof of concept
- Whether the bug can break anonymity (IP leak, correlation, downgrade)

We aim to acknowledge reports within 72 hours.

## What counts as a security issue

- **Anonymity leaks**: any path where anonymous traffic escapes the tunnel
  (trackers, DHT, UDP, DNS, uTP), kill-switch bypass, circuit correlation
- **SSRF/API abuse**: control-plane endpoints reaching non-authorized targets
- **Cryptography flaws**: circuit crypto, IPv8 signatures, key handling
- **Remote code execution / memory safety** in network-facing parsers
  (bencode, peer-wire, IPv8 packets)

## Scope notes

- OnionBit is in **alpha** — do not rely on it for high-stakes anonymity yet.
- Anonymous mode protects against IP-level identification by swarm peers and
  observers of your local link; it is **not** equivalent to Tor against a global
  passive adversary. See the threat-model notes in the docs.
