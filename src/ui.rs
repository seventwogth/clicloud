use crate::{
    Result,
    cache::{self, Cache},
    clean, clean_lines,
    config::{self, Settings},
    cover::{self, Cover, Mode},
    lang,
    library::{self, Library},
    megabytes,
    playback::{self, Origin, Playback},
    player::{self, Download},
    setup,
    soundcloud::{self, Details, Extractor, SoundCloud, Track},
    theme::{self, Theme},
};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseButton, MouseEventKind,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::layout::Position;
use ratatui::{prelude::*, widgets::*};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    io::{self, IsTerminal},
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant, SystemTime},
};

// What the header says after the name. One of them is picked for a run; add as many
// as you like, but keep them short: the header clips what does not fit beside the name.
const TAGLINES: [&str; 4] = [
    "not sponsored by shadow wizard money gang",
    "punks not dead",
    "save that shit",
    "stay hydrated",
];

// The mark that turns while something runs, ASCII like every other mark here.
const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];
// The drawings behind the list of tracks, by their names in the settings; the first is none.
const BACKDROPS: [(&str, &str); 3] = [
    ("none", ""),
    ("reaper", include_str!("../backdrops/reaper")),
    ("pentagram", include_str!("../backdrops/pentagram")),
];

// The name drawn large, a mark to a dot. The header shows it small, eight dots to a cell.
const LOGO: &str = include_str!("../logo.txt");

// Frames, buttons and marks are ASCII in every color scheme.
const BORDER: symbols::border::Set = symbols::border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// The programs the client cannot work without and did not find, and what it offers
/// to do about each: fetch yt-dlp, which is one file, and name the command for mpv.
struct Setup {
    yt_dlp: bool,
    mpv: bool,
    /// What the dialog says about the last attempt.
    message: String,
    /// The download that runs now; while it does, the dialog waits for it.
    receiver: Option<mpsc::Receiver<std::result::Result<PathBuf, String>>>,
}

impl Setup {
    /// What is missing of the two, or None when both can be started.
    fn needed(yt_dlp: &str, mpv: &str) -> Option<Self> {
        let (yt_dlp, mpv) = (!crate::present(yt_dlp), !crate::present(mpv));
        (yt_dlp || mpv).then(|| Self {
            yt_dlp,
            mpv,
            message: String::new(),
            receiver: None,
        })
    }

    fn busy(&self) -> bool {
        self.receiver.is_some()
    }
}

// What a search sends back: the tracks as they are found, and how it ended.
enum Found {
    Track(Track),
    Over(std::result::Result<(), String>),
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Search,
    Library,
    Queue,
    Recent,
}

#[derive(Clone, Copy, PartialEq)]
enum Setting {
    Theme,
    Backdrop,
    Cover,
    Language,
    Proxy,
    ProxyAddress,
    Cache,
    CacheLimit,
    SearchLimit,
    SeekStep,
    Volume,
    Normalize,
    Device,
    Likes,
}

const SETTINGS: [Setting; 14] = [
    Setting::Theme,
    Setting::Backdrop,
    Setting::Cover,
    Setting::Language,
    Setting::Proxy,
    Setting::ProxyAddress,
    Setting::Cache,
    Setting::CacheLimit,
    Setting::SearchLimit,
    Setting::SeekStep,
    Setting::Volume,
    Setting::Normalize,
    Setting::Device,
    Setting::Likes,
];
const SEEK_STEPS: [u16; 5] = [5, 10, 15, 30, 60];

impl Setting {
    fn name(self) -> &'static str {
        match self {
            Self::Theme => t!("Цветовая схема"),
            Self::Backdrop => t!("Фоновый рисунок"),
            Self::Cover => t!("Обложка"),
            Self::Language => t!("Язык"),
            Self::Proxy => t!("Прокси"),
            Self::ProxyAddress => t!("Адрес прокси"),
            Self::Cache => t!("Кеш треков"),
            Self::CacheLimit => t!("Размер кеша"),
            Self::SearchLimit => t!("Результатов поиска"),
            Self::SeekStep => t!("Шаг перемотки"),
            Self::Volume => t!("Громкость при запуске"),
            Self::Normalize => t!("Ровная громкость"),
            Self::Device => t!("Аудиоустройство"),
            Self::Likes => t!("Лайки SoundCloud"),
        }
    }
    fn hint(self) -> &'static str {
        match self {
            Self::Theme => {
                t!(
                    "Enter - список схем с предпросмотром, Left/Right - соседняя.\nterminal повторяет цвета терминала, mono обходится без цвета,\nостальные - темы Ghostty."
                )
            }
            Self::Backdrop => {
                t!(
                    "Enter или Left/Right - сменить рисунок за списком треков.\nнет - список без рисунка."
                )
            }
            Self::Cover => {
                t!(
                    "Enter или Left/Right - как рисовать обложку на вкладке трека (t):\nблоками или точками Брайля, в её цветах или в тонах схемы."
                )
            }
            Self::Language => {
                t!(
                    "Enter или Left/Right - сменить язык интерфейса и сообщений.\nСправка командной строки (--help) остаётся на русском."
                )
            }
            Self::Proxy => {
                t!("Enter - включить или выключить.\nДействует со следующего поиска и трека.")
            }
            Self::ProxyAddress => {
                t!(
                    "Enter - изменить: http, https, socks5 или socks5h с портом.\nTor: socks5h://127.0.0.1:9050. Пустая строка убирает адрес."
                )
            }
            Self::Cache => {
                t!(
                    "Enter - включить или выключить сохранение треков на диск.\nУже сохранённое остаётся на месте."
                )
            }
            Self::CacheLimit => {
                t!(
                    "Left/Right - по 256 МБ, ноль снимает ограничение.\nЛишнее удаляется, начиная с давно не игравших треков."
                )
            }
            Self::SearchLimit => t!("Left/Right - по 5, от 5 до 50 треков на один поиск."),
            Self::SeekStep => t!("Left/Right - 5, 10, 15, 30 или 60 секунд на одно нажатие."),
            Self::Volume => t!("Left/Right - по 5. Применяется при следующем запуске."),
            Self::Normalize => {
                t!(
                    "Enter - включить или выключить: тихие и громкие треки\nприводятся к одной громкости. Действует со следующего трека."
                )
            }
            Self::Device => {
                t!(
                    "Enter или Left/Right - следующее из устройств, что видит mpv.\nавто оставляет выбор за ним. Действует сразу."
                )
            }
            Self::Likes => {
                t!(
                    "Enter - имя профиля или ссылка, ещё раз Enter - добавить его\nлайки в избранное. Лайки должны быть видны в профиле.\nПока список читается, Enter прерывает его."
                )
            }
        }
    }
}

/// What the interface starts with: the outcome of the flags and the settings file.
pub struct Session {
    pub yt_dlp: String,
    pub mpv: String,
    pub proxy: Option<String>,
    pub direct: bool,
    pub cache: Option<Cache>,
    /// Where the cache is, also while it is switched off.
    pub cache_dir: Option<PathBuf>,
    pub settings: Settings,
    /// The file that changes to the settings are written to.
    pub config: Option<PathBuf>,
}

// The list of color schemes; the one under the cursor is shown at once.
struct Picker {
    state: ListState,
    original: Theme,
}

// What happens when the list a track was started from has been played to its end.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Repeat {
    Off,
    All,
    One,
}
// By their names in the settings; a name that is not known repeats nothing.
const REPEATS: [(&str, Repeat); 3] = [
    ("off", Repeat::Off),
    ("all", Repeat::All),
    ("one", Repeat::One),
];

// Whether the link of an entry leads to a list of tracks and not to one of them.
fn leads_to_list(url: &str) -> bool {
    matches!(
        soundcloud::page(url),
        Some((soundcloud::Page::Set | soundcloud::Page::Listing, _))
    )
}

/// `tracks` in an order of chance. The standard library hands out no random numbers,
/// but the hasher that hash maps seed from the system does.
fn shuffled(mut tracks: Vec<Track>) -> Vec<Track> {
    use std::hash::{BuildHasher, Hasher, RandomState};
    let mut state = RandomState::new().build_hasher().finish() | 1;
    for last in (1..tracks.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        tracks.swap(last, (state % (last as u64 + 1)) as usize);
    }
    tracks
}

// The most that is taken of a list behind a link: a playlist costs a request for each
// of its tracks, and a profile may have thousands.
const LIST_LIMIT: u16 = 500;
// A failed track must not stop the queue, but a dead network must not drain it either.
const SKIP_LIMIT: u8 = 3;
#[derive(Clone, Copy)]
enum Action {
    View(View),
    Search,
    Submit,
    Play,
    Favorite,
    Enqueue,
    Download,
    Evict,
    Previous,
    Pause,
    Next,
    Stop,
    Quieter,
    Louder,
    Shuffle,
    Repeat,
    // The two tabs of the main screen: the lists, and the track that plays.
    Lists,
    Track,
}

// The likes of a profile on their way into the favorites.
struct Import {
    profile: String,
    tracks: Vec<Track>,
    receiver: mpsc::Receiver<Found>,
    cancel: Arc<AtomicBool>,
    worker: std::thread::JoinHandle<()>,
}

// What was fetched for the tab of a track: what was looked up of it, and its picture
// where one was asked for.
type Page = (Option<Details>, Option<Option<Cover>>);

// A track being downloaded into the cache without being played.
struct Fetch {
    // Fetched because it plays next, not because it was asked for: nothing is said of it.
    ahead: bool,
    // Whether what yt-dlp notes of the track is still to be looked for.
    unknown: bool,
    track: Track,
    download: Download,
    log: PathBuf,
}

struct App {
    cancel: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    yt_dlp: String,
    mpv: String,
    proxy: Option<String>,
    direct: bool,
    cache: Option<Cache>,
    cache_dir: Option<PathBuf>,
    settings: Settings,
    config: Option<PathBuf>,
    // The proxy that switching it on brings back, even if only a flag named it.
    address: Option<String>,
    theme: Theme,
    // Whether the header may draw the logo: the console of Linux has no letters for it.
    logo: bool,
    tagline: &'static str,
    // When the interface started, which is what the turning mark is timed by.
    since: Instant,
    setup: Option<Setup>,
    themes: Vec<String>,
    picker: Option<Picker>,
    // The settings window: whether it is open, its current line and where its lines are.
    // It covers the line of messages, so one that came after `greeted` is shown in it.
    greeted: String,
    options: bool,
    setting: usize,
    setting_rows: Rect,
    // The first of the lines that the window shows, when it cannot show them all.
    setting_first: usize,
    // What is being typed into the current line: a proxy address or a profile.
    input: Option<String>,
    import: Option<Import>,
    // How the last import of this run ended, which its line shows until the next one.
    imported: Option<String>,
    // As of `listed`: the tracks in the cache that are not among the favorites, which
    // the library shows after them, and the number and size of all that is in the cache.
    stored: Vec<Track>,
    stored_count: usize,
    stored_size: u64,
    listed: Instant,
    // One download at a time: the connection is better spent on the track that plays.
    fetch: Option<Fetch>,
    pending: VecDeque<Track>,
    library_path: PathBuf,
    library: Library,
    // The file of the library as this process last read or wrote it, and what told it
    // apart then: another process may write it too.
    base: Library,
    stamp: Option<(SystemTime, u64)>,
    synced: Instant,
    view: View,
    // The words the library is narrowed to, and whether they are being typed.
    filter: String,
    filtering: bool,
    results: Vec<Track>,
    // What a search has already answered this run, and the question the list answers.
    // Asking it again is how a search is repeated, so that is never served from here.
    searched: HashMap<(String, u16), Vec<Track>>,
    shown: Option<(String, u16)>,
    // The questions whose lists were shown before the current one, for the way back.
    trail: Vec<String>,
    queue: VecDeque<Track>,
    // What plays when the queue is empty: the rest of the list the current track was
    // started from, in the order it will play, and that whole list to go round again.
    ahead: VecDeque<Track>,
    source: Vec<Track>,
    previous: Vec<Track>,
    current: Option<Track>,
    player: Option<Playback>,
    failures: u8,
    volume: f64,
    // How fast tracks play, for as long as the interface runs.
    speed: f64,
    // The pictures of tracks by the names of their files; None for one that could not
    // be had, which is not asked for again. Where each picture is, as a download told.
    covers: HashMap<String, Option<Cover>>,
    // All that was looked up of tracks, likewise; a download tells it by the way.
    told: HashMap<String, Option<Details>>,
    // The tab of the track that plays, in place of the lists, and how far its text is
    // scrolled.
    track_open: bool,
    scroll: u16,
    // What is being fetched for that tab: the name of the track and its link, and
    // what comes back, the details and then the picture where one was asked for.
    page_job: Option<(String, String, mpsc::Receiver<Page>)>,
    // The picture that was last drawn, as cells.
    drawn: Option<(String, u16, u16, Mode, String, Vec<cover::Cell>)>,
    // The tracks that did not play when they were last tried in this run, by the names
    // of their files.
    broken: std::collections::HashSet<String>,
    // The devices mpv can play on, once it was asked.
    devices: Vec<(String, String)>,
    // The track that was last fetched ahead of its turn: one that fails is not asked for
    // again on every tick.
    fetched_ahead: Option<String>,
    query: String,
    editing: bool,
    searching: bool,
    // Whether the list is already being filled by the search that runs.
    filling: bool,
    receiver: Option<mpsc::Receiver<Found>>,
    table: TableState,
    message: String,
    help: bool,
    details: bool,
    detail_scroll: u16,
    last_error: Option<String>,
    hits: Vec<(Rect, Action)>,
    focus: Option<usize>,
    rows: Rect,
}

