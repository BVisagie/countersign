# Using Countersign

## First run

1. **The Bitwarden CLI.** If Countersign cannot find a suitable CLI, it
   offers to install the official one; see
   [INSTALL.md](INSTALL.md#the-bitwarden-cli).
2. **Sign in.** Choose your server: **US cloud**, **EU cloud** or
   **Self-hosted** (enter its address). Then either:
   - **Sign in with email.** The official CLI asks for your email, master
     password and two-step code, in a terminal inside Countersign. When it
     succeeds, the vault opens without typing the password again. You can
     paste your password there with **Ctrl+V** or the paste button; a
     paste never presses Enter for you. The CLI's two-step login works
     with an authenticator app, email codes or a YubiKey OTP. It can't use
     a security key, passkey or Duo: if those are your account's only
     two-step methods, use an API key.
   - **Use an API key.** Enter the personal API key from the Bitwarden web
     vault (**Settings → Security → Keys**). You still unlock with your
     master password.
3. **Unlock** with your master password.

Opening the vault can take a few seconds, longest the first time. Until it
opens, or Countersign asks you to try again, the unlock form, quick access
and the vault's shortcuts take no input. Quitting still works, and locks the
vault.

Your session lives in Countersign's background service, `countersignd`,
which the app starts when needed. The app receives secrets only to show or
edit them: an item's notes and live verification code when you open it, a
field you reveal, a password you generate, and the whole item or Send when
you edit it.

## Quick access

Press your binding (`Super+Shift+P` if you used the suggested one) and type a
few letters of a name, username or website. Before you type, the items you
copied most recently come first. When the vault is locked, quick access asks
for your master password.

| Key | Action |
| --- | --- |
| `Enter` | Copy the password, card number, note or SSH public key |
| `Shift+Enter` | Copy the username or card security code |
| `Ctrl+C` / `Ctrl+B` / `Ctrl+T` | Copy the password / username / verification code (on a card: number / cardholder name / security code) |
| `Ctrl+U` | Open the website |
| `Ctrl+O` or `Ctrl+Enter` | Show the item in the main window |
| `↑` `↓`, `Ctrl+N` / `Ctrl+P`, `Ctrl+J` / `Ctrl+K`, `Page Up` / `Page Down` | Move through the results |
| `Ctrl+L` | Lock |
| `Esc` | Clear the search, then close |

`Enter` on an item with nothing to copy, such as an identity, shows it in the
main window instead.

## The main window

The sidebar filters the vault: all items, favorites, each item type, your
folders, organization collections and the trash. **Tools** holds the
password generator and Send.

Select an item to see it. Passwords, card numbers, security codes and
hidden fields stay masked until you press the eye button, and hide again
after a minute. Notes show in full, except on items that ask for the master
password. Verification codes show and count down live (they need Bitwarden
Premium).
Each field has a copy button, websites open in your browser, and the item
shows its password history and attachments.

| Key | Action |
| --- | --- |
| `Ctrl+F` or `/` | Search; `↑` `↓` move through the results |
| `Enter` | Copy the selected item's password, card number or note |
| `Ctrl+C` / `Ctrl+B` / `Ctrl+T` | Copy the password / username / verification code |
| `Ctrl+U` | Open the website |
| `Ctrl+N` | New login |
| `Ctrl+G` | Password generator |
| `Ctrl+R` | Sync |
| `Ctrl+L` | Lock |
| `Ctrl+,` | Settings |
| `Ctrl+Q` | Quit |

## Copying

- Copying from your vault happens in the background service; the value
  never passes through the app. Generated passwords are the exception: the
  app already holds them and hands them to the service to copy.
- A copy clears itself after 30 seconds, unless something else has replaced
  it on the clipboard by then.
- Copies are marked so clipboard history, including Omarchy's, skips them.
- The bar at the bottom of the window shows what you copied and counts
  down.
- **The code after the password.** When you copy the password of a login
  that has a verification code, the code replaces it on the clipboard five
  seconds later, ready for the next prompt. **Keep password**, in the bar or
  the notification, stops that. Verification codes need Bitwarden Premium.
- Copies made while the main window is not focused, for example from quick
  access, show a desktop notification. It goes away when the copy clears.
- Locking clears Countersign's copy straight away.

All of this covers the copies Countersign makes: its copy buttons, `Enter`
and the copy shortcuts. Text you select and copy yourself, for example in
the editor, is an ordinary copy: it is not marked for clipboard history and
is not cleared.

## Editing

- **New items.** The **+** button above the list offers a new login, card,
  identity or note; `Ctrl+N` starts a login. A new item goes into the
  folder you are viewing.
- **Edit an item** with the pencil button above its details. The editor
  covers the name, folder, favorite, **Ask for the master password**, the
  item's own fields, websites, notes and custom fields (text, hidden or
  checkbox). The password field has a button that generates one. Save or
  cancel at the top.
