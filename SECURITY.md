# Security policy

## Reporting a vulnerability

Please do not open a public issue for a security problem. Report it privately with GitHub's **Report a vulnerability** button, on the Security tab of this repository (private security advisories).

Include:

- The KLM version (the release you downloaded, or the version in `Cargo.toml` if you built it; the app does not show it) and your Windows version
- Steps to reproduce, and what an attacker could do with it
- Relevant log output, with API keys and personal paths removed

## Supported versions

Only the latest release is supported. Fixes are made in the latest release; older releases are not patched.

## Credentials

API keys are stored in Windows Credential Manager, never in `settings.json`. If you find a key written to a file, a log or the clipboard, report it as a vulnerability.
