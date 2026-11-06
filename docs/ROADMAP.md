# Roadmap

Countersign covers everyday vault work: quick access, the whole vault,
editing, Send, the generator and locking. [CHANGELOG.md](../CHANGELOG.md)
lists what each release changed.

## Known limitations

- **Attachments and file Sends have not been tried on a Premium account.**
  They are built, but so far only tested against a free account, which the
  CLI refuses them for. Verification codes, including the code after the
  password, have been checked on a Premium account.
- **Archiving has not been tried on a Premium account.** Showing the archive
  and refusing to archive without Premium have been tested; archiving and
  unarchiving need Premium. If Premium ends, the CLI cannot list archived
  items, so they stay in the web vault until they are unarchived there.
- **The ARM (aarch64) build has not been tried on an ARM computer.** Each
  release builds and tests it on ARM and starts it offscreen on the demo
  vault, but nobody has used it on Omarchy on ARM yet.
- **SSH keys cannot be created**, and an existing key's key material cannot
  be changed, because the official CLI cannot generate or import keys.
  Countersign will add both if the CLI does.

## Later

Next: an AUR package.

Ideas, not scheduled: auto-type, an SSH agent, polkit or fingerprint quick
unlock, multiple accounts, import and export, and a native sign-in form.
