# clicloud

English | [Русский](README.ru.md)

A terminal client for SoundCloud, written in Rust: search, listen, keep a
library. `yt-dlp` does the searching and fetches the audio, `mpv` plays it.
Tracks you have played stay in a cache on disk: the second time they start at
once and play without a network.

The interface speaks English, Russian and Japanese. English is the default on
first launch. Change Language in settings (`o`), or set `"language": "ru"` or
`"language": "ja"` in the settings file. An existing language choice is preserved.

## Installation

You need Rust/Cargo 1.88+ (edition 2024), `yt-dlp` no older than 2023.01 (the
search uses `--lazy-playlist`) and `mpv` with its built-in `ytdl_hook` (the
usual build with Lua).

Install `yt-dlp` with pip, pipx or uv rather than as the single file from
GitHub: the PyInstaller build unpacks itself on every start and takes about
1.6 seconds against 0.4 for a packaged install, and it starts on every search.

Manjaro/Arch Linux:

```sh
sudo pacman -S mpv yt-dlp
```

Ubuntu/Debian:

```sh
sudo apt install mpv pipx
pipx install yt-dlp
pipx ensurepath
```

Open a new terminal after PATH has changed. Build the client:

```sh
cargo build --release
./target/release/clicloud doctor
```

The binary is `target/release/clicloud`. To put it on PATH:

```sh
cargo install --path . --locked
```

For a yt-dlp of the project's own, without touching the system Python, `uv`
will do:

```sh
uv venv .tools/venv
uv pip install --python .tools/venv/bin/python yt-dlp
export CLICLOUD_YT_DLP="$PWD/.tools/venv/bin/yt-dlp"
```

The `.tools` directory is ignored by Git. mpv is installed separately. A binary
run from the project's `target` directory (`cargo run`,
`./target/release/clicloud`) finds `.tools/venv/bin/yt-dlp` of that project by
itself when there is no `yt-dlp` on PATH. The current directory does not matter
and is never searched. One installed with `cargo install` needs `yt-dlp` on
PATH, `--yt-dlp` or `CLICLOUD_YT_DLP`. An explicit `--yt-dlp` or
`CLICLOUD_YT_DLP` outranks what is found automatically.

### When the programs are missing

When the interface finds no `yt-dlp` or no `mpv`, it opens a window named
PROGRAMS.

yt-dlp is something the client can fetch itself: on `Enter` it downloads the
file of the latest release with `curl`, through the proxy that is set, checks
its SHA-256 against the sum published in that release, and only then puts it
next to the library, in `~/.local/share/clicloud/bin`. From then on it finds it
there by itself; nothing is written into the system and no administrator rights
are needed. The sum guards against a download that was damaged or replaced on
its way, not against a compromised release: it comes from the same release.

mpv cannot be had that way. It is a system package with libraries and codecs
that only a package manager lays out, so the window shows the command for your
system, and running it is up to you. `Esc` closes the window and leaves things
as they are.

## Usage

### The terminal interface

```sh
cargo run --
cargo run -- --tor
cargo run -- ui --library ./my-library.json
```

Started without a command, the client opens a TUI built on Ratatui (Linux,
macOS and Windows, at least 80×24). Frames, buttons and marks are ASCII, and
the colors come from a scheme: by default the interface follows the colors of
the terminal, and other schemes are chosen in the settings (`o`). On a wide
screen the queue and the tracks played last are shown on the right. On a screen
taller than 24 lines the header draws the logo instead of spelling the name. It
is drawn in Braille dots, one of the two things in the interface that are not
ASCII; the Linux console, which has no such letters, keeps the spelled name.
The drawing itself is in `logo.txt`.

The main screen has two tabs in the right corner of the header, as a browser
has them at its top: Lists, which is the search, the library, the queue and the
history, and Track, which is what plays now. `t` goes from one to the other,
and so does a click; `Esc`, the keys `1`-`4` and `/` go back to the lists. The
tab of the track has the whole main screen: the cover on the left, beside it
the title, the author, the length and the marks the track has in a list (`*` a
favorite, `v` stored), below them the genre, the date, how often it was played,
its likes, reposts, comments and tags, and on a wide screen a column of its own
for what the author wrote of it; `↑`/`↓` and `PgUp`/`PgDn` move through that.
The keys of a track are about the one that plays here: `f` makes it a favorite,
`d` stores it, `m` opens tracks like it, `u` its author. The player stays below.

