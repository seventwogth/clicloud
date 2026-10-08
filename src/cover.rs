//! The artwork of a track, drawn in the cells of a terminal.
//!
//! A picture is fetched once and kept as plain pixels, small and square. It is drawn
//! in half blocks, two pixels to a cell, or in the dots of Braille, eight to a cell;
//! in its own colors, or in the tones between two colors of the scheme.
use crate::{Result, setup, soundcloud::Extractor};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};
use url::Url;

/// The side of a kept picture in pixels: more than a terminal shows of it.
pub const SIDE: usize = 128;

pub type Rgb = [u8; 3];

pub struct Cover {
    pixels: Vec<Rgb>,
}

/// How a picture is drawn, by its name in the settings.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Blocks,
    Braille,
    BlockTones,
    BrailleTones,
    Off,
}

pub const MODES: [(&str, Mode); 5] = [
    ("blocks", Mode::Blocks),
    ("braille", Mode::Braille),
    ("block-tones", Mode::BlockTones),
    ("braille-tones", Mode::BrailleTones),
    ("none", Mode::Off),
];

impl Mode {
    /// A name that is not known draws nothing.
    pub fn parse(name: &str) -> Self {
        (MODES.iter())
            .find(|(known, _)| *known == name)
            .map_or(Self::Off, |(_, mode)| *mode)
    }
}

/// One cell of a drawn picture: its mark, and the colors of the mark and behind it,
/// where the picture gives the cell any.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Cell {
    pub symbol: char,
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
}

/// What the scheme lets a picture be drawn with.
#[derive(Clone, Copy)]
pub struct Palette {
    /// Whether cells may be colored at all; without it marks alone make the picture.
    pub colors: bool,
    /// The two colors that tones run between, the darker first.
    pub tones: (Rgb, Rgb),
    /// Whether marks are lighter than what is behind them, as on a dark terminal.
    pub light_marks: bool,
}

// How light a color looks, from 0 to 1.
fn light(color: Rgb) -> f64 {
    (0.299 * f64::from(color[0]) + 0.587 * f64::from(color[1]) + 0.114 * f64::from(color[2]))
        / 255.0
}

fn blend(from: Rgb, to: Rgb, share: f64) -> Rgb {
    let part = |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * share).round() as u8;
    [
        part(from[0], to[0]),
        part(from[1], to[1]),
        part(from[2], to[2]),
    ]
}

impl Cover {
    /// A picture from its pixels, red, green and blue of each, row by row.
    pub fn new(bytes: &[u8]) -> Option<Self> {
        (bytes.len() == SIDE * SIDE * 3).then(|| Self {
            pixels: bytes.as_chunks::<3>().0.to_vec(),
        })
    }

    pub fn read(file: &Path) -> Option<Self> {
        Self::new(&fs::read(file).ok()?)
    }

    // The picture as `width` by `height` points, each the mean of what falls to it.
    fn grid(&self, width: usize, height: usize) -> Vec<Rgb> {
        let mut points = Vec::with_capacity(width * height);
        for y in 0..height {
            let (top, bottom) = (y * SIDE / height, ((y + 1) * SIDE / height).max(1));
            for x in 0..width {
                let (left, right) = (x * SIDE / width, ((x + 1) * SIDE / width).max(1));
                let (mut sum, mut count) = ([0u32; 3], 0u32);
                for row in top..bottom.max(top + 1).min(SIDE) {
                    for column in left..right.max(left + 1).min(SIDE) {
                        let pixel = self.pixels[row * SIDE + column];
                        for channel in 0..3 {
                            sum[channel] += u32::from(pixel[channel]);
                        }
                        count += 1;
                    }
                }
                points.push(sum.map(|channel| (channel / count.max(1)) as u8));
            }
        }
        points
    }

