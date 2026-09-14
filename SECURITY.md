# Security policy

NoriShell is in pre-release development. Security fixes target the current
development line; no older release has a separate security-support commitment.
Build, unit-test and UI results do not constitute a complete security audit.

## Report privately

On the project's GitHub repository, open **Security → Advisories → Report a
vulnerability**. Submit a private report containing the affected version or
commit, platform, reproduction steps using synthetic data, and expected impact.
Do not include real passwords, private keys, Vault contents, authentication
headers, or customer data.

If the private-report button is unavailable, open an issue containing only a
request for a private reporting channel. Do not include vulnerability details,
proof-of-concept code or sensitive attachments in that public issue. A
maintainer must establish the private channel before receiving those details.

## Maintainer handling

Before public launch, enable [GitHub private vulnerability reporting](https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/configure-vulnerability-reporting/configure-for-a-repository)
and verify the report entry point. This file alone does not enable it.
Review reports privately, reproduce them in an isolated environment, coordinate
the fix and disclosure with the reporter, and publish the affected and fixed
versions when a remediation is available. Do not promise a response deadline
that the maintenance team cannot support.

Reports involving server identity verification, Vault secrets, plugin isolation,
permission boundaries, terminal input ownership or incomplete resource cleanup
are in scope. Dependency findings should identify the resolved lockfile version
and the reachable behavior rather than including unfiltered scanner output.