yt-dlp tells all of that by the way when it fetches a track, or in one request
while the tab is open; it is kept in the cache, in `info/`. The cover is drawn
in the cells of the terminal in one of six ways, chosen in the settings
(`cover` in the file):

- **colored blocks** (`blocks`, the default): half blocks, two pixels to a
  cell, each with its own color. The cover is easiest to recognize this way;
- **colored Braille** (`braille`): Braille dots, eight to a cell, the cell in
  the color of what its dots stand for. Finer, but darker;
- **blocks in scheme colors** (`block-scheme`) and **Braille in scheme colors**
  (`braille-scheme`): the cover made of the colors of the scheme itself. Each
  point takes the nearest of them and what it is off by goes to its neighbors,
  so a shade is a mix of the two or three colors of the scheme it lies between.
  Under the `terminal` scheme, which has no colors of its own, the cover keeps
  its own;
- **blocks in tones** (`block-tones`) and **Braille in tones**
  (`braille-tones`): the same in one tone, from the background of the scheme to
  the color of its text;
- **none** (`none`): the tab without a cover, and nothing is fetched for it.

A terminal with 24-bit color gets the colors as they are, any other the nearest
of the 256. Under the `mono` scheme, and wherever colors are switched off, the
picture is made of marks alone: a dot or a half is there or it is not. The
cover is the second thing in the interface after the logo that is not ASCII,
and the only one colored by anything but the scheme.

The cover is fetched once, while the tab of the track is open: its address
comes with the rest that is known of the track, `curl` downloads a 300×300
picture through the same proxy, and mpv turns it into pixels. After that it
lies in the cache beside the audio, or in memory until you quit when there is
no cache. At 80×24 the cover has 18×9 cells; it grows with the screen.

Buttons are pressed with the mouse or with `Tab` / `Shift+Tab` and `Enter`.

| Key | What it does |
|---|---|
| `/`, then `Enter` | Type and send a search, or a SoundCloud link |
| `Esc` | Cancel the search that runs |
| `1`, `2`, `3`, `4` | Search, library, queue, history |
| `t` | The tab of the track instead of the lists, and back: the cover and all that is known of the track |
| `o` | Settings and color schemes |
| `↑` / `↓`, `j` / `k` | Select a track |
| `PgUp` / `PgDn` | Move through the list by a screen |
| `Home` / `End`, `g` / `G` | To the top / the end of the list |
| `Ctrl+F` | Filter the library by words of the title and the artist; `Esc` clears |
| `Enter` | Play the selected track, and the rest of its list after it / press the focused button |
| `m` | Tracks like the selected one: its SoundCloud station |
| `u` | The page of the author of the selected track; `[` / `]` go through its sections |
| `b` | Back to the list that was open before |
| `F` | Add every track of the list to the favorites |
| `z` | Play a list in order or shuffled |
| `r` | Repeat: off, the whole list, one track |
| `Space` | Pause / go on |
| `f` | Add to the favorites or remove from them |
| `a` | Add the selected track to the queue |
| `d` / `D` | Store the selected track / every track of the list in the cache, without playing them |
| `x` | Remove the selected track from the cache |
| `Delete` | Take the selected track out of the queue |
| `p` / `n` | The track before / the next one |
| `←` / `→` | Seek by the step from the settings (10 seconds) |
| `,` / `.` | Slower / faster by 0.1, from x0.5 to x2.0 |
| `−` / `+` | Volume |
| `s`, `q`, `?` | Stop, quit, help |
| `e` | The full text of the last error of a search or of playback; ↑↓ scroll, Esc closes |

A search runs in the background, and the list fills as yt-dlp finds the tracks
rather than after it has found them all: the first track shows many times
sooner than the last, because each costs yt-dlp about a second. That is why 10
tracks are asked for by default, which can be changed (`search_limit`). A
question that was asked before in this run is answered at once from memory; to
refresh its results, send the same question again. On the very first run, while
the cache of yt-dlp is empty, the client makes one request to SoundCloud ahead
of time: yt-dlp learns the `client_id` while you type, and the first search
does not wait an extra round, which could make it several times longer than
the others. With `--no-cache`, and on every later run, that request is not
made.