    /// The picture in `columns` by `rows` cells. A cell is twice as tall as it is
    /// wide, so a square picture takes twice as many columns as rows.
    pub fn cells(&self, columns: usize, rows: usize, mode: Mode, palette: Palette) -> Vec<Cell> {
        let tone = |light: f64| blend(palette.tones.0, palette.tones.1, light.clamp(0.0, 1.0));
        match mode {
            Mode::Off => Vec::new(),
            // Two pixels to a cell: the upper half is the mark, the lower what is behind.
            Mode::Blocks | Mode::BlockTones if palette.colors => {
                let points = self.grid(columns, rows * 2);
                let shade = |point: Rgb| match mode {
                    Mode::Blocks => point,
                    _ => tone(light(point)),
                };
                (0..rows * columns)
                    .map(|cell| {
                        let (row, column) = (cell / columns, cell % columns);
                        Cell {
                            symbol: '▀',
                            fg: Some(shade(points[row * 2 * columns + column])),
                            bg: Some(shade(points[(row * 2 + 1) * columns + column])),
                        }
                    })
                    .collect()
            }
            // Without colors a half is either there or not.
            Mode::Blocks | Mode::BlockTones => {
                let marked = dither(&self.grid(columns, rows * 2), columns, palette.light_marks);
                (0..rows * columns)
                    .map(|cell| {
                        let (row, column) = (cell / columns, cell % columns);
                        let upper = marked[row * 2 * columns + column];
                        let lower = marked[(row * 2 + 1) * columns + column];
                        Cell {
                            symbol: match (upper, lower) {
                                (true, true) => '█',
                                (true, false) => '▀',
                                (false, true) => '▄',
                                (false, false) => ' ',
                            },
                            fg: None,
                            bg: None,
                        }
                    })
                    .collect()
            }
            // Eight dots to a cell, two across and four down; the cell has one color
            // for all of them, that of what its dots stand for.
            Mode::Braille | Mode::BrailleTones => {
                const DOTS: [[u32; 2]; 4] =
                    [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];
                let width = columns * 2;
                let points = self.grid(width, rows * 4);
                let marked = dither(&points, width, palette.light_marks);
                (0..rows * columns)
                    .map(|cell| {
                        let (row, column) = (cell / columns, cell % columns);
                        let (mut dots, mut sum, mut count) = (0, [0u32; 3], 0u32);
                        for (down, across) in (0..4).flat_map(|down| [(down, 0), (down, 1)]) {
                            let at = (row * 4 + down) * width + column * 2 + across;
                            if marked[at] {
                                dots |= DOTS[down][across];
                                for channel in 0..3 {
                                    sum[channel] += u32::from(points[at][channel]);
                                }
                                count += 1;
                            }
                        }
                        let mean = sum.map(|channel| (channel / count.max(1)) as u8);
                        let color = match mode {
                            Mode::Braille => mean,
                            // Dots are thin: a tone too near to what is behind them is lost.
                            _ if palette.light_marks => tone(0.45 + 0.55 * light(mean)),
                            _ => tone(0.55 * light(mean)),
                        };
                        Cell {
                            symbol: char::from_u32(0x2800 + dots)
                                .filter(|_| dots != 0)
                                .unwrap_or(' '),
                            fg: (palette.colors && dots != 0).then_some(color),
                            bg: None,
                        }
                    })
                    .collect()
            }
        }
    }
}

// Which of the points get a mark, when a mark is all a point can be: light points
// where marks are light, dark ones where they are dark. The light that a point is off
// by goes to its neighbors still to come, so that an area keeps its tone on the whole.
fn dither(points: &[Rgb], width: usize, light_marks: bool) -> Vec<bool> {
    let mut lights: Vec<f64> = points.iter().map(|point| light(*point)).collect();
    // A dim picture uses the whole range between no marks and all of them.
    let (least, most) = (lights.iter().copied()).fold((1.0f64, 0.0f64), |(least, most), light| {
        (least.min(light), most.max(light))
    });
    if most - least > 0.05 {
        for light in &mut lights {
            *light = (*light - least) / (most - least);
        }
    }
    let height = lights.len() / width.max(1);
    let mut marked = vec![false; lights.len()];
    for y in 0..height {
        for x in 0..width {
            let at = y * width + x;
            let lit = lights[at] > 0.5;
            let rest = lights[at] - f64::from(u8::from(lit));
            marked[at] = lit == light_marks;
            for (across, down, share) in [(1, 0, 7.0), (-1, 1, 3.0), (0, 1, 5.0), (1, 1, 1.0)] {
                let (nx, ny) = (x as isize + across, y + down);
                if nx >= 0 && (nx as usize) < width && ny < height {
                    lights[ny * width + nx as usize] += rest * share / 16.0;
                }
            }
        }
    }
    marked
}

/// A smaller picture than the one at `artwork`, where its address says what size it
/// is; None for an address that is not one of the pictures of SoundCloud.
pub fn small(artwork: &str) -> Option<String> {
    let url = Url::parse(artwork).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https" || !(host == "sndcdn.com" || host.ends_with(".sndcdn.com")) {
        return None;
    }
    let sized = ["-original.", "-t500x500.", "-large.", "-crop."]
        .iter()
        .find(|size| artwork.contains(*size))
        .map(|size| artwork.replacen(size, "-t300x300.", 1));
    Some(sized.unwrap_or_else(|| artwork.to_owned()))
}