impl App {
    fn new(session: Session, library_path: PathBuf, library: Library) -> Self {
        // What was to play the last time waits to be played now.
        let waiting: VecDeque<Track> = library.queue.iter().cloned().collect();
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            worker: None,
            yt_dlp: session.yt_dlp,
            mpv: session.mpv,
            address: session.settings.proxy.clone().or(session.proxy.clone()),
            proxy: session.proxy,
            direct: session.direct,
            cache: session.cache,
            cache_dir: session.cache_dir,
            theme: Theme::load(&session.settings.theme),
            logo: !std::env::var("TERM").is_ok_and(|term| term == "linux"),
            tagline: tagline(),
            since: Instant::now(),
            setup: None,
            themes: vec![],
            picker: None,
            greeted: String::new(),
            options: false,
            setting: 0,
            setting_rows: Rect::default(),
            setting_first: 0,
            input: None,
            import: None,
            imported: None,
            stored: vec![],
            stored_count: 0,
            stored_size: 0,
            listed: Instant::now(),
            volume: f64::from(session.settings.volume),
            settings: session.settings,
            config: session.config,
            fetch: None,
            pending: VecDeque::new(),
            base: library.clone(),
            stamp: library::stamp(&library_path),
            synced: Instant::now(),
            library_path,
            library,
            view: View::Search,
            filter: String::new(),
            filtering: false,
            results: vec![],
            searched: HashMap::new(),
            shown: None,
            trail: vec![],
            message: match waiting.len() {
                0 => t!("Нажмите /, чтобы найти музыку. ? - все клавиши").into(),
                count => t!("С прошлого раза в очереди треков: {}. n - включить", count),
            },
            queue: waiting,
            ahead: VecDeque::new(),
            source: vec![],
            previous: vec![],
            current: None,
            player: None,
            failures: 0,
            speed: 1.0,
            covers: HashMap::new(),
            told: HashMap::new(),
            track_open: false,
            scroll: 0,
            page_job: None,
            drawn: None,
            broken: Default::default(),
            devices: vec![],
            fetched_ahead: None,
            query: String::new(),
            editing: false,
            searching: false,
            filling: false,
            receiver: None,
            table: TableState::default().with_selected(0),

            help: false,
            details: false,
            detail_scroll: 0,
            last_error: None,
            hits: vec![],
            focus: None,
            rows: Rect::default(),
        }
    }
    fn extractor(&self) -> Extractor<'_> {
        Extractor {
            program: &self.yt_dlp,
            proxy: self.proxy.as_deref(),
            no_proxy: self.direct,
            cache: self.cache.as_ref().map(Cache::extractor),
        }
    }
    fn tracks(&self) -> Vec<&Track> {
        match self.view {
            View::Search => self.results.iter().collect(),
            // What the user keeps: the favorites, then the rest of what is stored,
            // narrowed to those that have every word of the filter in their names.
            View::Library => {
                let words: Vec<String> = (self.filter.split_whitespace())
                    .map(str::to_lowercase)
                    .collect();
                (self.library.favorites.iter())
                    .chain(&self.stored)
                    .filter(|track| {
                        words.is_empty() || {
                            let names = format!("{} {}", track.artist, track.title).to_lowercase();
                            words.iter().all(|word| names.contains(word))
                        }
                    })
                    .collect()
            }
            // What will play, in its order: the queue, then the rest of the list.
            View::Queue => self.queue.iter().chain(&self.ahead).collect(),
            View::Recent => self.library.recent.iter().collect(),
        }
    }
    // How the list plays, in words: for the player's frame and for the messages.
    fn order(&self) -> &'static str {
        if self.settings.shuffle {
            t!("вперемешку")
        } else {
            t!("по порядку")
        }
    }
    fn repeating(&self) -> &'static str {
        match self.repeat() {
            Repeat::Off => t!("без повтора"),
            Repeat::All => t!("повтор списка"),
            Repeat::One => t!("повтор трека"),
        }
    }
    fn repeat(&self) -> Repeat {
        (REPEATS.iter())
            .find(|(name, _)| *name == self.settings.repeat)
            .map_or(Repeat::Off, |(_, repeat)| *repeat)
    }
    // Whether a track would follow the current one.
    fn more(&self) -> bool {
        !self.queue.is_empty()
            || !self.ahead.is_empty()
            || (self.repeat() == Repeat::All && !self.source.is_empty())
    }
    // Takes a track out of what will play, by its place in the view of the queue.
    fn take_upcoming(&mut self, index: usize) -> Option<Track> {
        match index.checked_sub(self.queue.len()) {
            None => self.queue.remove(index),
            Some(index) => self.ahead.remove(index),
        }
    }
    // The track at `index` of `list` is about to play: the rest of the list follows it,
    // but for the playlists that a page lists among its tracks.
    fn follow(&mut self, list: Vec<Track>, index: usize) {
        let chosen = list[index].url.clone();
        let list: Vec<Track> = (list.into_iter())
            .filter(|track| track.url == chosen || !leads_to_list(&track.url))
            .collect();
        let index = (list.iter())
            .position(|track| track.url == chosen)
            .unwrap_or(0);
        self.ahead = if self.settings.shuffle {
            let mut rest = list.clone();
            rest.remove(index);
            shuffled(rest).into()
        } else {
            list[index + 1..].to_vec().into()
        };
        self.source = list;
    }
    fn selected(&self) -> Option<Track> {
        // On its tab, the track that plays is the one the keys are about.
        if self.track_open {
            return self.current.clone();
        }
        self.tracks()
            .get(self.table.selected().unwrap_or(0))
            .map(|t| (*t).clone())
    }
    fn select_view(&mut self, view: View) {
        self.view = view;
        self.track_open = false;
        self.table = TableState::default().with_selected(0);
        self.editing = false;
        self.filtering = false;
        if view == View::Library {
            self.list_stored();
        }
    }
    fn list_stored(&mut self) {
        let (tracks, size) = match &self.cache {
            Some(cache) => (cache.tracks(), cache.usage().1),
            None => (vec![], 0),
        };
        // A favorite may spell the link to a stored track differently.
        let favorites: std::collections::HashSet<String> = (self.library.favorites.iter())
            .filter_map(|track| cache::key(&track.url))
            .collect();
        (self.stored_count, self.stored_size) = (tracks.len(), size);
        self.stored = tracks
            .into_iter()
            .filter(|track| cache::key(&track.url).is_none_or(|key| !favorites.contains(&key)))
            .collect();
        self.listed = Instant::now();
        if self.view == View::Library {
            self.navigate(0);
        }
    }
    fn evict(&mut self) {
        let Some(track) = self.selected() else {
            return;
        };
        let Some(cache) = &self.cache else {
            self.message = t!("Кеш отключён").into();
            return;
        };
        self.message = match cache.remove(&track.url) {
            Ok(true) => t!("Удалено из кеша: {} - {}", track.artist, track.title),
            Ok(false) => t!("Этого трека нет в кеше").into(),
            Err(error) => t!("Не удалось удалить из кеша: {}", error),
        };
        self.list_stored();
    }
    fn navigate(&mut self, delta: isize) {
        // The tab of the track has a text to move through, not a list.
        if self.track_open {
            let delta = delta.clamp(-1000, 1000) as i16;
            self.scroll = self.scroll.saturating_add_signed(delta).min(1000);
            return;
        }
        let len = self.tracks().len();
        self.table.select(if len == 0 {
            None
        } else {
            Some(
                self.table
                    .selected()
                    .unwrap_or(0)
                    .saturating_add_signed(delta)
                    .min(len - 1),
            )
        });
    }
    fn save(&mut self) {
        // What another process wrote meanwhile is not written over.
        self.sync();
        // What plays and what is ahead of it is kept for the next run, up to a point:
        // a list may be thousands of tracks long.
        self.library.queue = (self.current.iter())
            .chain(&self.queue)
            .chain(&self.ahead)
            .take(500)
            .cloned()
            .collect();
        match library::save(&self.library_path, &self.library) {
            Ok(()) => {
                self.base = self.library.clone();
                self.stamp = library::stamp(&self.library_path);
            }
            Err(error) => self.message = t!("Не удалось сохранить библиотеку: {}", error),
        }
    }
    /// Takes over what another process did to the library file since this one last
    /// read or wrote it, such as an import of likes; true if the library changed.
    fn sync(&mut self) -> bool {
        self.synced = Instant::now();
        let stamp = library::stamp(&self.library_path);
        // A file that is gone tells nothing of what was removed from it.
        if stamp == self.stamp || stamp.is_none() {
            return false;
        }
        let Ok(disk) = library::load(&self.library_path) else {
            return false;
        };
        let changed = self.library.adopt(&self.base, &disk);
        self.base = disk;
        self.stamp = stamp;
        changed
    }
    /// What has become known of the track at `url` replaces what its link or a list of
    /// likes told, wherever the track is listed. One with a length was looked up before.
    fn learn(&mut self, url: &str, details: &Details) {
        let Some(name) = cache::key(url) else {
            return;
        };
        // What a download or a lookup told in full is kept for the tab of the track.
        if details.title.is_some() {
            self.told.insert(name.clone(), Some(details.clone()));
        }
        let known = |track: &mut Track| {
            let same = track.duration.is_none() && cache::key(&track.url).as_deref() == Some(&name);
            if same {
                details.apply(track);
            }
            same
        };
        let mut kept = None;
        for track in (self.library.favorites.iter_mut()).chain(&mut self.library.recent) {
            if known(track) {
                kept = Some(track.clone());
            }
        }
        (self.results.iter_mut())
            .chain(&mut self.queue)
            .chain(&mut self.ahead)
            .chain(&mut self.source)
            .chain(&mut self.previous)
            .chain(&mut self.stored)
            .chain(&mut self.pending)
            .chain(&mut self.current)
            .for_each(|track| {
                known(track);
            });
        if let Some(track) = kept {
            if let Some(cache) = &self.cache {
                cache.correct(&track);
            }
            self.save();
        }
    }
    fn persist(&mut self) {
        let Some(file) = &self.config else {
            self.message = t!("Файл настроек не определён: изменение действует до выхода").into();
            return;
        };
        if let Err(error) = self.settings.save(file) {
            self.message = t!("Не удалось сохранить настройки: {}", error);
        }
    }
    fn value(&self, setting: Setting) -> String {
        let switch = |on: bool| if on { t!("вкл") } else { t!("выкл") }.to_owned();
        match setting {
            Setting::Theme => self.theme.name.clone(),
            Setting::Backdrop => match BACKDROPS[self.backdrop()].0 {
                "reaper" => t!("жнец"),
                "pentagram" => t!("пентаграмма"),
                _ => t!("нет"),
            }
            .into(),
            Setting::Cover => match Mode::parse(&self.settings.cover) {
                Mode::Blocks => t!("цветные блоки"),
                Mode::Braille => t!("цветной брайль"),
                Mode::BlockTones => t!("блоки в тонах"),
                Mode::BrailleTones => t!("брайль в тонах"),
                Mode::Off => t!("нет"),
            }
            .into(),
            Setting::Language => lang::current().name().into(),
            Setting::Proxy if self.proxy.is_none() && self.extractor().proxied() => {
                t!("выкл, действует прокси из окружения").into()
            }
            Setting::Proxy => switch(self.proxy.is_some()),
            _ if self.input.is_some() && SETTINGS[self.setting] == setting => {
                format!("{}_", self.input.as_deref().unwrap_or_default())
            }
            Setting::ProxyAddress => match &self.address {
                Some(address) => address.clone(),
                None => t!("не задан").into(),
            },
            Setting::Cache => switch(self.cache.is_some()),
            Setting::CacheLimit => match self.settings.cache_limit_mb {
                0 => t!("без ограничения").into(),
                limit => t!("{} МБ", limit),
            },
            Setting::SearchLimit => self.settings.search_limit.to_string(),
            Setting::SeekStep => t!("{} с", self.settings.seek_step),
            Setting::Volume => self.settings.volume.to_string(),
            Setting::Normalize => switch(self.settings.normalize),
            Setting::Device => match &self.settings.audio_device {
                None => t!("авто").into(),
                // Called what mpv calls it, once mpv was asked; by its name until then.
                Some(device) => (self.devices.iter())
                    .find(|(name, described)| name == device && !described.is_empty())
                    .map_or_else(|| device.clone(), |(_, described)| described.clone()),
            },
            Setting::Likes => match (&self.import, &self.imported) {
                (Some(import), _) => {
                    t!("{}: получено {}", import.profile, import.tracks.len())
                }
                (None, Some(outcome)) => outcome.clone(),
                (None, None) => match &self.settings.soundcloud_profile {
                    Some(profile) => profile.clone(),
                    None => t!("не задан").into(),
                },
            },
        }
    }
    fn set_theme(&mut self, name: &str) {
        self.theme = Theme::load(name);
        self.settings.theme = name.into();
        self.persist();
    }
    fn themes(&mut self) -> usize {
        if self.themes.is_empty() {
            self.themes = theme::names();
        }
        self.themes
            .iter()
            .position(|name| *name == self.settings.theme)
            .unwrap_or(0)
    }
    // The place of the chosen drawing; a name that is not known shows none.
    fn backdrop(&self) -> usize {
        BACKDROPS
            .iter()
            .position(|(name, _)| *name == self.settings.backdrop)
            .unwrap_or(0)
    }
    fn move_setting(&mut self, delta: isize) {
        self.setting = self
            .setting
            .saturating_add_signed(delta)
            .min(SETTINGS.len() - 1);
    }
    // Enter on a setting.
    fn activate(&mut self) {
        match SETTINGS[self.setting] {
            Setting::Theme => {
                let current = self.themes();
                self.picker = Some(Picker {
                    state: ListState::default().with_selected(Some(current)),
                    original: self.theme.clone(),
                });
            }
            Setting::Proxy => {
                if self.proxy.is_some() {
                    self.proxy = None;
                } else if let Some(address) = &self.address {
                    self.proxy = Some(address.clone());
                } else {
                    self.message = t!("Сначала задайте адрес прокси строкой ниже").into();
                    return;
                }
                self.settings.proxy_enabled = self.proxy.is_some();
                self.persist();
            }
            Setting::ProxyAddress => {
                self.input = Some(self.address.clone().unwrap_or_else(|| config::TOR.into()));
            }
            Setting::Likes if self.import.is_some() => self.cancel_import(),
            Setting::Likes => {
                self.input = Some(self.settings.soundcloud_profile.clone().unwrap_or_default());
            }
            Setting::Cache => {
                if self.cache.take().is_some() {
                    self.fetch = None;
                    self.pending.clear();
                } else {
                    let limit = self.settings.cache_limit_mb.saturating_mul(1024 * 1024);
                    let opened = self
                        .cache_dir
                        .clone()
                        .ok_or_else(|| t!("Каталог кеша не определён; задайте --cache-dir.").into())
                        .and_then(|root| Cache::open(root, limit));
                    match opened {
                        Ok(cache) => self.cache = Some(cache),
                        Err(error) => {
                            self.message = error.to_string();
                            return;
                        }
                    }
                }
                self.settings.cache_enabled = self.cache.is_some();
                self.list_stored();
                self.persist();
            }
            _ => self.adjust(1),
        }
    }
    // Left and right on a setting.
    fn adjust(&mut self, delta: i8) {
        let step = |value: u64, by: u64, most: u64| {
            if delta < 0 {
                value.saturating_sub(by)
            } else {
                (value + by).min(most)
            }
        };
        match SETTINGS[self.setting] {
            Setting::Theme => {
                let (current, count) = (self.themes(), self.themes.len());
                let next = (current + count).saturating_add_signed(isize::from(delta)) % count;
                let name = self.themes[next].clone();
                self.set_theme(&name);
                return;
            }
            Setting::Backdrop => {
                let count = BACKDROPS.len();
                let next = (self.backdrop() + count).saturating_add_signed(isize::from(delta));
                self.settings.backdrop = BACKDROPS[next % count].0.into();
            }
            Setting::Cover => {
                let at = (cover::MODES.iter())
                    .position(|(_, mode)| *mode == Mode::parse(&self.settings.cover))
                    .unwrap_or(0);
                let count = cover::MODES.len();
                let next = (at + count).saturating_add_signed(isize::from(delta)) % count;
                self.settings.cover = cover::MODES[next].0.into();
            }
            Setting::Language => {
                let at = (lang::ALL.iter())
                    .position(|language| *language == lang::current())
                    .unwrap_or(0);
                let count = lang::ALL.len();
                let next = (at + count).saturating_add_signed(isize::from(delta)) % count;
                lang::set(lang::ALL[next]);
                self.settings.language = lang::ALL[next].code().into();
                // The line at the bottom still reads in the language it was written in.
                self.message = t!("Нажмите /, чтобы найти музыку. ? - все клавиши").into();
            }
            Setting::Proxy | Setting::Cache => return self.activate(),
            Setting::ProxyAddress | Setting::Likes => return,
            Setting::CacheLimit => {
                self.settings.cache_limit_mb = step(self.settings.cache_limit_mb, 256, 1024 * 1024);
                if let Some(cache) = &mut self.cache {
                    cache.set_limit(self.settings.cache_limit_mb.saturating_mul(1024 * 1024));
                }
            }
            Setting::SearchLimit => {
                self.settings.search_limit =
                    step(u64::from(self.settings.search_limit), 5, 50).max(5) as u8;
            }
            Setting::SeekStep => {
                let at = SEEK_STEPS
                    .iter()
                    .position(|step| *step >= self.settings.seek_step)
                    .unwrap_or(SEEK_STEPS.len() - 1);
                let next = at.saturating_add_signed(isize::from(delta));
                self.settings.seek_step = SEEK_STEPS[next.min(SEEK_STEPS.len() - 1)];
            }
            Setting::Volume => {
                self.settings.volume = step(u64::from(self.settings.volume), 5, 100) as u8;
            }
            Setting::Normalize => self.settings.normalize = !self.settings.normalize,
            Setting::Device => {
                // mpv is asked once what it can play on; its own choice is the first.
                if self.devices.is_empty() {
                    self.devices = player::devices(&self.mpv);
                    self.devices.retain(|(name, _)| name != "auto");
                }
                if self.devices.is_empty() {
                    self.message = t!("mpv не назвал ни одного устройства").into();
                    return;
                }
                let count = self.devices.len() + 1;
                let at = (self.settings.audio_device.as_ref())
                    .and_then(|device| self.devices.iter().position(|(name, _)| name == device))
                    .map_or(0, |at| at + 1);
                let next = (at + count).saturating_add_signed(isize::from(delta)) % count;
                self.settings.audio_device =
                    next.checked_sub(1).map(|at| self.devices[at].0.clone());
                // What plays now moves to the device at once.
                let device = self.settings.audio_device.as_deref().unwrap_or("auto");
                if let Some(player) = &mut self.player {
                    let _ = player.send(json!(["set_property", "audio-device", device]));
                }
            }
        }
        self.persist();
    }
    // Enter on what was typed into the current line.
    fn submit(&mut self) {
        match SETTINGS[self.setting] {
            Setting::Likes => self.submit_profile(),
            _ => self.submit_address(),
        }
    }
    // Enter on the profile that was typed: its likes are read into the favorites.
    fn submit_profile(&mut self) {
        let Some(input) = self.input.take() else {
            return;
        };
        self.imported = None;
        // An empty line forgets the profile and asks for nothing.
        if input.trim().is_empty() {
            self.settings.soundcloud_profile = None;
            self.persist();
            return;
        }
        let profile = match soundcloud::profile(&input) {
            Ok(profile) => profile,
            Err(error) => {
                self.message = error.to_string();
                self.input = Some(input);
                return;
            }
        };
        self.settings.soundcloud_profile = Some(profile.clone());
        self.persist();
        let (sender, receiver) = mpsc::channel();
        let (binary, proxy, direct) = (self.yt_dlp.clone(), self.proxy.clone(), self.direct);
        let cache = self
            .cache
            .as_ref()
            .map(|cache| cache.extractor().to_owned());
        let cancel = Arc::new(AtomicBool::new(false));
        let (stop, name) = (cancel.clone(), profile.clone());
        let language = lang::current();
        let worker = std::thread::spawn(move || {
            // The errors of the listing are worded in this thread.
            lang::set(language);
            let result = SoundCloud::new(Extractor {
                program: &binary,
                proxy: proxy.as_deref(),
                no_proxy: direct,
                cache: cache.as_deref(),
            })
            .quiet()
            .cancellable(&stop)
            .likes(&name, |track| {
                let _ = sender.send(Found::Track(track));
            })
            .map_err(|e| e.to_string());
            let _ = sender.send(Found::Over(result));
        });
        self.import = Some(Import {
            profile,
            tracks: vec![],
            receiver,
            cancel,
            worker,
        });
    }
    // The worker kills yt-dlp; what it listed so far is dropped with it.
    fn cancel_import(&mut self) {
        if let Some(import) = self.import.take() {
            import.cancel.store(true, Ordering::Relaxed);
            self.imported = Some(t!("{}: прервано", import.profile));
        }
    }
    // The likes that were listed join the favorites; `failure` is why the list broke off.
    fn finish_import(&mut self, import: Import, failure: Option<String>) {
        let _ = import.worker.join();
        let merged = self.library.merge(import.tracks);
        self.message = t!(
            "Лайки {}: добавлено {}, уже было {}, пропущено {}",
            import.profile,
            merged.added,
            merged.known,
            merged.skipped
        );
        self.imported = Some(if failure.is_some() {
            t!("{}: оборвалось, добавлено {}", import.profile, merged.added)
        } else {
            t!(
                "{}: добавлено {}, уже было {}",
                import.profile,
                merged.added,
                merged.known
            )
        });
        // As far as a list came it is kept: asking again adds the rest.
        if let Some(error) = failure {
            self.message =
                t!("Список лайков оборвался. e - подробности, Esc - закрыть окно").into();
            self.last_error = Some(error);
            self.details = true;
            self.detail_scroll = 0;
        }
        if merged.added > 0 {
            self.save();
            // The favorites that are stored leave the other part of the library.
            self.list_stored();
        }
    }
    // Enter on the proxy address that was typed.
    fn submit_address(&mut self) {
        let Some(input) = self.input.take() else {
            return;
        };
        let input = input.trim();
        if input.is_empty() {
            self.address = None;
            self.proxy = None;
            self.settings.proxy = None;
            self.settings.proxy_enabled = false;
        } else {
            match config::validate(input) {
                Ok(address) => {
                    if self.proxy.is_some() {
                        self.proxy = Some(address.clone());
                    }
                    self.settings.proxy = Some(address.clone());
                    self.address = Some(address);
                }
                Err(error) => {
                    self.message = error.to_string();
                    self.input = Some(input.to_owned());
                    return;
                }
            }
        }
        self.persist();
    }
    // The scheme under the cursor of the list replaces the current one for a look.
    fn pick(&mut self, delta: isize) {
        let Some(picker) = &mut self.picker else {
            return;
        };
        let last = self.themes.len().saturating_sub(1);
        let at = picker
            .state
            .selected()
            .unwrap_or(0)
            .saturating_add_signed(delta)
            .min(last);
        picker.state.select(Some(at));
        self.theme = Theme::load(&self.themes[at]);
    }
    fn close_picker(&mut self, keep: bool) {
        let Some(picker) = self.picker.take() else {
            return;
        };
        match picker.state.selected().filter(|_| keep) {
            Some(at) => {
                let name = self.themes[at].clone();
                self.set_theme(&name);
            }
            None => self.theme = picker.original,
        }
    }
    /// Where yt-dlp would keep what it has yet to learn about SoundCloud; None when
    /// there is no cache to keep it in, or when something is kept there already.
    fn to_learn(&self) -> Option<PathBuf> {
        let cache = self
            .cache
            .as_ref()
            .map(|cache| cache.extractor().to_owned())?;
        let kept = fs::read_dir(&cache).is_ok_and(|mut kept| kept.next().is_some());
        (!kept).then_some(cache)
    }

    /// Lets yt-dlp learn what it needs about SoundCloud before it is asked for music.
    ///
    /// Its first request of a run resolves the client id of SoundCloud and keeps it in
    /// its cache, so doing that now spares the first search of the run a round trip.
    /// Nothing is asked where there is no cache to keep it in, or where it is kept
    /// already: that is every run after the first.
    fn warm(&self) {
        let Some(cache) = self.to_learn() else {
            return;
        };
        let (binary, proxy, direct) = (self.yt_dlp.clone(), self.proxy.clone(), self.direct);
        std::thread::spawn(move || {
            // The results are thrown away; what yt-dlp learned on the way is kept.
            let _ = SoundCloud::new(Extractor {
                program: &binary,
                proxy: proxy.as_deref(),
                no_proxy: direct,
                cache: Some(&cache),
            })
            .quiet()
            .search("a", 1);
        });
    }

    /// Fetches yt-dlp into the directory of the client, when that is what is missing.
    fn install(&mut self) {
        let Some(setup) = &mut self.setup else {
            return;
        };
        if setup.busy() || !setup.yt_dlp {
            return;
        }
        let Some(directory) = setup::directory() else {
            setup.message = t!("Непонятно, куда положить программу").into();
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let proxy = self.proxy.clone();
        let language = lang::current();
        std::thread::spawn(move || {
            // The errors of the download are worded in this thread.
            lang::set(language);
            let fetched = setup::install(proxy.as_deref(), &directory).map_err(|e| e.to_string());
            let _ = sender.send(fetched);
        });
        setup.receiver = Some(receiver);
        setup.message = t!("Скачиваю и проверяю...").into();
    }

    fn search(&mut self) {
        if self.searching {
            self.message = t!("Поиск уже выполняется...").into();
            return;
        }
        if self.query.trim().is_empty() {
            self.message = t!("Введите исполнителя или название трека").into();
            return;
        }
        self.last_error = None;
        // A link is opened as the list it leads to; anything else is searched for.
        let link = soundcloud::page(&self.query).is_some();
        let limit = if link {
            LIST_LIMIT
        } else {
            self.settings.search_limit.into()
        };
        let asked = (self.query.trim().to_owned(), limit);
        if let Some((before, _)) = &self.shown
            && *before != asked.0
        {
            self.trail.push(before.clone());
            if self.trail.len() > 50 {
                self.trail.remove(0);
            }
        }
        if self.shown.as_ref() != Some(&asked)
            && let Some(tracks) = self.searched.get(&asked)
        {
            let found = t!("Найдено треков: {}", tracks.len());
            self.results = tracks.clone();
            self.shown = Some(asked);
            self.select_view(View::Search);
            self.message = found;
            return;
        }
        self.shown = Some(asked);
        let (sender, receiver) = mpsc::channel();
        let (binary, proxy, direct, query) = (
            self.yt_dlp.clone(),
            self.proxy.clone(),
            self.direct,
            self.query.clone(),
        );
        let cache = self
            .cache
            .as_ref()
            .map(|cache| cache.extractor().to_owned());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let language = lang::current();
        self.worker = Some(std::thread::spawn(move || {
            // The errors of the search are worded in this thread.
            lang::set(language);
            let result = SoundCloud::new(Extractor {
                program: &binary,
                proxy: proxy.as_deref(),
                no_proxy: direct,
                cache: cache.as_deref(),
            })
            .quiet()
            .cancellable(&cancel)
            .ask(&query, limit, |track| {
                let _ = sender.send(Found::Track(track));
            })
            .map_err(|e| e.to_string());
            let _ = sender.send(Found::Over(result));
        }));
        self.receiver = Some(receiver);
        self.searching = true;
        self.filling = false;
        self.editing = false;
        self.select_view(View::Search);
        self.message = if link {
            t!("Читаю список... Esc - отмена")
        } else {
            t!("Ищем треки... Esc - отмена")
        }
        .into();
    }
    // Puts a link where a question is typed and asks it: lists are opened like that.
    fn open(&mut self, link: String) {
        self.query = link;
        self.search();
    }
    // Back to the list that was shown before this one.
    fn back(&mut self) {
        if self.searching {
            self.cancel_search();
        }
        let Some(before) = self.trail.pop() else {
            self.message = t!("Раньше ничего не открывалось").into();
            return;
        };
        self.query = before;
        // The list to leave is not one to come back to.
        self.shown = None;
        self.search();
    }
    // The station of the selected track: that track and what SoundCloud plays after it.
    fn similar(&mut self) {
        match self
            .selected()
            .and_then(|track| soundcloud::station(&track.url))
        {
            Some(station) => self.open(station),
            None => self.message = t!("Станция есть только у отдельного трека").into(),
        }
    }
    // The page of whoever published the selected track or list, at its tracks.
    fn author(&mut self) {
        match self
            .selected()
            .and_then(|track| soundcloud::owner(&track.url))
        {
            Some((profile, _)) => self.open(soundcloud::section(&profile, 0)),
            None => self.message = t!("У этой ссылки нет страницы автора").into(),
        }
    }
    // The profile whose page is shown and the section of it that the page is.
    fn profile(&self) -> Option<(String, Option<usize>)> {
        let (question, _) = self.shown.as_ref()?;
        match soundcloud::page(question)? {
            (soundcloud::Page::Listing, link) => soundcloud::owner(&link),
            _ => None,
        }
    }
    // The next or the previous section of the profile whose page is shown.
    fn turn(&mut self, delta: isize) {
        if self.view != View::Search {
            return;
        }
        let Some((profile, at)) = self.profile() else {
            return;
        };
        let count = soundcloud::SECTIONS.len();
        let next = match at {
            Some(at) => (at + count).saturating_add_signed(delta) % count,
            None => 0,
        };
        self.open(soundcloud::section(&profile, next));
    }
    // Every track of the list that is shown joins the favorites.
    fn favor_all(&mut self) {
        let tracks: Vec<Track> = self.tracks().into_iter().cloned().collect();
        let merged = self.library.merge(tracks);
        self.message = t!(
            "В избранное добавлено: {}, уже было: {}",
            merged.added,
            merged.known
        );
        if merged.added > 0 {
            self.save();
            self.list_stored();
        }
    }
    fn cancel_search(&mut self) {
        // The worker kills yt-dlp; its late result goes nowhere.
        self.cancel.store(true, Ordering::Relaxed);
        self.receiver = None;
        self.searching = false;
        self.message = t!("Поиск отменён").into();
    }
    fn start(&mut self, track: Track) {
        self.player = None;
        self.last_error = None;
        // Being fetched ahead of its turn, the track is fetched by its player instead:
        // two downloads of it would write the same place.
        if self
            .fetch
            .as_ref()
            .is_some_and(|f| f.track.url == track.url)
        {
            self.fetch = None;
        }
        match Playback::start(
            &self.mpv,
            self.extractor(),
            &track,
            self.cache.as_ref(),
            self.settings.sound(),
            self.volume,
            self.speed,
        ) {
            Ok(player) => {
                self.message = match player.origin {
                    Origin::Cache => t!("Трек из кеша"),
                    Origin::Download => t!("Загрузка трека; перемотка - в пределах загруженного"),
                    Origin::Pipe => t!("Загрузка через прокси, перемотка ограничена буфером"),
                    Origin::Url => t!("Подключение к аудиопотоку..."),
                }
                .into();
                self.library.recent.retain(|t| t.url != track.url);
                self.library.recent.insert(0, track.clone());
                self.library.recent.truncate(30);
                self.current = Some(track);
                self.player = Some(player);
                self.save();
            }
            Err(error) => {
                self.current = None;
                self.message = error.to_string();
                self.last_error = Some(self.message.clone());
            }
        }
    }
    fn next(&mut self) {
        let track = (self.queue.pop_front())
            .or_else(|| self.ahead.pop_front())
            .or_else(|| {
                // The list is over: round it again, if that is what was asked for.
                if self.repeat() != Repeat::All {
                    return None;
                }
                let list = self.source.clone();
                self.ahead = if self.settings.shuffle {
                    shuffled(list).into()
                } else {
                    list.into()
                };
                self.ahead.pop_front()
            });
        if let Some(track) = track {
            if let Some(current) = self.current.take() {
                self.previous.push(current);
                // A list that goes round would keep every round of it.
                if self.previous.len() > 200 {
                    self.previous.remove(0);
                }
            }
            self.start(track);
        } else {
            self.player = None;
            self.message = t!("Очередь закончилась").into();
        }
    }
    // The player went on to the next track by itself: the lists follow it there.
    fn advanced(&mut self) {
        let Some(track) = self.queue.pop_front().or_else(|| self.ahead.pop_front()) else {
            return;
        };
        if let Some(current) = self.current.take() {
            self.previous.push(current);
            if self.previous.len() > 200 {
                self.previous.remove(0);
            }
        }
        // Played now, it is the last one to be evicted.
        if let Some(cache) = &self.cache {
            cache.find(&track.url);
        }
        self.failures = 0;
        self.library.recent.retain(|t| t.url != track.url);
        self.library.recent.insert(0, track.clone());
        self.library.recent.truncate(30);
        self.current = Some(track);
        self.save();
    }
    // The current track played to its end.
    fn ended(&mut self) {
        match self.current.clone() {
            Some(track) if self.repeat() == Repeat::One => self.start(track),
            _ => self.next(),
        }
    }
    fn failed(&mut self, error: String, details: Option<String>) {
        self.player = None;
        self.failures += 1;
        // Marked in its lists until it plays: for this run, since the network may be
        // the reason as well as the track.
        if let Some(name) = self.current.as_ref().and_then(|t| cache::key(&t.url)) {
            self.broken.insert(name);
        }
        let mut report = error.clone();
        if let Some(track) = &self.current {
            report = format!("{} - {}\n{report}", track.artist, track.title);
        }
        if let Some(details) = details {
            report = format!("{report}\n\n{details}");
        }
        if !self.more() {
            self.message = error;
        } else if self.failures < SKIP_LIMIT {
            self.next();
            if self.player.is_none() {
                // The next track did not even start; that error is already shown.
                return;
            }
            self.message = t!("Трек не проигрался, включён следующий. e - подробности").into();
        } else {
            self.message =
                t!("Очередь остановлена после нескольких ошибок подряд. e - подробности").into();
        }
        self.last_error = Some(report);
    }
    // Being downloaded or waiting for it, whether for playback or on request.
    fn fetching(&self, url: &str) -> bool {
        self.fetch.as_ref().is_some_and(|f| f.track.url == url)
            || self.pending.iter().any(|t| t.url == url)
            || (self.current.as_ref().is_some_and(|t| t.url == url)
                && self
                    .player
                    .as_ref()
                    .is_some_and(|p| p.origin == Origin::Download && p.receiving()))
    }
    fn download(&mut self, tracks: Vec<Track>) {
        let Some(cache) = &self.cache else {
            self.message = t!("Кеш отключён: треки не сохраняются").into();
            return;
        };
        let (mut added, mut lists) = (0, 0);
        for track in tracks {
            if !cache.accepts(&track.url) {
                lists += 1;
            } else if !cache.contains(&track.url) && !self.fetching(&track.url) {
                self.pending.push_back(track);
                added += 1;
            }
        }
        self.message = match (added, lists) {
            (0, 0) => t!("Уже в кеше или загружается").into(),
            (0, _) => t!("В кеш сохраняются только отдельные треки").into(),
            _ => t!(
                "Загрузка в кеш, осталось треков: {}",
                self.pending.len() + usize::from(self.fetch.is_some())
            ),
        };
        self.fetch_next();
    }
    /// Fetches the track that plays next into the cache while the current one plays, so
    /// that it starts at once. It waits until the current one has all of its audio and
    /// nothing else is being fetched: the connection is first for what is heard.
    fn fetch_ahead(&mut self) {
        let idle = self.fetch.is_none() && self.pending.is_empty();
        let playing = (self.player.as_ref()).is_some_and(|p| p.loaded && !p.receiving());
        let (Some(cache), true) = (&self.cache, idle && playing) else {
            return;
        };
        let Some(next) = self.queue.front().or(self.ahead.front()) else {
            return;
        };
        if !cache.accepts(&next.url)
            || cache.contains(&next.url)
            || self.fetched_ahead.as_deref() == Some(next.url.as_str())
        {
            return;
        }
        let next = next.clone();
        self.fetched_ahead = Some(next.url.clone());
        if let Ok(Some((download, log))) = self.fetch(&next) {
            self.fetch = Some(Fetch {
                ahead: true,
                unknown: next.duration.is_none(),
                track: next,
                download,
                log,
            });
        }
    }
    /// Has what the tab of the track shows ready while it is open: all that is known
    /// of the track that plays, and its picture. Each is read from the cache, or
    /// fetched once in the background.
    fn fetch_page(&mut self) {
        if let Some((name, url, receiver)) = &self.page_job {
            let (details, cover) = match receiver.try_recv() {
                Ok(page) => page,
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => (None, Some(None)),
            };
            // A run plays few tracks; a set that somehow grows large is dropped whole.
            if self.covers.len() >= 64 || self.told.len() >= 256 {
                self.covers.clear();
                self.told.clear();
            }
            let (name, url) = (name.clone(), url.clone());
            self.page_job = None;
            if let Some(cover) = cover {
                self.covers.insert(name.clone(), cover);
            }
            match details {
                // What it tells also puts right a track known by its link alone.
                Some(details) => self.learn(&url, &details),
                None => {
                    self.told.entry(name).or_insert(None);
                }
            }
        }
        let Some(track) = self.current.as_ref().filter(|_| self.track_open) else {
            return;
        };
        let Some(name) = cache::key(&track.url) else {
            return;
        };
        let pictured = Mode::parse(&self.settings.cover) != Mode::Off;
        let (told, drawn) = (
            self.told.contains_key(&name),
            self.covers.contains_key(&name),
        );
        if told && (drawn || !pictured) {
            return;
        }
        let known = self.told.get(&name).cloned().flatten();
        let (binary, proxy, direct) = (self.yt_dlp.clone(), self.proxy.clone(), self.direct);
        let (mpv, url) = (self.mpv.clone(), track.url.clone());
        let learned = (self.cache.as_ref()).map(|cache| cache.extractor().to_owned());
        let root = self.cache.as_ref().map(|cache| cache.root().to_owned());
        let kept = self.cache.as_ref().map(|cache| cache.art().join(&name));
        let link = url.clone();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let extractor = Extractor {
                program: &binary,
                proxy: proxy.as_deref(),
                no_proxy: direct,
                cache: learned.as_deref(),
            };
            // The cache is opened anew: it is the place that matters, not the limit.
            let cache = root.and_then(|root| Cache::open(root, 0).ok());
            let details = known
                .or_else(|| cache.as_ref().and_then(|cache| cache.info(&url)))
                .or_else(|| {
                    let details = SoundCloud::new(extractor).quiet().details(&url).ok()?;
                    if let Some(cache) = &cache {
                        cache.note(&url, &details);
                    }
                    Some(details)
                });
            let picture = || -> Option<Cover> {
                if let Some(cover) = kept.as_deref().and_then(Cover::read) {
                    return Some(cover);
                }
                let artwork = details.as_ref()?.artwork.as_deref()?;
                let scratch = player::scratch_directory().ok()?;
                // Without a cache the pixels live as long as the interface runs.
                let file = match &kept {
                    Some(kept) => {
                        fs::create_dir_all(kept.parent()?).ok()?;
                        kept.clone()
                    }
                    None => scratch.join("pixels"),
                };
                let fetched = cover::fetch(extractor, &mpv, artwork, &scratch, &file);
                let cover = fetched.ok().and_then(|()| Cover::read(&file));
                let _ = fs::remove_dir_all(scratch);
                cover
            };
            let cover = (pictured && !drawn).then(picture);
            let _ = sender.send((details, cover));
        });
        self.page_job = Some((name, link, receiver));
    }
    // Faster or slower by a tenth, between half and twice the speed of the recording.
    fn pace(&mut self, tenths: f64) {
        self.speed = ((self.speed * 10.0 + tenths).round() / 10.0).clamp(0.5, 2.0);
        if let Some(player) = &mut self.player {
            let _ = player.send(json!(["set_property", "speed", self.speed]));
        }
        self.message = t!("Скорость: x{}", format!("{:.1}", self.speed));
    }
    fn fetch_next(&mut self) {
        while self.fetch.is_none()
            && let Some(track) = self.pending.pop_front()
        {
            match self.fetch(&track) {
                Ok(fetch) => {
                    self.fetch = fetch.map(|(download, log)| Fetch {
                        ahead: false,
                        unknown: track.duration.is_none(),
                        track,
                        download,
                        log,
                    })
                }
                Err(error) => {
                    // Neither the disk nor yt-dlp will do better for the rest.
                    self.pending.clear();
                    self.message = t!("Не удалось начать загрузку: {}", error);
                    self.last_error = Some(self.message.clone());
                }
            }
        }
    }
    fn fetch(&self, track: &Track) -> Result<Option<(Download, PathBuf)>> {
        let Some(cache) = self.cache.as_ref().filter(|c| !c.contains(&track.url)) else {
            return Ok(None);
        };
        let Some(mut partial) = cache.store(&track.url)? else {
            return Ok(None);
        };
        partial.describe(track);
        let log = partial.directory().join("log");
        let mut command = player::downloader(
            self.extractor(),
            &track.url,
            partial.directory(),
            false,
            true,
        );
        command.stderr(fs::File::create(&log)?);
        Ok(Some((Download::start(command, partial)?, log)))
    }
    fn control(&mut self, command: serde_json::Value) {
        if let Some(player) = &mut self.player {
            if let Err(error) = player.send(command) {
                self.message = error.to_string();
            }
        } else {
            self.message = t!("Сначала выберите трек и нажмите Enter").into();
        }
    }
    fn action(&mut self, action: Action) {
        if matches!(
            action,
            Action::Play | Action::Pause | Action::Previous | Action::Next
        ) {
            self.failures = 0;
        }
        match action {
            Action::Submit => self.search(),
            Action::View(view) => self.select_view(view),
            Action::Lists => self.track_open = false,
            Action::Track => {
                self.track_open = true;
                self.editing = false;
                self.filtering = false;
                // What could not be had the last time is asked for once more: the
                // network may have been the reason.
                if let Some(name) = self.current.as_ref().and_then(|t| cache::key(&t.url)) {
                    if self.told.get(&name).is_some_and(Option::is_none) {
                        self.told.remove(&name);
                    }
                    if self.covers.get(&name).is_some_and(Option::is_none) {
                        self.covers.remove(&name);
                    }
                }
            }
            Action::Search => {
                // The tab of the track has no place to type in.
                self.track_open = false;
                self.editing = true;
                self.filtering = false;
                self.focus = None;
            }
            Action::Shuffle => {
                self.settings.shuffle = !self.settings.shuffle;
                // What is ahead changes its order at once; the queue stays as it was built.
                let ahead: Vec<Track> = self.ahead.drain(..).collect();
                let at = (self.current.as_ref())
                    .and_then(|now| self.source.iter().position(|track| track.url == now.url));
                self.ahead = match at {
                    _ if self.settings.shuffle => shuffled(ahead),
                    Some(at) => self.source[at + 1..].to_vec(),
                    None => ahead,
                }
                .into();
                self.message = t!("Порядок: {}", self.order());
                self.persist();
            }
            Action::Repeat => {
                let at = (REPEATS.iter())
                    .position(|(_, repeat)| *repeat == self.repeat())
                    .unwrap_or(0);
                self.settings.repeat = REPEATS[(at + 1) % REPEATS.len()].0.into();
                self.message = t!("Повтор: {}", self.repeating());
                self.persist();
            }
            // The tab of the track has nothing to start but the track itself.
            Action::Play if self.track_open => {
                if self.player.is_some() || self.current.is_some() {
                    self.action(Action::Pause);
                }
            }
            Action::Play => {
                let index = self.table.selected().unwrap_or(0);
                // An entry that is a playlist or a page is opened as the list it is.
                if self.view != View::Queue
                    && let Some(entry) = self.selected()
                    && leads_to_list(&entry.url)
                {
                    return self.open(entry.url);
                }
                let track = if self.view == View::Queue {
                    self.take_upcoming(index)
                } else {
                    // The rest of the list plays after the track, whatever the list is.
                    let list: Vec<Track> = self.tracks().into_iter().cloned().collect();
                    let track = list.get(index).cloned();
                    if track.is_some() {
                        self.follow(list, index);
                    }
                    track
                };
                if let Some(track) = track {
                    if let Some(current) = self.current.take() {
                        self.previous.push(current);
                    }
                    self.start(track);
                    if self.view == View::Recent {
                        // The started track moved to the top of the history.
                        self.table.select(Some(0));
                    }
                }
            }
            Action::Favorite => {
                if let Some(track) = self.selected() {
                    if let Some(index) = self
                        .library
                        .favorites
                        .iter()
                        .position(|t| t.url == track.url)
                    {
                        self.library.favorites.remove(index);
                        self.message = t!("Трек удалён из библиотеки").into();
                    } else {
                        self.library.favorites.push(track);
                        self.message = t!("Трек добавлен в библиотеку").into();
                    }
                    self.save();
                    // A stored track moves between the two parts of the library.
                    self.list_stored();
                    self.navigate(0);
                }
            }
            Action::Enqueue => {
                if let Some(track) = self.selected() {
                    self.queue.push_back(track);
                    self.message = t!("Добавлено в конец очереди").into();
                }
            }
            Action::Download => {
                if let Some(track) = self.selected() {
                    self.download(vec![track]);
                }
            }
            Action::Evict => self.evict(),
            Action::Pause => {
                if self.player.is_some() {
                    self.control(json!(["cycle", "pause"]));
                } else if let Some(track) = self.current.clone() {
                    self.start(track);
                } else {
                    self.action(Action::Play);
                }
            }
            Action::Previous => {
                if let Some(track) = self.previous.pop() {
                    if let Some(current) = self.current.take() {
                        self.queue.push_front(current);
                    }
                    self.start(track);
                }
            }
            Action::Next => self.next(),
            Action::Stop => {
                self.player = None;
                self.message = t!("Воспроизведение остановлено").into();
            }
            Action::Quieter | Action::Louder => {
                let step = if matches!(action, Action::Louder) {
                    5.0
                } else {
                    -5.0
                };
                if let Some(player) = &mut self.player {
                    // Relative, so quick presses add up before mpv reports the new value.
                    match player.send(json!(["add", "volume", step])) {
                        Ok(()) => player.volume = (player.volume + step).clamp(0.0, 100.0),
                        Err(error) => self.message = error.to_string(),
                    }
                    self.volume = player.volume;
                } else {
                    self.volume = (self.volume + step).clamp(0.0, 100.0);
                }
            }
        }
    }
    fn tick(&mut self) {
        // Taken out whole: what arrives is written back into the interface.
        if let Some(mut setup) = self.setup.take() {
            if let Some(receiver) = &setup.receiver {
                match receiver.try_recv() {
                    Ok(Ok(path)) => {
                        setup.receiver = None;
                        setup.yt_dlp = false;
                        setup.message = t!("yt-dlp на месте: {}", path.display());
                        self.yt_dlp = path.to_string_lossy().into_owned();
                    }
                    Ok(Err(error)) => {
                        setup.receiver = None;
                        setup.message = t!("Не вышло: {}", clean_lines(&error));
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        setup.receiver = None;
                        setup.message = t!("Загрузка прервалась").into();
                    }
                    Err(mpsc::TryRecvError::Empty) => (),
                }
            }
            self.setup = Some(setup);
        }
        if let Some(receiver) = self.receiver.take() {
            let (mut over, mut keep) = (None, true);
            loop {
                match receiver.try_recv() {
                    Ok(Found::Track(track)) => {
                        // The tracks of the search before are shown until this one has any.
                        if !self.filling {
                            self.filling = true;
                            self.results.clear();
                            self.table.select(Some(0));
                        }
                        self.results.push(track);
                    }
                    Ok(Found::Over(result)) => {
                        over = Some(result);
                        keep = false;
                        break;
                    }
                    // The worker is gone without a word; nothing more will come.
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.searching = false;
                        self.message = t!("Поиск прерван").into();
                        keep = false;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            if keep {
                self.receiver = Some(receiver);
            }
            match over {
                None => (),
                Some(Ok(())) => {
                    self.searching = false;
                    // A search that found nothing leaves no list of the one before.
                    if !self.filling {
                        self.results.clear();
                        self.table.select(Some(0));
                    }
                    self.message = t!("Найдено треков: {}", self.results.len());
                    if let Some(asked) = self.shown.clone() {
                        // A run asks few questions; a list that somehow grows long is
                        // dropped whole rather than kept in order.
                        if self.searched.len() >= 64 {
                            self.searched.clear();
                        }
                        self.searched.insert(asked, self.results.clone());
                    }
                }
                Some(Err(error)) => {
                    self.searching = false;
                    self.last_error = Some(error);
                    self.message = t!("Ошибка поиска. e - подробности, Esc - закрыть окно").into();
                    self.details = true;
                    self.detail_scroll = 0;
                }
            }
        }
        if let Some(mut import) = self.import.take() {
            let mut over = None;
            loop {
                match import.receiver.try_recv() {
                    Ok(Found::Track(track)) => import.tracks.push(track),
                    Ok(Found::Over(result)) => {
                        over = Some(result.err());
                        break;
                    }
                    // The worker is gone without a word; nothing more will come.
                    Err(mpsc::TryRecvError::Disconnected) => {
                        over = Some(Some(t!("Чтение лайков прервано").into()));
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            match over {
                Some(failure) => self.finish_import(import, failure),
                None => self.import = Some(import),
            }
        }
        // Looked for before the download is asked how it goes: one that is over takes
        // its directory, and what yt-dlp noted in it, along into the cache.
        if let Some(fetch) = &mut self.fetch
            && fetch.unknown
            && let Some(details) = fetch.download.directory().and_then(Details::read)
        {
            fetch.unknown = false;
            let url = fetch.track.url.clone();
            self.learn(&url, &details);
        }
        if let Some(fetch) = &mut self.fetch
            && let Some(saved) = match fetch.download.poll() {
                Ok(status) => status.map(|status| status.success()),
                Err(_) => Some(false),
            }
        {
            let name = format!("{} - {}", fetch.track.artist, fetch.track.title);
            if fetch.ahead {
                // Its turn will tell whether it plays.
            } else if saved {
                self.message = t!("В кеше: {}", name);
            } else {
                let mut report = t!("{}\nyt-dlp не смог загрузить трек в кеш.", name);
                let details = playback::tail(&fetch.log);
                if !details.is_empty() {
                    report = format!("{report}\n\n{}", details.join("\n"));
                }
                self.last_error = Some(report);
                self.message = t!("Трек не загрузился в кеш. e - подробности").into();
            }
            self.fetch = None;
            self.fetch_next();
        }
        self.fetch_ahead();
        self.fetch_page();
        // An import from the command line writes the library while this one is open.
        if self.synced.elapsed() > Duration::from_secs(2) && self.sync() {
            self.list_stored();
            self.navigate(0);
        }
        // Tracks get there from playback and other instances too.
        if self.view == View::Library && self.listed.elapsed() > Duration::from_secs(1) {
            self.list_stored();
        }
        // A track known by its link gets its names from the yt-dlp that fetches it, and
        // its length from mpv once that has the whole of it.
        let unknown = self.current.as_ref().is_some_and(|t| t.duration.is_none());
        let noted = self.player.as_mut().and_then(|player| {
            player.learn().or_else(|| {
                (unknown && player.loaded && !player.receiving() && player.duration > 0.0)
                    .then_some(Details {
                        duration: Some(player.duration),
                        ..Details::default()
                    })
            })
        });
        if let (Some(details), Some(url)) = (noted, self.current.as_ref().map(|t| t.url.clone())) {
            self.learn(&url, &details);
        }
        if let Some(player) = &mut self.player {
            let was_loaded = player.loaded;
            let result = player.tick();
            if !was_loaded && player.loaded {
                if let Some(name) = self.current.as_ref().and_then(|t| cache::key(&t.url)) {
                    self.broken.remove(&name);
                }
                // After a skip the notice about the failed track stays on screen.
                if self.failures == 0 {
                    self.message = if player.origin == Origin::Cache {
                        t!("Воспроизведение из кеша. Space - пауза, n - следующий")
                    } else {
                        t!("Воспроизведение. Space - пауза, n - следующий")
                    }
                    .into();
                }
                self.failures = 0;
            }
            self.volume = player.volume;
            let (advanced, astray) = (player.advanced(), player.astray());
            let outcome = match result {
                Ok(over) => Ok(over),
                Err(error) => Err((error.to_string(), player.details())),
            };
            if advanced {
                self.advanced();
            }
            if astray && let Some(track) = self.current.clone() {
                // It plays what was to follow before that was changed: start over.
                return self.start(track);
            }
            // The same player goes on to the next track where that one is stored: a
            // new player would leave a gap between the two.
            let next = match (&self.cache, self.repeat() != Repeat::One) {
                (Some(cache), true) => (self.queue.front().or(self.ahead.front()))
                    .and_then(|track| cache.stored(&track.url)),
                _ => None,
            };
            if let Some(player) = &mut self.player {
                player.follow(next.as_deref());
            }
            match outcome {
                Ok(true) => self.ended(),
                Err((error, details)) => self.failed(error, details),
                _ => (),
            }
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(import) = self.import.take() {
            import.cancel.store(true, Ordering::Relaxed);
            let _ = import.worker.join();
        }
    }
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
    }
}

/// One of `TAGLINES`, drawn for as long as the interface runs. The standard library
/// hands out no random numbers, but the hasher that hash maps seed from the system
/// does, and one number per run is all this takes.
fn tagline() -> &'static str {
    use std::hash::{BuildHasher, Hasher, RandomState};
    let seed = RandomState::new().build_hasher().finish() as usize;
    TAGLINES[seed % TAGLINES.len()]
}

// A signal leaves the interface through its own path, so that mpv is stopped and the
// terminal restored. Windows has no such signals; there Ctrl+C arrives as a key below.
#[cfg(unix)]
fn terminated() -> Result<Arc<AtomicBool>> {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    let flag = Arc::new(AtomicBool::new(false));
    for signal in [SIGTERM, SIGHUP, SIGINT] {
        signal_hook::flag::register(signal, flag.clone())?;
    }
    Ok(flag)
}

#[cfg(not(unix))]
fn terminated() -> Result<Arc<AtomicBool>> {
    Ok(Arc::new(AtomicBool::new(false)))
}

pub fn run(session: Session, library: Option<PathBuf>) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(t!("Интерфейсу нужен интерактивный терминал. Для скриптов используйте search или play --first.").into());
    }
    let path = library::path(library, "ui")?;
    let library = library::load(&path)?;
    let terminate = terminated()?;
    let mut app = App::new(session, path, library);
    app.setup = Setup::needed(&app.yt_dlp, &app.mpv);
    app.warm();
    terminal::enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        previous_hook(info);
    }));
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    loop {
        // Leave through the normal path so that Drop stops mpv and removes its directory.
        if terminate.load(Ordering::Relaxed) {
            break;
        }
        app.tick();
        terminal.draw(|frame| draw(frame, &mut app))?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    break;
                }
                if app.setup.is_some() {
                    let busy = app.setup.as_ref().is_some_and(Setup::busy);
                    match key.code {
                        KeyCode::Enter if !busy => app.install(),
                        KeyCode::Esc | KeyCode::Char('q') if !busy => app.setup = None,
                        _ => (),
                    }
                    continue;
                }
                if app.help {
                    app.help = false;
                    continue;
                }
                if app.details {
                    match key.code {
                        KeyCode::Down => app.detail_scroll = app.detail_scroll.saturating_add(1),
                        KeyCode::Up => app.detail_scroll = app.detail_scroll.saturating_sub(1),
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('e') => app.details = false,
                        KeyCode::Char('q') => break,
                        _ => (),
                    }
                    continue;
                }
                if app.filtering {
                    match key.code {
                        KeyCode::Esc => {
                            app.filter.clear();
                            app.filtering = false;
                        }
                        KeyCode::Enter => app.filtering = false,
                        KeyCode::Backspace => {
                            app.filter.pop();
                        }
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.filter.clear()
                        }
                        KeyCode::Char(c) if !c.is_control() && app.filter.chars().count() < 200 => {
                            app.filter.push(c)
                        }
                        _ => (),
                    }
                    // The list under the words is another one after every letter.
                    app.table.select(Some(0));
                    app.navigate(0);
                    continue;
                }
                if app.editing {
                    match key.code {
                        KeyCode::Esc => app.editing = false,
                        KeyCode::Enter => app.search(),
                        KeyCode::Backspace => {
                            app.query.pop();
                        }
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.query.clear()
                        }
                        KeyCode::Char(c) if !c.is_control() && app.query.chars().count() < 200 => {
                            app.query.push(c)
                        }
                        _ => (),
                    }
                    continue;
                }
                if app.picker.is_some() {
                    match key.code {
                        KeyCode::Down | KeyCode::Char('j') => app.pick(1),
                        KeyCode::Up | KeyCode::Char('k') => app.pick(-1),
                        KeyCode::PageDown => app.pick(10),
                        KeyCode::PageUp => app.pick(-10),
                        KeyCode::Home => app.pick(isize::MIN),
                        KeyCode::End => app.pick(isize::MAX),
                        KeyCode::Enter => app.close_picker(true),
                        KeyCode::Esc | KeyCode::Char('q') => app.close_picker(false),
                        _ => (),
                    }
                    continue;
                }
                if let Some(input) = &mut app.input {
                    match key.code {
                        KeyCode::Esc => app.input = None,
                        KeyCode::Enter => app.submit(),
                        KeyCode::Backspace => {
                            input.pop();
                        }
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            input.clear()
                        }
                        KeyCode::Char(c) if !c.is_control() && input.chars().count() < 200 => {
                            input.push(c)
                        }
                        _ => (),
                    }
                    continue;
                }
                if app.options {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('o') => app.options = false,
                        KeyCode::Char('q') => break,
                        KeyCode::Down | KeyCode::Char('j') => app.move_setting(1),
                        KeyCode::Up | KeyCode::Char('k') => app.move_setting(-1),
                        KeyCode::Left | KeyCode::Char('h') => app.adjust(-1),
                        KeyCode::Right | KeyCode::Char('l') => app.adjust(1),
                        KeyCode::Enter | KeyCode::Char(' ') => app.activate(),
                        _ => (),
                    }
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('?') => app.help = true,
                    KeyCode::Char('o') => {
                        app.options = true;
                        app.greeted = app.message.clone();
                        app.focus = None;
                    }
                    // Before the letter alone, which is another key.
                    KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.select_view(View::Library);
                        app.filtering = true;
                        app.focus = None;
                    }
                    KeyCode::Char('e') => {
                        app.details = true;
                        app.detail_scroll = 0;
                    }
                    KeyCode::Char('/') => app.action(Action::Search),
                    KeyCode::Char('1') => app.select_view(View::Search),
                    KeyCode::Char('2') => app.select_view(View::Library),
                    KeyCode::Char('3') => app.select_view(View::Queue),
                    KeyCode::Char('4') => app.select_view(View::Recent),
                    KeyCode::Char('t') => app.action(if app.track_open {
                        Action::Lists
                    } else {
                        Action::Track
                    }),
                    KeyCode::Down | KeyCode::Char('j') => {
                        app.focus = None;
                        app.navigate(1);
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        app.focus = None;
                        app.navigate(-1);
                    }
                    KeyCode::PageDown => app.navigate(app.rows.height.max(1) as isize),
                    KeyCode::PageUp => app.navigate(-(app.rows.height.max(1) as isize)),
                    KeyCode::Home | KeyCode::Char('g') => app.navigate(isize::MIN),
                    KeyCode::End | KeyCode::Char('G') => app.navigate(isize::MAX),
                    KeyCode::Tab => {
                        app.focus = Some(app.focus.map_or(0, |i| (i + 1) % app.hits.len().max(1)))
                    }
                    KeyCode::BackTab => {
                        app.focus = Some(
                            app.focus
                                .unwrap_or(0)
                                .checked_sub(1)
                                .unwrap_or(app.hits.len().saturating_sub(1)),
                        )
                    }
                    KeyCode::Esc if app.searching => app.cancel_search(),
                    KeyCode::Esc if app.view == View::Library && !app.filter.is_empty() => {
                        app.filter.clear();
                        app.navigate(0);
                    }
                    KeyCode::Esc if app.track_open => app.track_open = false,
                    KeyCode::Esc => app.focus = None,
                    KeyCode::Enter => app.action(
                        app.focus
                            .and_then(|i| app.hits.get(i).map(|h| h.1))
                            .unwrap_or(Action::Play),
                    ),
                    KeyCode::Char(' ') => app.action(Action::Pause),
                    KeyCode::Char('f') => app.action(Action::Favorite),
                    KeyCode::Char('a') => app.action(Action::Enqueue),
                    KeyCode::Char('d') => app.action(Action::Download),
                    KeyCode::Char('D') => {
                        let tracks = app.tracks().into_iter().cloned().collect();
                        app.download(tracks);
                    }
                    KeyCode::Char('n') => app.action(Action::Next),
                    KeyCode::Char('p') => app.action(Action::Previous),
                    KeyCode::Char('s') => app.action(Action::Stop),
                    KeyCode::Char('m') => app.similar(),
                    KeyCode::Char('u') => app.author(),
                    KeyCode::Char(']') => app.turn(1),
                    KeyCode::Char('[') => app.turn(-1),
                    KeyCode::Char('b') => app.back(),
                    KeyCode::Char('F') => app.favor_all(),
                    KeyCode::Char('.') => app.pace(1.0),
                    KeyCode::Char(',') => app.pace(-1.0),
                    KeyCode::Char('z') => app.action(Action::Shuffle),
                    KeyCode::Char('r') => app.action(Action::Repeat),
                    KeyCode::Char('+') | KeyCode::Char('=') => app.action(Action::Louder),
                    KeyCode::Char('-') => app.action(Action::Quieter),
                    KeyCode::Char('x') => app.action(Action::Evict),
                    KeyCode::Left => {
                        let step = -i32::from(app.settings.seek_step);
                        app.control(json!(["seek", step, "relative"]))
                    }
                    KeyCode::Right => {
                        app.control(json!(["seek", app.settings.seek_step, "relative"]))
                    }
                    KeyCode::Delete if app.view == View::Queue => {
                        app.take_upcoming(app.table.selected().unwrap_or(0));
                        app.navigate(0);
                    }
                    _ => (),
                }
            }
            Event::Mouse(mouse) if app.picker.is_some() => match mouse.kind {
                MouseEventKind::ScrollDown => app.pick(1),
                MouseEventKind::ScrollUp => app.pick(-1),
                _ => (),
            },
            // In the settings window a click picks a line; on the current one it is Enter.
            Event::Mouse(mouse) if app.options && app.input.is_none() => match mouse.kind {
                MouseEventKind::Down(MouseButton::Left)
                    if app
                        .setting_rows
                        .contains(Position::new(mouse.column, mouse.row)) =>
                {
                    let line = usize::from(mouse.row - app.setting_rows.y) + app.setting_first;
                    if line == app.setting {
                        app.activate();
                    } else {
                        app.setting = line.min(SETTINGS.len() - 1);
                    }
                }
                MouseEventKind::ScrollDown => app.move_setting(1),
                MouseEventKind::ScrollUp => app.move_setting(-1),
                _ => (),
            },
            Event::Mouse(mouse)
                if !app.help && !app.details && app.input.is_none() && app.setup.is_none() =>
            {
                match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        let pos = Position::new(mouse.column, mouse.row);
                        if let Some((_, action)) =
                            app.hits.iter().find(|(rect, _)| rect.contains(pos))
                        {
                            let action = *action;
                            app.focus = None;
                            app.action(action);
                        } else if app.rows.contains(pos) {
                            let index = usize::from(mouse.row - app.rows.y) + app.table.offset();
                            if index < app.tracks().len() {
                                app.table.select(Some(index));
                                app.focus = None;
                                app.editing = false;
                            }
                        }
                    }
                    MouseEventKind::ScrollDown => app.navigate(1),
                    MouseEventKind::ScrollUp => app.navigate(-1),
                    _ => (),
                }
            }
            _ => (),
        }
    }
    // What plays and what is ahead of it is kept as it stands at the end of the run.
    app.save();
    Ok(())
}