`Enter` starts a track, and the rest of its list plays after it, whether that
is the results of a search, the library or the history. What you put in the
queue yourself with `a` comes first, then the rest of the list; the queue (`3`)
shows both, and `Delete` takes a track out of either. `z` plays in a shuffled
order: what is ahead is mixed at once, and a track started while it is on has
all the others of its list after it. `r` goes round the ways to repeat: off,
the whole list, one track. Both choices show in the frame of the player and are
kept in the settings file (`shuffle`, `repeat`: `off`, `all` or `one`).

A SoundCloud link can be pasted where a search is typed, and it opens as a
list: a playlist or an album, a page of an author (`/name/tracks`, `albums`,
`sets`, `reposts`, `likes`), the station of a track. The keys do the same: `m`
opens the station of the selected track, which is that track and what
SoundCloud plays after it, `u` the tracks of its author, and `[` and `]` go
through the sections of the author: tracks, albums, playlists, reposts, likes.
A playlist among tracks is marked `>>` where the length would be, and `Enter`
on it opens it instead of playing it. `b` goes back to the list before without
asking again, and `F` adds every track of the open list to the favorites, which
is how playlists and reposts are brought over. At most 500 tracks are taken of
a list.

A page of an author and a station tell the title and the link of a track, so
the artist is at first read from the link and there is no length, as with
likes; both are put right when the track is first fetched. A playlist does not
tell the links and titles of all its tracks, so yt-dlp looks each one up: the
list fills at about a track a second, but with the artist and the length from
the start. A length of 0:30 means that SoundCloud hands out a preview only.

Only the rows that show are drawn of a long list, so a library of thousands of
tracks moves as fast as a short one; the lower right corner of the frame says
where the selection stands and how many there are. `Ctrl+F` narrows the library
to the tracks that have every word typed in their title or artist: `Enter`
keeps the filter, `Esc` clears it, and `Enter` on a track plays on through the
narrowed list.

While a track plays and all of it has arrived, the client fetches the one that
follows into the cache ahead of time, and once it is there the player itself
goes on to it without stopping: no new mpv is started and there is no gap
between the tracks. Everything that is already in the cache plays on like that.
Without a cache (`--no-cache`), when one track is repeated, and when the next
track has not arrived in time, it starts as before, in a player of its own. On
the bar of a track that is still arriving `~` shows how far it has come; `,`
and `.` change the speed for the rest of the run, and it stands in the frame of
the player while it is not the usual one.

The next track starts by itself when the current one ends. A track that cannot
be played is skipped and the next one starts; after three failures in a row the
queue stops, so that a dead network does not drain it. Pause, progress and
volume come from mpv over a local connection: a Unix socket, or a named pipe on
Windows. While a track is being fetched, seeking is limited to what has
arrived, and the length is the one the search told. When playback or storing
fails, the window that `e` opens shows the error messages of mpv and the output
of yt-dlp. On SIGTERM, SIGHUP and SIGINT, and when the window of its terminal
is closed, the interface on Unix ends in an orderly way: it stops mpv and
yt-dlp and removes the temporary directory with the socket and the downloads
that were not finished.

The library (`2`) is all that you have kept: the favorites first, then the rest
of the tracks in the cache. It is a local list, not synchronized with a
SoundCloud account; the likes of a profile are brought into it by the line
SoundCloud likes of the settings or by the `import` command. `f` removes a
track from the favorites, `x` from the cache; a track that is in neither
disappears from the library. The library file may be changed by another process
too, the `import` command or a second interface: an open interface notices it
within a couple of seconds and before every save, adds the tracks that are new
there to its own and removes those that were removed there. The favorites and
the last 30 tracks played are kept in `$XDG_DATA_HOME/clicloud/library.json` or
`~/.local/share/clicloud/library.json`. The history opens with `4`, and its
tracks can be played again. What played and what was ahead of it, the queue and
the rest of the list, up to 500 tracks, is kept there too when you quit: the
next time it waits in the queue, and `n` starts it from the track it stopped
at. The path can be changed with `ui --library`. A damaged library file is not
overwritten, including if it becomes damaged while the interface is open. Updates
from TUI sessions and imports share a lock beside the JSON file (`library.json.lock`);
keep that file in place. A failed save on exit returns an error. Older client
versions and external editors do not participate in this locking protocol.

A track that could not be played is marked `!!` in place of its number for the
rest of the run, or until it plays: the network may have been the reason.

In the last column of a list `v` marks a track that is in the cache and `~` one
that is being fetched. More of that under "Cache and offline".

