# Security policy

## Supported versions

Wallflex is before 1.0, so only the latest release gets security fixes.
Please check the problem still happens there before reporting it.

| Version        | Supported |
| -------------- | --------- |
| Latest release | Yes       |
| Older releases | No        |

## Reporting a vulnerability

Please don't open a public issue for a security problem. Report it
privately through GitHub instead:
[Security > Report a vulnerability](https://github.com/BonelessPig/Wallflex/security/advisories/new).

Include what you can of:

- what an attacker could do, and what they'd need first (a local
  account, a crafted config file, control of a Discord message, and so
  on);
- the steps or a proof of concept that shows it;
- the Wallflex version and Windows version you saw it on.

You should hear back within a week. Once the problem is confirmed, a
fix goes into the next release, and you'll be credited in the advisory
and the changelog unless you'd rather not be.

## Scope

Things that matter most here:

- Wallflex runs as you, without admin rights, and reads its settings
  from `%APPDATA%\Wallflex\config.toml`.
- With Discord set up, it holds an OAuth token
  (`discord-token.json`) and optionally a bot token, talks to Discord's
  local pipe, and downloads avatars and album art over HTTPS.
- It decodes pictures from the network and from Windows' media
  sessions.

Leaks of those tokens, code running from downloaded or decoded data,
and anything that lets another user or a website act through Wallflex
are all in scope. Problems that need an attacker who can already edit
your files or run code as you are generally not.