- **SSH keys** show their public key and fingerprint; the key itself cannot
  be changed. Everything else can.
- While you edit, syncs leave your draft alone. Locking throws it away.
- **More actions** are in the **⋮** menu: add or remove a favorite, **Add
  attachment…**, **Move to organization…** and **Move to trash**. Moving to
  an organization needs at least one collection and cannot be undone from
  Countersign. Attachments need Bitwarden Premium.
- **In the trash**, the menu offers **Restore** and **Delete forever…**.
- **Delete an attachment** with the bin button on its row.
- **Folders.** The **+** button offers **New folder…**. Right-click a folder
  in the sidebar to rename or delete it; deleting a folder keeps its items,
  which move to **No folder**.

Items set to ask for the master password ask for it before they can be
edited.

## Send

**Tools → Send** shares text or a file through Bitwarden Send, end-to-end
encrypted. The list takes a few seconds to load the first time.

- **+** creates a **Text Send** or a **File Send**. File Sends can be up to
  500 MB and need Bitwarden Premium.
- Choose when it deletes itself (one hour to 30 days, seven days by default),
  a maximum number of views, a password, whether to hide your email, and
  private notes. Text can be hidden until the recipient chooses to show it.
- Creating a Send copies its link.
- Each Send has a link button that copies its link, and a **⋮** menu with
  **Edit…**, **Remove password** and **Delete…**.
- **Editing a Send with a password.** Type the password again to keep it, or
  a new one to change it; the Bitwarden CLI cannot keep a password it is not
  given. Turn off **Require a password** to remove it.

## Password generator

**Tools → Generator**, or `Ctrl+G`. Make a password (5 to 128 characters,
with the character sets and minimums you choose) or a passphrase (3 to 20
words). It starts with a 20-character password. Digits and symbols are
colored so similar characters are easier to tell apart. **Copy** clears
like any other copy.

## Locking

Lock with `Ctrl+L`, the tray's **Lock**, or `countersign --lock`. Quitting
locks the vault too, whether with `Ctrl+Q`, the tray's **Quit**, or closing
the window with **Keep running when closed** off.
Countersign also locks:

- after 15 minutes without using it,
- when your screen locks, through Hyprland or logind,
- before the computer sleeps.

Locking clears Countersign's copy, closes any editor and forgets the shown
item and generated password.

Closing the window keeps Countersign running, so quick access opens
instantly, and leaves the vault as it is.

## Syncing

Countersign syncs when you unlock, every 10 minutes after that, and when you
press `Ctrl+R`. The sidebar shows when it last synced.

## Settings

Open them with `Ctrl+,` or the menu. Changes apply as you make them.

| Setting | Default | Notes |
| --- | --- | --- |
| Clear after | 30 seconds | 5 to 600 |
| Copy the verification code next | On | The code after the password |
| Code follows after | 5 seconds | Always shorter than **Clear after** |
| Lock after inactivity | 15 minutes | 0 never locks for inactivity |
| Lock with the screen | On | |
| Lock before sleep | On | |
| Sync every | 10 minutes | 0 syncs only when unlocking |
| Keep running when closed | On | Off quits, and locks, when you close the window |

**Desktop** has the Hyprland lines with copy buttons. **Account** signs you
out. **Bitwarden CLI** shows the CLI in use and lets you choose another; see
[INSTALL.md](INSTALL.md#use-your-own-cli).

## The tray

The tray icon shows whether the vault is open. Click it for the main window,
or middle-click for quick access. Its menu has **Open Countersign**, **Quick
access**, **Lock** and **Quit**.

## Signing out

**Settings → Account → Sign out…**, or **Sign out…** in the main menu.
Countersign forgets the account on this computer; your vault stays on
Bitwarden's servers.