### Plain commands

```sh
cargo run -- search "burial" --limit 10
cargo run -- search "ambient" --json
cargo run -- search "https://soundcloud.com/artist/sets/album" --limit 50
cargo run -- play "burial"
cargo run -- play "ambient" --first
cargo run -- play "https://soundcloud.com/artist/track"
cargo run -- import my-profile
cargo run -- cache
cargo run -- doctor
```

`play` with a question shows a list and asks for a number. `q` or EOF cancels
the choice. In scripts use `--first` or a link. `search --json` prints an array
of objects with `title`, `artist`, `duration` (seconds or null) and `url`;
messages go to stderr. `--limit` takes 1–50. A link in place of a question is
opened as a list: a playlist, a page of an author, the station of a track.

While it plays: **space** pauses, **←/→** seek, **9/0** change the volume, **q**
quits. A link to a playlist is handed to mpv whole; a search returns single
tracks. On SIGTERM, SIGHUP or SIGINT `play` asks mpv to end, stops yt-dlp and
exits with 128 plus the number of the signal, rather than leaving the player
playing in the background.

The help of the command line (`--help`) is in English only.

### Bringing over the likes of a SoundCloud profile

In the interface this is the last line of the settings (`o`), SoundCloud likes:
`Enter` opens a field for the name of a profile or a link to it, and a second
`Enter` starts. The list is read in the background: the window can be closed
and the music goes on. The line shows how many tracks have arrived, and at the
end how many were added and how many were there already; `Enter` on it while
the list is being read stops it, and the favorites stay as they were. The name
of the profile is remembered in the settings, so the next time two presses of
`Enter` are enough; an empty line forgets it.

The command does the same without the interface:

```sh
clicloud import my-profile            # or @my-profile, soundcloud.com/my-profile
clicloud import my-profile --dry-run  # show what would be added and write nothing
clicloud import https://soundcloud.com/my-profile/reposts    # another list than the likes
clicloud import https://soundcloud.com/my-profile/sets/mix   # a playlist
```

It reads the page of likes of the profile and adds its tracks to the favorites,
after those already there, the latest likes first. No account is signed in to,
but the likes must be visible in the profile: hidden ones cannot be had. The
name of a profile is what stands in the address of its page;
`soundcloud.com/you/likes` does not contain your name.

It goes one way and only adds: a track whose like was taken back stays a
favorite, and the local favorites are not sent to the site. Asking again adds
only what is new, so after the network broke off it is enough to ask again:
what had arrived was saved. The outcome names three numbers: added, there
already, and skipped, which are likes of playlists and albums rather than of
single tracks.

The page of likes tells the title and the link, but neither the artist nor the
length. At first the artist is the name of the author's profile from the link
(`low-sea` → `low sea`) and a dash stands for the length. The real title,
author and length are put in when the track is first fetched, by playing it or
with `d`: yt-dlp learns them on the way and the client writes them into the
library. Whether a track can be played is not known ahead either: one that was
deleted or is closed in your region becomes a favorite and is skipped when its
turn comes.

`import` can be run while the interface is open: the interface picks up the
new tracks by itself. Another file is named with `--library`, as for `ui`. For
the pages of a profile yt-dlp asks for the impersonation of a browser, warns
without it, and SoundCloud may answer 403; it comes with `curl_cffi`:
`pipx install "yt-dlp[default,curl-cffi]"`.

The programs can be named with `--yt-dlp /path/to/yt-dlp`, `--mpv /path/to/mpv`
or the variables `CLICLOUD_YT_DLP`, `CLICLOUD_MPV`. Give the path of an
executable, not a command with arguments. The programs are started without a
shell. The user's own configuration of yt-dlp and mpv is switched off, so that
the client behaves the same everywhere.

### Settings and color schemes

`o` opens the settings window: `↑`/`↓` choose a line, `←`/`→` change its value,
`Enter` switches or opens, `Esc` closes the window. With the mouse, a click
chooses a line and a click on the chosen one switches it. A change applies at
once and is written to the settings file. The window covers the line of
messages, so what the client says while it is open, such as why a proxy address
would not do, is shown in the window itself.

