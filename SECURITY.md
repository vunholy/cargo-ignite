# Security Policy

## Supported Versions

Only the latest version of cargo-ignite on the `main` branch receives security fixes.

## Reporting a Vulnerability

**Please do not open a public GitHub issue for security vulnerabilities.**

Report vulnerabilities on GitHub at https://github.com/vunholy/cargo-feat/security/advisories. Include:

- A description of the vulnerability and its potential impact
- Steps to reproduce or a proof of concept
- Any suggested fix if you have one

You should receive a response within 72 hours. If you do not, follow up to make sure the report was received.

Once the issue is confirmed, a fix will be prepared and released. You will be credited in the release notes unless you prefer otherwise.

## Scope

Things that are in scope:

- Checksum verification bypass when downloading `.crate` files
- Arbitrary code execution via malformed index entries or tarballs
- Path traversal during tarball extraction
- Credential or token exposure via cache files or logs

Things that are out of scope:

- Vulnerabilities in Rust's standard library or cargo itself
- Issues that require the attacker to already control the user's cargo index cache
