# Security Policy

Kupferdraht follows the security policy of the Schwarz Digits open-source
program: <https://github.com/SchwarzDigits/.github/blob/main/SECURITY.md>.
Report vulnerabilities privately to <opensource@digits.schwarz>, not in a
public issue, discussion or pull request. The policy says what to include,
how we respond, and how the coordinated disclosure within 90 days works.

## What this project adds

- **Scope.** The supported line is the latest release tag (`v0.1.0`,
  2026-10-31) and `main`; fixes land on `main` and ship with the next tag.
  There is no crates.io release (`publish = false`). Name the component
  you observed the problem in: `kupferdraht-core`, the CLI, a Kotlin or
  Swift binding, or the browser surface.
- **What counts.** [`docs/THREAT_MODEL.md`](docs/THREAT_MODEL.md) states
  what Kupferdraht guarantees, under which assumptions, and which limits
  it accepts by design. A report that breaks a stated guarantee is a
  vulnerability; one that confirms an accepted limit is welcome as an
  ordinary issue.
- **Not here.** Findings in Wire's backend or in Wire's own clients
  belong to Wire, not to this project.
- **Credit.** Reports are credited in the changelog unless you prefer
  otherwise.
