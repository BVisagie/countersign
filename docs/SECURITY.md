# Security

Countersign gives access to a decrypted Bitwarden vault. Security fixes come
before features.

## Threat model

Trusted: the local user account and kernel, the Wayland compositor, the
official Bitwarden CLI and Countersign's installed files.

Out of scope: malware running as the same user, debuggers or memory readers
with access to Countersign or the CLI, a compromised compositor or CLI, and
someone reading item names off the screen.

Countersign never implements Bitwarden's encryption, key derivation or
server protocol. Those stay in the official CLI.

## How secrets move

- **Signing in with email.** `bw login` runs in a terminal inside the app,
  so your email, master password and two-step codes go to the official
  CLI's own prompts. Countersign's code does not read your typing, and
  clears its copy of anything you paste. The CLI prints its session key in
  the terminal; the app reads it, sends it to the daemon over the private
  socket, and wipes the terminal.
- **Master password.** Typed into GTK's password entry, which keeps its text
  out of swap. It is copied once to send it to the daemon over the private
  socket, then to the CLI in an HTTP body over the socket pair, and cleared
  from the entry. Never in arguments, the environment or files.
- **Session key.** Returned by the CLI on unlock and held only in the daemon,
  cleared on lock. It reaches the CLI again only through the environment of
  a child the daemon starts: a restarted `bw serve`, or a one-off
  `bw send list` or `bw send create`.
- **The CLI's server** listens on no port and no path. Only the daemon holds
  the other end of its socket pair.
- **The daemon's socket** lives in a folder only you can open, and the daemon
  checks each connection's user id.
- **Item data.** The app receives items with passwords, codes, card numbers,
  security codes, hidden fields, password history and passkey keys removed.
  A secret is sent only when you reveal it, and hidden again after a minute,
  or when you open the item in the editor.
- **Sends.** The app lists Sends without their text, their key or the access
  link, which carries the key. The text is sent when you edit a Send; links
  are copied by the daemon like any other secret. A Send's password goes to
  the CLI on stdin or in an HTTP body, never in arguments.
- **Copying** happens inside the daemon. The value never reaches the app.
  Copies carry the password-manager hint so clipboard history skips them,
  and are cleared after the timeout or on lock, but never if another app has
  replaced them since.

## The CLI download

Installing the CLI trusts Bitwarden's GitHub release and HTTPS, and the
SHA-256 recorded in Countersign's source with each pin; a mismatch is
refused before anything is extracted or run. Countersign never downloads or
updates anything in the background.

## Memory

The daemon disables core dumps and same-user debugger attachment before it
starts. Secrets are held in types that clear their memory when dropped, and
response buffers are cleared after parsing. This is best effort: the CLI is a
Node program whose memory Countersign cannot clear, and GTK copies text it
displays.

## Supported versions

There are no releases yet. Security fixes go to `main`.

## Reporting a vulnerability

[Report it privately](https://github.com/BVisagie/countersign/security/advisories/new)
through GitHub's private vulnerability reporting. Do not open a public issue
with an exploit, credential, session key or vault data.
