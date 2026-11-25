# Settings > Apps, Developer options, and the per-app controls

Built 2026-10-15/16. Code: `packages/settings/src/appmgr.rs` (what is known about each app, and changes to it),
`apps_ui.rs` (the pages), the power-limit helper `packages/power-helpers/src/powerlimit.rs` (installed as `/usr/lib/yggdrasil/powerlimit`; the Python one it replaced and
its tests were kept in `helpers/python/` and `tests/` until 2026-11-15), and, for the per-app network rules and sandbox, the operating system's launcher declared in its
`apps` integration (INTEGRATIONS.md; on Labyrinth `labyrinth-launch` and `labyrinth-appnet`, tested in Labyrinth's repository).
`cargo test` in `packages/settings`.

## The layout (2026-11-13)

I reorganised Settings on the model of a tablet's settings (its structure, not its look; the colours stay Yggdrasil's):
the sidebar holds grouped rows with the open page as a filled pill in the accent colour, and each page is rounded cards of
rows, a row that opens a page ending in ›, and a small grey note under a card where it needs one. The sidebar's groups, in
order: Connections, Devices (Battery last, after Power), Personal (General at its top, then Accessibility, Appearance, the
assistant), Apps, Privacy & security, System. Settings opens on General all the same.

- **The way back** (`src/nav.rs`): one header above every page, outside the scrolling part, shows the path ("Appearance ›
  Wallpaper") and, on a sub-page, Back. Back, the mouse's back button and Alt+Left go up one level at a time; a click on the
  open page's sidebar row returns it to its first level. Each level keeps its own scroll position; a level just opened starts at
  its top. Before this a sub-page's own back button scrolled away with the page (the new level kept the old one's scroll
  offset), so the way back was often out of sight.
