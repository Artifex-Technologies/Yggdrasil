# yggdrasil-colorpicker

Pick a colour anywhere on the screen and copy it, as PowerToys' Color Picker does. **Win + Shift + C** (Settings > Keyboard
Shortcuts > Color Picker also has a "Pick a colour" button).

The screens are captured the moment it starts and shown frozen, full-screen, so menus and tooltips stay put while you choose. A loupe
next to the pointer shows the pixels around it, the one in the middle framed, with its colour and hex value.

| Input | Does |
| --- | --- |
| move the pointer, or the arrow keys (one pixel each) | choose the pixel |
| wheel | show fewer or more pixels in the loupe |
| click or Enter | pick: copied, kept in the history, shown in a notification |
| Esc or right-click | cancel |

The colour is copied in the format chosen in Settings: HEX `#7C54D6`, RGB `rgb(124, 84, 214)`, HSL `hsl(258, 61%, 58%)`, HSV
`hsv(258, 61%, 84%)`, CMYK `cmyk(42%, 61%, 0%, 16%)` or Float `(0.49f, 0.33f, 0.84f, 1f)`. The last 20 picks are kept in
`~/.local/state/yggdrasil/colors.txt` (Settings shows them; a click copies one again); the format is `color_format` in
`~/.config/yggdrasil/tools.conf`. The history of Labyrinth's earlier picker (`~/.local/state/labyrinth/colors.txt`) and its format
setting are read when there is nothing newer.

Runs only while picking; nothing stays in the background. Needs a compositor with wlr-layer-shell, and `grim` to capture the
screens (the same way `yggdrasil-shot` does).

    yggdrasil-colorpicker                 pick
    yggdrasil-colorpicker --print         and print what was copied
    yggdrasil-colorpicker --format hsl    this once in another format
    yggdrasil-colorpicker --timeout 5     give up after 5 seconds (tests: the picker takes the keyboard)

The crate is also a small library (`default-features = false`): the formats, the history and the chosen format, which Settings uses.

Licence: MPL-2.0, like the rest of Yggdrasil. The behaviour follows Microsoft PowerToys' Color Picker (MIT); no code is taken from it.