fn block<'a>(title: &'a str, theme: &Theme) -> Block<'a> {
    Block::bordered()
        .border_set(BORDER)
        .border_style(theme.border)
        .title(Line::from(vec![
            Span::styled("-", theme.border),
            Span::styled(title, theme.accent),
        ]))
}
// A window over the interface; `Clear` alone would drop the background of the scheme.
// The mark turns on a clock of its own, not on the rate at which events arrive.
fn spinner(app: &App) -> &'static str {
    let step = app.since.elapsed().as_millis() / 120;
    SPINNER[step as usize % SPINNER.len()]
}

fn modal<'a>(title: &'a str, theme: &Theme) -> Block<'a> {
    block(title, theme)
        .border_style(theme.accent)
        .style(theme.base)
}
fn label(frame: &mut Frame, area: Rect, text: impl Into<Text<'static>>, style: Style) {
    frame.render_widget(Paragraph::new(text).style(style), area);
}
fn button(frame: &mut Frame, app: &mut App, area: Rect, text: &str, action: Action, active: bool) {
    let focused = app.focus == Some(app.hits.len());
    app.hits.push((area, action));
    let (style, edges) = if focused {
        (app.theme.focused, [">", "<"])
    } else if active {
        (app.theme.selected, ["[", "]"])
    } else {
        (app.theme.text, ["[", "]"])
    };
    frame.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .style(style),
        area,
    );
    if area.width >= 2 {
        for (x, edge) in [area.x, area.right() - 1].into_iter().zip(edges) {
            if let Some(cell) = frame.buffer_mut().cell_mut((x, area.y)) {
                cell.set_symbol(edge);
            }
        }
    }
}
// The cells a text takes on screen: a letter of Japanese is two cells wide.
fn cells(text: &str) -> usize {
    Line::raw(text).width()
}
// The text with spaces after it up to `width` cells.
fn pad(text: &str, width: usize) -> String {
    format!("{text}{}", " ".repeat(width.saturating_sub(cells(text))))
}
/// `text` in lines of at most `width` cells, broken between words where it has any.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split_inclusive(' ') {
        let line = lines.last_mut().expect("a line to write on");
        if !line.is_empty() && cells(line) + cells(word.trim_end()) > width {
            line.truncate(line.trim_end().len());
            lines.push(String::new());
        }
        // A word wider than a line, or a language without spaces, breaks by the letter.
        for letter in word.chars() {
            let line = lines.last_mut().expect("a line to write on");
            if cells(line) + cells(letter.encode_utf8(&mut [0; 4])) > width {
                lines.push(String::new());
            }
            lines.last_mut().expect("a line to write on").push(letter);
        }
    }
    lines.retain(|line| !line.trim().is_empty());
    lines
}
fn time(value: f64) -> String {
    let seconds = value.max(0.0) as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
// The played and the remaining part of the bar and the times after it.
// The bar of a track: what has played, what has arrived beyond that, the rest, and
// the times after it.
fn progress(
    position: f64,
    arrived: f64,
    duration: f64,
    width: u16,
) -> (String, String, String, String) {
    let times = format!(" {} / {}", time(position), time(duration));
    let cells = usize::from(width).saturating_sub(times.len() + 2);
    let part = |seconds: f64| {
        if duration > 0.0 {
            ((seconds / duration).clamp(0.0, 1.0) * cells as f64) as usize
        } else {
            0
        }
    };
    let filled = part(position);
    let loaded = part(arrived).max(filled) - filled;
    let head = if filled > 0 { ">" } else { "" };
    (
        format!("{}{head}", "=".repeat(filled.saturating_sub(1))),
        "~".repeat(loaded),
        "-".repeat(cells - filled - loaded),
        times,
    )
}

fn draw(frame: &mut Frame, app: &mut App) {
    let size = frame.area();
    let theme = app.theme.clone();
    app.hits.clear();
    app.rows = Rect::default();
    frame.render_widget(Block::new().style(theme.base), size);
    if size.width < 80 || size.height < 24 {
        label(
            frame,
            size,
            t!("\n  CLICLOUD\n\n  Увеличьте терминал до 80x24.\n  q - выход"),
            theme.accent,
        );
        return;
    }
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            // The first lines a screen has to spare go to the logo and the gap below it.
            Constraint::Length(
                2 + if app.logo {
                    (size.height - 24).min(2)
                } else {
                    0
                },
            ),
            // The tab of the track has the lines of the search for its picture.
            Constraint::Length(if app.track_open { 0 } else { 3 }),
            Constraint::Min(8),
            Constraint::Length(7),
            Constraint::Length(2),
        ])
        .split(size);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(if size.width >= 115 {
            vec![
                Constraint::Length(21),
                Constraint::Min(35),
                Constraint::Length(27),
            ]
        } else {
            vec![Constraint::Length(19), Constraint::Min(35)]
        })
        .spacing(1)
        .split(outer[2]);
    // Buttons register in app.hits in drawing order, which is also the Tab order.
    draw_header(frame, app, &theme, outer[0]);
    if app.track_open {
        // The tab of the track has the whole of the main screen to itself.
        draw_track(frame, app, &theme, outer[2]);
    } else {
        draw_search(frame, app, &theme, outer[1]);
        draw_navigation(frame, app, &theme, body[0]);
        draw_tracks(frame, app, &theme, body[1]);
        if let Some(area) = body.get(2) {
            draw_upcoming(frame, app, &theme, *area);
        }
    }
    draw_player(frame, app, &theme, outer[3]);
    // Last, so that the order of the other buttons is the one it was.
    draw_tabs(frame, app, &theme, outer[0]);
    label(
        frame,
        Rect::new(outer[4].x, outer[4].y, outer[4].width, 1),
        clean(&app.message),
        theme.strong,
    );
    label(
        frame,
        Rect::new(outer[4].x, outer[4].y + 1, outer[4].width, 1),
        t!(" / поиск  f избранное  a очередь  d в кеш  o настройки  ? помощь  q выход"),
        theme.muted,
    );
    if app.options {
        draw_settings(frame, app, &theme, size);
    }
    if app.picker.is_some() {
        draw_picker(frame, app, &theme, size);
    }
    if app.help {
        draw_help(frame, &theme, size);
    }
    if app.details {
        draw_details(frame, app, &theme, size);
    }
    if app.setup.is_some() {
        draw_setup(frame, app, &theme, size);
    }
}

