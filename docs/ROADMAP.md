# Roadmap

Countersign covers everyday vault work: quick access, the whole vault,
editing, Send, the generator and locking. There is no release yet; you build
it from source ([INSTALL.md](INSTALL.md)).

## Known limitations

- **Premium features have not been tried on a Premium account.**
  Verification codes, the code after the password, attachments and file
  Sends are built, but so far only tested against a free account, which the
  CLI refuses them for.
- **SSH keys cannot be created**, and an existing key's key material cannot
  be changed, because the official CLI cannot generate or import keys.
  Countersign will add both if the CLI does.
- **An edit made right after unlocking** was once seen to show its old value
  until the next sync, although the server had the new one.

## Later

Ideas, not scheduled: auto-type, an SSH agent, polkit or fingerprint quick
unlock, multiple accounts, import and export, a native sign-in form, and
packages.
