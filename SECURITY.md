# Security Policy

## Reporting Security Issues

We take the security and integrity of the Distributed Rust MMO Platform seriously. If you believe you have discovered a vulnerability, please follow the guidelines below.

### Responsible Disclosure

Please **do not** report security vulnerabilities through public GitHub issues or public discussions.

Instead, please report security issues privately by:
- Using [GitHub Private Vulnerability Reporting](https://github.com/Jakousa/rust-server/security/advisories/new) on the repository, OR
- Contacting the maintainer directly via email: `jami.kousa@gmail.com`

Please include:
1. A description of the vulnerability.
2. Steps or proof-of-concept to reproduce the behavior.
3. The affected crates, components, or network protocols (e.g., gateway edge auth, cluster RPC, kinematic anti-cheat, session token handling).
4. Any potential mitigations you have identified.

### Scope

Security reports concerning the following areas are especially appreciated:
- Network ingress and edge authentication bypasses (QUIC / WebTransport / JWT).
- Distributed state desynchronization, transaction race conditions, or inventory duplication bugs.
- Kinematic anti-cheat evasion or remote crash vulnerabilities in simulation nodes.
- Memory corruption or panic DOS vulnerabilities in binary codecs (`mmo-schema`).

Thank you for helping keep the platform safe and reliable for everyone!