| Setting | What it changes | In the file |
|---|---|---|
| Color scheme | The colors of the interface | `theme` |
| Backdrop | The ASCII drawing behind the list of tracks: `reaper`, `pentagram`, or `none` | `backdrop` |
| Cover | How the tab of the track draws the cover: in blocks or in Braille; in its own colors, those of the scheme or tones; or not at all | `cover` |
| Language | Русский, English or 日本語: `ru`, `en`, `ja` | `language` |
| Proxy | Switches the proxy on and off from the next search and track | `proxy_enabled` |
| Proxy address | HTTP(S) or SOCKS5 with a port | `proxy` |
| Track cache | Storing tracks on disk | `cache_enabled` |
| Cache size | The limit in megabytes; 0 lifts it | `cache_limit_mb` |
| Search results | From 5 to 50 tracks for a search in the interface, 10 by default | `search_limit` |
| Seek step | 5, 10, 15, 30 or 60 seconds | `seek_step` |
| Start volume | From 0 to 100 | `volume` |
| Even loudness | Quiet and loud tracks are brought to one loudness; from the next track | `normalize` |
| Audio device | One of the devices mpv sees; `auto` leaves the choice to it | `audio_device` |
| Media keys | Pause, stop and the tracks around from the keyboard and the panel of the desktop, by the plugin `mpv-mpris` | `media_keys` |
| yt-dlp | Tells its version; a yt-dlp that the client fetched is replaced by the latest release on `Enter` | — |
| SoundCloud likes | Adds the likes of a profile to the favorites; remembers its name | `soundcloud_profile` |

The flags `--tor`, `--proxy`, `--no-proxy`, `--cache-dir` and `--no-cache` set
the state for one run; switching it in the settings replaces that.

The language changes at once and covers everything the client writes: the
interface, the error messages and the output of `search`, `play`, `import`,
`cache` and `doctor`. Only `--help` stays in English. Japanese letters take two
cells, so the terminal needs a font that has them.

Schemes:

- `terminal` (the default): the colors of the terminal. In Ghostty the client
  reads its configuration and takes the theme from there, so the shades match
  the rest of the terminal; the background of the terminal, a transparent one
  included, is left alone. In other terminals their 16 colors are used.
- `mono`: no color; bold, dim and reverse.
- Ghostty themes: 45 are built in (Catppuccin, Dracula, Gruvbox, Nord,
  TokyoNight, Rose Pine, Kanagawa, Everforest, Solarized and others), and where
  Ghostty is installed all of its themes and your own from
  `~/.config/ghostty/themes` are available.

`Enter` on the line of the scheme opens a list: the scheme under the cursor is
applied at once for a look, `Enter` keeps it, `Esc` brings back the one before.
A theme gives the palette only. The roles of the colors (titles, the selection,
the track that plays, the marks) are assigned by the client, which moves the
shades as far as it takes to read them, so light and dark themes alike stay
legible.

### Media keys