/// Fetches the picture of the track at `track` and keeps its pixels in `file`.
/// `artwork` is where the picture is, if a download told; yt-dlp is asked otherwise.
/// `scratch` is a directory for the picture as it arrives.
pub fn fetch(
    extractor: Extractor,
    mpv: &str,
    track: &str,
    artwork: Option<&str>,
    scratch: &Path,
    file: &Path,
) -> Result<()> {
    let artwork = match artwork {
        Some(artwork) => artwork.to_owned(),
        None => {
            let mut command = extractor.command();
            let output = command
                .args([
                    "--ignore-no-formats-error",
                    "--extractor-args",
                    "soundcloud:formats=none",
                    "--skip-download",
                    "--no-playlist",
                    "--socket-timeout",
                    if extractor.proxied() { "30" } else { "15" },
                    "--print",
                    "thumbnail",
                    "--",
                    track,
                ])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()?;
            (String::from_utf8_lossy(&output.stdout).lines())
                .map(str::trim)
                .rfind(|line| line.starts_with("https://"))
                .ok_or("yt-dlp named no picture")?
                .to_owned()
        }
    };
    let artwork = small(&artwork).ok_or("not a picture of SoundCloud")?;
    let fetched = scratch.join("cover");
    // curl takes a proxy from the environment by itself; a direct route is said.
    let proxy = match (extractor.proxy, extractor.no_proxy) {
        (Some(proxy), _) => Some(proxy),
        (None, true) => Some(""),
        (None, false) => None,
    };
    setup::fetch(proxy, &artwork, &fetched, 60)?;
    pixels(mpv, &fetched, file)
}

