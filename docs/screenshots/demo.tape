# The screenshots of the README. Run from the repository root: vhs docs/screenshots/demo.tape
#
# Needs clicloud, yt-dlp and mpv on PATH, a network, and a library whose first favorite
# is in the cache, so that it starts at once. The settings they were taken with:
# "theme": "Black Metal (Khold)" (a theme of Ghostty), "cover": "braille",
# "backdrop": "none", "search_limit": 15. Give mpv --ao=null to record in silence.
#
# The other pictures are taken the same way, with other settings and keys:
#   library.png       "backdrop": "reaper", Height 1080; keys 2, Enter
#   light.png         "theme": "Rose Pine Dawn", "language": "ru", "backdrop": "pentagram",
#                     background #faf4ed, Height 1080
Output target/demo.gif
Set Shell bash
Set FontFamily "Hack"
Set FontSize 17
Set Width 1500
Set Height 860
Set Padding 20
Set Theme { "name": "black", "background": "#000000", "foreground": "#c1c1c1", "black": "#000000", "red": "#5f8787", "green": "#dd9999", "yellow": "#a06666", "blue": "#888888", "magenta": "#999999", "cyan": "#aaaaaa", "white": "#c1c1c1", "brightBlack": "#333333", "brightRed": "#5f8787", "brightGreen": "#dd9999", "brightYellow": "#a06666", "brightBlue": "#888888", "brightMagenta": "#999999", "brightCyan": "#aaaaaa", "brightWhite": "#c1c1c1", "cursor": "#c1c1c1", "selection": "#333333" }
Env COLORTERM "truecolor"

# Play the first favorite, then search; the list fills as the tracks are found.
Hide
Type "clear && clicloud" Enter
Sleep 2s
Type "2"
Sleep 1s
Enter
Sleep 2s
Type "/"
Type "funeral"
Enter
Wait+Screen@150s /can it get better/
Sleep 15s
Show
Screenshot docs/screenshots/lists.png
Sleep 1s

# The tab of the track.
Type "t"
Sleep 3s
Screenshot docs/screenshots/track.png
Sleep 1s

# The list of color schemes, thirty below the one in use.
Escape
Type "o"
Sleep 1s
Enter
Sleep 1s
PageDown 3
Sleep 1s
Screenshot docs/screenshots/themes.png
Sleep 1s
Escape
Escape
Hide
Type "q"
Sleep 1s