- **General** is the page Settings opens on: About, Software Update (below; the system's version beside it), Storage (each disk's use, read when the page opens), Date & Time, Keyboard (Peripherals > Keyboard) and
  Language & Region (`localectl`: the system's language for every account, and a separate format region when there is one).
  About and Date & Time moved here from the sidebar; the search still finds them, and `yggdrasil-settings about`,
  `datetime`, `storage` and `language` open them inside General. A page inside another is marked by `parent()` in main.rs.
  **Software Update** (`src/sysupdate.rs`, since 2026-11-17; laid out like Windows 11's Windows Update page, drawn in
  Yggdrasil's look) has a big update icon with a badge (a green check when up to date, an accent dot when updates wait, a
  shield when the assistant approved them), "You're up to date" or "6 updates available" with "Last checked: Today, 21:20"
  (12- or 24-hour as the panel's clock), and Check for updates. Checking needs no root and never touches pacman's own
  databases: it is Agora's checker, moved into `packages/updates` (yggdrasil-updates) so both use one, which refreshes a
  private copy of the sync databases in the session's runtime folder with `fakeroot pacman -Sy --dbpath`, as checkupdates
  does, and asks `pacman -Sup` for each package's download size and repository. The page checks when it opens if the last
  check is over 15 minutes old (and updates are not paused), and on the button; never in the background (there is no timer).
  The last answer is kept in `~/.cache/yggdrasil/system-updates`, so the page shows it at once. **More options**:
  - **Available Updates** (the count): every update with old -> new version, size ("downloaded" when pacman's cache has it)
    and repository, grouped Kernel, Drivers and firmware, System (the core repositories), Apps and libraries; "Update in
    Agora" opens Agora's Updates, which installs them (one administrator password, its progress and cancel). Settings runs no
    package command.
  - **Pause updates**: "Pause for 1 week" (Resume while paused): no automatic checks, neither this page's on opening nor
    Agora's at its start and every half hour (`yggdrasil_updates::pause`, `~/.local/state/yggdrasil/updates-paused-until`).
    Asking by hand still works, and pacman in a terminal is never held; Game time already holds updates during games.
  - **Update history**: every package transaction in pacman's log (`/var/log/pacman.log`, readable by all), newest first (the
    newest 300), each opening to the command that ran it and every package upgraded, installed, removed, downgraded or
    reinstalled with its versions; a terminal's `pacman -Syu` and Agora's updates alike. The parser is
    `yggdrasil_updates::history`, kept apart from the window so an assistant can use it to tell which update came before a
    problem (the hook for Sarina's update follow-up).
  - **App Updates** opens Agora's Updates (apps, Flatpaks, our own packages). No count: Agora works it out live (yay and
    Flatpak over the network), which is not cheap enough for this page.
  - **Related support > Help with updates**: a few lines on what each part does.
  **Validated before installing**: Sarina (optional) is to try updates in a Limbo guest before they are installed (her
  limbo-validate, with a profile I write). Not built yet; the hook is `sysupdate::validation`: with no assistant nothing is
  said; with one, "Not validated yet: Sarina's update check comes later" until its integration names an `update_verdict`
  file (docs/INTEGRATIONS.md) whose verdict covers exactly the updates available now. Only "approved" turns Check for updates
  into **Install updates** (which opens Agora's Updates) and the badge into a shield.

- **Battery** (`src/battery.rs`; in the sidebar only on a computer with a battery of its own, a power_supply of type
  Battery that is not a device's): the charge in large type with "Last charged to N%" and a bar (a line at the charge
  limit), the last ten days as bars of the battery used each day while unplugged, App & System activity, Battery Health
  (maximum capacity, charge cycles) and the charge limit, which moved here from Power (Power keeps a Battery row that opens
  it, and the power mode). Nothing runs in the background for it: UPower already records the charge (its files in
  /var/lib/upower are readable by root only, so the same samples are asked for over D-Bus with GetHistory), and an app's
  share is the processor time of the systemd scopes the launcher starts it in (lapp_*.scope, app-*.scope, matched to apps by
  yggdrasil-appusage), against the rest of the session. Both are read when the page opens and every 30 seconds while it is
  open. The apps' share counts the apps open now; apps already closed would need a small record kept in the background,
  which I left out.

- **Wi-Fi & Network** (`src/wifi_ui.rs`): joining goes through `packages/netjoin` (shared with the installer), NetworkManager's
  AddAndActivateConnection over D-Bus with the password inside the message. A network that joins but gives out no address
  (NetworkManager's "IP configuration could not be reserved" or "the base network connection was interrupted", as a phone's
  hotspot does with a weak mobile signal) is said in plain words (`src/noaddress.rs`): the first time "joined but gave no
  address this time", the second in a row "Joined NAME, but it is not giving out an address", with the phone's signal to
  check. Since 2026-11-14 Other Networks ends with
  **Hidden network…**: the name, the security (None or WPA/WPA2/WPA3 Personal) and the password, for a network that does not
  broadcast its name (the profile is marked hidden). The join window shows the address the network will see, with a Copy
  button: the adapter's own one, or, while NetworkManager's configuration gives each Wi-Fi network a private address (as
  Labyrinth's Heimdall does), "a private address of its own" and the choice **Use this computer's real address (for networks
  that reserve or allow addresses)**, which gives the new profile `cloned-mac-address=permanent` and shows the adapter's
  permanent address. A saved network's own page keeps its Private address choice (Private, New each time, Real, Custom) as
  before. Heimdall's Wi-Fi guard switches a profile on a network it does not trust back to a private address after it
  connects; trusting the network in Heimdall keeps the real one.
  Since 2026-11-15 there is no **Scan again** button: while the list, or a Wi-Fi network's own page (its "Last seen"), is on
  screen and the window is in front, Settings asks NetworkManager for a scan every 12 seconds (NetworkManager allows one
  about every 10) and reads the lists 5 seconds later (`src/wifi_live.rs`; the request is RequestScan over D-Bus in
  `packages/netjoin/src/scan.rs`). A quiet "Searching…" under the lists says so. The list stays in place as results come in:
  a network keeps its row when its signal changes, a new one is added at the end, one not heard in the last scan is faded
  ("Not heard just now") and one not heard for two minutes goes. Nothing is scanned while the page is closed, the window is
  in the background or minimised, a network is being joined, or Settings has quit; the scanning thread then waits without
  waking. Tests: the schedule (never more often than every 12 s, also when the page is closed and opened again) and the list
  order, without NetworkManager; the request against a made-up NetworkManager on a private socket.
  Since 2026-11-15, **Several connections at once** (`src/multilink_ui.rs`, shown when Heimdall's `heimdall-multilink-git` is
  installed; Heimdall/docs/MULTILINK.md) has its own page under the lists. It has the switch, every connection with its role
  (**Use**, **Backup only**, **Off**), and under **Limits** a largest share of new connections (none, 25, 50 or 75 %) and a
  data cap in MB per day or month. While the feature is on, each connection's line shows its speed now and its share of new
  connections, and the top line shows the combined speed. Notes under the cards say when it helps and when it does not: the
  connections must reach the internet by different ways (Wi-Fi and Ethernet from one router do not), one download over one
  connection is never faster, sites may ask for a login again, and it pauses with a VPN on. Changes run `pkexec
  /usr/bin/heimdall-multilink on|off|set ...` (the administrator's password, remembered for a few minutes). The figures come
  from the service's status file once a second, and from `heimdall-multilink status --json` every 5 seconds while it is off.
  They are read only while the page is open and the window is in front, and the window is woken only when they changed.
  Since 2026-11-17 a saved network's Heimdall group (`src/wifitrust.rs`) also has **This network is** (Home router, Mesh,
  Phone hotspot, Travel / hotel, Other) under the trust level and the discoverable switch, with what Heimdall makes of the
  choice under it. It is read with `heimdall-wifi-guard kind --uuid UUID` when the page opens and set with `pkexec
  /usr/bin/heimdall-wifi-guard kind KIND --uuid UUID` (polkit's default: the administrator's password), then read again. On an
  untrusted network the list is shown off with "Trust this network first", and the guard is not asked. Heimdall has no
  suggestion of its own yet, so none is shown (the to-do is in Heimdall's docs/WIFI_TRUST.md).
  Since 2026-11-18 a saved **wired** network's page has a Heimdall group too (`wired_trust_group` in `src/wifi_ui.rs`, the
  model in `src/wifitrust.rs`; Heimdall's docs/WIFI_TRUST.md, "Wired networks"). A wired network is untrusted (a hardware
  address of its own, no computer name sent) until made home. **This is my home network** has **Make it home**, with **Keep
  my real hardware address** (for a router that reserves an address for this computer) beside it; it runs `pkexec
  /usr/bin/heimdall-wifi-guard trust --home [--real-address] --uuid UUID` and works only while that network is the one
  connected (the guard binds home to the router it sees; otherwise the button is off and a note says to connect first). A
  home network shows **Forget** (`pkexec /usr/bin/heimdall-wifi-guard untrust UUID`) and, while connected, whether the
  hardware address there is the real one. The state is read from `heimdall-wifi-guard status` when the page opens and after a
  change: the line `  NAME: home (wired), router ...`, and for the connected one `wired now: NAME, LEVEL; a hardware address
  of its own here: yes|no`. A Heimdall without that line for the connected wired network gets "Needs a newer Heimdall".
  Also since 2026-11-17, a device's page in **Devices** has **What it is**: "This device is" with TV or streaming stick, Phone
  or tablet, Computer, Game console, Speaker, Printer, Smart-home device, Router or Other, correcting the list's guess (the
  grey line names the guess once corrected, and the list offers the guess back). The security integration keeps it: its
  `devices_kind` command (Heimdall: `heimdalld lan kind`, docs/INTEGRATIONS.md) runs through pkexec with the hardware address
  and the kind, and the list's `owner_kind` is shown over `kind` everywhere in Devices. Without `devices_kind` the guess is
  shown and cannot be changed. Nothing is polled for either: both are read when their page opens and after a change.

- **Bluetooth** (`src/bluetooth_ui.rs`), laid out like Wi-Fi: the switch; My Devices (paired, each Connected or Not
  Connected; a tap connects, the (i) opens its page with its kind, battery, maker, address, Connect/Disconnect and Forget
  This Device, asked twice; the maker comes from Heimdall's lookup in the IEEE's lists, as on the Wi-Fi pages, for a public
  address, and a random one, which the device makes up and changes, says "Private address"); Other Devices, found while the page is open (a tap pairs, trusts and connects; a code to compare or
  type comes up in yggdrasil-polkit, BlueZ's agent); then the game controllers as before. The search is a BlueZ discovery
  session on a D-Bus connection of the page's own, read every two seconds and dropped when the page is left (BlueZ ends a
  client's discovery when its connection closes, so it stops even if Settings is killed). The "Searching" mark stands still:
  a turning one redrew the whole window several times a second (15 % of a core with software drawing).

- **VPN** (`src/vpn.rs`, under Connections): NetworkManager's VPN and WireGuard connections with a switch each (`nmcli
  connection up/down`) and their state; Tailscale when it is installed, its state and a switch (`tailscale up` / `down`,
  nothing else of it touched); and Add VPN Configuration: WireGuard from a pasted or chosen .conf (read in Settings and handed
  to NetworkManager with AddConnection over D-Bus, the private key inside the message, never in a file of ours), OpenVPN
  from an .ovpn file (`nmcli connection import`; needs the networkmanager-openvpn package, the page says so when it is
  missing), IKEv2 with a user name (`nmcli connection add type vpn vpn-type strongswan`; the password is asked by
  NetworkManager's agent when it first connects; needs networkmanager-strongswan). The security monitor owns the network's
  security policy; this page is only the user interface. `yggdrasil-settings vpn:add` opens Add VPN Configuration.

- **Peripherals > Camera and Microphone** (`src/media.rs`): rows on Peripherals only when the computer has a camera (a
  capture node in /sys/class/video4linux) or an input device. Camera: each camera, which one is the default (WirePlumber's
  default video source, `wpctl set-default`) and a live preview of it while the page is open (ffmpeg reads the camera and
  hands raw frames over a pipe, five a second; it is stopped when the page is left; without ffmpeg the page says so).
  Microphone: the input device, a level meter while the page is open (`pw-record`, 16 kHz mono, the loudest sample of each
  50 ms; stopped when the page is left), volume and mute. Sound keeps the output and a row to Microphone.

- **Private microphone** (`src/privmic.rs`, 2026-11-15): a switch on Security under Privacy, and the same switch under
  Peripherals > Microphone. Heimdall's "Microphone (private)" sits between the real microphone and every app (calls in the
  browser, Teams, Zoom, Webex, Discord, recorders), and takes out the sounds that can place or date a call or a recording:
  aircraft overhead (their pitch glide can be matched to public flight tracks) and mains hum (it follows the power grid's
  frequency, which is logged). It is on by default; I turn it off for music, where the full bass matters. Settings only asks
  and switches: `heimdall-private-mic status --json` when the page opens (and with the page's usual reading while it stays
  open), `heimdall-private-mic on|off` when the switch moves (argv, no shell; it takes effect at once and is kept). Without
  Heimdall's program the row says "Not installed"; on but with the node missing it says to log out and in again. How it works
  and what it cannot do is in Heimdall's docs. On Labyrinth OS Sarina's voice input records the private microphone like every
  app; there is no exception and no switch for one.

- **Sarina's voice input uses the real microphone** (LykanOS only, 2026-11-15): a second switch under Private microphone, off
  by default, shown only when /etc/os-release says `ID=lykanos` and `heimdall-private-mic status --json` says
  `"lykanos":true`, and only while the private microphone is on. On, the one stream of Sarina's voice questions (the panel's
  Super+S recording, which names itself with the node property `heimdall.private-mic.exempt = "sarina-voice"`) records the
  real microphone; every other app stays on the private one. It runs `heimdall-private-mic sarina-raw on|off` (argv). The
  property is on every voice recording and means nothing unless that switch is on, so Labyrinth OS ignores it.

- **Disguise my voice** (2026-11-18): a switch under Private microphone, off by default, and while it is on a "Voice" row
  with Lower or Higher. The private microphone then shifts my voice's pitch and formants, with a little variation per
  session, so it is not recognised at a casual listen. The row's grey line says plainly that a determined listener can partly
  undo it (it is light signal processing; how and how well is in Heimdall's docs/VOICE_DISGUISE.md). It runs
  `heimdall-private-mic disguise on|off|lower|higher` (argv, as my own user like the other two switches: the setting lives
  in my WirePlumber, so no password is asked) and reads `"disguise"` and `"disguise_preset"` from `status --json`. It is
  shown only while the private microphone is on (off, apps record the real microphone) and only when Heimdall's program
  reports the disguise (an older one leaves the fields out, and the row stays hidden).

- **The privacy pack** (`src/privpack.rs`, 2026-11-15): four more switches in the same Privacy card on Security, one per part,
  all on by default. I want people safe without having to find these, so off is the choice that needs a click.
  - **Check downloads**: Heimdall's downloads guard (`heimdall-fileguard`, a user service) looks at each finished download
    (~/Downloads and the torrent clients' download folders) and says in a notification when the file would contact a server
    as soon as it is opened (an Office template on the web, a PDF that opens a web address by itself, a playlist or shortcut
    pointing online). It never changes the file. "Not installed" without the program.
  - **Remove hidden info in Minos**: shows Minos' right-click entry (docs/keys/minos.md), which writes a copy without the
    place a photo was taken, the device, the author and the rest. "Not installed" without `heimdall-strip`.
  - **Viewer apps ask before going online**: the per-app network rules start PDF viewers, office apps, image viewers and media
    players on "Ask", so the first time one tries to connect a notification asks. This one is the system's setting for the
    person, so it runs `pkexec NETWORK viewers on|off` (NETWORK is the apps integration's `network` command,
    docs/INTEGRATIONS.md) and asks for the password; the page reads `NETWORK viewers status --json`
    (`{"on":bool,"apps":[ids]}`). "Not available" when the system's rules have no such command.
  - **Warn about look-alike names**: Settings > Apps and Agora put "Look-alike name" beside an app whose name imitates another
    (a Cyrillic а in "Vivаldi", fullwidth or mathematical letters, invisible or direction-changing characters). The app's page
    then says why in plain words, shows the name with its hidden characters written out (`<U+200B>`), and the program it
    really starts. The check is Unicode's TR39 in a small form (`yggdrasil-ui/src/lookalike.rs`, the same file as Heimdall's
    and Labyrinth's), so every place decides the same way.

  The three switches that belong to the person (downloads guard, Remove hidden info, look-alike warnings) are one file,
  `~/.config/heimdall/privacy.json`: `{"downloads_guard": true, "remove_hidden_info": true, "lookalike_warnings": true}`. Each
  is on unless the file says `false` (a missing file or key is on). Settings writes it whole (a temporary file, then a rename)
  and keeps any other key; Heimdall's guard follows it by itself; Minos and Agora read it when they need it
  (`yggdrasil-ui/src/privacy_prefs.rs`). A made-up system (the screenshots' fixture) shows every switch on and changes nothing.

- **Appearance > Wallpaper** has a preview card on top (`src/wallpaper.rs`): the lock screen and the desktop side by side in
  the current wallpaper, style, accent, icon pack and lock screen design. The lock screen is the lock's own drawing
  (`yggdrasil-lock --preview` of the design in use, 640x360, nothing locked; a likeness drawn by Settings stands in while it
  is made: the lock is a program, its design code is not a library Settings can call); the desktop is drawn by Settings (the
  top bar, a window with the accent's border, the dock with the first pinned apps in the icon pack). Both are made again
  only when one of those changes, and dropped when the page is left.

- **Appearance > Fonts** (`src/fonts_ui.rs`, since 2026-11-18; `yggdrasil-settings appearance:fonts`), laid out like Windows
  11's Fonts page since my first try of it the same evening: **Add fonts** at the top (the drop area and "Browse and install
  fonts"), then **System font** as two rows, **Text** and **Code and terminals**, each showing the font in use drawn in its own
  letters and opening a page of its own (Appearance › Fonts › Text, `appearance:font-text`; `appearance:font-code`), then the
  installed fonts as cards. Each kind's page lists the choices, each with a line of text in it, a plain note on what it was
  made for, and Use or In use: Atkinson Hyperlegible (the default since 2026-11-19) and Atkinson Hyperlegible Next (the
  Braille Institute's, for low vision readers), Lexend, OpenDyslexic and Noto Sans (the default until then, with letters for
  almost every language); for code Atkinson Hyperlegible Mono (the default), OpenDyslexic Mono and Noto Sans Mono. A font
  that is not installed says so and cannot be chosen. The package depends on all six, `ttf-atkinson-hyperlegible` (Arch's),
  `ttf-atkinson-hyperlegible-next`, `ttf-atkinson-hyperlegible-mono`, `ttf-lexend`, `otf-opendyslexic` and
  `otf-opendyslexic-mono` (Labyrinth's `[labyrinth]` repository), so an update brings them to a system installed without
  them. How it works:
  - The choice is a fontconfig file (`yggdrasil-conf/src/fonts.rs`): `~/.config/fontconfig/conf.d/56-yggdrasil-font.conf`
    prefers the family for `sans-serif` (and `sans`, `system-ui`) and for `monospace`, so every program that asks fontconfig
    follows it. Only families from the list are ever written. Nothing chosen: no file, and the default.
  - **The default is a file of its own**, yggdrasil-theme's `/usr/share/fontconfig/conf.avail/58-yggdrasil-default-font.conf`
    (the source: `packages/theme/data`), linked from `/usr/share/fontconfig/conf.default` as fontconfig's own files are, so
    fontconfig's pacman hook links it into `/etc/fonts/conf.d` (and unlinks it when the package goes). It prefers Atkinson
    Hyperlegible for `sans-serif`, `sans` and `system-ui` and Atkinson Hyperlegible Mono for `monospace`, so a system with no
    56 file (installed before, or with the defaults) has them too. The theme package depends on those two fonts. At 58 it is
    read after both 56 files, so a chosen font wins, and before the distribution's 60-latin.conf (Noto Sans and DejaVu).
    `the_default_file_names_the_defaults` (yggdrasil-conf) checks that it names exactly the first of each list, and runs
    `fc-pattern` on a scratch configuration: fontconfig reads it without a complaint, it comes before a stand-in
    60-latin.conf, and a 56 file render() wrote comes before it, for all four generic names.
  - **The font chosen is the system font, the login screen's too** (no separate switch since 2026-11-18). Use writes your own
    file at once, no password, and then the computer's, `/etc/fonts/conf.d/56-yggdrasil-font.conf` (the login screen's, and
    every account's without a choice of its own), through pkexec and `/usr/lib/yggdrasil/fontconf set SANS MONO` (ids from the
    list or `-`; `org.yggdrasil.fontconf.set`, an administrator's password kept a few minutes, like the other system-wide
    changes in Settings). When the computer's file already says the same, no password is asked. Password cancelled: your own
    choice still applies, and a small note under the fonts says "The login screen keeps Atkinson Hyperlegible until the font
    is set again", with "Set again" beside it (the note is read from the two files, so it is there after a restart too). The
    installer writes the computer's file. Your own file is read first (50-user.conf), so it wins over the computer's, and
    either wins over the default (58) and the distribution's Noto Sans (60-latin.conf).
  - **No size here:** text size is display scaling's and Accessibility's "Text and control size" (panel.conf's `text_scale`);
    the Fonts page no longer repeats it.
  - **A stale font cache** (found on my PC on 2026-11-18): after `pacman -S ttf-atkinson-hyperlegible` and, in the same
    second, `pacman -U` of the other five font packages, `fc-list` showed only Atkinson Hyperlegible; `fc-cache -v` said
    "/usr/share/fonts/TTF: skipping, existing cache is valid: 4 fonts". Fontconfig keeps a folder's cache while the folder's
    modification time is the same, and Arch's fontconfig 2.18.4 records it in whole seconds only (the cache's nanoseconds
    field is 0). The first transaction's hook (`fc-cache -s`) cached /usr/share/fonts/TTF with its 4 files at 21:37:31.28;
    the second put five more in at 21:37:31.72, the same second, so its hook kept the old cache, and the cache of
    /usr/share/fonts, made in that second too, did not list the new OTF folder at all. Reproduced with a scratch fontconfig
    (`FONTCONFIG_FILE`, a cache folder of its own). The fontconfig upgrade minutes earlier and the files' own times play no
    part. "Skipping, looped directory detected" is harmless (a folder reached twice). Two fixes: Labyrinth's five font
    recipes run `fc-list -q FAMILY || fc-cache -fs /usr/share/fonts` after installing (only when their font is not listed:
    about two seconds), and Settings, the first time the Fonts page opens, compares `fc-list : family` with the choices'
    files on disk (`Choice::file`, where each package puts its regular face): a font whose file is there but which is not
    listed makes it run `fc-cache -f` once (your own cache, no password; fontconfig takes the newer of the two caches) and
    read the fonts again (`fontlib::heal`). Until that is done such a font says "Looking for it…", not "Not installed".
  - **Who follows, and when:** Yggdrasil's egui apps (Settings, Minos, Agora, Calculator, Task Manager, Saga MD, Clipboard,
    the password box, Keyring, Player, Authenticator, Sudoku, 2048, the installer) within a second
    (`yggdrasil-ui/src/fonts.rs`: they load the fc-matched regular and monospace files over egui's own fonts, which stay behind
    them for emoji and symbols, and look at the two files' metadata once a second); the top bar and dock at once (Settings notes
    the choice in appearance.conf, which the panel watches); the lock screen at the next lock; the login screen, the colour
    picker, the boot screen's messages and Yggdrasil's own compositor at their next start; GTK, Qt and other programs when they
    start again. A variable font (one file for every weight: Atkinson Hyperlegible Next and Mono, Lexend) is set to the
    regular weight, and the bold weight for bold, everywhere (its file's default can be ExtraLight).
  - Cost (measured 2026-11-18, release build, an egui context laying out a page of text twice): egui's own fonts alone 3.8 ms
    and +4.7 MB; with the system's Noto Sans and Noto Sans Mono 13 ms (6 to 9 ms of it the fc-match calls, run side by side)
    and +7.1 MB; with Atkinson Hyperlegible Next and Mono 7.5 ms and +5.2 MB. The real bold face is loaded only by an app that
    asks for it (`fonts::want_bold`, the installer's headings): it would add about 1.4 MB to every app that never draws it.
  - **Adding and removing fonts**, like Windows 11's Fonts page (`src/fontlib.rs`): at the top a drop area ("Drag and
    drop to install": .ttf, .otf, .ttc, .woff2, or a .zip of them) and "Browse and install fonts" (the desktop's file
    chooser, the FileChooser portal); below the system font the installed fonts as cards, each with a sample line drawn in it (on a thread of its
    own, only for the cards on screen), its name and how many styles it has, a search box above them, and on each card "···"
    with Remove for a font you added (the system's are removed with their package, not here). Your fonts go to
    `~/.local/share/fonts` (no password), then `fc-cache` for that folder. Dropping works on Wayland through yggdrasil-ui's
    `dnd.rs` (Minos's drag and drop, shared since 2026-11-18: winit has none there).
  - **Every file is checked before it is installed**, whatever it came from. It is first copied into a private folder
    (`~/.cache/yggdrasil/font-import`, 0700); Heimdall's file guard checks that copy (`heimdall-fileguard scan --json`; a .zip
    is unpacked by the guard itself, `heimdall-fileguard extract --json --only ttf,otf,ttc,woff2`, with its own limits:
    Settings never unpacks an archive); Settings reads it as a font (skrifa, Rust and memory-safe: a name, a character map,
    letters; WOFF2 by its header and fontconfig's own reading in a process of its own), 50 MB at most; and that same copy is
    what goes into the fonts folder. Refused (by the guard, or not a font): nothing is installed and the reason is shown. A
    warning: shown, with "Install anyway". A guard without `scan` and `extract` (an older Heimdall) or none at all: the page
    says installing needs a newer Heimdall privacy package and takes nothing.

- **Accessibility** (`src/accessibility.rs`, near the top of the sidebar; Appearance keeps a row to it): text and control
  size, high contrast, pointer size, zoom, colour filters, reduce motion, and rows to key repeat and the on-screen keyboard. What each does
  and what is still missing: docs/ACCESSIBILITY.md.

## The pages

- **The list** (2026-11-13, as on a tablet): a card at the top with Default Apps ("Manage default apps", no longer in the
  sidebar; `yggdrasil-settings default` still opens it, inside Apps), Optional software and Minos (they were tabs), then a
  search field and every visible desktop entry A to Z, a card per letter. It can also be sorted by Usage (how often it was
  opened from here) or Size (its folders under ~/.config, ~/.cache, ~/.local/share, ~/.var/app). System tools (settings panels, Avahi, ...) are hidden
  unless "Show system tools" is ticked. Each row has icon-only buttons: restart, close (both only while it runs), remove
  (asks first; shows the package; the desktop's own packages are refused).
- **An app's page** (click a row, or `yggdrasil-settings apps:ID`): Network (switch; click the name too), open at login,
  status, storage with Clear cache and Clear data (the latter lists the folders and asks for the password), the privacy
  sandbox, Remove.
- **Optional software** and **Minos** are rows on the card at the top, each its own page.
- **Developer options** (About: click the logo five times while holding the ` key): adds "Any connection / Ethernet only /
  Wi-Fi only" next to an app's network switch, and the selected power mode's CPU and graphics-card limits under Power. Stored as
  `developer = yes` in ~/.config/yggdrasil/panel.conf.

- **Games** (`yggdrasil-settings games`, 2026-11-05): the graphics cards a game can draw on, fixes for particular games, and frame generation.
  - Every card, with what it does: which one shows the screen, which one the computer started on, its driver. Read by
    `yggdrasil-gpu-run list --json` (packages/settings-helpers, Rust, installed as /usr/bin/yggdrasil-gpu-run), the same code that later starts
    the game, so the numbers on the page are the numbers it takes.
  - With two or more cards each has **Copy launch option**: `yggdrasil-gpu-run 2 %command%` in a game's Steam launch options runs
    it on card 2. For an NVIDIA card (its own driver) that is PRIME render offload and `__VK_LAYER_NV_optimus=NVIDIA_only`; for
    AMD or Intel (Mesa) `DRI_PRIME=pci-...` and `MESA_VK_DEVICE_SELECT`, hiding an NVIDIA card with `non_NVIDIA_only`; for every
    card `DXVK_FILTER_DEVICE_NAME` and `VKD3D_FILTER_DEVICE_NAME` (Proton's DirectX translation) when vulkaninfo names it. The
    picture still reaches the screen through the card the monitor is plugged into (the drivers copy it across). Tested against
    a pretend two-card /sys (an RTX 2070 with an Arc B580); not yet on a real second card.
  - **Game fixes** (2026-11-05): per-game fixes for installed Steam games, each with Apply / Undo. `helpers/game-fix`
    (/usr/lib/yggdrasil/game-fix) applies the catalogue `data/game-fixes.json` (/usr/share/yggdrasil/game-fixes.json): per game,
    settings files ("Key = value" or ini) and the values to set, which can be the screen's size ({width}, {height}: the
    largest monitor, from the compositor through yggdrasil-wm, or xrandr). A file is backed up before its first change (FILE.yggdrasil-backup), or marked
    as made (FILE.yggdrasil-created, from a template in the game's folder or lines in the catalogue); Undo restores it. A game
    Proton has not started yet shows "Start the game once first". No administrator rights. First fixes: **Dead Space 2**
    (its menu never offers 21:9; settings.txt at the screen's size, V-Sync off: true 21:9 tried at 3440x1440) and
    **Fallout 4** (the settings files its launcher would make, from its templates, at the screen's size, borderless; the
    world fills 21:9 (confirmed by the owner in play), the HUD stays 16:9 and stretched; and a launch option that starts the game without Bethesda's launcher, which kept the keyboard
    once the game was up: tried by the owner 2026-11-05, keys work). A fix's `launch` is offered with Copy launch option (with
    `kairos` in front when frame generation is installed); Steam keeps launch options to itself, so it is copied,
    not written. Tests: `packages/settings-helpers/src/gamefix/tests.rs`. Patches to games' code come per game.
  - **Frame generation (preview)**, Kairos, Labyrinth's own (`labyrinth-framegen` until 2026-11-07; from Agora): every installed Steam game (from the
    appmanifests in all Steam libraries; Proton and Steam's runtimes left out) with Off / 2x / 3x / 4x, saved at once to
    `~/.config/kairos/games.conf` (`<app id> <off|2|3|4>`; the old `~/.config/labyrinth-framegen/games.conf` is read until the first
    change). The game needs `kairos %command%` (the old `labyrinth-framegen %command%` still works) in its
    Steam launch options once (Copy launch option); the launcher reads the choice when the game starts.
  - **Lossless Scaling frame generation (lsfg-vk):** whether lsfg-vk is installed (its Vulkan layer; Get it in Agora otherwise, where
    Labyrinth's catalog has it as "Frame generation", from the AUR's source recipe), whether Lossless Scaling is installed in
    any Steam library (it supplies the frame generation; Open in Steam otherwise), and "Choose games", which opens lsfg-vk's own
    window. lsfg-vk is not part of the desktop: its licence (CC BY-NC-ND 4.0) forbids changing it and commercial use.
  - **On a second card (experimental):** Lossless Scaling's dual-card mode (the game on one card, the extra frames on another) is
    not in lsfg-vk ("will be available in future releases"), and its licence forbids a changed copy. So it is built from
    unchanged parts: `yggdrasil-gpu-run N --frames-on M [--multiplier X] %command%` runs the game on card N inside gamescope
    (SDL backend: it presents through a Vulkan swapchain) on card M, switches lsfg-vk on for gamescope alone (`LSFGVK_ENV=1`,
    `LSFGVK_MULTIPLIER`) and off in the game (`DISABLE_LSFGVK=1`, gamescope's card variables taken away). The extra frames are
    made on card M from gamescope's output. With two cards the page offers it with Copy launch option (once lsfg-vk and
    Lossless Scaling are there): the frames on the card the monitor is plugged into, the game on the other.
    Checked on one card (2026-11-05): gamescope's SDL output loads implicit Vulkan layers (Mesa's device-select was inserted)
    and presents through vkCreateSwapchainKHR / vkQueuePresentKHR, where lsfg-vk works. (MangoHud cannot show this: it skips
    any process named gamescope.) Not yet run with lsfg-vk and Lossless Scaling, nor on two real cards. Costs a little input
    latency, as on Windows.

  ![Settings > Games](../assets/screenshots/settings-games.png)

- **Peripherals** (`yggdrasil-settings peripherals`, `mouse`, `keyboard`; 2026-11-12): the mouse and the keyboard, each on its own
  page, with the lights, DPI and polling rate of the devices that have them. It took the old Keyboard page's place in the
  sidebar; `yggdrasil-settings keyboard:shortcuts` and `shortcuts` still open the shortcut list.
  - **Device software** (2026-11-24): what this computer's devices ask for that is not installed, as the sign-in offer
    finds it (`/usr/lib/yggdrasil/device-offers --json`, packages/device-offers, in Agora's package), with Install, and Ask again for the ones answered
    "Don't ask again". Shown only when there is something; nothing is installed without the click.
  - **Mouse:** pointer speed, mouse acceleration (adaptive, or flat as most games want), scroll speed, natural scrolling and
    which side the main button is on, for every mouse, and under "Each mouse" for one mouse alone (its switch gives it settings
    of its own, starting from everyone's). Find my mouse moved here from Keyboard.
  - **Keyboard:** repeat delay and rate with a box to try them, the layout and its variant (the system's, for every account and
    the sign-in screen: `localectl set-x11-keymap`, which asks for the password, then the running session at once), and
    "Keyboard and mouse lights off while dimmed", moved here from Power (the same `lights_off` in
    `~/.config/yggdrasil/idle.conf`). Below them Shortcuts, Spelling and languages, and the Color Picker, as before.
  - **Where the settings go:** `~/.config/yggdrasil/input.conf` (its form is in `packages/wm/src/input.rs`), which the session's
    `yggdrasil.lua` reads at every start and reload, so they survive restarts; and the running compositor through yggdrasil-wm
    (`Desk::apply_input`): on Hyprland its Lua (`hl.config({ input = ... })`) at once, and a reload when a mouse's own settings
    change (Hyprland keeps `hl.device` rules until its config is read again). A mouse's settings are given to its other parts
    too (Hyprland lists a mouse's extra buttons as NAME-1, NAME-2, ...). **Not on Yggdrasil's own compositor yet:** there the
    file is kept and the page says so; the compositor is meant to read the same input.conf on start and `reload` (an item for
    its gap list, with libinput on the hardware backend).
  - **Razer** (when OpenRazer is installed): each device from OpenRazer's daemon on the session bus (`org.razer`), what it
    offers read from its introspection: the battery, a mouse's DPI (in steps of 50, up to its own maximum) and polling rate, and
    every lighting zone (the whole device, logo, scroll wheel, backlight, left and right) with its brightness, its effect (off,
    one colour, spectrum, breathing, wave: only those the device has) and its colour. Keyboards and keypads are on the Keyboard
    page, mice and mouse mats on the Mouse page, anything else on Peripherals itself. Every call has a 2-second limit and runs
    off the window's thread; a colour is sent once it has stayed put for 0.4 s; the daemon is never started by asking (a row
    offers "Start it" when it is installed but not running). `src/razer.rs`.
  - **Other makers:** a row that opens Polychromatic, Solaar (Logitech), ckb-next (Corsair), OpenRGB or Piper when installed;
    with none of them and no OpenRazer, one line points to Agora > Devices.
  - **Tests:** the OpenRazer client against a made-up daemon on a peer-to-peer D-Bus socket pair (devices, zones and effects
    from introspection, every change reaching the right method, a daemon that hangs giving up at the time limit); the
    input.conf form, its writer (temporary folders only) and the Hyprland requests (yggdrasil-wm, against its stand-in
    socket); the session's reader in plain Lua (`packages/session/tests/input-conf.lua`). The screenshots are made-up devices
    (`tests/fixtures/peripherals.json`) on Yggdrasil's headless compositor.
  - **Cost** (release build on the headless compositor, Mesa's software renderer, three runs each, before and after): the
    window shows after 0.154-0.174 s and 0.164 s; idle 0.9-1.1 % and 0.9-1.0 % of a core on the Keyboard page; 148-149 MB and
    151 MB resident (115-116 MB and 117-118 MB proportional), the extra being the layout list and the bigger page. The program
    is 0.6 MB larger. No new libraries (zbus was there for Wi-Fi).

  ![Settings > Peripherals > Mouse](../assets/screenshots/settings-peripherals-mouse.png)
  ![Settings > Peripherals > Mouse, a Razer mouse](../assets/screenshots/settings-peripherals-razer.png)
  ![Settings > Peripherals > Keyboard](../assets/screenshots/settings-peripherals-keyboard.png)

## Every page

Each page of the sidebar on a made-up computer (`scripts/screenshots.sh`, the README's "Making them again"; the fixtures are in
`packages/settings/tests/fixtures`: `system.json` for a desktop, `laptop.json` for a laptop). Developer options, shown under
About only when switched on, have no picture.

| Page | |
| --- | --- |
| **Accessibility**: text and pointer size, zoom, colour filters, reduce motion. | ![Settings > Accessibility](../assets/screenshots/settings-accessibility.png) |
| **General**: the page Settings opens on. | ![Settings > General](../assets/screenshots/settings-general.png) |
| **General > Software Update**: 6 made-up updates, More options (`system.json`). | ![Settings > General > Software Update](../assets/screenshots/settings-software-update.png) |
| **General > Software Update > Available Updates**: grouped, with versions, sizes and repositories. | ![Settings > Software Update > Available Updates](../assets/screenshots/settings-software-update-available.png) |
| **General > Software Update > Update History**: pacman's log (made up), one transaction open. | ![Settings > Software Update > Update History](../assets/screenshots/settings-software-update-history.png) |
| **General > Storage**: each disk's use (`system.json`'s made-up disks). | ![Settings > General > Storage](../assets/screenshots/settings-storage.png) |
| **General > Language & Region**: the system's language, a separate region for dates and times. | ![Settings > General > Language & Region](../assets/screenshots/settings-language.png) |
| **Wi-Fi & Network**: the network in use, Known Networks and Other Networks (`wifi.json`). | ![Settings > Wi-Fi & Network](../assets/screenshots/settings-network.png) |
| **Wi-Fi & Network > a network**: the Heimdall group, with the discoverable switch and "This network is" (`wifi.json`). | ![Settings > Wi-Fi & Network > Lantern Hall](../assets/screenshots/settings-network-heimdall.png) |
| **Bluetooth**: My Devices (Connected / Not Connected, an (i) each), Other Devices found nearby, a game controller. | ![Settings > Bluetooth](../assets/screenshots/settings-bluetooth.png) |
| **VPN**: a VPN and a WireGuard connection, Tailscale. | ![Settings > VPN](../assets/screenshots/settings-vpn.png) |
| **VPN > Add VPN Configuration**: WireGuard from a file or pasted. | ![Settings > VPN > Add VPN Configuration](../assets/screenshots/settings-vpn-add.png) |
| **Displays**: three displays attached, one disconnected; the arrangement box with the main display (1) selected, its settings below. | ![Settings > Displays](../assets/screenshots/settings-displays.png) |
| **Displays, a disconnected display selected**: grey and smaller in the box, Connect among its settings. | ![Settings > Displays, a disconnected display](../assets/screenshots/settings-displays-connect.png) |
| **Displays, one display** (`displays-single.json`): its settings at once, no box, no Duplicate or Disconnect. | ![Settings > Displays, one display](../assets/screenshots/settings-displays-single.png) |
| **Sound**: the output with its volume, and a row to Microphone. | ![Settings > Sound](../assets/screenshots/settings-sound.png) |
| **Peripherals > Mouse**: every mouse, or one by one (`peripherals.json`). | ![Settings > Peripherals > Mouse](../assets/screenshots/settings-peripherals-mouse.png) |
| **Peripherals > Mouse, a Razer mouse**: OpenRazer's battery, DPI, polling rate and lights. | ![Settings > Peripherals > Mouse, a Razer mouse](../assets/screenshots/settings-peripherals-razer.png) |
| **Peripherals > Keyboard**: key repeat, the layout, the lights while dimmed. | ![Settings > Peripherals > Keyboard](../assets/screenshots/settings-peripherals-keyboard.png) |
| **Peripherals > Camera**: the default camera and its preview (`system.json`; ffmpeg's test picture stands in for a camera). | ![Settings > Peripherals > Camera](../assets/screenshots/settings-camera.png) |
| **Peripherals > Microphone**: the input device, its level, volume and mute (a made-up level). | ![Settings > Peripherals > Microphone](../assets/screenshots/settings-microphone.png) |
| **Printers**: two printers, their ink and toner. | ![Settings > Printers](../assets/screenshots/settings-printers.png) |
| **Power, a desktop**: the power mode, Cooling and the idle timers (no battery). | ![Settings > Power, a desktop](../assets/screenshots/settings-power.png) |
| **Power, a laptop**: a row to Battery, the fan the firmware runs (`laptop.json`). | ![Settings > Power, a laptop](../assets/screenshots/settings-power-laptop.png) |
| **Battery** (`laptop.json`): the charge, the last ten days, the apps' share, health and the charge limit. | ![Settings > Battery](../assets/screenshots/settings-battery.png) |
| **Power > Cooling**: a fan controller's pump and fans, each with its speed now. | ![Settings > Power > Cooling](../assets/screenshots/settings-cooling.png) |
| **Power > Cooling, a fan's curves**: the processor's curve, dragged point by point. | ![Settings > Power > Cooling, a fan's curves](../assets/screenshots/settings-cooling-curves.png) |
| **Appearance**: light or dark, the accent, the pages inside it, a row to Accessibility. | ![Settings > Appearance](../assets/screenshots/settings-appearance.png) |
| **Appearance > Fonts**: adding fonts first (here waiting for a newer Heimdall), then the system font's two rows, each in its own letters, then the installed fonts. | ![Settings > Appearance > Fonts](../assets/screenshots/settings-font.png) |
| **Appearance › Fonts › Text**: the fonts for text, each with a line in it, and Use (for you and the login screen). | ![Settings > Appearance > Fonts > Text](../assets/screenshots/settings-font-text.png) |
| **Appearance › Fonts › Code and terminals**: the same for terminals and code. | ![Settings > Appearance > Fonts > Code and terminals](../assets/screenshots/settings-font-code.png) |
| **Appearance > Fonts**, further down: the installed fonts as cards, each with a sample, and the search box. | ![Settings > Appearance > Fonts, the installed fonts](../assets/screenshots/settings-fonts-installed.png) |
| **Appearance > Wallpaper**: the lock screen and the desktop previewed side by side, the wallpapers below. | ![Settings > Appearance > Wallpaper](../assets/screenshots/settings-wallpaper.png) |
| **Appearance > Lock screen**: the designs with their previews. | ![Settings > Appearance > Lock screen](../assets/screenshots/settings-lock-screen-designs.png) |
| **Sarina**: the operating system's assistant, from its integration. | ![Settings > Sarina](../assets/screenshots/settings-assistant.png) |
| **Apps**: Default Apps, Optional software and Minos on a card at the top, then every installed app A to Z under its letter, with a search. | ![Settings > Apps](../assets/screenshots/settings-apps-list.png) |
| **Apps > Default Apps**: what opens each kind of file. | ![Settings > Apps > Default Apps](../assets/screenshots/settings-default-apps.png) |
| **Games**: graphics cards, game fixes, frame generation per game. | ![Settings > Games](../assets/screenshots/settings-games.png) |
| **Security**: the security monitor, the password logs, the distress password, Limbo, the private microphone. | ![Settings > Security](../assets/screenshots/settings-security.png) |
| **Security > Privacy**: the private microphone and the privacy pack's four switches. | ![Settings > Security > Privacy](../assets/screenshots/settings-security-privacy.png) |
| **Security > Distress password**: it opens only after your account password. | ![Settings > Security > Distress password](../assets/screenshots/settings-distress-open.png) |
| **Security > Distress password, the settings**: the password, and how many times within how long. | ![Settings > Security > Distress password, the settings](../assets/screenshots/settings-distress.png) |
| **Security > Distress password, the two sections**: Section 1, and Section 2 that asks for your password again. | ![Settings > Security > Distress password, the two sections](../assets/screenshots/settings-distress-sections.png) |
| **Security > Distress password, saving Section 2**: the warning, and your password once more. | ![Settings > Security > Distress password, saving Section 2](../assets/screenshots/settings-distress-warning.png) |
| **Firewall**: on or off. | ![Settings > Firewall](../assets/screenshots/settings-firewall.png) |
| **Users**: two accounts, the keyring. | ![Settings > Users](../assets/screenshots/settings-users.png) |
| **Restore points**: Snapper's restore points, and how to go back to one. | ![Settings > Restore points](../assets/screenshots/settings-restore.png) |
| **Date & Time**: the time zone, set automatically or by hand: click your part of the map, or search the short list (about 80 friendly zones by UTC offset). | ![Settings > Date & Time](../assets/screenshots/settings-date-time.png) |
| **Date & Time, light style**: the chosen zone in the highlight colour, orange unless the OS's branding names another. | ![Settings > Date & Time, light](../assets/screenshots/settings-date-time-light.png) |
| **About**: the operating system, the computer and its parts. | ![Settings > About](../assets/screenshots/settings-about.png) |

## How it works

- **Nothing drawn while nothing changes** (2026-11-15, `src/refresh.rs`): egui draws a whole new picture each time the window
  wakes, and on a compositor that draws with the GPU each picture is a buffer to compose. Settings used to wake every 5 seconds
  on every page, every second for the text size (the shared look's check of panel.conf) and every 4 seconds on the pages that
  follow changes made elsewhere, only to look. Now those pages (About, Displays, Sound, Appearance, Firewall, Date & Time,
  Security, VPN, Power; Battery every 30 s, Bluetooth's controllers every 5 s, the app list every 4 s) are read again on a
  thread of their own, and the window wakes only when what was read differs from what the page shows; the reads stop while
  something is dragged or typed and while the window is in the background. The settings folder is watched with inotify (the
  text size, reduced motion, the style and the accent wake it when written). Live readouts (the graphics card's and the
  processor's graphs, the fans, the camera and the microphone) still draw while they are on screen, and only then; the Wi-Fi
  list wakes the window once per scan (every 12 s) while it is on screen and the window is in front. Measured
  in the compositor's test machine (`packages/compositor/tests/drm-vm.sh`, OpenGL ES on virgl, 60 s idle with the panel and
  Settings on General): the compositor 185 ms of CPU = 0.31 % of a core before, 4.7 ms = 0.008 % after (the panel alone:
  0.012 %); Settings itself 0.6 ms in 30 s. On Displays: 0.71 % before, 0.020 % after. Headless (Mesa in software, 30 s on
  General): 10 new pictures and 0.93 % of a core in Settings before, none and 0.001 % after.
- **SIGTERM, SIGINT and SIGHUP** (2026-11-15, `src/quit.rs`) close the window as its close button does, and the
  single-instance socket goes with it (a forced end after 3 s if the window cannot draw any more). Before, Settings started by
  my compositor on the hardware inherited these signals blocked and never ended on them, so a shutdown waited for it ("A
  stop job is running"); the compositor no longer passes them on blocked either. Tests: `quit.rs` (each signal, with the
  mask blocked by the parent and not), `tests/apps.rs` in the compositor (the real Settings ends with 0 on SIGTERM).
- **labyrinth-launch ID** starts an app inside its own systemd scope and slice (`lapp_<id>.slice`; no dash in the name, so it
  sits directly under the user manager). The panel (dock, start menu) and Settings use it. It counts launches
  (~/.local/share/labyrinth/app-usage.json) and applies the app's sandbox settings
  (~/.config/labyrinth/apps/ID.json).
- **Network** (labyrinth-appnet, run with pkexec): one nftables table `inet labyrinth_apps` with a rule per limited app
  matching its slice (`socket cgroupv2 level 4 "user.slice/user-UID.slice/user@UID.service/lapp_ID.slice"`). Modes: block
  (also refuses lookups to the local DNS resolver), Ethernet only (`en*`, `eth*`), Wi-Fi only (`wl*`). Rules live in
  /etc/labyrinth/app-network.json. nft needs the cgroup to exist when it loads a rule, so
  `labyrinth-appnet-prepare.service` (user) starts the slices at login and asks `labyrinth-appnet.service` (system) to load
  the rules; a polkit rule lets an active local user start only that service. Verified on real traffic: block, Ethernet-only
  (this PC is on Wi-Fi, so blocked) and Wi-Fi-only each behaved, and an app outside the slice was unaffected.
- **Privacy sandbox** (experimental): bubblewrap, set per app. "Chosen folders" gives the app a tmpfs home with only those
  folders (and its own config/cache/data folders) bound in; "cannot see other programs" gives it its own PID namespace.
  Verified with the calculator: its home showed only the shared folder, and only two processes were visible.
  Also started, with the sandbox on and chosen folders empty: Ghostty (GTK) and Vivaldi (Chromium): both came up and Vivaldi
  opened a window (not driven further). Because the app's own folders are shared, Vivaldi used its normal profile.
- **Power limits** (`/usr/lib/yggdrasil/powerlimit`): /etc/yggdrasil/power-limits.json (read from /etc/labyrinth/ until the first
  change); the root service `yggdrasil-powerlimit.service` (the operating system switches it on) applies the active mode's limits on every mode change (CPU: `scaling_max_freq` as a percent
  of `cpuinfo_max_freq`; GPU: `nvidia-smi -pl`). Verified on the RTX 2080 Ti and the Intel pstate CPU; restored to defaults.
  On the Power page they sit in the Power mode group, under the mode selector, for the mode that is selected: Processor, then
  Graphics card (each with a reset icon left of its slider); choosing another mode shows and changes that mode's limits (my layout, 2026-10-27).
  While the graphics card unlock is armed or measured, `/var/lib/yggdrasil/gpu-unlock/power-hold` exists and no mode goes above
  the card's default watts (the modes' own limits stay saved and come back when the hold ends); `powerlimit get` reports it as
  `gpu.hold` and the slider stops there. Rust since pkgrel 36 (ported 2026-11-09 with its tests and dry-run parity,
  below); `powerlimit watch` is now a part of yggdrasil-power.
- **Graphics card page** (built 2026-10-27, my design; `src/gpu.rs`): the Graphics card row's title opens it. An
  Afterburner-style panel for an NVIDIA card: a live readout (temperature, power against the limit, core and memory clocks,
  load, video memory, fans, and what is holding the clocks back) with the last two minutes as small graphs, each on its own
  fixed scale, the value at a moment on hover; the selected power mode's power limit; and the fans (`/usr/lib/yggdrasil/gpufan`):
  the card's own control, one speed, or a curve edited by dragging its points. The readout is one `nvidia-smi -lms 1000` that
  runs only while the panel is on screen. Nothing on the page overclocks.
  - **Pop-out:** the icon next to the page's title opens the same panel in its own window, `yggdrasil-settings --gpu-window`
    (app id `yggdrasil-gpu`), started as a process of its own, so it stays open after Settings is closed. Only one at a time:
    a second start brings the first forward.
  - **Fans:** `gpufan set` (pkexec, `auth_admin_keep`) saves /etc/yggdrasil/gpu-fan.json and starts or stops the root
    service `yggdrasil-gpufan`, which drives the fans through NVML (no X server needed). Never below the card's lowest
    speed; at 85 °C or more, full speed. A speed comes down only once the card is 3 °C cooler. If the temperature cannot be
    read, the fans go back to the card. Stopping the service, or a crash (`ExecStopPost`), gives the fans back to the card;
    checked on the RTX 2080 Ti with a kill -9. Rust since 2026-11-09 (below). Tests: `packages/power-helpers/src/gpufan/tests.rs`.
  - **Fixtures:** debug builds read `$YGGDRASIL_GPUFAN_FIXTURE` for screenshots of each mode.
- **Cooling: case fans and pumps** (helper and service 2026-11-04; Settings > Power > Cooling 2026-11-05, `src/fans.rs`): `casefan` drives a Corsair
  Commander Pro (kernel driver `corsair_cpro`) as the root service `yggdrasil-casefan`.
  - **The page** (Power > Cooling, or `yggdrasil-settings power:cooling`; `power:fans` also works): the processor's and the card's temperatures,
    whether fan control is on, and for each port its speed now (percent and RPM, or "no speed signal").
    - Each card is headed by its port. Its first line is a text box with the person's own name for what is plugged in
      (up to 60 characters; "Graphics card pump", "Top exhaust"), with the speed now on the right. Kind makes the port a
      pump or a fan, because no two computers are wired alike.
    - A pump gets one slider, 60-100 %.
    - A fan gets Quiet, Balanced (casefan's default) or Cool, or Custom: its own processor and graphics card curves, dragged
      in the same editor as the graphics card's fan curve (`src/curve.rs`, shared).
    - A fan with no speed signal is never set below 35 %, because it cannot be seen stopping.
    - **Follows:** a fan can run on another fan's curves, so one setting drives both (`"follow": "5"` in the setting; casefan
      copies the leader's curves in when it loads it). Both run at the same percent, each at its own share of its top speed.
      Only a fan that follows nobody can be followed (no chains, no loops), and a pump never follows. A fan with no speed
      signal following curves that go below 35 % gets a warning.
    - Connected ports the setting leaves alone are listed, with "Control it as a fan".
    - Apply sends the whole setting to `pkexec casefan set -` on stdin (policy `org.yggdrasil.casefan.set`), which cleans it,
      saves it and restarts the service. The page reads `casefan get` every 3 s only while it is open.

    ![Settings > Power > Cooling, a fan's curves](../assets/screenshots/settings-cooling-curves.png)

  - Each connected port has a role: a pump keeps one speed, never below 60 %; a fan follows two curves, one for the processor's
    package temperature and one for the graphics card's, and runs at whichever asks for more.
  - At 92 °C on the processor or 83 °C on the card every port runs at 100 %. A temperature it cannot read puts every port at
    100 % and ends the service, which systemd starts again.
  - After a clean stop, fans go to 60 % and the pump to 100 %; after a crash (`SERVICE_RESULT` not success), everything goes to
    100 %. The Commander Pro keeps the last speed it was given until it loses power; it does not go back to its own stored
    curves by itself.
  - Setting: `/etc/yggdrasil/case-fans.json`. Rust since 2026-11-09 (below). Tests: `packages/power-helpers/src/casefan/tests.rs`.
  - On the owner's PC: port 1 is the graphics card's pump (about 3,830 RPM whatever it is set to), port 6 the 3,000 RPM Noctua
    exhaust, and port 5 the 80mm Noctua over the cables and RAM, which reports no speed (its own curves, never below 35 %).
- **Security > Limbo** (2026-11-05, `src/limbo_ui.rs`; `yggdrasil-settings security:limbo`): Limbo's sandboxes, written against
  Limbo's protocol (Limbo `docs/SETTINGS_PAGE.md`, "Protocol") through the `limbo` command, so the page never speaks HTTP.
  - Shown without a password (`limbo settings show --json` over `user.sock`, for members of `limbo-users`): Limbo's version,
    whether KVM is available, the space used (base images, the guests' own changes, evidence), each guest (system, running,
    stopped or held, processors, memory, network, its own changes, owner), the runs and hand-started guests, and the Windows
    licenses (masked).
  - **Ask for my password for Limbo changes:** the switch (`pkexec limbo settings password on|off`).
  - **Start, Stop, Free space** on the person's own guests: run as the person; when Limbo answers `password_required` the
    same command runs again through pkexec. A held guest has no buttons; its row says "held by Heimdall".
  - **Network** of each own guest (root's choice, so always through pkexec; from its next start): No network, Internet only,
    or Listed addresses. A guest on listed addresses shows each one (address, protocol, port, from the effective `allow` list
    Limbo gives) with Remove, and a field to add one (`ADDRESS:tcp|udp:PORT`, IPv6 and `/BITS` too). `--allow` replaces the
    whole list, so every add or remove sends all of it (`limbo instance set NAME --network allowlist --allow A,B,...`); the
    last address cannot be removed (No network closes it instead). Choosing Listed addresses asks for the first address and
    sends it with the mode.
  - **Windows licenses:** add (name, and the key on stdin, never a command line) and remove, through pkexec.
  - **Save a report…**: `pkexec limbo report --since 7d`, with `--plain` (the timeline in plain words, one sentence an event)
    when this Limbo's `limbo report -h` lists it; kept as Markdown in Documents and opened.
  - Quarantine decisions are not here: they are Heimdall's (Security > the monitor's page).
  - A Limbo from before this protocol is named as such ("older than this page"); one that does not run, or a person not in
    `limbo-users`, gets a plain line saying what to do. Read every 5 s only while the page is open.
- **Processor page** (built 2026-10-27, my design; `src/cpu.rs`): the Processor row's title opens it, like the Graphics
  card's, with its own pop-out window (`--cpu-window`, app id `yggdrasil-cpu`).
  - **Readout:** package temperature and the hottest core, clocks (average and fastest), load and package power, with the
    same graphs (they fill their box from the first seconds, then cover the last two minutes). Read from sysfs and /proc once a second while the panel is shown; the power comes from the
    tuning service, because the energy counter is root-only.
  - **Limits:** the selected mode's speed cap (powerlimit), turbo, and the long- and short-term power limits (PL1/PL2,
    35-250 W, with the BIOS's values as the reset).
  - **Multipliers** (a K processor): all cores at once, or by how many cores are busy; never higher with more cores busy,
    up to 3 above the processor's own top.
- **Tuning and its crash guard** (`/usr/lib/yggdrasil/tune`, root service `yggdrasil-tune`; `src/tune.rs`): the processor's
  multipliers (MSR 0x1AD) and the graphics card's clocks (NVML offsets: Afterburner's Core and Mem, and a clock cap that,
  with an offset, undervolts). NVIDIA gives Linux no way to set the card's voltage.
  - **Try, Keep, Undo:** `try` applies values for this session; `keep` makes them start with the computer; Undo and
    Defaults go back.
  - **The guard:** a marker with the boot id exists exactly while something is tuned, and only a clean shutdown removes it.
    A start that finds it from an earlier start (a freeze, a reset, a power cut while tuned) applies nothing and says why,
    until "Turn back on". An NVIDIA Xid in the kernel log while the card's clocks are tuned puts them back at once.
  - **What is guarded:** power limits and turbo are kept in /etc/yggdrasil/tuning.json and applied at every start
    unguarded, since they cannot freeze anything. The processor's own values are read once per start of the computer.
  - **Checked on the i9-9900K and the 2080 Ti** with lower values only (multipliers down one, core -50 MHz) and back.
    Rust since 2026-11-09 (below). Tests: `packages/power-helpers/src/tune/tests.rs`.
- **tune, casefan, gpufan and powerlimit in Rust** (2026-11-09, `packages/power-helpers`): the four root helpers above run
  all the time (gpufan while a fan curve is on) and were Python on a Rust desktop. The port keeps their interface so Settings
  and the units do not change: commands, arguments, environment variables, JSON (byte for byte), settings and state files,
  exit codes, log lines, and every safety rule. The other helpers (account, bootscreen, gpuunlock, game-fix, gpu-run) stay
  Python for now.
  - **Status:** the package installs the Rust tune, casefan, gpufan and powerlimit at the same `/usr/lib/yggdrasil/` paths
    (tune, casefan and gpufan since pkgrel 35, powerlimit since 36, with the merged service below). The Python scripts and
    their tests were kept in `helpers/python/` for a rollback until 2026-11-15 (git has them); the packages before
    (pkgrel 35 for the merged service, 34 for the Python helpers) are the way back.
  - **Light:** no crate but libc, no async runtime, one thread each; libnvidia-ml is opened at run time (dlopen), so
    without the NVIDIA driver they run and say so. `tune watch` reads the kernel's messages from `/dev/kmsg` instead of a
    `journalctl -k -f` child (journalctl only when /dev/kmsg cannot be opened; the service logs which one it reads).
  - **Memory, measured 2026-11-09:** each Rust helper is about 2.8 MB on its own; `nvmlInit` adds about 26 MB in any
    process (the NVIDIA 620 driver maps libcuda and starts a thread), so with the card open each is about 29 MB. The Python
    ones showed 16.5 MB (casefan) and 19 MB (tune) resident but had about 23 MB each swapped to zram, so about 40 MB each,
    and tune's journalctl child held another 46 MB: about 88 MB for tune, about 29 MB in Rust.
  - **Dry run:** every command takes `--dry-run` (each write is printed as `dry run: ...`, nothing is written: no sysfs, MSR,
    NVML set call, file, systemctl or logger), and with it `--root DIR` (every system path under DIR, the card from
    DIR/nvml.json: no hardware at all) and `--steps N` (N rounds of a service's loop on a virtual clock).
  - **Parity:** the 44 cases of the Python tests are Rust tests with the same names and expectations, and powerlimit's six
    (`tests/test_settings_powerlimit.py`, and `test_gpuunlock.py`'s PowerHold, which tests powerlimit's hold).
    `parity/compare.py` runs the Rust helpers and the Python scripts (through `parity/py_dryrun.py`, which replaces only
    their outside) in dry runs on the same fake trees: the tests' fixtures, more cases around them, and samples of a real
    computer from `parity/record.py` (read-only), plus casefan's loop over 1,216 temperature sequences. In a fake tree the
    commands that only ask (busctl for the power mode, `nvidia-smi --query-gpu`) answer from `ROOT/commands.json`, which
    record.py fills from this computer's answers. On 2026-11-09, with a sample of my PC: 120 of 120 cases (37 of them
    powerlimit's) and all sequences the same.
  - **Different on purpose:** a setting whose "ports" is not an object crashed the Python casefan, leaving the ports at
    their last speed (its `reset` crashed too); the Rust one treats it as a failure: every port at 100 %, after a clean stop
    too, since which port is the pump cannot be told. The help text adds the dry-run paragraph. Python tracebacks become one
    "helper: message" line, still exit code 1. `tune watch` logs where it reads the kernel's messages from. powerlimit: an
    infinite number from nvidia-smi (Python crashed on `round(inf)`) counts as no answer, like "[N/A]".
- **One service, yggdrasil-power** (2026-11-09, `packages/power-helpers/src/power.rs`, `/usr/lib/yggdrasil/power`):
  `nvmlInit` costs every process about 26 MB, and three always-on services paid it separately (tune, casefan, and gpufan
  with a curve). yggdrasil-power runs the loops of tune watch, casefan run, gpufan run and powerlimit watch in one process,
  one thread each (no async runtime), with the card opened once and shared (NVML is thread-safe; it is shut down when no
  part holds it).
  - **The old units are switches:** yggdrasil-tune, -casefan, -gpufan and -powerlimit stay, each a oneshot unit that
    requires yggdrasil-power: its start runs `power on PART` (a token in `/run/yggdrasil/power/on/PART`, and it waits until
    the service has the part), its stop `power off PART` (it waits for the part's clean stop). So `systemctl enable`,
    `disable`, `start`, `stop`, `is-active` and the restart that `casefan set` and `gpufan set` run work by the old names, a
    restart of yggdrasil-casefan is still a clean stop of the fans then a fresh read of the setting, and it touches no other
    part (an alias of yggdrasil-power would have restarted tune too, which clears a trial, and `gpufan set auto`'s
    `disable --now` would have stopped everything). yggdrasil-power has no [Install]; the switches pull it in, and it stops
    when none is on. Settings, the polkit rules and the installer (which enables yggdrasil-powerlimit) do not change.
  - **Requires, not BindsTo** (checked 2026-11-09 with stand-in units in a user manager): with BindsTo a crash of the service
    stopped the switches before its ExecStopPost ran, so that saw no part switched on (casefan's failure stop would have been
    missed), and the restart found nothing to run. With Requires a crash or a kill -9 leaves the switches on for
    ExecStopPost, then systemd restarts them with the service; restarting one switch leaves the service and the others
    alone; stopping the last one stops the service; at the service's start limit the switches end inactive, so `is-active`
    tells the truth.
  - **Upgrading:** the package's pacman hook (`90-yggdrasil-power.hook`, after systemd's reload) runs `power adopt`, which
    restarts each of the four units that still runs its old helper process, so an upgraded system moves into the one service
    at once instead of at its next restart.
  - **Each part's safety stays its own:** a part's loop that fails makes its old ExecStopPost's stop on its thread (casefan:
    every port 100 %; gpufan: fans back to the card) and starts again after its old RestartSec, up to its old start limit
    (casefan and gpufan 10 in 120 s; then it stays at its failure stop and its unit is stopped, so `is-active` says so); the
    other parts go on. A panic is caught on the part's thread, which makes its failure stop; every other part makes its
    failure stop too and the service ends with exit code 1. If casefan's loop stops moving for 20 s (stuck in NVML or a
    driver), the supervisor, which never calls NVML, puts every Commander Pro port at 100 % through sysfs and ends the
    service with exit code 1. Whatever way the service ends, ExecStopPost (`power stopped`) makes the stop of each part still
    switched on from a new process, and systemd starts the service again after 5 s. Without NVML, casefan cannot read the
    card, so its ports go to 100 % as before, while tune and powerlimit go on.
  - **Parity:** `compare.py` also runs the merged service's dry run (each line it prints starts with `[part]`) and checks,
    part by part, the same lines in the same order and the same exit code as each helper run on its own followed by its old
    ExecStopPost (`casefan reset`, `gpufan reset`): 17 of 17 cases the same on 2026-11-09 (the tests' trees, a card whose
    readings run out, no driver, a failing init, no Commander Pro, gpufan on auto, the tuning guard, an Xid, mode changes,
    parts switched off, and my PC's sample). The supervisor's isolation rules have their own tests (`src/power/tests.rs`).
  - **Memory, measured 2026-11-09** (dry runs on a sample of my PC with the real card loaded, `--real-card`): the merged
    service 29.6 MB resident (24.0 MB proportional), six threads; on their own tune 29.1, casefan 28.7, gpufan 28.8 and the
    Rust powerlimit 2.7 MB, 89.2 MB together. Running on my PC before the switch: tune 29.8, casefan 29.4 and the Python
    powerlimit 11.5 MB (plus 2.9 MB in swap), and gpufan another 29 MB while a curve is on. CPU is noise either way
    (about 0.1 % of a core for all four).
  - **Status:** the package installs it (pkgrel 36): `/usr/lib/yggdrasil/power`, yggdrasil-power.service, the four units as
    its switches, and the Rust powerlimit. Kept on the branch `power-service-switch` (the daily publish builds main) until it
    has run on my PC. To go back: reinstall pkgrel 35, then start the four units again (they are separate services again).
- **Graphics card unlock** (`/usr/lib/yggdrasil/gpuunlock`, built 2026-10-27 as Phase 0 of Labyrinth OS's
  [GPU_UNLOCK.md](https://github.com/Artifex-Technologies/Labyrinth_OS/blob/main/docs/GPU_UNLOCK.md)): a "Tensor-rate unlock"
  row under the Graphics card row, only with Developer options on and only for a card listed exactly (device, board, VBIOS, driver series)
  in `/usr/share/yggdrasil/gpu-unlock/supported.json`; any other card gets no row at all, and `gpuunlock status --json` keeps
  the reason.
  - **Method:** since 2026-10-27, nlzy/turing-unlock's kernel module, which loads before the NVIDIA driver and acts only with
    `turing_unlock.targets=` on the command line.
  - **Arming:** asks for the password every time (polkit `auth_admin`), needs the module, its firmware and its mkinitcpio
    hook installed and a visible boot menu, takes a snapshot, and holds the power.
  - **The one-try entry:** arming adds "Labyrinth Linux, graphics card unlock (one try)" to the boot menu. It is a copy of
    the first entry with the option, printed by `/etc/grub.d/41_gpu_unlock` only while armed. The owner picks it by hand at
    a restart; it is never the default.
  - **Following an attempt:** `yggdrasil-gpu-unlock-boot.service` and `-verify.service` follow it. The attempt is known by
    the option in /proc/cmdline, the result by the module's kernel log, and the entry goes away after its one try. The helper is Rust since 2026-11-13 (`packages/settings-helpers`, `gpuunlock`; the fragment calls `gpuunlock grub-entry`). Tests: `packages/settings-helpers/src/gpuunlock_tests.rs`, and `parity/gpuunlock.py` against the Python script, taken from git (`ff5b6de`, the commit before the switch; the copy kept in `helpers/python/` with its tests went on 2026-11-15). A debug build reads the status from
  `$YGGDRASIL_GPUUNLOCK_FIXTURE` (for screenshots of the other states).

## Ask before connecting (built 2026-10-16)

An app's page has "Ask before connecting to a new place". In that mode (`labyrinth-appnet set APP ask`) the app may use loopback and
the destinations you have allowed; any other connection is dropped by nftables after one rate-limited log line (with the user id).
Three small pieces turn that into a question:

- `labyrinth-appnet-ask` (root service `labyrinth-appnet-ask.service`) reads those kernel-log lines and makes ONE question per app and
  destination address (not per connection). Allow adds the address to the app's nftables set at once (the app's next retry connects,
  and TCP retries by itself within a second or so) and remembers it in `/var/lib/labyrinth/app-ask.json` (root only, 0600). Deny is
  remembered and never asked again. "Allow all" puts the app back to any connection. No answer: asked again after a minute.
- `labyrinth-appnet-ask-agent` (user service) shows each question as a notification with Allow / Allow all / Deny, with the host name
  when a reverse lookup answers quickly.
- Settings lists "Places you decided about" with Forget (asked again next time). It talks to the service through
  `/run/labyrinth/appnet-ask.sock`, so changing a decision needs no password; the kernel tells the service whose apps are whose.

Only apps you set to Ask are affected, so the package manager, time sync and Heimdall are never asked about (the roadmap's "do not
repeat Vipre's over-protectiveness"). Honest limits: an address, not a name (a big content network can look like many addresses);
your own programs could answer the questions too (OpenSnitch and Little Snitch share this); a connection waits for your answer, so
apps may show a short "connecting" delay the first time. Not built: names from DNS or SNI, incoming connections, a default of Ask for
new apps.

Tests: `python tests/test_labyrinth_appnet_ask.py` (rules, question engine, the real socket service with a fake kernel log and a fake
notify-send); `unshare -Urn bash tests/appnet_ask/netns_test.sh` (real nftables: dropped, allowed, allowed at run time, in a throw-away
namespace); `sudo bash tests/appnet_ask/kernel_log_check.sh` (the real kernel log line against the parser: NOT run yet, it needs root).

## Limits (be honest about them)

- Network rules cover apps started through labyrinth-launch (dock, start menu, Settings). An app started another way, or one
  that was already running, is not in its slice until restarted. Single-instance apps already running elsewhere (a browser)
  need a full restart.
- The sandbox does not cover Flatpak apps (they have their own); Location is not controlled yet.
- "Usage" is the time an app has been running (`labyrinth-appusage`, a user service from packages/appusage that follows each app's scopes in the cgroup tree and counts from an app's first scope to its last; apps running before it started are counted from then; it writes when an app stops and every five minutes while one runs), then launches. The sizes are the app's folders in your home, not the package.
- The Optional software tab and the start of removing packages use pkexec; the live image has a passwordless polkit rule, so
  prompts only appear on an installed system.

## Known limitations

- Flatpak apps: no per-app Location or "which other apps it can see" here (Flatpak has its own permissions).
- No network usage per app (the running time is counted).
- The dock has no right-click "App settings" entry; `yggdrasil-settings apps:ID` opens an app's page.
- Software Update: the updates count as validated only when an assistant writes the `update_verdict` file (docs/INTEGRATIONS.md);
  there are no advanced options (check automatically, update source, AUR updates).
- The network kind ("This network is") is your choice only: Heimdall's suggestion is not shown, and a new network that cannot
  be classified is not asked about.