The keys for pause, next, previous and stop on the keyboard, and the player in
the panel of the desktop, do not talk to the client but to mpv, over the MPRIS
protocol, which mpv learns from the plugin
[mpv-mpris](https://github.com/hoyon/mpv-mpris). Where the plugin is installed
(`sudo pacman -S mpv-mpris`, `sudo apt install mpv-mpris`), the client finds it
by itself and gives it to every player; in the panel a track is named "artist -
title". The interface follows what the keys did: pause and stop show in the
player, next goes on to the next track if that one is in the cache already
(otherwise mpv has nowhere to go; press `n`), previous starts the track before.
`media_keys` in the settings file: `auto` looks for the plugin in the usual
places, `off` does not use it, anything else is the path of `mpris.so`. Without
the plugin the setting changes nothing. Windows and macOS have no MPRIS.

### Terminals

The interface needs only what every terminal has: the alternate screen, the
mouse, bold, dim and reverse. The colors adapt to the terminal by themselves:

- **24-bit color** is used where the terminal announces it: the variable
  `COLORTERM=truecolor`, Windows Terminal, Ghostty, kitty, Alacritty, foot,
  WezTerm, iTerm2, VS Code, and inside tmux, which brings colors down to what
  the outer terminal can do. Konsole and GNOME Terminal behave the same.
- **256 colors** everywhere else, such as Terminal.app, xterm, GNU screen, or
  over ssh, where `COLORTERM` is not passed on. The theme is picked from the
  standard palette; the shades are a little coarser, but everything stays
  legible.
- **The 16 colors of the terminal**: the `terminal` scheme outside Ghostty. It
  colors nothing by itself and suits any color terminal, the Linux console
  included.
- **No color**: the `mono` scheme. The variable `NO_COLOR` makes the `terminal`
  scheme the same.

Not every terminal has dim text: there the secondary texts look like the
others. Russian and Japanese need a font with those letters; the Linux console
has no Japanese.

**Windows and PowerShell.** Every command and the whole interface work: in
place of a Unix socket mpv listens on a named pipe, `\\.\pipe\clicloud-...`.
Windows has no SIGTERM or SIGHUP, so the interface ends on `q` and `Ctrl+C`,
which arrive as keys. The settings, the cache and the library are in
`%USERPROFILE%\.config\clicloud`, `%USERPROFILE%\.cache\clicloud` and
`%USERPROFILE%\.local\share\clicloud`; the `XDG_*` variables count here too.
If `yt-dlp` or `mpv` is not on `PATH`, name the `.exe` with `--mpv` and
`--yt-dlp`. The yt-dlp of the project is looked for in
`.tools\venv\Scripts\yt-dlp.exe`, where `py -m venv` puts it. If output is
piped in PowerShell 5 and Cyrillic turns into garbage, run
`[Console]::OutputEncoding = [Text.Encoding]::UTF8` or switch to English
(`"language": "en"`). The interface also runs in WSL, Windows Terminal
included.

## Cache and offline

A track is fetched once: yt-dlp writes it into the cache while mpv plays what
has arrived. As soon as the download is over, usually in tens of seconds, the
track stays on disk, and from then on it starts at once and without a network,
in the interface and for `play` with a link, with a proxy and without. If you
stop or change the track sooner, what had arrived is removed: there are no
incomplete files in the cache. While a track is being fetched, seeking works
within what has arrived.

To get ready for a time without a network, press `d` on a track in the
interface, or `D` on a whole list, the library for one: the tracks are stored
one at a time without interrupting the music. All that is stored shows in the
library (`2`), tracks that are not favorites included: from there they play
without a network, and `x` removes a track from the cache. The title of the
library says how many tracks are stored and how much they take. There is no
search without a network. On the command line `clicloud play` with a link to a
stored track works.

Only single tracks are stored. A link to a playlist or a profile, and a short
`on.soundcloud.com` link, play as before, without being stored.

The cache is in `$XDG_CACHE_HOME/clicloud` or `~/.cache/clicloud`:

- `audio/`: the tracks as SoundCloud hands them out (usually AAC or MP3), named
  `artist.track`;
- `yt-dlp/`: what yt-dlp keeps for itself. With it yt-dlp does not ask
  SoundCloud for the client id before every search and track, and those start
  a few seconds sooner. The cache of yt-dlp in your home directory is not
  used;
- `tracks/`: the title, the artist and the length of every track for the
  library, as yt-dlp told them when it fetched the track; where it did not, a
  track stored by `play` with a link is named after that link;
- `art/` and `info/`: the covers and what is known of the tracks, for the tab
  of the track. A cover is a square of 128×128 pixels, about 48 KB a track.
  They do not count toward the limit of the cache;
- `partial/`: the downloads that run now.

By default the audio takes no more than 1024 MB: when there is no room left,
the tracks that were played longest ago are removed. The settings are in the
same `config.json` as the proxy:

```json
{
  "cache_enabled": true,
  "cache_dir": "/mnt/music/clicloud-cache",
  "cache_limit_mb": 4096
}
```

`cache_limit_mb: 0` lifts the limit. The directory can also be given with the
`--cache-dir` flag or the `CLICLOUD_CACHE_DIR` variable; it must be empty or
belong to clicloud already, which marks it with a `CACHEDIR.TAG` file. The
client does not take a directory with someone else's files and removes nothing
in it. `--no-cache` or `CLICLOUD_NO_CACHE=1` switches the cache off altogether:
nothing is written to disk, and mpv fetches the stream itself, as described
under the proxy.

```sh
clicloud cache          # where the cache is and how much it takes
clicloud cache --clear  # remove the audio and the data of yt-dlp
```

## Proxy and Tor

One run through a Tor that is already running (SOCKS on `127.0.0.1:9050`):

```sh
cargo run -- --tor search "burial"
cargo run -- --tor play "burial"
```

Tor is started separately; clicloud neither installs nor starts it. For Tor
Browser use the port of its SOCKS proxy, usually 9150:

```sh
cargo run -- --proxy socks5h://127.0.0.1:9150 play "burial"
cargo run -- --proxy http://127.0.0.1:8080 search "ambient"
cargo run -- --no-proxy play "ambient"
```

Lasting settings: `$XDG_CONFIG_HOME/clicloud/config.json` if the variable is
set, otherwise `~/.config/clicloud/config.json`. Its content:

```json
{
  "proxy_enabled": true,
  "proxy": "socks5h://127.0.0.1:9050"
}
```

To switch the setting off, change `proxy_enabled` to `false` or switch the
proxy in the settings of the interface (`o`). The repository has a
`config.example.json`, which can be tried without copying it:

```sh
cargo run -- --config config.example.json search "ambient"
```

HTTP, HTTPS, SOCKS5 and SOCKS5h are supported; SOCKS needs a port. Tor uses
SOCKS5h: the names of servers are resolved through the proxy. In place of the
file there are `CLICLOUD_PROXY` and `CLICLOUD_CONFIG`. The order is
`--no-proxy` → `--tor` → `--proxy` → `CLICLOUD_PROXY` → the file. `--tor` and
`--no-proxy` cannot be used together. With no setting of its own the client
leaves the external programs to do what they do with the system's proxy
variables; `--no-proxy` forces a direct connection.

A single track is always fetched by yt-dlp, through the proxy that is set, and
without one through the proxy from `HTTPS_PROXY` and `ALL_PROXY`, SOCKS
included. mpv gets bytes, not a network address, so the limits of SOCKS in mpv
and FFmpeg do not concern it. When the proxy fails the client does not fall
back to a direct connection.

A playlist, and with `--no-cache` any track, plays the old way. With a proxy
set, yt-dlp hands the audio to mpv through a pipe; seeking is limited to the
buffer, and a link to a playlist plays its first track only. Without one, mpv
fetches the stream itself through its built-in yt-dlp hook. yt-dlp reads
`HTTPS_PROXY` and `ALL_PROXY`, while FFmpeg inside mpv reads only `http_proxy`,
so clicloud hands an HTTP proxy from those variables to mpv explicitly:
otherwise the address of the stream would be looked up through the proxy and
the stream itself would go past it. mpv cannot use a SOCKS proxy from the
environment; set it with `--proxy` or `--tor`. Tor may slow downloads, and
SoundCloud may turn its exit nodes away. The network timeout of a search is 45
seconds through a proxy, set in clicloud or in the environment, and 15 seconds
without one. The error of a search opens in a window of its own, which `e`
opens again until a new search or track is started.

## Limits

This is not the official SoundCloud API. It depends on the current extractor
of yt-dlp; when something fails, update that first (`pipx upgrade yt-dlp`). A
search result is no promise of playback: private, paid, deleted and
region-locked tracks may give an error or a preview. Signing in to an account
and synchronizing with a SoundCloud library are not supported: bringing likes
over only adds the likes that are visible in a profile to the favorites. The
cache keeps tracks as they come and without tags; it is not an export of a
collection. With `--no-cache` no audio is written to disk: mpv buffers the
stream, and in proxy mode yt-dlp keeps the current fragment in a temporary
directory of the client, which is removed at the end.

## Checks

```sh
cargo fmt --check
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --locked
python3 tests/tui_smoke.py
```

The integration tests on Unix use stand-ins for the external programs and need
neither a network nor an audio device. A real search is checked separately:
`clicloud search "ambient" --limit 3`.

`cargo test` passes on Windows too: the stand-ins there are batch files, and
the player that answers on a named pipe is written in PowerShell. The
`tests/cli.rs` suite stays on Unix: it rests on shell scripts.

`tui_smoke.py` runs the real TUI in a PTY with stand-ins for yt-dlp and mpv. It
checks the search, the saving of favorites, the commands sent to the player,
the storing of a track in the cache, playing it again from there and removing
it, the writing of a setting to the file, bringing over the likes of a profile
from the settings, the restoring of the terminal with and without a proxy, the
stopping of mpv on SIGTERM, and that the interface ends when its terminal is
closed. It needs Python 3 and permission for local Unix sockets; neither the
internet nor an audio device.

The built-in color schemes are copies of the themes that ship with Ghostty,
which takes them from the iTerm2-Color-Schemes collection (MIT); see
`themes/README.md`.

Documentation: [yt-dlp](https://github.com/yt-dlp/yt-dlp),
[mpv](https://mpv.io/manual/stable/).