/// A drawing made of marks, shrunk: every two marks across and four down become one
/// cell, a letter of Braille with a dot for each of them that is not a space.
fn dots(drawing: &str) -> Vec<String> {
    const DOTS: [[u32; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];
    let rows: Vec<&[u8]> = drawing.lines().map(str::as_bytes).collect();
    let width = rows.iter().map(|row| row.len()).max().unwrap_or(0);
    (rows.chunks(4))
        .map(|rows| {
            let cell = |left: usize| {
                let mut dots = 0;
                for (row, marks) in rows.iter().zip(DOTS) {
                    for (mark, dot) in marks.into_iter().enumerate() {
                        if row.get(left + mark).is_some_and(|mark| *mark != b' ') {
                            dots |= dot;
                        }
                    }
                }
                // An empty cell is a space: some fonts draw the empty letter as rings.
                (char::from_u32(0x2800 + dots).filter(|_| dots != 0)).unwrap_or(' ')
            };
            let line: String = (0..width).step_by(2).map(cell).collect();
            line.trim_end().to_owned()
        })
        .collect()
}

fn draw_header(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    static SMALL: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let logo = SMALL.get_or_init(|| dots(LOGO));
    // A screen with a line to spare shows the logo, and what follows the name beside it.
    if usize::from(area.height) >= logo.len() {
        let width = logo.iter().map(|line| cells(line)).max().unwrap_or(0) as u16;
        let last = logo.len().saturating_sub(1) as u16;
        for (row, line) in logo.iter().enumerate() {
            label(
                frame,
                Rect::new(area.x + 1, area.y + row as u16, area.width - 1, 1),
                line.clone(),
                theme.accent,
            );
        }
        let beside = (area.x + width + 3).min(area.right());
        label(
            frame,
            Rect::new(beside, area.y + last, area.right() - beside, 1),
            format!("/ {}", app.tagline),
            theme.muted,
        );
        return;
    }
    // The right corner belongs to the tabs.
    let header = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(20), Constraint::Length(tabs().2 + 1)])
        .split(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" CLICLOUD ", theme.accent),
            Span::styled(format!("/ {}", app.tagline), theme.muted),
        ])),
        header[0],
    );
}

// The names of the two tabs of the main screen, and the width they take with the key
// that goes from one to the other.
fn tabs() -> (&'static str, &'static str, u16) {
    let (lists, track) = (t!("Списки"), t!("Трек"));
    (lists, track, (cells(lists) + cells(track)) as u16 + 11)
}

// The tabs, in the right corner of the header as a browser has them at its top: the
// lists, and the track that plays.
fn draw_tabs(frame: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let (lists, track, width) = tabs();
    let (lists_width, track_width) = (cells(lists) as u16 + 4, cells(track) as u16 + 4);
    let left = area.right().saturating_sub(width).max(area.x);
    label(frame, Rect::new(left, area.y, 2, 1), "t", theme.muted);
    button(
        frame,
        app,
        Rect::new(left + 2, area.y, lists_width, 1),
        lists,
        Action::Lists,
        !app.track_open,
    );
    button(
        frame,
        app,
        Rect::new(left + 3 + lists_width, area.y, track_width, 1),
        track,
        Action::Track,
        app.track_open,
    );
}

fn draw_search(frame: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    // While the library is narrowed, the box holds the words that narrow it.
    let narrowed = app.filtering || (app.view == View::Library && !app.filter.is_empty());
    let query = if narrowed {
        format!(
            "{}{}",
            clean(&app.filter),
            if app.filtering { "_" } else { "" }
        )
    } else if app.query.is_empty() && !app.editing {
        t!("Найти исполнителя, трек, новый звук...").into()
    } else {
        format!(
            "{}{}",
            clean(&app.query),
            if app.editing { "_" } else { "" }
        )
    };
    let search = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(20), Constraint::Length(11)])
        .spacing(1)
        .split(area);
    app.hits.push((search[0], Action::Search));
    let title = if narrowed {
        t!(" ФИЛЬТР БИБЛИОТЕКИ   Enter - оставить, Esc - сбросить ").to_owned()
    } else if app.searching {
        t!(" ПОИСК {} Esc - отмена ", spinner(app))
    } else {
        t!(" / ПОИСК   Enter - найти ").to_owned()
    };
    let typing = app.editing || app.filtering;
    frame.render_widget(
        Paragraph::new(query)
            .block(
                block(&title, theme).border_style(if typing || app.focus == Some(0) {
                    theme.accent
                } else {
                    theme.border
                }),
            )
            .style(if typing { theme.text } else { theme.muted }),
        search[0],
    );
    button(
        frame,
        app,
        Rect::new(search[1].x, search[1].y + 1, search[1].width, 1),
        t!("Найти"),
        Action::Submit,
        false,
    );
}

fn draw_navigation(frame: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    frame.render_widget(block(t!(" ОБЗОР "), theme), area);
    let nav = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let views = [
        (t!("1  Поиск"), View::Search),
        (t!("2  Библиотека"), View::Library),
        (t!("3  Очередь"), View::Queue),
        (t!("4  Недавние"), View::Recent),
    ];
    let step = if nav.height >= 2 * views.len() as u16 - 1 {
        2
    } else {
        1
    };
    for (i, (name, view)) in views.into_iter().enumerate() {
        button(
            frame,
            app,
            Rect::new(nav.x, nav.y + i as u16 * step, nav.width, 1),
            &pad(name, usize::from(nav.width) - 2),
            Action::View(view),
            app.view == view,
        );
    }
    // As much of the summary as fits below the buttons, in whole lines of thought.
    let below = (views.len() as u16 - 1) * step + 2;
    let room = usize::from(nav.height.saturating_sub(below));
    let loading = app.pending.len() + usize::from(app.fetch.is_some());
    let mut lines = vec![
        t!("{} избранных", app.library.favorites.len()),
        t!("{} в очереди", app.queue.len() + app.ahead.len()),
    ];
    if loading > 0 {
        lines.push(t!("{} качается", loading));
    }
    if app.cache.is_some() && room >= lines.len() + 4 {
        lines.extend(["", t!("v - в кеше,"), t!("играет и"), t!("без сети")].map(String::from));
    }
    if room >= lines.len() {
        label(
            frame,
            Rect::new(nav.x, nav.y + below, nav.width, nav.height - below),
            lines.join("\n"),
            theme.muted,
        );
    }
}

fn draw_tracks(frame: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let center = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(2)])
        .split(area);
    let (library, author);
    let title = match (app.view, app.profile()) {
        // A page of a profile says whose it is and which of its sections.
        (View::Search, Some((profile, Some(section)))) => {
            let sections = [
                t!("треки"),
                t!("альбомы"),
                t!("плейлисты"),
                t!("репосты"),
                t!("лайки"),
            ];
            author = t!(
                " АВТОР {} | {} | [ ] - раздел ",
                clean(&profile),
                sections[section]
            );
            &author
        }
        _ => match app.view {
            View::Search => t!(" РЕЗУЛЬТАТЫ "),
            View::Library if app.cache.is_none() => t!(" МОЯ БИБЛИОТЕКА "),
            View::Library => {
                library = t!(
                    " МОЯ БИБЛИОТЕКА | избранное: {} | в кеше: {}, {} ",
                    app.library.favorites.len(),
                    app.stored_count,
                    megabytes(app.stored_size)
                );
                &library
            }
            View::Queue => t!(" ОЧЕРЕДЬ ВОСПРОИЗВЕДЕНИЯ "),
            View::Recent => t!(" НЕДАВНИЕ "),
        },
    };
    let tracks = app.tracks();
    let selected = tracks.get(app.table.selected().unwrap_or(0)).copied();
    let evict =
        selected.is_some_and(|track| app.cache.as_ref().is_some_and(|c| c.contains(&track.url)));
    let taken = if tracks.is_empty() {
        let message = match app.view {
            View::Search => {
                t!(
                    "\n\nВаша следующая любимая песня - здесь.\n\nНажмите / и введите поисковый запрос.\nEnter - слушать, f - сохранить"
                )
            }
            View::Library if !app.filter.is_empty() => {
                t!("\n\nПо этому фильтру ничего нет.\n\nEsc - сбросить фильтр.")
            }
            View::Library => {
                t!(
                    "\n\nСоберите свою коллекцию.\n\nf - добавить трек в избранное.\nЗдесь же всё, что сохранено в кеш (d)\nи играет без сети."
                )
            }
            View::Queue => {
                t!(
                    "\n\nМузыка без перерывов.\n\nНажмите a, чтобы добавить трек в очередь.\nСледующий трек запустится автоматически."
                )
            }
            View::Recent => {
                t!(
                    "\n\nИстория прослушивания.\n\nЗдесь появятся последние включённые треки.\nEnter - включить снова."
                )
            }
        };
        frame.render_widget(
            Paragraph::new(message)
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: false })
                .style(theme.muted)
                .block(block(title, theme)),
            center[0],
        );
        0
    } else {
        // Only the rows that show are made: a library may hold thousands of tracks,
        // and each row asks the disk whether its track is stored.
        let count = tracks.len();
        let height = usize::from(center[0].height.saturating_sub(4)).max(1);
        let chosen = app.table.selected().map(|chosen| chosen.min(count - 1));
        let at = chosen.unwrap_or(0);
        let offset = (app.table.offset())
            .clamp(at.saturating_sub(height - 1), at)
            .min(count.saturating_sub(height));
        let digits = count.to_string().len().max(2);
        let rows: Vec<Row> = tracks
            .iter()
            .enumerate()
            .skip(offset)
            .take(height)
            .map(|(i, track)| {
                let favorite = app.library.favorites.iter().any(|t| t.url == track.url);
                let playing = app.current.as_ref().is_some_and(|t| t.url == track.url)
                    && app.player.is_some();
                let mark = |on: bool, text: &'static str, style: Style| {
                    if on {
                        Cell::from(text).style(style)
                    } else {
                        Cell::from(".").style(theme.border)
                    }
                };
                let stored = app.cache.as_ref().is_some_and(|c| c.contains(&track.url));
                Row::new(vec![
                    if playing {
                        Cell::from("|>")
                    } else if cache::key(&track.url).is_some_and(|name| app.broken.contains(&name))
                    {
                        // It did not play the last time it was tried.
                        Cell::from("!!").style(theme.error)
                    } else {
                        Cell::from(format!("{:0digits$}", i + 1)).style(theme.muted)
                    },
                    Cell::from(clean(&track.title)),
                    Cell::from(clean(&track.artist)).style(theme.muted),
                    // A playlist among the tracks has no length: Enter goes into it.
                    Cell::from(match track.duration {
                        Some(duration) => time(duration),
                        None if leads_to_list(&track.url) => ">>".into(),
                        None => "-".into(),
                    })
                    .style(theme.muted),
                    mark(favorite, "*", theme.favorite),
                    if !stored && app.fetching(&track.url) {
                        Cell::from("~").style(theme.waiting)
                    } else {
                        mark(stored, "v", theme.stored)
                    },
                ])
                .style(if playing { theme.playing } else { theme.text })
            })
            .collect();
        let table = Table::new(
            rows,
            [
                Constraint::Length(digits as u16 + 1),
                Constraint::Min(12),
                Constraint::Percentage(27),
                Constraint::Length(6),
                Constraint::Length(1),
                Constraint::Length(1),
            ],
        )
        .header(
            Row::new(["#", t!("ТРЕК"), t!("ИСПОЛНИТЕЛЬ"), t!("ВРЕМЯ"), "*", "v"])
                .style(theme.muted)
                .bottom_margin(1),
        )
        .block(block(title, theme).title_bottom(
            // Where in the list the selection is, which a long list does not show.
            Line::styled(format!(" {}/{count} ", at + 1), theme.muted).right_aligned(),
        ))
        .row_highlight_style(theme.selected)
        .highlight_symbol("> ");
        let mut window = TableState::default().with_selected(chosen.map(|chosen| chosen - offset));
        frame.render_stateful_widget(table, center[0], &mut window);
        *app.table.offset_mut() = offset;
        app.rows = Rect::new(
            center[0].x + 1,
            center[0].y + 3,
            center[0].width.saturating_sub(2),
            center[0].height.saturating_sub(4),
        );
        2 + ((count - offset) as u16).min(app.rows.height)
    };
    draw_backdrop(
        frame.buffer_mut(),
        BACKDROPS[app.backdrop()].1,
        center[0].inner(Margin::new(1, 1)),
        taken,
        theme.border,
    );
    let buttons = buttons(center[1]);
    button(frame, app, buttons[0], t!("|> Играть"), Action::Play, false);
    button(
        frame,
        app,
        buttons[1],
        t!("* Избранное"),
        Action::Favorite,
        false,
    );
    button(
        frame,
        app,
        buttons[2],
        t!("+ В очередь"),
        Action::Enqueue,
        false,
    );
    if evict {
        button(
            frame,
            app,
            buttons[3],
            t!("x Из кеша"),
            Action::Evict,
            false,
        );
    } else {
        button(
            frame,
            app,
            buttons[3],
            t!("v В кеш"),
            Action::Download,
            false,
        );
    }
}

// A count with its thousands apart, as it is read.
fn counted(count: u64) -> String {
    let digits = count.to_string();
    let mut text = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            text.push(' ');
        }
        text.push(digit);
    }
    text
}

