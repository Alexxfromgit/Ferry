# Security policy

Report vulnerabilities through GitHub's private reporting form. Include the Ferry version or
commit, the operating systems involved, steps to reproduce, and the impact. Use sample files and
test identities when you can. [Report a vulnerability](https://github.com/Alexxfromgit/Ferry/security/advisories/new)

Reports about link and ticket parsing, files written outside the chosen destination, access to
stopped or password-locked shares (the `ferry/1` protocol in `crates/ferry-core/src/protocol.rs`),
or unintended exposure of files, share tokens, passwords and device keys are especially useful.

Security fixes target the latest release. For general bugs, use the
[issue tracker](https://github.com/Alexxfromgit/Ferry/issues). Ferry's
[networking guide](docs/NETWORKING.md) explains what tickets and discovery reveal.
