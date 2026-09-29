# Security

## Reporting

Please report a vulnerability privately. Open a GitHub security advisory on [EntasisLabs/mori](https://github.com/EntasisLabs/mori/security/advisories/new), or email the maintainers if that form is unavailable. Do not file a public issue for an unfixed security bug.

Include what you ran, what you expected, and what happened. A small reproduction helps.

## What to keep private

`.mori/kv/` is the memory. `.mori/config.json` can contain a remote username and password if you passed `--password` to `mori remote add`. Do not commit either, and do not paste them into an issue.

mori does not run a background service. A local folder is only open while a command is running. A remote command opens the SurrealDB address you configured, then disconnects.