// The track that plays: its picture, drawn the way the settings say, and beside it all
// that is known of it.
fn draw_track(frame: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let panel = block(t!(" ТРЕК "), theme);
    let inner = panel.inner(area).inner(Margin::new(1, 0));
    frame.render_widget(panel, area);
    let Some(track) = app.current.clone() else {
        frame.render_widget(
            Paragraph::new(t!(
                "\n\nНичего не играет.\n\nEnter на треке в списке включает его."
            ))
            .alignment(Alignment::Center)
            .style(theme.muted),
            inner,
        );
        return;
    };
    // A cell is twice as tall as wide, so a square picture is twice as many columns as
    // rows; it leaves the greater part of the width to the words.
    let mode = Mode::parse(&app.settings.cover);
    let rows = inner.height.min(inner.width / 5);
    let columns = rows * 2;
    let name = cache::key(&track.url).unwrap_or_default();
    let mut taken = 0;
    if mode != Mode::Off && rows >= 4 {
        taken = columns + 2;
        let place = Rect::new(inner.x, inner.y, columns, rows);
        match app.covers.get(&name) {
            Some(Some(cover)) => {
                let fresh = (app.drawn.as_ref()).is_some_and(|(of, wide, high, how, scheme, _)| {
                    (of, *wide, *high, *how, scheme) == (&name, columns, rows, mode, &theme.name)
                });
                if !fresh {
                    let cells = cover.cells(columns.into(), rows.into(), mode, theme.palette());
                    app.drawn =
                        Some((name.clone(), columns, rows, mode, theme.name.clone(), cells));
                }
                let truecolor = theme.truecolor();
                let cells = app.drawn.as_ref().map(|drawn| &drawn.5);
                let buffer = frame.buffer_mut();
                for (at, cell) in cells.into_iter().flatten().enumerate() {
                    let (x, y) = (at as u16 % columns, at as u16 / columns);
                    let drawn = &mut buffer[(place.x + x, place.y + y)];
                    drawn.set_char(cell.symbol).set_style(theme.text);
                    if let Some(color) = cell.fg {
                        drawn.set_fg(theme.paint(color, truecolor));
                    }
                    if let Some(color) = cell.bg {
                        drawn.set_bg(theme.paint(color, truecolor));
                    }
                }
            }
            waited => {
                let said = match waited {
                    Some(_) => t!("обложки нет"),
                    None => t!("ждём обложку"),
                };
                label(
                    frame,
                    Rect::new(place.x, place.y + rows / 2, columns, 1),
                    Line::styled(said, theme.muted).centered(),
                    theme.muted,
                );
            }
        }
    }
    let words = Rect::new(
        inner.x + taken,
        inner.y,
        inner.width.saturating_sub(taken),
        inner.height,
    );
    // Wide enough, the words stand in two columns: what the track is, and what its
    // maker wrote of it. Otherwise the one follows the other.
    let two = words.width >= 64;
    let first = if two { 34 } else { words.width };
    let mut facts: Vec<Line> = wrap(&clean(&track.title), first.into())
        .into_iter()
        .take(2)
        .map(|line| Line::styled(line, theme.strong))
        .collect();
    facts.push(Line::styled(clean(&track.artist), theme.text));
    // Its length, and the marks it has in a list: a favorite, stored.
    let favorite = app.library.favorites.iter().any(|t| t.url == track.url);
    let stored = (app.cache.as_ref()).is_some_and(|cache| cache.contains(&track.url));
    let mut marks = vec![Span::styled(
        track.duration.map(time).unwrap_or_else(|| "-".into()),
        theme.muted,
    )];
    if favorite {
        marks.push(Span::styled("  *", theme.favorite));
    }
    if stored {
        marks.push(Span::styled("  v", theme.stored));
    }
    facts.push(Line::from(marks));
    // The least screen has no line to spare for a gap.
    if words.height >= 12 {
        facts.push(Line::raw(""));
    }
    let told = app.told.get(&name);
    let mut written = Vec::new();
    match told {
        Some(Some(details)) => {
            let date = (details.date.as_deref())
                .filter(|date| date.len() == 8 && date.is_ascii())
                .map(|date| format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..]));
            let tags = details.tags.as_ref().map(|tags| tags.join(", "));
            let rows = [
                (t!("Жанр"), details.genre.clone()),
                (t!("Дата"), date),
                (t!("Слушали"), details.plays.map(counted)),
                (t!("Лайки"), details.likes.map(counted)),
                (t!("Репосты"), details.reposts.map(counted)),
                (t!("Комментарии"), details.comments.map(counted)),
                (t!("Теги"), tags),
            ];
            let rows: Vec<(&str, String)> = (rows.into_iter())
                .filter_map(|(name, value)| Some((name, value?)))
                .filter(|(_, value)| !value.trim().is_empty())
                .collect();
            let names = rows.iter().map(|(name, _)| cells(name)).max().unwrap_or(0);
            for (name, value) in rows {
                facts.push(Line::from(vec![
                    Span::styled(format!("{}  ", pad(name, names)), theme.muted),
                    Span::styled(clean(&value), theme.text),
                ]));
            }
            let width = usize::from(if two { words.width - first - 2 } else { first });
            for paragraph in details.description.as_deref().unwrap_or_default().lines() {
                match wrap(&clean(paragraph), width) {
                    lines if lines.is_empty() => written.push(String::new()),
                    lines => written.extend(lines),
                }
            }
        }
        Some(None) => facts.push(Line::styled(t!("сведений о треке нет"), theme.muted)),
        None => facts.push(Line::styled(t!("ищем сведения о треке..."), theme.muted)),
    }
    // The text goes no further up than its last line at the foot of its place.
    let place = if two {
        Rect::new(
            words.x + first + 2,
            words.y,
            words.width - first - 2,
            words.height,
        )
    } else {
        let below = (facts.len() as u16 + 1).min(words.height);
        Rect::new(words.x, words.y + below, words.width, words.height - below)
    };
    app.scroll = (app.scroll).min(written.len().saturating_sub(place.height.into()) as u16);
    let text: Vec<Line> = (written.into_iter())
        .skip(app.scroll.into())
        .map(|line| Line::styled(line, theme.muted))
        .collect();
    frame.render_widget(
        Paragraph::new(facts),
        Rect::new(words.x, words.y, first, words.height),
    );
    frame.render_widget(Paragraph::new(text), place);
}

// The drawing in the middle of the panel, in the cells nothing else has taken: the first
// `taken` lines are the list's own, and a text below them keeps a margin on both sides.
fn draw_backdrop(buffer: &mut Buffer, drawing: &str, area: Rect, taken: u16, style: Style) {
    let lines: Vec<&str> = drawing.lines().collect();
    let width = lines.iter().map(|line| line.len()).max().unwrap_or(0);
    let left = i32::from(area.x) + (i32::from(area.width) - width as i32) / 2;
    let top = i32::from(area.y) + (i32::from(area.height) - lines.len() as i32) / 2;
    for y in area.y.saturating_add(taken)..area.bottom() {
        let line = usize::try_from(i32::from(y) - top)
            .ok()
            .and_then(|line| lines.get(line));
        let Some(line) = line else { continue };
        let mut written = (area.x..area.right()).filter(|&x| buffer[(x, y)].symbol() != " ");
        let first = written.next();
        let text = first.map(|first| {
            let last = written.next_back().unwrap_or(first);
            i32::from(first) - 2..=i32::from(last) + 2
        });
        for (cell, symbol) in line.bytes().enumerate() {
            let x = left + cell as i32;
            let free = text.as_ref().is_none_or(|text| !text.contains(&x));
            if symbol != b' ' && free && x >= i32::from(area.x) && x < i32::from(area.right()) {
                buffer[(x as u16, y)]
                    .set_char(char::from(symbol))
                    .set_style(style);
            }
        }
    }
}

// The row of four buttons under the middle panel.
fn buttons(area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 4); 4])
        .spacing(1)
        .split(Rect::new(area.x + 1, area.y, area.width - 2, 1))
}

// The settings window. It stays as tall for every setting, whose hints differ in length.
fn draw_settings(frame: &mut Frame, app: &mut App, theme: &Theme, size: Rect) {
    let hints = SETTINGS
        .iter()
        .map(|setting| setting.hint().lines().count())
        .max()
        .unwrap_or(0);
    // A screen too short for every line shows those around the current one.
    let height = ((SETTINGS.len() + hints + 8) as u16).min(size.height);
    let shown = (usize::from(height).saturating_sub(hints + 8)).clamp(1, SETTINGS.len());
    let first = (app.setting + 1).saturating_sub(shown);
    let width = 72.min(size.width - 4);
    let area = Rect::new(
        (size.width - width) / 2,
        (size.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, area);
    let window = modal(t!(" НАСТРОЙКИ | Esc - закрыть "), theme);
    let inner = window.inner(area);
    frame.render_widget(window, area);
    let names = SETTINGS
        .iter()
        .map(|setting| cells(setting.name()))
        .max()
        .unwrap_or(0);
    let mut lines: Vec<Line> = SETTINGS
        .iter()
        .enumerate()
        .skip(first)
        .take(shown)
        .map(|(i, setting)| {
            let current = i == app.setting;
            let text = format!(
                "{} {}  {}",
                if current { ">" } else { " " },
                pad(setting.name(), names),
                clean(&app.value(*setting)),
            );
            // The whole width, so that the selection is a bar like in the lists.
            let text = pad(&text, usize::from(inner.width));
            Line::styled(text, if current { theme.selected } else { theme.text })
        })
        .collect();
    let note = |text: String| Line::styled(format!("  {text}"), theme.muted);
    lines.push(Line::raw(""));
    let hint = SETTINGS[app.setting].hint();
    lines.extend(hint.lines().map(|line| note(line.into())));
    lines.resize(shown + 2 + hints, Line::raw(""));
    lines.push(note(match &app.config {
        Some(file) => t!("Файл: {}", clean(&file.to_string_lossy())),
        None => t!("Файл настроек не определён: изменения действуют до выхода").into(),
    }));
    // The window covers the line of messages; what was said since it opened is said here.
    let said = if app.message == app.greeted {
        vec![]
    } else {
        wrap(
            &clean(&app.message),
            usize::from(inner.width).saturating_sub(4),
        )
    };
    for line in 0..2 {
        let text = said.get(line).cloned().unwrap_or_default();
        lines.push(Line::styled(format!("  {text}"), theme.strong));
    }
    lines.push(note(
        t!("Up/Down - выбор   Left/Right - изменить   Enter - переключить").into(),
    ));
    frame.render_widget(Paragraph::new(lines), inner);
    app.setting_rows = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        inner.height.min(shown as u16),
    );
    app.setting_first = first;
}

fn draw_picker(frame: &mut Frame, app: &mut App, theme: &Theme, size: Rect) {
    // At the side: the interface behind it shows the scheme under the cursor.
    let width = 38.min(size.width - 4);
    let area = Rect::new(size.width - width - 1, 1, width, size.height - 2);
    frame.render_widget(Clear, area);
    let items: Vec<ListItem> = app
        .themes
        .iter()
        .map(|name| ListItem::new(format!(" {}", clean(name))))
        .collect();
    let list = List::new(items)
        .block(modal(t!(" СХЕМА | Enter - выбрать | Esc "), theme))
        .style(theme.text)
        .highlight_style(theme.selected);
    if let Some(picker) = &mut app.picker {
        frame.render_stateful_widget(list, area, &mut picker.state);
    }
}

fn draw_upcoming(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let mut lines = vec![Line::styled(t!("ДАЛЕЕ"), theme.accent), Line::raw("")];
    for track in app.queue.iter().chain(&app.ahead).take(4) {
        lines.push(Line::styled(clean(&track.title), theme.text));
        lines.push(Line::styled(clean(&track.artist), theme.muted));
        lines.push(Line::raw(""));
    }
    if app.queue.is_empty() && app.ahead.is_empty() {
        lines.push(Line::styled(t!("Очередь пока пуста"), theme.muted));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(t!("НЕДАВНО"), theme.accent));
    for track in app.library.recent.iter().take(3) {
        lines.push(Line::styled(clean(&track.title), theme.text));
    }
    frame.render_widget(
        Paragraph::new(lines).block(block(t!(" НА СЛУХУ "), theme)),
        area,
    );
}

fn draw_player(frame: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    // The frame also tells how a list plays, with the keys that change it.
    let mut title = format!(
        "{}| z {} | r {} ",
        t!(" ПЛЕЕР "),
        app.order(),
        app.repeating()
    );
    if app.speed != 1.0 {
        title.push_str(&format!("| x{:.1} ", app.speed));
    }
    frame.render_widget(block(&title, theme), area);
    let area = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let (position, duration, state, state_style) = if let Some(player) = &app.player {
        let listed = app.current.as_ref().and_then(|t| t.duration).unwrap_or(0.0);
        let (state, style) = if !player.loaded {
            (t!("ЗАГРУЗКА"), theme.waiting)
        } else if player.paused {
            (t!("ПАУЗА"), theme.waiting)
        } else {
            (t!("ИГРАЕТ"), theme.playing)
        };
        (
            player.position,
            // While audio arrives, mpv reports the length of what it has got so far.
            if player.duration > 0.0 && !(player.receiving() && listed > 0.0) {
                player.duration
            } else {
                listed
            },
            state,
            style,
        )
    } else {
        (0.0, 0.0, t!("СТОП"), theme.muted)
    };
    let song = app
        .current
        .as_ref()
        .map(|t| format!("{} - {}", clean(&t.artist), clean(&t.title)))
        .unwrap_or_else(|| t!("Выберите трек, чтобы начать").into());
    label(
        frame,
        Rect::new(area.x, area.y, area.width, 1),
        Line::from(vec![
            Span::styled(state, state_style),
            Span::styled("  /  ", theme.muted),
            Span::styled(song, theme.text),
        ]),
        theme.text,
    );
    // What has arrived of a track that is still coming shows ahead of what has played.
    let arrived = (app.player.as_ref())
        .filter(|player| player.origin != Origin::Cache)
        .map_or(0.0, |player| player.buffered);
    let (played, loaded, left, times) = progress(position, arrived, duration, area.width);
    label(
        frame,
        Rect::new(area.x, area.y + 2, area.width, 1),
        Line::from(vec![
            Span::styled("[", theme.muted),
            Span::styled(played, theme.bar),
            Span::styled(loaded, theme.muted),
            Span::styled(left, theme.border),
            Span::styled("]", theme.muted),
            Span::styled(times, theme.text),
        ]),
        theme.text,
    );
    let controls = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(8),
            Constraint::Length(13),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Min(1),
            Constraint::Length(5),
            Constraint::Length(9),
            Constraint::Length(5),
        ])
        .spacing(1)
        .split(Rect::new(area.x, area.y + 4, area.width, 1));
    button(frame, app, controls[0], "|< p", Action::Previous, false);
    button(
        frame,
        app,
        controls[1],
        if app.player.as_ref().is_some_and(|p| !p.paused) {
            t!("|| Пауза")
        } else {
            t!("|> Играть")
        },
        Action::Pause,
        false,
    );
    button(frame, app, controls[2], "n >|", Action::Next, false);
    button(frame, app, controls[3], t!("s Стоп"), Action::Stop, false);
    button(frame, app, controls[5], "-", Action::Quieter, false);
    label(
        frame,
        controls[6],
        format!(" VOL {:3.0}", app.volume),
        theme.muted,
    );
    button(frame, app, controls[7], "+", Action::Louder, false);
}

fn draw_help(frame: &mut Frame, theme: &Theme, size: Rect) {
    let modal_area = Rect::new(size.width / 2 - 32, size.height / 2 - 12, 64, 24);
    frame.render_widget(Clear, modal_area);
    let keys = [
        ("/", t!("Поиск (Enter отправляет, Esc отменяет)")),
        (
            "1 2 3 4 / t",
            t!("Поиск, библиотека, очередь, недавние / трек"),
        ),
        ("Up Down, j k", t!("Выбрать трек     Enter  Проиграть")),
        ("PgUp PgDn", t!("Листать список   Home End  К краям списка")),
        ("Ctrl+F", t!("Фильтр библиотеки (Esc сбрасывает)")),
        ("Tab", t!("Выбрать кнопку   Enter  Нажать")),
        ("f", t!("Добавить / удалить из избранного")),
        ("a", t!("Добавить в очередь   Del  Убрать из очереди")),
        ("d / D", t!("Загрузить в кеш трек / весь список")),
        ("x", t!("Удалить трек из кеша")),
        ("Space", t!("Пауза / продолжить")),
        ("p / n", t!("Предыдущий / следующий трек")),
        ("z / r", t!("Вперемешку / повтор: нет, список, трек")),
        ("m / u", t!("Похожие треки / автор   [ ]  Его разделы")),
        ("F / b", t!("Весь список в избранное / назад к списку")),
        (
            "Left / Right",
            t!("Перемотка        , .  Медленнее / быстрее"),
        ),
        ("- / +", t!("Громкость    s  Стоп    q  Выход")),
        ("o", t!("Настройки и цветовые схемы")),
        ("e", t!("Подробности последней ошибки")),
    ];
    let mut lines = vec![Line::raw("")];
    lines.extend(keys.iter().map(|(key, action)| {
        Line::from(vec![
            Span::styled(format!(" {key:<13}"), theme.accent),
            Span::styled(*action, theme.text),
        ])
    }));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        t!(" Мышь работает. Любая клавиша закрывает справку."),
        theme.muted,
    ));
    frame.render_widget(
        Paragraph::new(lines).block(modal(t!(" УПРАВЛЕНИЕ "), theme)),
        modal_area,
    );
}

