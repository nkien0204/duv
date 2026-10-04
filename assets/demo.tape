# Records the README demo (assets/demo.gif) and screenshots with VHS
# (https://github.com/charmbracelet/vhs), on a made-up folder from
# assets/demo/make_demo.py so no real files appear.
#
# From the repository root:
#
#   cargo build --release
#   vhs assets/demo.tape
#
# Nothing is deleted: the delete confirmation is opened and cancelled.

Output assets/demo.gif

Set Shell "bash"
Set FontSize 18
Set Width 1100
Set Height 640
Set Padding 16
Set Theme "Catppuccin Mocha"
Set WindowBar Colorful
Set TypingSpeed 80ms

# Setup, not recorded: use this build of duv and a fresh demo folder.
Hide
Type `export PATH="$PWD/target/release:$PATH" PS1='$ ' && cd "$(python3 assets/demo/make_demo.py)" && clear`
Enter
Sleep 2s
Show

# Scan the folder: the largest entries come first.
Type "duv ."
Sleep 500ms
Enter
Sleep 2.5s

# Sort by name, then by modified time, then back to size.
Type "t"
Sleep 1.5s
Type "t"
Sleep 1s
Screenshot assets/sort.png
Sleep 1s
Type "t"
Sleep 1.2s

# Filter by name as you type, then open the match.
Type "/"
Sleep 400ms
Type "do"
Sleep 1s
Screenshot assets/filter.png
Sleep 600ms
Enter
Sleep 500ms
Type "l"
Sleep 1.5s

# Ask to move the largest download to the Trash, then cancel.
Type "d"
Sleep 1.5s
Screenshot assets/delete.png
Sleep 1.2s
Escape
Sleep 800ms

# Back up, then show every key.
Type "h"
Sleep 1s
Type "?"
Sleep 2s
Screenshot assets/help.png
Sleep 1s
Escape
Sleep 600ms

# Quit: q, pick Yes, Enter.
Type "q"
Sleep 800ms
Type "h"
Sleep 500ms
Enter
Sleep 800ms