/// Has mpv turn the picture in `picture` into the pixels that are kept, in `file`:
/// it reads every kind of picture there is, and is there already.
pub fn pixels(mpv: &str, picture: &Path, file: &Path) -> Result<()> {
    let raw = file.with_extension("raw");
    let mut output = std::ffi::OsString::from("--o=");
    output.push(&raw);
    let status = Command::new(mpv)
        .args([
            "--no-config",
            "--no-terminal",
            "--no-audio",
            "--frames=1",
            &format!("--vf=scale={SIDE}:{SIDE},format=rgb24"),
            "--of=rawvideo",
            "--ovc=rawvideo",
        ])
        .arg(output)
        .arg("--")
        .arg(picture)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    let whole = fs::metadata(&raw).is_ok_and(|raw| raw.len() == (SIDE * SIDE * 3) as u64);
    if !status.success() || !whole {
        let _ = fs::remove_file(&raw);
        return Err("mpv did not turn the picture into pixels".into());
    }
    fs::rename(raw, file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Left half black, right half white; the lower right quarter red.
    fn picture() -> Cover {
        let mut bytes = Vec::new();
        for y in 0..SIDE {
            for x in 0..SIDE {
                bytes.extend(match (x < SIDE / 2, y < SIDE / 2) {
                    (true, _) => [0, 0, 0],
                    (false, true) => [255, 255, 255],
                    (false, false) => [255, 0, 0],
                });
            }
        }
        Cover::new(&bytes).unwrap()
    }

    fn palette(colors: bool, light_marks: bool) -> Palette {
        Palette {
            colors,
            tones: ([0, 0, 40], [200, 200, 255]),
            light_marks,
        }
    }

    fn text(cells: &[Cell], columns: usize) -> Vec<String> {
        (cells.chunks(columns))
            .map(|row| row.iter().map(|cell| cell.symbol).collect())
            .collect()
    }

    #[test]
    fn a_picture_is_drawn_in_blocks_or_dots_in_colors_or_tones() {
        let cover = picture();
        assert!(Cover::new(&[0; 12]).is_none());
        assert!(cover.cells(8, 4, Mode::Off, palette(true, true)).is_empty());

        // Blocks: a cell is two pixels, one above the other, each with its own color.
        let cells = cover.cells(8, 4, Mode::Blocks, palette(true, true));
        assert_eq!(cells.len(), 32);
        assert!(cells.iter().all(|cell| cell.symbol == '▀'));
        assert_eq!((cells[0].fg, cells[0].bg), (Some([0; 3]), Some([0; 3])));
        assert_eq!(cells[7].fg, Some([255; 3]));
        assert_eq!(cells[31].bg, Some([255, 0, 0]));
        // In tones the same halves run between the two colors of the scheme.
        let cells = cover.cells(8, 4, Mode::BlockTones, palette(true, true));
        assert_eq!(cells[0].fg, Some([0, 0, 40]));
        assert_eq!(cells[7].fg, Some([200, 200, 255]));
        let red = cells[31].bg.unwrap();
        assert!(red[0] > 0 && red[0] < 200 && red[0] == red[1], "{red:?}");

        // Dots: light ones for what is light, colored by what they stand for.
        let cells = cover.cells(8, 4, Mode::Braille, palette(true, true));
        assert_eq!(text(&cells, 8)[0], "    ⣿⣿⣿⣿");
        assert_eq!((cells[0].fg, cells[4].fg), (None, Some([255; 3])));
        assert_eq!((cells[31].fg, cells[31].bg), (Some([255, 0, 0]), None));
        let cells = cover.cells(8, 4, Mode::BrailleTones, palette(true, true));
        assert_eq!(cells[4].fg, Some([200, 200, 255]));
        // On a light terminal the marks are the dark of the picture.
        let cells = cover.cells(8, 4, Mode::Braille, palette(true, false));
        assert_eq!(text(&cells, 8)[0], "⣿⣿⣿⣿    ");

        // A scheme without colors has marks alone, for dots and for blocks alike.
        let cells = cover.cells(8, 4, Mode::Braille, palette(false, true));
        assert!(
            cells
                .iter()
                .all(|cell| cell.fg.is_none() && cell.bg.is_none())
        );
        let cells = cover.cells(8, 4, Mode::Blocks, palette(false, true));
        assert_eq!(text(&cells, 8)[0], "    ████");
        assert!(
            cells
                .iter()
                .all(|cell| cell.fg.is_none() && cell.bg.is_none())
        );
        // More cells than the picture has pixels still draw all of it.
        let cells = cover.cells(200, 100, Mode::Blocks, palette(true, true));
        assert_eq!((cells[0].fg, cells[199].fg), (Some([0; 3]), Some([255; 3])));

        assert_eq!(Mode::parse("braille-tones"), Mode::BrailleTones);
        assert_eq!(Mode::parse("sixel"), Mode::Off);
    }

    #[test]
    fn areas_keep_their_tone_in_marks() {
        // A gray between black and white gets about half of its points marked.
        let gray: Vec<Rgb> = (0..64 * 64)
            .map(|at| match at % 64 {
                0..16 => [0; 3],
                16..48 => [128; 3],
                _ => [255; 3],
            })
            .collect();
        let marked = dither(&gray, 64, true);
        let share = |from: usize, to: usize| {
            let lit = (marked.iter().enumerate())
                .filter(|(at, lit)| (from..to).contains(&(at % 64)) && **lit)
                .count();
            lit as f64 / (64 * (to - from)) as f64
        };
        assert_eq!((share(0, 16), share(48, 64)), (0.0, 1.0));
        assert!((0.4..0.6).contains(&share(20, 44)), "{}", share(20, 44));
        // Dark marks stand for the dark of it.
        let dark = dither(&gray, 64, false);
        assert!(dark[0] && !dark[63]);
    }

    #[test]
    fn only_pictures_of_soundcloud_are_fetched_and_small_ones() {
        for (artwork, expected) in [
            (
                "https://i1.sndcdn.com/artworks-000024253883-mazkrb-original.jpg",
                Some("https://i1.sndcdn.com/artworks-000024253883-mazkrb-t300x300.jpg"),
            ),
            (
                "https://i1.sndcdn.com/artworks-abc-large.png",
                Some("https://i1.sndcdn.com/artworks-abc-t300x300.png"),
            ),
            (
                "https://a1.sndcdn.com/images/default_avatar.png",
                Some("https://a1.sndcdn.com/images/default_avatar.png"),
            ),
            ("http://i1.sndcdn.com/artworks-abc-large.jpg", None),
            ("https://sndcdn.com.evil.test/artworks-abc-large.jpg", None),
            ("file:///etc/passwd", None),
            ("--output=/tmp/x", None),
        ] {
            assert_eq!(small(artwork).as_deref(), expected, "{artwork}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_picture_is_kept_only_when_mpv_made_all_of_its_pixels() {
        use std::os::unix::fs::PermissionsExt;
        let directory = std::env::temp_dir().join(format!("clicloud-cover-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let (picture, file) = (directory.join("cover"), directory.join("kept"));
        fs::write(&picture, b"jpeg").unwrap();
        // A stand-in for mpv that writes as many bytes as it is told to where `--o=` says.
        let program = directory.join("mpv");
        let script = |bytes: usize| {
            let source = format!(
                "#!/bin/sh\nfor arg in \"$@\"; do case \"$arg\" in --o=*) out=\"${{arg#--o=}}\";; esac; done\n\
                 head -c {bytes} /dev/zero > \"$out\"\n"
            );
            let mut writer = Command::new("sh")
                .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
                .arg(&program)
                .stdin(Stdio::piped())
                .spawn()
                .unwrap();
            std::io::Write::write_all(&mut writer.stdin.take().unwrap(), source.as_bytes())
                .unwrap();
            assert!(writer.wait().unwrap().success());
            assert!(fs::metadata(&program).unwrap().permissions().mode() & 0o111 != 0);
        };
        script(100);
        assert!(pixels(&program.to_string_lossy(), &picture, &file).is_err());
        assert!(!file.exists() && !file.with_extension("raw").exists());
        script(SIDE * SIDE * 3);
        pixels(&program.to_string_lossy(), &picture, &file).unwrap();
        assert!(Cover::read(&file).is_some());
        assert!(pixels("clicloud-no-such-program", &picture, &file).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