fn draw_setup(frame: &mut Frame, app: &App, theme: &Theme, size: Rect) {
    let Some(setup) = &app.setup else {
        return;
    };
    let mut lines = vec![
        Line::raw(""),
        Line::styled(
            t!(" Нет программ, без которых клиент не работает:"),
            theme.text,
        ),
        Line::raw(""),
    ];
    if setup.yt_dlp {
        lines.push(Line::from(vec![
            Span::styled(" yt-dlp", theme.accent),
            Span::styled(t!(" - поиск и загрузка аудио"), theme.muted),
        ]));
        lines.push(Line::styled(format!("   {}", setup::url()), theme.text));
        lines.push(Line::styled(
            t!("   Сумма SHA-256 из релиза сверяется до запуска файла."),
            theme.muted,
        ));
        lines.push(Line::styled(t!("   Enter - скачать"), theme.accent));
        lines.push(Line::raw(""));
    }
    if setup.mpv {
        lines.push(Line::from(vec![
            Span::styled(" mpv", theme.accent),
            Span::styled(t!(" - воспроизведение"), theme.muted),
        ]));
        lines.push(Line::styled(
            t!("   Системный пакет; поставьте сами:"),
            theme.muted,
        ));
        lines.push(Line::styled(format!("   {}", setup::mpv()), theme.text));
        lines.push(Line::raw(""));
    }
    if !setup.message.is_empty() {
        let mark = if setup.busy() {
            format!("{} ", spinner(app))
        } else {
            String::new()
        };
        lines.push(Line::styled(
            format!(" {mark}{}", clean(&setup.message)),
            theme.text,
        ));
    }
    // The window is as tall as what it has to say, and no taller.
    let width = size.width.clamp(24, 76);
    let height = (lines.len() as u16 + 2).clamp(5, size.height);
    let area = Rect::new(
        (size.width - width) / 2,
        (size.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(modal(t!(" ПРОГРАММЫ | Esc - продолжить без них "), theme)),
        area,
    );
}

fn draw_details(frame: &mut Frame, app: &App, theme: &Theme, size: Rect) {
    let area = Rect::new(
        4,
        4,
        size.width.saturating_sub(8),
        size.height.saturating_sub(8),
    );
    frame.render_widget(Clear, area);
    let detail = clean_lines(app.last_error.as_deref().unwrap_or(&app.message));
    frame.render_widget(
        Paragraph::new(detail)
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll, 0))
            .block(modal(
                t!(" ПОДРОБНОСТИ | Up/Down прокрутка | Esc закрыть "),
                theme,
            )),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::Lang;

    fn track() -> Track {
        Track {
            title: "Ночной эфир".into(),
            artist: "Test artist".into(),
            duration: Some(235.0),
            url: "https://soundcloud.com/test/night".into(),
        }
    }
    fn app(path: PathBuf) -> App {
        let mut app = App::new(
            Session {
                yt_dlp: "yt-dlp".into(),
                mpv: "mpv".into(),
                proxy: None,
                direct: false,
                cache: None,
                cache_dir: None,
                settings: Settings {
                    theme: theme::MONO.into(),
                    ..Settings::default()
                },
                config: None,
            },
            path,
            Library::default(),
        );
        app.results = vec![track()];
        app.query = "ночной эфир".into();
        app.message = "Найдено треков: 1".into();
        app
    }

    #[test]
    fn favorites_persist_and_toggle_without_duplicates() {
        let path =
            std::env::temp_dir().join(format!("clicloud-library-test-{}.json", std::process::id()));
        let mut app = app(path.clone());
        app.action(Action::Favorite);
        let stored = library::load(&path).unwrap();
        assert_eq!(stored.favorites.len(), 1);
        assert_eq!(stored.favorites[0].title, "Ночной эфир");
        app.action(Action::Favorite);
        assert!(library::load(&path).unwrap().favorites.is_empty());
        fs::write(&path, "broken json").unwrap();
        assert!(library::load(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "broken json");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn error_details_follow_the_latest_search_or_playback() {
        let mut app = app(PathBuf::new());
        app.yt_dlp = "/missing/yt-dlp".into();
        app.mpv = "/missing/mpv".into();
        app.last_error = Some("stale".into());
        app.search();
        assert!(app.last_error.is_none());
        while app.searching {
            app.tick();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.details);
        assert!(
            app.last_error
                .as_deref()
                .unwrap()
                .contains("/missing/yt-dlp")
        );
        app.action(Action::Play);
        assert!(app.player.is_none());
        let error = app.last_error.clone().unwrap();
        assert!(error.starts_with("mpv:") && error == app.message);

        app.details = true;
        app.last_error =
            Some("Поиск yt-dlp завершился:\nWARNING: first\nERROR: \x1b[31msecond".into());
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let row = |y| -> String { (0..80).map(|x| buffer[(x, y)].symbol()).collect() };
        assert!(row(6).contains("WARNING: first") && !row(6).contains("ERROR"));
        assert!(row(7).contains("ERROR: [31msecond"));
    }

    #[test]
    fn interface_is_ascii_and_colored_by_its_scheme_alone() {
        let screens = || {
            let mut busy = app(PathBuf::new());
            busy.library.favorites.push(track());
            busy.library.recent.push(track());
            busy.queue.push_back(track());
            busy.current = Some(track());
            busy.editing = true;
            busy.focus = Some(5);
            let mut empty = app(PathBuf::new());
            empty.results.clear();
            empty.query.clear();
            empty.searching = true;
            empty.proxy = Some("socks5h://127.0.0.1:9050".into());
            let mut help = app(PathBuf::new());
            help.help = true;
            let mut details = app(PathBuf::new());
            details.details = true;
            let mut library = app(PathBuf::new());
            library.select_view(View::Library);
            let mut settings = app(PathBuf::new());
            settings.options = true;
            settings.config = Some("/tmp/config.json".into());
            let mut low = app(PathBuf::new());
            low.options = true;
            low.setting = SETTINGS.len() - 1;
            let mut picker = app(PathBuf::new());
            picker.options = true;
            picker.themes = vec![theme::MONO.into(), "Dracula".into()];
            picker.picker = Some(Picker {
                state: ListState::default().with_selected(Some(0)),
                original: Theme::mono(),
            });
            [
                (busy, 120, 35),
                (empty, 80, 24),
                (help, 80, 24),
                (details, 100, 30),
                (library, 80, 24),
                (settings, 100, 30),
                (low, 80, 24),
                (picker, 80, 24),
                (app(PathBuf::new()), 60, 15),
            ]
        };
        for (language, name) in [
            (Lang::Russian, theme::MONO),
            (Lang::Russian, "Dracula"),
            (Lang::English, "Catppuccin Latte"),
            (Lang::Japanese, theme::MONO),
            (Lang::Japanese, "Dracula"),
        ] {
            lang::set(language);
            let scheme = Theme::with(name, true);
            assert_eq!(scheme.name, name);
            for (mut app, width, height) in screens() {
                app.theme = scheme.clone();
                let mut terminal =
                    Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
                terminal.draw(|frame| draw(frame, &mut app)).unwrap();
                let buffer = terminal.backend().buffer();
                for y in 0..height {
                    for x in 0..width {
                        let cell = &buffer[(x, y)];
                        let at = format!("{language:?} {name} {width}x{height} at {x},{y}");
                        // The second cell of a wide letter is not drawn by itself.
                        if x > 0 && cells(buffer[(x - 1, y)].symbol()) == 2 {
                            assert_eq!(cell.symbol(), " ", "{at}");
                            continue;
                        }
                        if name == theme::MONO {
                            assert_eq!((cell.fg, cell.bg), (Color::Reset, Color::Reset), "{at}");
                        } else {
                            // Nothing shows through from the terminal's own scheme.
                            assert!(matches!(cell.bg, Color::Rgb(..)), "{at}");
                            assert!(matches!(cell.fg, Color::Rgb(..)), "{at}");
                        }
                        // Letters of any language and its punctuation, but ASCII art;
                        // the logo alone is drawn in the dots of Braille.
                        assert!(
                            cell.symbol().chars().all(|c| c.is_ascii()
                                || c.is_alphabetic()
                                || ('\u{3000}'..='\u{303f}').contains(&c)
                                || ('\u{2800}'..='\u{28ff}').contains(&c) && y < 4),
                            "{:?} in {at}",
                            cell.symbol()
                        );
                        // A letter two cells wide has both of them to itself.
                        assert!(cells(cell.symbol()) < 2 || x + 1 < width, "{at}");
                    }
                }
            }
        }
    }

    // What the screen reads; a letter two cells wide counts once.
    fn screen(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal =
            Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..height {
            let mut skip = false;
            for x in 0..width {
                let symbol = buffer[(x, y)].symbol();
                if !std::mem::take(&mut skip) {
                    text.push_str(symbol);
                }
                skip = cells(symbol) == 2;
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn the_logo_is_shown_small_where_the_screen_has_a_line_for_it() {
        // Eight marks to a cell: two across, four down, and nothing for a space.
        assert_eq!(dots("MM\nMM\nMM\nMM"), ["\u{28ff}"]);
        assert_eq!(
            dots("M   M\n\n\n M  M\n M"),
            ["\u{2881} \u{2841}", "\u{2808}"]
        );
        let small = dots(LOGO);
        assert_eq!((small.len(), cells(&small[0])), (3, 36));
        assert!(small.iter().all(|line| !line.contains('\u{2800}')));

        let mut app = app(PathBuf::new());
        app.logo = true;
        // One that fits beside the logo on the narrowest screen: a longer one is cut.
        app.tagline = TAGLINES[3];
        let plain = screen(&mut app, 80, 24);
        assert!(plain.contains(" CLICLOUD / ") && !plain.contains(&small[0]));
        // One line to spare holds the logo; the next one parts it from the search.
        for (height, search) in [(25, 4), (26, 5), (40, 5)] {
            let text = screen(&mut app, 80, height);
            let lines: Vec<&str> = text.lines().collect();
            for (row, line) in small.iter().enumerate() {
                assert!(
                    lines[1 + row].starts_with(&format!("  {line}")),
                    "{height}\n{text}"
                );
            }
            assert!(lines[3].contains(&format!("/ {}", app.tagline)), "{text}");
            assert!(lines[search].contains("+- / ПОИСК"), "{height}\n{text}");
        }
        // Where the letters of the logo are missing, the name stays spelled.
        app.logo = false;
        let spelled = screen(&mut app, 80, 40);
        assert!(spelled.contains(" CLICLOUD / ") && !spelled.contains(&small[0]));
    }

    #[test]
    fn every_language_fits_its_places() {
        for language in lang::ALL {
            lang::set(language);
            for (width, height) in [(80, 24), (120, 35)] {
                let at = format!("{language:?} {width}x{height}");
                let mut app = app(PathBuf::new());
                app.library.favorites.push(track());
                app.queue.push_back(track());
                let text = screen(&mut app, width, height);
                for part in [
                    t!("1  Поиск"),
                    t!("2  Библиотека"),
                    t!("3  Очередь"),
                    t!("4  Недавние"),
                    t!(" РЕЗУЛЬТАТЫ "),
                    t!("ТРЕК"),
                    t!("ИСПОЛНИТЕЛЬ"),
                    t!("ВРЕМЯ"),
                    t!("|> Играть"),
                    t!("* Избранное"),
                    t!("+ В очередь"),
                    t!("v В кеш"),
                    t!(" ПЛЕЕР "),
                    t!("СТОП"),
                    t!("s Стоп"),
                    t!("Найти"),
                    t!(" / ПОИСК   Enter - найти "),
                    t!(" / поиск  f избранное  a очередь  d в кеш  o настройки  ? помощь  q выход"),
                ] {
                    assert!(text.contains(part), "{at}: {part:?}\n{text}");
                }

                app.options = true;
                app.config = Some("/tmp/config.json".into());
                for (index, setting) in SETTINGS.iter().enumerate() {
                    app.setting = index;
                    let text = screen(&mut app, width, height);
                    // A screen too short for every line shows those around the current.
                    let all = usize::from(height) >= SETTINGS.len() + 11;
                    for name in SETTINGS.map(Setting::name) {
                        assert!(
                            text.contains(name) || !all && name != setting.name(),
                            "{at}: {name:?}\n{text}"
                        );
                    }
                    for line in setting.hint().lines().chain([
                        t!(" НАСТРОЙКИ | Esc - закрыть "),
                        t!("Up/Down - выбор   Left/Right - изменить   Enter - переключить"),
                    ]) {
                        assert!(text.contains(line), "{at}: {line:?}\n{text}");
                    }
                }
                app.options = false;

                app.help = true;
                let text = screen(&mut app, width, height);
                for line in [
                    t!("Поиск (Enter отправляет, Esc отменяет)"),
                    t!("Добавить в очередь   Del  Убрать из очереди"),
                    t!("Загрузить в кеш трек / весь список"),
                    t!("Громкость    s  Стоп    q  Выход"),
                    t!("Настройки и цветовые схемы"),
                    t!(" Мышь работает. Любая клавиша закрывает справку."),
                ] {
                    assert!(text.contains(line), "{at}: {line:?}\n{text}");
                }
            }
        }
    }

    #[test]
    fn settings_change_the_session_and_the_file() {
        let file =
            std::env::temp_dir().join(format!("clicloud-settings-{}.json", std::process::id()));
        let root = std::env::temp_dir().join(format!("clicloud-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut app = app(PathBuf::new());
        app.config = Some(file.clone());
        let choose = |app: &mut App, setting: Setting| {
            app.setting = SETTINGS.iter().position(|s| *s == setting).unwrap();
        };

        // A proxy needs an address before it can be switched on.
        choose(&mut app, Setting::Proxy);
        app.activate();
        assert!(app.proxy.is_none() && app.message.contains("адрес"));
        choose(&mut app, Setting::ProxyAddress);
        app.activate();
        assert_eq!(app.input.as_deref(), Some(config::TOR));
        app.input = Some("ftp://nope".into());
        app.submit_address();
        assert!(app.input.is_some() && app.address.is_none());
        app.input = Some(" http://localhost:8080 ".into());
        app.submit_address();
        assert!(app.input.is_none() && app.proxy.is_none());
        assert_eq!(app.value(Setting::ProxyAddress), "http://localhost:8080");
        choose(&mut app, Setting::Proxy);
        app.activate();
        assert_eq!(app.proxy.as_deref(), Some("http://localhost:8080"));
        assert_eq!(app.value(Setting::Proxy), "вкл");

        choose(&mut app, Setting::SearchLimit);
        app.adjust(1);
        assert_eq!(app.settings.search_limit, 15);
        (0..10).for_each(|_| app.adjust(-1));
        assert_eq!(app.value(Setting::SearchLimit), "5");
        choose(&mut app, Setting::SeekStep);
        app.adjust(1);
        assert_eq!(app.settings.seek_step, 15);
        (0..5).for_each(|_| app.adjust(1));
        assert_eq!(app.value(Setting::SeekStep), "60 с");
        choose(&mut app, Setting::CacheLimit);
        app.adjust(1);
        assert_eq!(app.value(Setting::CacheLimit), "1280 МБ");
        (0..9).for_each(|_| app.adjust(-1));
        assert_eq!(app.value(Setting::CacheLimit), "без ограничения");
        choose(&mut app, Setting::Volume);
        app.adjust(1);
        assert_eq!((app.settings.volume, app.volume), (75, 70.0));

        // The drawing behind the list is changed in a circle, and taken away.
        let drawn = |app: &mut App| screen(app, 120, 35).contains("DOOOO");
        choose(&mut app, Setting::Backdrop);
        assert_eq!(app.value(Setting::Backdrop), "жнец");
        assert!(drawn(&mut app));
        app.adjust(1);
        assert_eq!(app.value(Setting::Backdrop), "пентаграмма");
        assert!(!drawn(&mut app) && screen(&mut app, 120, 35).contains("\"-.-\""));
        app.activate();
        assert_eq!(app.value(Setting::Backdrop), "нет");
        assert_eq!(config::load(Some(&file)).unwrap().backdrop, "none");
        assert!(!drawn(&mut app) && !screen(&mut app, 120, 35).contains("\"-.-\""));
        app.adjust(-1);
        app.adjust(-1);
        assert_eq!(app.settings.backdrop, "reaper");
        for (_, drawing) in BACKDROPS {
            assert!(drawing.is_ascii() && !drawing.contains('\t'));
        }

        // The language changes at once, for the names of the settings too.
        choose(&mut app, Setting::Language);
        assert_eq!(app.value(Setting::Language), "Русский");
        app.adjust(1);
        assert_eq!(app.value(Setting::Language), "English");
        assert_eq!(Setting::Language.name(), "Language");
        assert_eq!(app.message, "Press / to find music. ? - all keys");
        app.adjust(1);
        assert_eq!(
            (lang::current(), app.settings.language.as_str()),
            (Lang::Japanese, "ja")
        );
        app.activate();
        assert_eq!(app.settings.language, "ru");
        app.adjust(-1);
        assert_eq!(app.value(Setting::Language), "日本語");
        // Nothing on the screen is left in the language before.
        app.options = false;
        app.results.clear();
        app.query.clear();
        let text = screen(&mut app, 120, 35);
        assert!(
            !text
                .chars()
                .any(|c| ('а'..='я').contains(&c.to_lowercase().next().unwrap())),
            "{text}"
        );
        assert!(text.contains(t!("Нажмите /, чтобы найти музыку. ? - все клавиши")));
        app.adjust(1);
        assert_eq!(app.value(Setting::Volume), "75");
        assert_eq!(config::load(Some(&file)).unwrap().language, "ru");

        // The cache is opened and left by the same switch.
        choose(&mut app, Setting::Cache);
        app.activate();
        assert!(app.cache.is_none() && app.message.contains("Каталог кеша"));
        app.cache_dir = Some(root.clone());
        app.activate();
        assert!(app.cache.is_some() && app.settings.cache_enabled);
        app.activate();
        assert!(app.cache.is_none() && root.join("CACHEDIR.TAG").exists());

        // The list of schemes shows each one at once and keeps it only on Enter.
        choose(&mut app, Setting::Theme);
        app.activate();
        assert_eq!(app.picker.as_ref().unwrap().state.selected(), Some(1));
        app.pick(1);
        assert_eq!(app.theme.name, app.themes[2]);
        app.close_picker(false);
        assert_eq!(
            (app.theme.name.as_str(), app.settings.theme.as_str()),
            ("mono", "mono")
        );
        app.activate();
        let dracula = app
            .themes
            .iter()
            .position(|name| name == "Dracula")
            .unwrap();
        app.pick(dracula as isize - 1);
        app.pick(isize::MAX);
        app.pick(isize::MIN);
        assert_eq!(app.theme.name, theme::TERMINAL);
        app.pick(dracula as isize);
        app.close_picker(true);
        assert_eq!(
            (app.theme.name.as_str(), app.settings.theme.as_str()),
            ("Dracula", "Dracula")
        );
        app.adjust(1);
        assert_eq!(app.theme.name, app.themes[dracula + 1]);
        app.adjust(-1);
        assert_eq!(app.value(Setting::Theme), "Dracula");

        let saved = config::load(Some(&file)).unwrap();
        assert_eq!(saved.proxy.as_deref(), Some("http://localhost:8080"));
        assert!(saved.proxy_enabled && !saved.cache_enabled);
        assert_eq!(
            (saved.search_limit, saved.seek_step, saved.volume),
            (5, 60, 75)
        );
        assert_eq!((saved.cache_limit_mb, saved.theme.as_str()), (0, "Dracula"));

        // An empty address removes the proxy for good.
        choose(&mut app, Setting::ProxyAddress);
        app.input = Some("  ".into());
        app.submit_address();
        assert!(app.proxy.is_none() && app.address.is_none());
        let saved = config::load(Some(&file)).unwrap();
        assert!(saved.proxy.is_none() && !saved.proxy_enabled);
        fs::remove_file(file).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_likes_of_a_profile_join_the_favorites_from_the_settings() {
        let stamp = std::process::id();
        let path = std::env::temp_dir().join(format!("clicloud-likes-{stamp}.json"));
        let file = std::env::temp_dir().join(format!("clicloud-likes-settings-{stamp}.json"));
        let mut app = app(path.clone());
        app.config = Some(file.clone());
        app.library.favorites.push(track());
        app.options = true;
        app.setting = SETTINGS.len() - 1;
        assert_eq!(app.value(Setting::Likes), "не задан");
        let liked = |name: &str| {
            Found::Track(Track {
                title: name.into(),
                artist: "someone".into(),
                duration: None,
                url: format!("https://soundcloud.com/someone/{name}"),
            })
        };
        let listing = |app: &mut App| {
            let (sender, receiver) = mpsc::channel();
            app.import = Some(Import {
                profile: "someone".into(),
                tracks: vec![],
                receiver,
                cancel: Arc::new(AtomicBool::new(false)),
                worker: std::thread::spawn(|| ()),
            });
            sender
        };

        // What is no profile stays in the line to be put right, and nothing is asked.
        app.activate();
        assert_eq!(app.input.as_deref(), Some(""));
        app.input = Some("https://soundcloud.com/artist/night".into());
        app.submit();
        assert!(app.import.is_none() && app.message.contains("имя профиля"));
        assert_eq!(
            app.value(Setting::Likes),
            "https://soundcloud.com/artist/night_"
        );
        // The address of the proxy is another line, with a text of its own.
        assert_eq!(app.value(Setting::ProxyAddress), "не задан");
        app.input = None;

        // The line counts what arrives, and the favorites wait for the end of the list.
        let sender = listing(&mut app);
        sender.send(liked("one")).unwrap();
        // The favorite under another spelling of its link, and a liked playlist.
        sender
            .send(Found::Track(Track {
                url: "https://www.soundcloud.com/test/night?si=1".into(),
                ..track()
            }))
            .unwrap();
        sender.send(liked("sets")).unwrap();
        app.tick();
        assert_eq!(app.value(Setting::Likes), "someone: получено 3");
        assert_eq!(app.library.favorites.len(), 1);
        sender.send(liked("two")).unwrap();
        sender.send(Found::Over(Ok(()))).unwrap();
        app.tick();
        assert!(app.import.is_none());
        assert_eq!(
            app.value(Setting::Likes),
            "someone: добавлено 2, уже было 1"
        );
        assert_eq!(
            app.message,
            "Лайки someone: добавлено 2, уже было 1, пропущено 1"
        );
        let titles = |library: &Library| -> Vec<String> {
            (library.favorites.iter())
                .map(|track| track.title.clone())
                .collect()
        };
        assert_eq!(titles(&app.library), ["Ночной эфир", "one", "two"]);
        assert_eq!(titles(&library::load(&path).unwrap()), titles(&app.library));

        // Enter on the line stops a list that is being read; nothing of it is kept.
        let sender = listing(&mut app);
        sender.send(liked("three")).unwrap();
        app.tick();
        let cancel = app.import.as_ref().unwrap().cancel.clone();
        app.activate();
        assert!(app.import.is_none() && cancel.load(Ordering::Relaxed));
        assert_eq!(app.value(Setting::Likes), "someone: прервано");
        assert_eq!(app.library.favorites.len(), 3);

        // A list that broke off is kept as far as it came, and the reason is shown.
        let sender = listing(&mut app);
        sender.send(liked("three")).unwrap();
        sender
            .send(Found::Over(Err("HTTP Error 403".into())))
            .unwrap();
        app.tick();
        assert_eq!(app.library.favorites.len(), 4);
        assert_eq!(
            app.value(Setting::Likes),
            "someone: оборвалось, добавлено 1"
        );
        assert!(app.details && app.last_error.as_deref() == Some("HTTP Error 403"));
        app.details = false;

        // Typed and entered, the name is kept in the settings and yt-dlp is asked.
        let lines = [
            r#"{"title":"Four","url":"https://soundcloud.com/low-sea/four"}"#,
            r#"{"title":"One","url":"https://soundcloud.com/someone/one"}"#,
        ];
        // The program notes what it was asked for in a file beside the library.
        let asked = path.with_extension("args");
        let program = script(
            "likes",
            &format!(
                "printf '%s\\n' \"$*\" > '{}'\ncat <<'END'\n{}\nEND",
                asked.display(),
                lines.join("\n")
            ),
            &format!(
                "echo %* > \"{}\"\necho {}\necho {}",
                asked.display(),
                lines[0],
                lines[1]
            ),
        );
        app.yt_dlp = program.to_string_lossy().into_owned();
        app.activate();
        app.input = Some(" @Some-One ".into());
        app.submit();
        assert!(app.input.is_none());
        assert_eq!(app.value(Setting::Likes), "Some-One: получено 0");
        for _ in 0..3000 {
            app.tick();
            if app.import.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            app.value(Setting::Likes),
            "Some-One: добавлено 1, уже было 1"
        );
        assert_eq!(
            (
                app.library.favorites[4].artist.as_str(),
                app.library.favorites[4].title.as_str()
            ),
            ("low sea", "Four")
        );
        let args = fs::read_to_string(&asked).unwrap();
        assert!(
            (args.trim_end()).ends_with("-- https://soundcloud.com/Some-One/likes"),
            "{args}"
        );
        let saved = config::load(Some(&file)).unwrap();
        assert_eq!(saved.soundcloud_profile.as_deref(), Some("Some-One"));

        // The name comes back to be entered again; an empty line forgets it.
        app.activate();
        assert_eq!(app.input.as_deref(), Some("Some-One"));
        app.input = Some(" ".into());
        app.submit();
        assert!(app.import.is_none() && app.settings.soundcloud_profile.is_none());
        assert_eq!(app.value(Setting::Likes), "не задан");
        assert!(
            config::load(Some(&file))
                .unwrap()
                .soundcloud_profile
                .is_none()
        );
        for path in [path, file, program, asked] {
            fs::remove_file(path).unwrap();
        }
    }

    fn numbered(number: usize) -> Track {
        Track {
            title: format!("Title {number}"),
            artist: format!("artist{}", number % 10),
            url: format!("https://soundcloud.com/test/{number}"),
            ..track()
        }
    }

    // Lets the search that runs come to its end.
    fn settle(app: &mut App) {
        for _ in 0..3000 {
            app.tick();
            if !app.searching {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the search did not end");
    }

    #[test]
    fn links_open_as_lists_to_go_through_and_back() {
        // What yt-dlp would answer for each page; a playlist tells all of every track.
        let program = script(
            "pages",
            r#"case "$*" in
*"--flat-playlist"*"/stations/track/test/night")
  echo '{"title":"Night","url":"https://soundcloud.com/test/night"}'
  echo '{"title":"Far","url":"https://soundcloud.com/other/far"}';;
*"--flat-playlist"*"/other/tracks")
  echo '{"title":"Far","url":"https://soundcloud.com/other/far"}'
  echo '{"title":"Near","url":"https://soundcloud.com/other/near"}';;
*"--flat-playlist"*"/other/albums")
  echo '{"title":"Album","url":"https://soundcloud.com/other/sets/album"}';;
*"soundcloud:formats=none"*"/other/sets/album")
  echo '{"title":"First","uploader":"Other","duration":61,"webpage_url":"https://soundcloud.com/other/first"}'
  echo '{"title":"Second","uploader":"Other","duration":62,"webpage_url":"https://soundcloud.com/other/second"}';;
*) echo "ERROR: unexpected $*" >&2; exit 1;;
esac"#,
            // A batch file has no `case`: the kind of the list first, then the page.
            r#"echo %*| findstr /c:"--flat-playlist" >nul
if errorlevel 1 goto full
echo %*| findstr /e /c:"/stations/track/test/night" >nul
if not errorlevel 1 goto station
echo %*| findstr /e /c:"/other/tracks" >nul
if not errorlevel 1 goto tracks
echo %*| findstr /e /c:"/other/albums" >nul
if not errorlevel 1 goto albums
goto unexpected
:full
echo %*| findstr /c:"soundcloud:formats=none" >nul
if errorlevel 1 goto unexpected
echo %*| findstr /e /c:"/other/sets/album" >nul
if errorlevel 1 goto unexpected
echo {"title":"First","uploader":"Other","duration":61,"webpage_url":"https://soundcloud.com/other/first"}
echo {"title":"Second","uploader":"Other","duration":62,"webpage_url":"https://soundcloud.com/other/second"}
exit /b 0
:station
echo {"title":"Night","url":"https://soundcloud.com/test/night"}
echo {"title":"Far","url":"https://soundcloud.com/other/far"}
exit /b 0
:tracks
echo {"title":"Far","url":"https://soundcloud.com/other/far"}
echo {"title":"Near","url":"https://soundcloud.com/other/near"}
exit /b 0
:albums
echo {"title":"Album","url":"https://soundcloud.com/other/sets/album"}
exit /b 0
:unexpected
echo ERROR: unexpected %* 1>&2
exit /b 1"#,
        );
        let path = std::env::temp_dir().join(format!("clicloud-pages-{}.json", std::process::id()));
        let mut app = app(path.clone());
        app.yt_dlp = program.to_string_lossy().into_owned();
        app.mpv = "clicloud-no-such-program".into();
        let titles = |app: &App| -> Vec<String> {
            app.results
                .iter()
                .map(|track| track.title.clone())
                .collect()
        };

        // `m` opens the station of the selected track: the link is asked like a question.
        app.table.select(Some(0));
        app.similar();
        assert_eq!(
            app.query,
            "https://soundcloud.com/stations/track/test/night"
        );
        assert_eq!(app.message, "Читаю список... Esc - отмена");
        settle(&mut app);
        assert_eq!(titles(&app), ["Night", "Far"]);
        assert_eq!(app.results[1].artist, "other");
        assert!(screen(&mut app, 80, 30).contains(" РЕЗУЛЬТАТЫ "));

        // `u` goes to whoever made the selected track, and the brackets through the
        // sections of that profile.
        app.table.select(Some(1));
        app.author();
        settle(&mut app);
        assert_eq!(
            (app.query.as_str(), titles(&app)),
            (
                "https://soundcloud.com/other/tracks",
                vec!["Far".to_owned(), "Near".to_owned()]
            )
        );
        assert!(screen(&mut app, 80, 30).contains(" АВТОР other | треки | [ ] - раздел "));
        app.turn(1);
        settle(&mut app);
        assert_eq!(titles(&app), ["Album"]);
        let text = screen(&mut app, 80, 30);
        assert!(
            text.contains(" АВТОР other | альбомы ") && text.contains(">>"),
            "{text}"
        );

        // Enter on a playlist goes into it instead of playing it.
        app.table.select(Some(0));
        app.action(Action::Play);
        assert!(app.current.is_none() && app.searching);
        settle(&mut app);
        assert_eq!(titles(&app), ["First", "Second"]);
        assert_eq!(
            (app.results[0].artist.as_str(), app.results[0].duration),
            ("Other", Some(61.0))
        );

        // `F` keeps all of the list; a playlist among tracks is not a favorite track.
        app.favor_all();
        assert_eq!(app.message, "В избранное добавлено: 2, уже было: 0");
        assert_eq!(library::load(&path).unwrap().favorites.len(), 2);
        app.favor_all();
        assert_eq!(app.message, "В избранное добавлено: 0, уже было: 2");

        // `b` goes back the way that was come, without asking again.
        app.yt_dlp = "clicloud-no-such-program".into();
        for (link, list) in [
            ("https://soundcloud.com/other/albums", vec!["Album"]),
            ("https://soundcloud.com/other/tracks", vec!["Far", "Near"]),
            (
                "https://soundcloud.com/stations/track/test/night",
                vec!["Night", "Far"],
            ),
        ] {
            app.back();
            assert!(!app.searching, "{link}");
            assert_eq!(
                (app.query.as_str(), titles(&app)),
                (link, list.iter().map(|t| t.to_string()).collect())
            );
        }
        // The list the run began with was not asked for, so it cannot be come back to.
        app.back();
        assert_eq!(app.message, "Раньше ничего не открывалось");

        // A list that plays on leaves out the playlists that stand among its tracks.
        app.results = vec![
            numbered(1),
            Track {
                url: "https://soundcloud.com/other/sets/album".into(),
                duration: None,
                ..numbered(2)
            },
            numbered(3),
        ];
        app.follow(app.results.clone(), 0);
        assert_eq!((app.ahead.len(), app.source.len()), (1, 2));
        assert_eq!(app.ahead[0].title, "Title 3");
        // Nothing but a single track has a station, and a station has no author.
        app.select_view(View::Search);
        app.table.select(Some(1));
        app.similar();
        assert_eq!(app.message, "Станция есть только у отдельного трека");
        drop(app);
        for file in [path, program] {
            fs::remove_file(file).unwrap();
        }
    }

    #[test]
    fn the_next_track_is_fetched_ahead_and_the_player_goes_on_to_it() {
        let root = std::env::temp_dir().join(format!("clicloud-ahead-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let extractor = script("ahead-fetch", "printf audio", "<nul set /p =audio");
        let player = idle("ahead-player");
        let library = root.with_extension("json");
        let mut app = app(library.clone());
        app.cache = Some(Cache::open(root.clone(), 0).unwrap());
        app.yt_dlp = extractor.to_string_lossy().into_owned();
        app.mpv = player.to_string_lossy().into_owned();
        app.results = (1..=3).map(numbered).collect();
        let stored = |app: &App, number: usize| {
            (app.cache.as_ref().unwrap()).contains(&numbered(number).url)
        };

        // While the track that plays is still arriving, nothing else is fetched.
        app.table.select(Some(0));
        app.action(Action::Play);
        app.fetch_ahead();
        assert!(app.fetch.is_none(), "{}", app.message);
        assert!(app.message.contains("Загрузка трека"), "{}", app.message);
        for _ in 0..500 {
            app.tick();
            if stored(&app, 1) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(stored(&app, 1) && !stored(&app, 2));
        // All of it is there and it plays: the one after it is fetched, quietly.
        app.player.as_mut().unwrap().loaded = true;
        app.message = "playing".into();
        app.fetch_ahead();
        assert!(app.fetch.as_ref().is_some_and(|fetch| fetch.ahead));
        assert_eq!(app.fetch.as_ref().unwrap().track.title, "Title 2");
        for _ in 0..500 {
            if app.fetch.is_none() {
                break;
            }
            app.tick();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(stored(&app, 2) && !stored(&app, 3));
        assert_eq!(app.message, "playing");

        // The player went on to it by itself: the lists follow, and no player starts.
        app.advanced();
        assert_eq!(app.current.as_ref().unwrap().title, "Title 2");
        assert_eq!(app.library.recent[0].title, "Title 2");
        assert_eq!(app.previous.last().unwrap().title, "Title 1");
        assert_eq!(app.ahead.len(), 1);
        // A track that could not be fetched ahead is not asked for on every tick.
        app.yt_dlp = "clicloud-no-such-program".into();
        app.fetch_ahead();
        app.fetch_ahead();
        assert!(app.fetch.is_none());
        assert_eq!(app.fetched_ahead.as_deref(), Some(numbered(3).url.as_str()));

        // The speed is kept for the tracks that follow, between half and twice.
        for _ in 0..15 {
            app.pace(1.0);
        }
        assert_eq!((app.speed, app.message.as_str()), (2.0, "Скорость: x2.0"));
        assert!(screen(&mut app, 80, 24).contains("| x2.0 "));
        for _ in 0..20 {
            app.pace(-1.0);
        }
        assert_eq!(app.speed, 0.5);
        drop(app);
        for file in [extractor, player, library] {
            fs::remove_file(file).unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mpv_is_told_how_to_sound_by_the_settings() {
        let file = std::env::temp_dir().join(format!("clicloud-sound-{}.json", std::process::id()));
        let player = script(
            "sound-player",
            "echo 'List of detected audio devices:'\necho \"  'auto' (Autoselect device)\"\necho \"  'pipewire' (Default (pipewire))\"\necho \"  'alsa/card' (Card)\"",
            "echo List of detected audio devices:\necho   'auto' (Autoselect device)\necho   'pipewire' (Default (pipewire))\necho   'alsa/card' (Card)",
        );
        let mut app = app(PathBuf::new());
        app.config = Some(file.clone());
        let choose = |app: &mut App, setting: Setting| {
            app.setting = SETTINGS.iter().position(|s| *s == setting).unwrap();
        };
        choose(&mut app, Setting::Normalize);
        assert_eq!(app.value(Setting::Normalize), "выкл");
        app.activate();
        assert!(app.settings.normalize && app.settings.sound().normalize);

        // mpv is asked once what it can play on; its own choice stands first.
        choose(&mut app, Setting::Device);
        assert_eq!(app.value(Setting::Device), "авто");
        app.mpv = "clicloud-no-such-program".into();
        app.activate();
        assert_eq!(app.message, "mpv не назвал ни одного устройства");
        app.mpv = player.to_string_lossy().into_owned();
        app.activate();
        assert_eq!(app.settings.audio_device.as_deref(), Some("pipewire"));
        assert_eq!(app.value(Setting::Device), "Default (pipewire)");
        app.adjust(1);
        assert_eq!(app.settings.sound().device, Some("alsa/card"));
        app.adjust(1);
        assert_eq!(app.value(Setting::Device), "авто");
        app.adjust(-1);
        assert_eq!(app.value(Setting::Device), "Card");
        let saved = config::load(Some(&file)).unwrap();
        assert!(saved.normalize && saved.audio_device.as_deref() == Some("alsa/card"));
        // A device that mpv does not list is still shown, by its name.
        app.settings.audio_device = Some("jack".into());
        assert_eq!(app.value(Setting::Device), "jack");
        for path in [file, player] {
            fs::remove_file(path).unwrap();
        }
    }

    // Left half black, right half white: a picture that every way of drawing tells apart.
    fn halves() -> Cover {
        let side = cover::SIDE;
        let bytes: Vec<u8> = (0..side * side)
            .flat_map(|at| [if at % side < side / 2 { 0 } else { 255 }; 3])
            .collect();
        Cover::new(&bytes).unwrap()
    }

    #[test]
    fn the_track_that_plays_has_a_tab_with_its_picture_and_all_that_is_known() {
        let mut app = app(PathBuf::new());
        app.logo = false;
        let lists = screen(&mut app, 80, 24);
        assert!(
            lists.contains("t [ Списки ] [ Трек ]") && lists.contains(" ОБЗОР "),
            "{lists}"
        );
        app.action(Action::Track);
        let text = screen(&mut app, 80, 24);
        assert!(
            text.contains(" ТРЕК ") && text.contains("Ничего не играет."),
            "{text}"
        );
        // The tab has the whole main screen: no search, no views, no list; the player stays.
        for gone in [" / ПОИСК ", " ОБЗОР ", " РЕЗУЛЬТАТЫ "] {
            assert!(!text.contains(gone), "{gone}\n{text}");
        }
        assert!(
            text.contains(" ПЛЕЕР ") && text.contains(" CLICLOUD / "),
            "{text}"
        );
        // Nothing plays, so there is nothing for Enter to start, and no end to it either.
        app.action(Action::Play);
        assert!(app.player.is_none());

        app.current = Some(track());
        app.queue.push_back(numbered(7));
        let name = cache::key(&track().url).unwrap();
        let text = screen(&mut app, 80, 24);
        for part in [
            "ждём обложку",
            "Ночной эфир",
            "Test artist",
            "3:55",
            "ищем сведения о треке...",
        ] {
            assert!(text.contains(part), "{part}\n{text}");
        }
        // What follows belongs to the queue, not here.
        assert!(
            !text.contains("Title 7") && !text.contains("ДАЛЕЕ"),
            "{text}"
        );
        app.covers.insert(name.clone(), None);
        app.told.insert(name.clone(), None);
        let text = screen(&mut app, 80, 24);
        assert!(
            text.contains("обложки нет") && text.contains("сведений о треке нет"),
            "{text}"
        );

        // All that was looked up of it, and the marks it has in a list.
        app.library.favorites.push(track());
        app.told.insert(
            name.clone(),
            Some(Details {
                title: Some("Ночной эфир".into()),
                genre: Some("Dance".into()),
                tags: Some(vec!["night".into(), "radio".into()]),
                description: Some("Первая строка описания.\n\nВторая, после пустой.".into()),
                plays: Some(1234567),
                likes: Some(321),
                reposts: Some(0),
                comments: None,
                date: Some("20120529".into()),
                ..Details::default()
            }),
        );
        let text = screen(&mut app, 80, 24);
        for part in [
            "3:55  *",
            "Жанр     Dance",
            "Дата     2012-05-29",
            "Слушали  1 234 567",
            "Лайки    321",
            "Репосты  0",
            "Теги     night, radio",
        ] {
            assert!(text.contains(part), "{part}\n{text}");
        }
        assert!(!text.contains("Комментарии"), "{text}");
        // A wider screen has a second column for what its maker wrote of it; the keys
        // that move through a list move through that.
        let wide = screen(&mut app, 120, 30);
        assert!(wide.contains("Первая строка описания.") && wide.contains("Вторая, после пустой."));
        app.navigate(1);
        assert_eq!(app.scroll, 1);
        screen(&mut app, 120, 30);
        assert_eq!(app.scroll, 0);
        app.told
            .get_mut(&name)
            .unwrap()
            .as_mut()
            .unwrap()
            .description = Some("слово ".repeat(600));
        app.navigate(isize::MAX);
        let wide = screen(&mut app, 120, 30);
        assert!(
            app.scroll > 0 && app.scroll < 1000 && wide.contains("слово"),
            "{}",
            app.scroll
        );
        app.navigate(isize::MIN);
        assert_eq!(app.scroll, 0);

        // Under a scheme without colors the halves of a block are there or not: nine
        // rows of eighteen cells at this size, the dark of the picture left empty.
        app.covers.insert(name.clone(), Some(halves()));
        let text = screen(&mut app, 80, 24);
        let rows: Vec<&str> = text.lines().filter(|line| line.contains('█')).collect();
        assert_eq!(rows.len(), 9, "{text}");
        assert!(
            rows[0].contains("|          █████████  Ночной эфир"),
            "{text}"
        );
        app.settings.cover = "braille".into();
        let text = screen(&mut app, 80, 24);
        assert!(
            text.contains("|          ⣿⣿⣿⣿⣿⣿⣿⣿⣿  ") && !text.contains('█'),
            "{text}"
        );
        // A screen with room to spare draws it larger.
        let tall = screen(&mut app, 120, 40);
        assert_eq!(
            tall.lines().filter(|line| line.contains('⣿')).count(),
            22,
            "{tall}"
        );

        // Under a scheme with colors every cell of the picture has its own: of the
        // picture itself, or between the two colors of the scheme.
        app.theme = Theme::with("Dracula", true);
        let colors = |app: &mut App| {
            let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
            terminal.draw(|frame| draw(frame, app)).unwrap();
            let buffer = terminal.backend().buffer();
            // The first and the last cell of the first row of the picture.
            let row = (0..24)
                .find(|y| ["▀", "⣿"].contains(&buffer[(20, *y)].symbol()))
                .unwrap();
            let cell = |x: u16| {
                let cell = &buffer[(x, row)];
                (cell.symbol().to_owned(), cell.fg, cell.bg)
            };
            (cell(3), cell(20))
        };
        app.settings.cover = "blocks".into();
        let (dark, light) = colors(&mut app);
        assert_eq!(dark, ("▀".into(), Color::Rgb(0, 0, 0), Color::Rgb(0, 0, 0)));
        assert_eq!(
            light,
            (
                "▀".into(),
                Color::Rgb(255, 255, 255),
                Color::Rgb(255, 255, 255)
            )
        );
        app.settings.cover = "block-tones".into();
        let (dark, light) = colors(&mut app);
        let (behind, text_color) = (app.theme.base.bg.unwrap(), app.theme.base.fg.unwrap());
        assert_eq!((dark.1, light.1), (behind, text_color));
        app.settings.cover = "braille-tones".into();
        let (dark, light) = colors(&mut app);
        assert_eq!(
            (dark.0.as_str(), light.0.as_str(), light.1),
            (" ", "⣿", text_color)
        );
        // A terminal of 256 colors gets the nearest of them.
        app.theme = Theme::with("Dracula", false);
        app.settings.cover = "blocks".into();
        let (dark, light) = colors(&mut app);
        assert_eq!((dark.1, light.1), (Color::Indexed(16), Color::Indexed(231)));

        // Without a picture the words have the whole width.
        app.settings.cover = "none".into();
        let text = screen(&mut app, 80, 24);
        assert!(
            text.contains("| Ночной эфир") && !text.contains('▀'),
            "{text}"
        );
        // The settings go round the ways of drawing it.
        app.setting = SETTINGS.iter().position(|s| *s == Setting::Cover).unwrap();
        let mut seen = vec![];
        for _ in 0..5 {
            app.adjust(1);
            seen.push(app.value(Setting::Cover));
        }
        assert_eq!(
            seen,
            [
                "цветные блоки",
                "цветной брайль",
                "блоки в тонах",
                "брайль в тонах",
                "нет"
            ]
        );

        // On its tab the track that plays is the one the keys are about.
        assert_eq!(app.selected().unwrap().url, track().url);
        app.action(Action::Favorite);
        assert!(app.library.favorites.is_empty());
        // A view, a search or the other tab shows the lists again.
        app.select_view(View::Library);
        assert!(!app.track_open);
        app.action(Action::Track);
        app.action(Action::Search);
        assert!(!app.track_open && app.editing);
        app.editing = false;
        app.action(Action::Track);
        app.action(Action::Lists);
        assert!(!app.track_open && screen(&mut app, 80, 24).contains(" ОБЗОР "));
    }

    #[test]
    fn what_the_tab_of_a_track_shows_is_read_from_the_cache_while_it_is_open() {
        let root = std::env::temp_dir().join(format!("clicloud-art-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut app = app(PathBuf::new());
        app.cache = Some(Cache::open(root.clone(), 0).unwrap());
        // Nothing may reach the network: neither program is there to be started.
        app.yt_dlp = "clicloud-no-such-program".into();
        app.mpv = "clicloud-no-such-program".into();
        app.current = Some(track());
        let name = cache::key(&track().url).unwrap();
        let cache = app.cache.as_ref().unwrap();
        fs::create_dir_all(cache.art()).unwrap();
        fs::write(
            cache.art().join(&name),
            vec![7; cover::SIDE * cover::SIDE * 3],
        )
        .unwrap();
        cache.note(
            &track().url,
            &Details {
                title: Some("Ночной эфир".into()),
                genre: Some("Dance".into()),
                ..Details::default()
            },
        );
        let settle = |app: &mut App| {
            for _ in 0..500 {
                app.fetch_page();
                if app.page_job.is_none() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        // While the lists are shown nothing is asked for.
        app.fetch_page();
        assert!(app.page_job.is_none());
        // With no picture wanted, what is known of the track is still read.
        app.action(Action::Track);
        app.settings.cover = "none".into();
        app.fetch_page();
        assert!(app.page_job.is_some());
        settle(&mut app);
        assert_eq!(
            app.told[&name].as_ref().unwrap().genre.as_deref(),
            Some("Dance")
        );
        assert!(!app.covers.contains_key(&name));
        app.settings.cover = "blocks".into();
        settle(&mut app);
        assert!(app.covers[&name].is_some());
        app.fetch_page();
        assert!(app.page_job.is_none());

        // A track of which nothing can be had is not asked for on every tick.
        app.current = Some(numbered(9));
        let other = cache::key(&numbered(9).url).unwrap();
        settle(&mut app);
        assert!(app.told[&other].is_none() && app.covers[&other].is_none());
        app.fetch_page();
        assert!(app.page_job.is_none());
        // Opening the tab again asks once more: the network may have been the reason.
        app.action(Action::Lists);
        app.action(Action::Track);
        assert!(!app.told.contains_key(&other) && !app.covers.contains_key(&other));
        settle(&mut app);
        assert!(app.told[&other].is_none());
        // What a download tells of a track in full is kept for its tab.
        let details = Details {
            title: Some("Title 9".into()),
            artwork: Some("https://i1.sndcdn.com/artworks-abc-large.jpg".into()),
            ..Details::default()
        };
        app.learn(&numbered(9).url, &details);
        assert_eq!(app.told[&other].as_ref(), Some(&details));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_settings_window_shows_the_lines_around_the_current_one() {
        let mut app = app(PathBuf::new());
        app.options = true;
        app.greeted = app.message.clone();
        let first = SETTINGS[0].name();
        let last = SETTINGS[SETTINGS.len() - 1].name();
        // The least screen has no room for every line.
        let text = screen(&mut app, 80, 24);
        assert!(text.contains(first) && !text.contains(last), "{text}");
        app.setting = SETTINGS.len() - 1;
        let text = screen(&mut app, 80, 24);
        assert!(
            !text.contains(first) && text.contains(&format!("> {last}")),
            "{text}"
        );
        assert_eq!(app.setting_first, 1);
        assert!(text.contains("Up/Down - выбор"), "{text}");
        // A taller one shows them all.
        let text = screen(&mut app, 80, 30);
        assert!(text.contains(first) && text.contains(last) && app.setting_first == 0);
    }

    #[test]
    fn what_was_to_play_is_kept_for_the_next_run_and_what_failed_is_marked() {
        let path = std::env::temp_dir().join(format!("clicloud-kept-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        let mut first = app(path.clone());
        first.mpv = "clicloud-no-such-program".into();
        first.current = Some(numbered(1));
        first.queue.push_back(numbered(2));
        first.ahead = [numbered(3), numbered(4)].into();
        first.save();
        // The next run has them in its queue, the track it stopped at first.
        let kept = library::load(&path).unwrap();
        let mut second = App::new(
            Session {
                yt_dlp: "yt-dlp".into(),
                mpv: "clicloud-no-such-program".into(),
                proxy: None,
                direct: false,
                cache: None,
                cache_dir: None,
                settings: Settings {
                    theme: theme::MONO.into(),
                    ..Settings::default()
                },
                config: None,
            },
            path.clone(),
            kept,
        );
        let titles: Vec<&str> = second.queue.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Title 1", "Title 2", "Title 3", "Title 4"]);
        assert_eq!(
            second.message,
            "С прошлого раза в очереди треков: 4. n - включить"
        );
        assert!(second.current.is_none() && second.ahead.is_empty());

        // A track that did not play is marked in its lists until it does.
        second.results = second.queue.iter().cloned().collect();
        second.current = Some(numbered(2));
        second.queue.clear();
        second.failed("boom".into(), None);
        let text = screen(&mut second, 80, 30);
        assert!(
            text.contains("!!  Title 2") && text.contains("01  Title 1"),
            "{text}"
        );
        assert!(!text.contains("02  Title 2"), "{text}");
        // An empty queue is kept as empty.
        second.current = None;
        second.save();
        assert!(library::load(&path).unwrap().queue.is_empty());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_list_plays_on_from_the_track_that_was_started() {
        let player = idle("order-player");
        let file = std::env::temp_dir().join(format!("clicloud-order-{}.json", std::process::id()));
        let mut app = app(PathBuf::new());
        app.config = Some(file.clone());
        app.mpv = player.to_string_lossy().into_owned();
        app.results = (0..6).map(numbered).collect();
        let playing = |app: &App| app.current.as_ref().unwrap().title.clone();
        let titles = |tracks: &VecDeque<Track>| -> Vec<String> {
            tracks.iter().map(|track| track.title.clone()).collect()
        };

        // Enter starts a track, and the rest of its list follows it.
        app.table.select(Some(2));
        app.action(Action::Play);
        assert_eq!(playing(&app), "Title 2");
        assert_eq!(titles(&app.ahead), ["Title 3", "Title 4", "Title 5"]);
        // What was put in the queue by hand plays before that.
        app.queue.push_back(numbered(40));
        app.next();
        assert_eq!(playing(&app), "Title 40");
        app.next();
        assert_eq!(playing(&app), "Title 3");

        // The view of the queue shows both, and a track goes out of either by its place.
        app.queue.push_back(numbered(41));
        app.select_view(View::Queue);
        let shown: Vec<&str> = app.tracks().iter().map(|t| t.title.as_str()).collect();
        assert_eq!(shown, ["Title 41", "Title 4", "Title 5"]);
        assert_eq!(app.take_upcoming(1).unwrap().title, "Title 4");
        // Started from the queue, a track leaves it, and the rest stays as it was.
        app.table.select(Some(1));
        app.action(Action::Play);
        assert_eq!(playing(&app), "Title 5");
        assert_eq!(
            (titles(&app.queue), app.ahead.len()),
            (vec!["Title 41".to_owned()], 0)
        );
        app.next();
        // Nothing follows the last track, and the player stops.
        assert!(!app.more());
        app.next();
        assert!(app.player.is_none() && app.message == "Очередь закончилась");

        // `r` goes round: the list again from its top, or the track again.
        app.select_view(View::Search);
        app.table.select(Some(5));
        app.action(Action::Play);
        assert!(app.ahead.is_empty() && !app.more());
        app.action(Action::Repeat);
        assert_eq!(
            (app.repeat(), app.message.as_str()),
            (Repeat::All, "Повтор: повтор списка")
        );
        assert!(app.more());
        app.ended();
        assert_eq!((playing(&app), app.ahead.len()), ("Title 0".to_owned(), 5));
        app.action(Action::Repeat);
        assert_eq!(app.repeat(), Repeat::One);
        app.ended();
        assert_eq!((playing(&app), app.ahead.len()), ("Title 0".to_owned(), 5));
        // Asked for the next one, it still moves on.
        app.next();
        assert_eq!(playing(&app), "Title 1");
        app.action(Action::Repeat);
        assert_eq!(app.repeat(), Repeat::Off);

        // `z` mixes what is ahead at once, and puts it back in the order of the list.
        app.results = (0..40).map(numbered).collect();
        app.table.select(Some(0));
        app.action(Action::Play);
        let straight = titles(&app.ahead);
        app.action(Action::Shuffle);
        assert_eq!(app.message, "Порядок: вперемешку");
        let mixed = titles(&app.ahead);
        assert_ne!(mixed, straight);
        let mut sorted = mixed.clone();
        sorted.sort_by_key(|title| straight.iter().position(|other| other == title));
        assert_eq!(sorted, straight);
        // A track started while it is on has all the others of the list after it.
        app.table.select(Some(7));
        app.action(Action::Play);
        assert_eq!(app.ahead.len(), 39);
        assert!(app.ahead.iter().all(|track| track.title != "Title 7"));
        app.action(Action::Shuffle);
        assert_eq!(titles(&app.ahead), straight[7..]);
        let saved = config::load(Some(&file)).unwrap();
        assert_eq!((saved.shuffle, saved.repeat.as_str()), (false, "off"));
        assert!(screen(&mut app, 80, 24).contains(" ПЛЕЕР | z по порядку | r без повтора "));

        drop(app);
        fs::remove_file(player).unwrap();
        fs::remove_file(file).unwrap();
    }

    #[test]
    fn a_long_list_is_paged_counted_and_narrowed() {
        let mut app = app(PathBuf::new());
        app.library.favorites = (1..=1200).map(numbered).collect();
        app.logo = false;
        app.select_view(View::Library);
        let text = screen(&mut app, 80, 30);
        // The first screen of it, numbered wide enough for the last, and where it stands.
        assert!(
            text.contains("> 0001  Title 1 ") && text.contains(" 1/1200 "),
            "{text}"
        );
        assert!(
            text.contains("0008  Title 8") && !text.contains("Title 9 "),
            "{text}"
        );
        // The keys move by what the screen holds, and to the ends.
        let page = app.rows.height as isize;
        assert_eq!(page, 8);
        app.navigate(page);
        app.navigate(page);
        let text = screen(&mut app, 80, 30);
        assert!(
            text.contains("> 0017  Title 17") && text.contains(" 17/1200 "),
            "{text}"
        );
        assert!(
            text.contains("0010  Title 10") && !text.contains("Title 9 "),
            "{text}"
        );
        app.navigate(isize::MAX);
        let text = screen(&mut app, 80, 30);
        assert!(
            text.contains("> 1200  Title 1200") && text.contains(" 1200/1200 "),
            "{text}"
        );
        assert!(
            text.contains("1193  Title 1193") && !text.contains("Title 1192"),
            "{text}"
        );
        // Going up, the selection becomes the first row that shows.
        app.navigate(-10);
        let text = screen(&mut app, 80, 30);
        assert!(
            text.contains("> 1190  Title 1190") && text.contains("1197  Title 1197"),
            "{text}"
        );
        app.navigate(isize::MIN);
        assert_eq!(app.table.selected(), Some(0));

        // The words of the filter are looked for in both names, in any order and case.
        app.filtering = true;
        app.filter = "TITLE 77 artist7".into();
        app.navigate(0);
        let found: Vec<String> = (app.tracks().iter())
            .map(|track| track.title.clone())
            .collect();
        let expected = [77, 177, 277, 377, 477, 577, 677, 777, 877, 977, 1077, 1177]
            .map(|number| format!("Title {number}"));
        assert_eq!(found, expected);
        let text = screen(&mut app, 80, 24);
        assert!(text.contains(" ФИЛЬТР БИБЛИОТЕКИ ") && text.contains("TITLE 77 artist7_"));
        assert!(text.contains(&format!(" 1/{} ", found.len())), "{text}");
        // The list that plays on is the narrowed one.
        app.filtering = false;
        app.mpv = "clicloud-no-such-program".into();
        app.action(Action::Play);
        assert_eq!(app.source.len(), found.len());
        app.filter = "no such track".into();
        app.navigate(0);
        assert!(app.selected().is_none());
        assert!(screen(&mut app, 80, 24).contains("По этому фильтру ничего нет."));
        // Another view has the box for the search again, and the filter waits.
        app.select_view(View::Search);
        assert!(screen(&mut app, 80, 24).contains(" / ПОИСК   Enter - найти "));
    }

    #[test]
    fn what_is_learned_of_a_track_reaches_every_list_it_is_in() {
        let path = std::env::temp_dir().join(format!("clicloud-learn-{}.json", std::process::id()));
        let mut app = app(path.clone());
        let liked = Track {
            title: "Remote Viewing".into(),
            artist: "low sea".into(),
            duration: None,
            url: "https://soundcloud.com/low-sea/remote-viewing".into(),
        };
        // Known in full already: the same link under another spelling, with a length.
        let known = Track {
            url: "https://www.soundcloud.com/low-sea/remote-viewing?si=1".into(),
            duration: Some(10.0),
            ..liked.clone()
        };
        app.library.favorites = vec![track(), liked.clone()];
        app.library.recent = vec![liked.clone()];
        app.queue = [liked.clone(), known.clone()].into();
        app.current = Some(liked.clone());
        let details = Details {
            title: Some("Remote Viewing".into()),
            artist: Some("low_sea".into()),
            duration: Some(196.645),
            ..Details::default()
        };
        app.learn(&liked.url, &details);
        for track in [
            &app.library.favorites[1],
            &app.library.recent[0],
            &app.queue[0],
        ]
        .into_iter()
        .chain(&app.current)
        {
            assert_eq!(
                (track.artist.as_str(), track.duration),
                ("low_sea", Some(196.645))
            );
        }
        assert_eq!(
            (app.queue[1].artist.as_str(), app.queue[1].duration),
            ("low sea", Some(10.0))
        );
        assert_eq!(app.library.favorites[0].artist, "Test artist");
        let saved = library::load(&path).unwrap();
        assert_eq!(saved.favorites[1].duration, Some(196.645));
        // Told again, there is nothing left to put right and nothing is written.
        fs::remove_file(&path).unwrap();
        app.learn(&liked.url, &details);
        assert!(!path.exists());
    }

    #[test]
    fn the_library_follows_what_another_process_writes() {
        let path = std::env::temp_dir().join(format!("clicloud-sync-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        let mut app = app(path.clone());
        app.table.select(Some(0));
        app.action(Action::Favorite);
        assert_eq!(app.library.favorites.len(), 1);
        assert!(!app.sync());

        // An import from the command line adds to the file while the interface is open.
        let mut theirs = library::load(&path).unwrap();
        theirs.favorites.push(numbered(1));
        theirs.favorites.push(numbered(2));
        library::save(&path, &theirs).unwrap();
        assert!(app.sync() && !app.sync());
        assert_eq!(app.library.favorites.len(), 3);

        // Written again there before this one saves: neither loses what the other did.
        theirs.favorites.push(numbered(3));
        theirs.favorites.remove(1);
        library::save(&path, &theirs).unwrap();
        app.select_view(View::Library);
        app.table.select(Some(0));
        app.action(Action::Favorite);
        let kept = |library: &Library| -> Vec<String> {
            (library.favorites.iter())
                .map(|track| track.title.clone())
                .collect()
        };
        assert_eq!(kept(&app.library), ["Title 2", "Title 3"]);
        assert_eq!(kept(&library::load(&path).unwrap()), ["Title 2", "Title 3"]);
        // A file that is gone takes nothing away, and one that is damaged is left out.
        fs::remove_file(&path).unwrap();
        assert!(!app.sync() && app.library.favorites.len() == 2);
        fs::write(&path, "{broken").unwrap();
        assert!(!app.sync() && app.library.favorites.len() == 2);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn what_is_said_while_the_settings_are_open_shows_in_them() {
        assert_eq!(
            wrap("Ожидается имя профиля SoundCloud или ссылка", 20),
            ["Ожидается имя", "профиля SoundCloud", "или ссылка"]
        );
        assert_eq!(wrap("abcdefgh ij", 3), ["abc", "def", "gh", "ij"]);
        assert!(wrap("  ", 10).is_empty());

        let mut app = app(PathBuf::new());
        app.options = true;
        app.greeted = app.message.clone();
        // The window covers the line of messages on the least screen.
        let quiet = screen(&mut app, 80, 24);
        assert!(!quiet.contains("Найдено треков: 1"), "{quiet}");
        app.setting = SETTINGS.len() - 1;
        app.activate();
        app.input = Some("https://soundcloud.com/artist/night".into());
        app.submit();
        let said = screen(&mut app, 80, 24);
        assert!(
            said.contains("Ожидается имя профиля SoundCloud или ссылка на него: name или")
                && said.contains("soundcloud.com/name."),
            "{said}"
        );
        assert!(said.contains("Up/Down - выбор"), "{said}");
    }

    #[test]
    fn library_holds_favorites_and_stored_tracks() {
        let root = std::env::temp_dir().join(format!("clicloud-ui-stored-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut app = app(PathBuf::new());
        app.select_view(View::Library);
        app.action(Action::Evict);
        assert!(app.selected().is_none());

        let cache = Cache::open(root.clone(), 0).unwrap();
        for (name, known) in [("night", true), ("by-link", false)] {
            let url = format!("https://soundcloud.com/test/{name}");
            let mut partial = cache.store(&url).unwrap().unwrap();
            if known {
                partial.describe(&track());
            }
            fs::write(partial.path(), [0; 1024 * 1024]).unwrap();
            partial.commit().unwrap();
        }
        app.cache = Some(cache);
        let other = Track {
            title: "Not stored".into(),
            url: "https://soundcloud.com/test/other".into(),
            ..track()
        };
        // A favorite that is stored too is listed once, however its link is spelled.
        app.library.favorites = vec![
            other.clone(),
            Track {
                url: "https://www.soundcloud.com/test/night?si=1".into(),
                ..track()
            },
        ];
        app.select_view(View::Library);
        let titles = |app: &App| -> Vec<String> {
            app.tracks()
                .iter()
                .map(|track| track.title.clone())
                .collect()
        };
        assert_eq!(titles(&app), ["Not stored", "Ночной эфир", "by link"]);
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = (0..30)
            .flat_map(|y| (0..80).map(move |x| (x, y)))
            .map(|at| buffer[at].symbol())
            .collect();
        assert!(text.contains("МОЯ БИБЛИОТЕКА | избранное: 2 | в кеше: 2, 2.0 МБ"));
        // The marks of the three tracks; the header of the columns reads "* v" as well.
        let marks = |pattern: &str| text.matches(pattern).count();
        assert_eq!((marks(" * ."), marks(" * v"), marks(" . v")), (1, 2, 1));
        // The button follows the selected track: this one is not in the cache.
        assert!(text.contains("v В кеш") && !text.contains("x Из кеша"));

        // Leaving the favorites, a stored track stays in the library; leaving the
        // cache as well, it is gone.
        app.navigate(1);
        app.action(Action::Favorite);
        assert_eq!(titles(&app), ["Not stored", "by link", "Ночной эфир"]);
        app.navigate(1);
        app.action(Action::Evict);
        assert_eq!(app.message, "Удалено из кеша: Test artist - Ночной эфир");
        assert_eq!(titles(&app), ["Not stored", "by link"]);
        assert_eq!(app.table.selected(), Some(1));
        app.action(Action::Favorite);
        assert_eq!(app.library.favorites.len(), 2);
        app.action(Action::Evict);
        assert_eq!(titles(&app), ["Not stored", "by link"]);
        assert!(app.cache.as_ref().unwrap().tracks().is_empty());
        app.navigate(-1);
        app.action(Action::Evict);
        assert_eq!(app.message, "Этого трека нет в кеше");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn progress_bar_fills_its_width() {
        let bar = |position, duration, width| {
            let (played, loaded, left, times) = progress(position, 0.0, duration, width);
            format!("[{played}{loaded}{left}]{times}")
        };
        // What has arrived stands between what has played and what is still to come.
        let (played, loaded, left, _) = progress(30.0, 45.0, 60.0, 24);
        assert_eq!(format!("{played}{loaded}{left}"), "====>~~---");
        let (played, loaded, left, _) = progress(30.0, 10.0, 60.0, 24);
        assert_eq!(format!("{played}{loaded}{left}"), "====>-----");
        let (_, loaded, left, _) = progress(0.0, 600.0, 60.0, 24);
        assert_eq!(format!("{loaded}{left}"), "~~~~~~~~~~");
        assert_eq!(bar(0.0, 0.0, 24), "[----------] 0:00 / 0:00");
        assert_eq!(bar(30.0, 60.0, 24), "[====>-----] 0:30 / 1:00");
        assert_eq!(bar(90.0, 60.0, 24), "[=========>] 1:30 / 1:00");
        assert_eq!(bar(1.0, 2.0, 3), "[] 0:01 / 0:02");
    }

    // A stand-in for an external program. What such a program is written in differs:
    // `sh` on Unix, `cmd` on Windows, where the name must also say so.
    fn script(name: &str, sh: &str, cmd: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "clicloud-{name}-{}{}",
            std::process::id(),
            if cfg!(windows) { ".cmd" } else { "" }
        ));
        // Written by a child process: a descriptor open for writing in this one would be
        // inherited by whatever another test thread starts, and the script could not be run.
        #[cfg(unix)]
        {
            use std::io::Write;
            let _ = cmd;
            let mut writer = std::process::Command::new("sh")
                .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
                .arg(&path)
                .stdin(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let source = format!("#!/bin/sh\n{sh}\n");
            (writer.stdin.take().unwrap())
                .write_all(source.as_bytes())
                .unwrap();
            assert!(writer.wait().unwrap().success());
        }
        #[cfg(not(unix))]
        {
            let _ = sh;
            // A batch file is read line by line, and cmd loses a block of its own
            // unless every line ends the way it expects, however this file is checked out.
            let body = cmd.replace("\r\n", "\n").replace('\n', "\r\n");
            fs::write(&path, format!("@echo off\r\n{body}\r\n")).unwrap();
        }
        path
    }

    // A program that does nothing until it is stopped. `ping` is how a batch file
    // waits; it keeps none of the inherited pipes, so stopping it is not waited for.
    fn idle(name: &str) -> PathBuf {
        script(name, "exec sleep 30", "ping -n 31 127.0.0.1 >nul 2>&1")
    }

    #[test]
    fn the_programs_that_are_missing_are_offered_or_named() {
        // One that is certainly there wherever the tests run, and one that is not.
        let there = if cfg!(windows) { "cmd" } else { "sh" };
        let absent = "clicloud-no-such-program";
        assert!(Setup::needed(there, there).is_none());
        let only_player = Setup::needed(there, absent).unwrap();
        assert!(!only_player.yt_dlp && only_player.mpv && !only_player.busy());
        let only_extractor = Setup::needed(absent, there).unwrap();
        assert!(only_extractor.yt_dlp && !only_extractor.mpv);

        let mut app = app(PathBuf::new());
        app.setup = Setup::needed(absent, absent);
        let (url, mpv) = (setup::url(), setup::mpv());
        let text = screen(&mut app, 80, 24);
        for part in [
            t!(" ПРОГРАММЫ | Esc - продолжить без них "),
            t!("   Enter - скачать"),
            t!("   Системный пакет; поставьте сами:"),
            url.as_str(),
            mpv.as_str(),
        ] {
            assert!(text.contains(part), "{part:?}\n{text}");
        }

        // Nothing is fetched for a program that is not ours to fetch.
        app.setup = Setup::needed(there, absent);
        app.install();
        let setup = app.setup.as_ref().unwrap();
        assert!(!setup.busy() && setup.message.is_empty());
    }

    #[test]
    fn the_mark_of_a_search_turns_through_its_frames() {
        let mut app = app(PathBuf::new());
        let mut seen = Vec::new();
        for step in 0..SPINNER.len() * 2 {
            let Some(since) = Instant::now().checked_sub(Duration::from_millis(120 * step as u64))
            else {
                return;
            };
            app.since = since;
            seen.push(spinner(&app));
        }
        // Every frame is used, and the turn comes back round.
        for frame in SPINNER {
            assert!(seen.contains(&frame), "{frame:?} {seen:?}");
        }
        assert_eq!(seen[..SPINNER.len()], seen[SPINNER.len()..]);
        // It is drawn where the search is, and only while one runs.
        app.since = Instant::now();
        app.searching = true;
        assert!(screen(&mut app, 80, 24).contains(&format!(" ПОИСК {} Esc", SPINNER[0])));
        app.searching = false;
        assert!(screen(&mut app, 80, 24).contains("/ ПОИСК"));
    }

    #[test]
    fn what_yt_dlp_learns_is_asked_for_only_where_it_is_kept_and_not_kept_yet() {
        let root = std::env::temp_dir().join(format!("clicloud-warm-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut app = app(PathBuf::new());

        // Without a cache there is nowhere to keep what would be learned.
        assert!(app.to_learn().is_none());

        app.cache = Some(Cache::open(root.clone(), 0).unwrap());
        let kept = app.cache.as_ref().unwrap().extractor().to_owned();
        assert_eq!(app.to_learn(), Some(kept.clone()));

        // Something is kept already: every run after the first asks for nothing.
        fs::create_dir_all(&kept).unwrap();
        fs::write(kept.join("client_id"), b"kept").unwrap();
        assert!(app.to_learn().is_none());

        // An empty directory is one that yt-dlp has not written to yet.
        fs::remove_file(kept.join("client_id")).unwrap();
        assert_eq!(app.to_learn(), Some(kept));

        drop(app);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_list_fills_while_the_search_is_still_running() {
        let mut app = app(PathBuf::new());
        let limit = u16::from(app.settings.search_limit);
        let found = |title: &str| {
            Found::Track(Track {
                title: title.into(),
                ..track()
            })
        };
        let (sender, receiver) = mpsc::channel();
        app.receiver = Some(receiver);
        app.searching = true;
        app.filling = false;
        app.shown = Some(("ambient".into(), limit));

        // The list of the search before stays until this one has a track of its own.
        app.tick();
        let titles: Vec<&str> = app.results.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Ночной эфир"]);
        assert!(app.searching);

        sender.send(found("One")).unwrap();
        app.tick();
        assert_eq!(app.results.len(), 1);
        assert_eq!(app.results[0].title, "One");
        // Still running: the rest is on its way.
        assert!(app.searching && app.receiver.is_some());

        sender.send(found("Two")).unwrap();
        sender.send(Found::Over(Ok(()))).unwrap();
        app.tick();
        assert!(!app.searching && app.receiver.is_none());
        assert_eq!(app.message, "Найдено треков: 2");
        assert_eq!(app.searched[&("ambient".to_owned(), limit)].len(), 2);

        // A search that finds nothing leaves no list of the one before.
        let (sender, receiver) = mpsc::channel();
        app.receiver = Some(receiver);
        app.searching = true;
        app.filling = false;
        app.shown = Some(("nothing".into(), limit));
        sender.send(Found::Over(Ok(()))).unwrap();
        app.tick();
        assert!(app.results.is_empty() && app.message == "Найдено треков: 0");
    }

    #[test]
    fn a_question_asked_before_is_answered_without_a_search() {
        let mut app = app(PathBuf::new());
        // No search of this test may reach the network, whatever is installed here.
        app.yt_dlp = "clicloud-no-such-program".into();
        let limit = u16::from(app.settings.search_limit);
        let kept = vec![
            Track {
                title: "One".into(),
                ..track()
            },
            Track {
                title: "Two".into(),
                ..track()
            },
        ];
        app.searched.insert(("ambient".into(), limit), kept);
        app.shown = Some(("night".into(), limit));

        app.query = "  ambient  ".into();
        app.search();
        let titles: Vec<&str> = app.results.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["One", "Two"]);
        assert_eq!(app.message, "Найдено треков: 2");
        assert!(app.view == View::Search);
        assert_eq!(app.shown, Some(("ambient".into(), limit)));
        // Nothing was started: no worker, and the interface is not waiting.
        assert!(!app.searching && app.worker.is_none());

        // Asking what the list already answers is how a search is repeated.
        app.search();
        assert!(app.searching && app.worker.is_some());
        app.cancel_search();
        app.worker.take().unwrap().join().unwrap();

        // A question with another limit is another question.
        app.shown = None;
        app.settings.search_limit += 1;
        app.search();
        assert!(app.searching);
        app.cancel_search();
        app.worker.take().unwrap().join().unwrap();
    }

    #[test]
    fn escape_cancels_a_running_search() {
        let extractor = idle("slow-search");
        let mut app = app(PathBuf::new());
        app.yt_dlp = extractor.to_string_lossy().into_owned();
        app.search();
        assert!(app.searching);
        let started = std::time::Instant::now();
        app.cancel_search();
        assert!(!app.searching && app.message == "Поиск отменён");
        app.worker.take().unwrap().join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
        app.tick();
        assert!(!app.details && app.last_error.is_none());
        // The cancelled search must not block the next one.
        app.search();
        assert!(app.searching);
        app.cancel_search();
        fs::remove_file(extractor).unwrap();
    }

    #[test]
    fn failed_track_skips_to_the_next_until_too_many_fail_in_a_row() {
        let player = idle("idle-player");
        let library =
            std::env::temp_dir().join(format!("clicloud-skip-test-{}.json", std::process::id()));
        let mut app = app(library.clone());
        app.mpv = player.to_string_lossy().into_owned();
        let numbered = |number: u8| Track {
            url: format!("https://soundcloud.com/test/{number}"),
            ..track()
        };
        app.current = Some(numbered(0));
        app.queue = (1..=4).map(numbered).collect();

        app.failed("boom".into(), Some("log line".into()));
        assert_eq!(app.current.as_ref().unwrap().url, numbered(1).url);
        assert!(app.player.is_some() && app.message.contains("включён следующий"));
        assert_eq!(
            app.last_error.as_deref(),
            Some("Test artist - Ночной эфир\nboom\n\nlog line")
        );
        app.failed("boom".into(), None);
        assert_eq!(app.current.as_ref().unwrap().url, numbered(2).url);
        app.failed("boom".into(), None);
        assert!(app.player.is_none() && app.message.contains("Очередь остановлена"));
        assert_eq!(app.queue.len(), 2);

        // A manual start gives the queue a fresh chance.
        app.action(Action::Next);
        assert_eq!(app.current.as_ref().unwrap().url, numbered(3).url);
        app.failed("boom".into(), None);
        assert_eq!(app.current.as_ref().unwrap().url, numbered(4).url);
        app.failed("last".into(), None);
        assert!(app.player.is_none() && app.message == "last");
        drop(app);
        fs::remove_file(player).unwrap();
        fs::remove_file(library).unwrap();
    }

    #[test]
    fn tracks_are_downloaded_into_the_cache_one_at_a_time() {
        let root = std::env::temp_dir().join(format!("clicloud-ui-cache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let extractor = script(
            "fetch",
            r#"case "$*" in *broken*) echo 'ERROR: gone' >&2; exit 1;; esac; printf audio"#,
            // `set /p` is how a batch file writes without a line break of its own, and
            // the last status is the one of `findstr` until the exit below says otherwise.
            r#"echo %*| findstr /c:broken >nul
if not errorlevel 1 (
echo ERROR: gone 1>&2
exit /b 1
)
<nul set /p =audio
exit /b 0"#,
        );
        let mut app = app(PathBuf::new());
        app.action(Action::Download);
        assert!(app.message.contains("Кеш отключён") && app.fetch.is_none());

        app.cache = Some(Cache::open(root.clone(), 0).unwrap());
        app.yt_dlp = extractor.to_string_lossy().into_owned();
        let named = |name: &str| Track {
            url: format!("https://soundcloud.com/test/{name}"),
            ..track()
        };
        let stored = |app: &App, name: &str| app.cache.as_ref().unwrap().contains(&named(name).url);
        let screen = |app: &mut App| {
            let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 30)).unwrap();
            terminal.draw(|frame| draw(frame, app)).unwrap();
            let buffer = terminal.backend().buffer();
            let text: String = (0..30)
                .flat_map(|y| (0..80).map(move |x| (x, y)))
                .map(|at| buffer[at].symbol())
                .collect();
            text
        };
        app.results = vec![named("one"), named("broken"), named("two"), named("sets")];
        app.download(app.results.clone());
        // The playlist link is left out; the rest waits behind the first download.
        assert!(app.fetch.is_some() && app.pending.len() == 2);
        assert!(app.fetching(&named("two").url) && !app.fetching(&named("sets").url));
        assert_eq!(screen(&mut app).matches(" . ~").count(), 3);
        while app.fetch.is_some() {
            app.tick();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(stored(&app, "one") && stored(&app, "two") && !stored(&app, "broken"));
        assert_eq!(app.message, "В кеше: Test artist - Ночной эфир");
        let error = app.last_error.clone().unwrap();
        assert!(error.starts_with("Test artist - Ночной эфир\n") && error.ends_with("ERROR: gone"));
        assert_eq!(fs::read_dir(root.join("partial")).unwrap().count(), 0);
        let text = screen(&mut app);
        assert_eq!(
            (text.matches(" . v").count(), text.matches(" . ~").count()),
            (2, 0)
        );

        app.download(vec![named("one")]);
        assert!(app.fetch.is_none() && app.message.contains("Уже в кеше"));
        app.download(vec![named("sets")]);
        assert!(app.message.contains("только отдельные треки"));
        drop(app);
        fs::remove_file(extractor).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recent_tracks_are_a_view_of_their_own() {
        let mut app = app(PathBuf::new());
        let other = Track {
            url: "https://soundcloud.com/test/other".into(),
            ..track()
        };
        app.library.recent = vec![other.clone(), track()];
        app.select_view(View::Recent);
        assert_eq!(app.selected().unwrap().url, other.url);
        app.navigate(1);
        assert_eq!(app.selected().unwrap().url, track().url);
        app.action(Action::Enqueue);
        assert_eq!(app.queue.len(), 1);
    }

    #[test]
    fn queue_and_selection_handle_empty_lists() {
        let mut app = app(PathBuf::new());
        app.action(Action::Enqueue);
        app.select_view(View::Queue);
        assert_eq!(app.selected().unwrap().url, track().url);
        app.navigate(999);
        assert_eq!(app.table.selected(), Some(0));
        app.queue.clear();
        app.navigate(-1);
        assert!(app.selected().is_none());
    }

    #[test]
    fn responsive_render_and_buttons_are_inside_terminal() {
        for (width, height) in [(120, 35), (80, 24), (60, 15)] {
            let mut app = app(PathBuf::new());
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            let buffer = terminal.backend().buffer();
            let mut text = String::new();
            for y in 0..height {
                for x in 0..width {
                    text.push_str(buffer[(x, y)].symbol());
                }
                text.push('\n');
            }
            // The name is there in letters, or drawn where the screen has a line for it.
            assert!(text.contains("CLICLOUD") || text.contains(&dots(LOGO)[0]));
            if width >= 80 {
                assert!(text.contains("Ночной эфир"));
                assert!(text.contains("ПЛЕЕР"));
                assert!(app.hits.len() >= 15);
                for (rect, _) in &app.hits {
                    assert!(rect.right() <= width && rect.bottom() <= height);
                }
            } else {
                assert!(text.contains("Увеличьте терминал"));
            }
        }
    }
}
