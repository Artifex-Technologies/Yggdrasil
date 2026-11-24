# yggdrasil-share-picker

The question before a screen is shared. yggdrasil-portal starts it when a program asks to share the screen (ScreenCast, on
Yggdrasil's own compositor: [packages/screencast](../screencast/README.md)).

    yggdrasil-share-picker [--app APP_ID] [--multiple]

It lists the screens (from whichever compositor runs, through yggdrasil-wm), each with its maker and model, its name and its
size. The person picks one, or several when the program asked for several (`--multiple`); a lone screen is picked from the
start, but nothing is shared until Share. Share prints the chosen screens' names, one a line, and exits 0; Cancel, Escape or
closing the window prints nothing and exits 1. Keys: 1 to 9 pick a screen, Enter shares, Escape cancels.

The program is named by its menu entry (`Name=` in `APP_ID.desktop`, the person's own entries first) when the portal knows its
app id; otherwise the question says "A program". It never guesses who is asking from the window in front.

It looks like the password box (yggdrasil-polkit): the same palette, accent and accessibility settings (`packages/yggdrasil-ui`).

`cargo test`: the options, the program's name (an entry found, none, an id that is not one), the screens' labels (a turned
screen's size turned too) and picking one screen or several.
