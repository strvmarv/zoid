# Security Policy

## Reporting a vulnerability

Please **do not** open a public GitHub issue for security vulnerabilities.

Instead, use GitHub's private vulnerability reporting: go to this
repository's **Security** tab and select **"Report a vulnerability"**. This
opens a private disclosure channel visible only to the maintainer until a
fix is ready.

If you're unable to use that flow, open a regular GitHub issue asking for an
alternative contact — without any vulnerability details in the issue itself.

## Scope

zoid executes tools on your behalf and communicates with LLM providers you
configure. Reports of particular interest include:

- Ways a crafted repository, file, or tool response could cause zoid to
  execute unintended commands.
- Credential or API-key handling issues (e.g. keys leaking into logs, config
  files, or provider requests they shouldn't reach).
- Anything that lets a plugin or MCP server escape its intended sandboxing.

## Response

This is a small, actively maintained project. Expect an initial response
within a few days; timelines for a fix depend on severity and complexity.
