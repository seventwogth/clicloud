use crate::{
    Result,
    cache::Cache,
    clean, clean_lines, config,
    playback::{self, Origin, Playback},
    player::{self, Download},
    soundcloud::{Extractor, SoundCloud, Track},
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
use serde::{Deserialize, Serialize};
use serde_json::json;
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::{
    collections::VecDeque,
    fs,
    io::{self, IsTerminal, Write},
    path::PathBuf,
    sync::mpsc,
    time::Duration,
};

// Monochrome: the terminal's own colors, told apart by bold, dim and reverse video.
const TEXT: Style = Style::new();
const MUTED: Style = Style::new().add_modifier(Modifier::DIM);
const ACCENT: Style = Style::new()
    .add_modifier(Modifier::BOLD)
    .remove_modifier(Modifier::DIM);
const INVERSE: Style = Style::new()
    .add_modifier(Modifier::REVERSED)
    .remove_modifier(Modifier::DIM);
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

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Library {
    favorites: Vec<Track>,
    recent: Vec<Track>,
}

fn load_library(path: &PathBuf) -> Result<Library> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)
            .map_err(|_| "Файл библиотеки повреждён; он не был перезаписан.")?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Library::default()),
        Err(e) => Err(e.into()),
    }
}

fn save_library(path: &PathBuf, library: &Library) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = fs::File::create(&temp)?;
        file.write_all(&serde_json::to_vec_pretty(library)?)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Search,
    Library,
    Queue,
    Recent,
}

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
    Previous,
    Pause,
    Next,
    Stop,
    Quieter,
    Louder,
}

// A track being downloaded into the cache without being played.
struct Fetch {
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
    // One download at a time: the connection is better spent on the track that plays.
    fetch: Option<Fetch>,
    pending: VecDeque<Track>,
    library_path: PathBuf,
    library: Library,
    view: View,
    results: Vec<Track>,
    queue: VecDeque<Track>,
    previous: Vec<Track>,
    current: Option<Track>,
    player: Option<Playback>,
    failures: u8,
    volume: f64,
    query: String,
    editing: bool,
    searching: bool,
    receiver: Option<mpsc::Receiver<std::result::Result<Vec<Track>, String>>>,
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
    fn new(
        yt_dlp: String,
        mpv: String,
        proxy: Option<String>,
        direct: bool,
        cache: Option<Cache>,
        library_path: PathBuf,
        library: Library,
    ) -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            worker: None,
            yt_dlp,
            mpv,
            proxy,
            direct,
            cache,
            fetch: None,
            pending: VecDeque::new(),
            library_path,
            library,
            view: View::Search,
            results: vec![],
            queue: VecDeque::new(),
            previous: vec![],
            current: None,
            player: None,
            failures: 0,
            volume: 70.0,
            query: String::new(),
            editing: false,
            searching: false,
            receiver: None,
            table: TableState::default().with_selected(0),
            message: "Нажмите /, чтобы найти музыку. ? - все клавиши".into(),
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
            View::Library => self.library.favorites.iter().collect(),
            View::Queue => self.queue.iter().collect(),
            View::Recent => self.library.recent.iter().collect(),
        }
    }
    fn selected(&self) -> Option<Track> {
        self.tracks()
            .get(self.table.selected().unwrap_or(0))
            .map(|t| (*t).clone())
    }
    fn select_view(&mut self, view: View) {
        self.view = view;
        self.table = TableState::default().with_selected(0);
        self.editing = false;
    }
    fn navigate(&mut self, delta: isize) {
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
        if let Err(error) = save_library(&self.library_path, &self.library) {
            self.message = format!("Не удалось сохранить библиотеку: {error}");
        }
    }
    fn search(&mut self) {
        if self.searching {
            self.message = "Поиск уже выполняется...".into();
            return;
        }
        if self.query.trim().is_empty() {
            self.message = "Введите исполнителя или название трека".into();
            return;
        }
        self.last_error = None;
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
        self.worker = Some(std::thread::spawn(move || {
            let result = SoundCloud::new(Extractor {
                program: &binary,
                proxy: proxy.as_deref(),
                no_proxy: direct,
                cache: cache.as_deref(),
            })
            .quiet()
            .cancellable(&cancel)
            .search(&query, 30)
            .map_err(|e| e.to_string());
            let _ = sender.send(result);
        }));
        self.receiver = Some(receiver);
        self.searching = true;
        self.editing = false;
        self.select_view(View::Search);
        self.message = "Ищем треки в SoundCloud... Esc - отмена".into();
    }
    fn cancel_search(&mut self) {
        // The worker kills yt-dlp; its late result goes nowhere.
        self.cancel.store(true, Ordering::Relaxed);
        self.receiver = None;
        self.searching = false;
        self.message = "Поиск отменён".into();
    }
    fn start(&mut self, track: Track) {
        self.player = None;
        self.last_error = None;
        match Playback::start(
            &self.mpv,
            self.extractor(),
            &track.url,
            self.cache.as_ref(),
            self.volume,
        ) {
            Ok(player) => {
                self.message = match player.origin {
                    Origin::Cache => "Трек из кеша",
                    Origin::Download => "Загрузка трека; перемотка - в пределах загруженного",
                    Origin::Pipe => "Загрузка через прокси, перемотка ограничена буфером",
                    Origin::Url => "Подключение к аудиопотоку...",
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
        if let Some(track) = self.queue.pop_front() {
            if let Some(current) = self.current.take() {
                self.previous.push(current);
            }
            self.start(track);
        } else {
            self.player = None;
            self.message = "Очередь закончилась".into();
        }
    }
    fn failed(&mut self, error: String, details: Option<String>) {
        self.player = None;
        self.failures += 1;
        let mut report = error.clone();
        if let Some(track) = &self.current {
            report = format!("{} - {}\n{report}", track.artist, track.title);
        }
        if let Some(details) = details {
            report = format!("{report}\n\n{details}");
        }
        if self.queue.is_empty() {
            self.message = error;
        } else if self.failures < SKIP_LIMIT {
            self.next();
            if self.player.is_none() {
                // The next track did not even start; that error is already shown.
                return;
            }
            self.message = "Трек не проигрался, включён следующий. e - подробности".into();
        } else {
            self.message =
                "Очередь остановлена после нескольких ошибок подряд. e - подробности".into();
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
            self.message = "Кеш отключён: треки не сохраняются".into();
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
            (0, 0) => "Уже в кеше или загружается".into(),
            (0, _) => "В кеш сохраняются только отдельные треки".into(),
            _ => format!(
                "Загрузка в кеш, осталось треков: {}",
                self.pending.len() + usize::from(self.fetch.is_some())
            ),
        };
        self.fetch_next();
    }
    fn fetch_next(&mut self) {
        while self.fetch.is_none()
            && let Some(track) = self.pending.pop_front()
        {
            match self.fetch(&track) {
                Ok(fetch) => {
                    self.fetch = fetch.map(|(download, log)| Fetch {
                        track,
                        download,
                        log,
                    })
                }
                Err(error) => {
                    // Neither the disk nor yt-dlp will do better for the rest.
                    self.pending.clear();
                    self.message = format!("Не удалось начать загрузку: {error}");
                    self.last_error = Some(self.message.clone());
                }
            }
        }
    }
    fn fetch(&self, track: &Track) -> Result<Option<(Download, PathBuf)>> {
        let Some(cache) = self.cache.as_ref().filter(|c| !c.contains(&track.url)) else {
            return Ok(None);
        };
        let Some(partial) = cache.store(&track.url)? else {
            return Ok(None);
        };
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
            self.message = "Сначала выберите трек и нажмите Enter".into();
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
            Action::Search => {
                self.editing = true;
                self.focus = None;
            }
            Action::Play => {
                if let Some(track) = self.selected() {
                    if self.view == View::Queue {
                        self.queue.remove(self.table.selected().unwrap_or(0));
                    }
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
                        self.message = "Трек удалён из библиотеки".into();
                    } else {
                        self.library.favorites.push(track);
                        self.message = "Трек добавлен в библиотеку".into();
                    }
                    self.save();
                    self.navigate(0);
                }
            }
            Action::Enqueue => {
                if let Some(track) = self.selected() {
                    self.queue.push_back(track);
                    self.message = "Добавлено в конец очереди".into();
                }
            }
            Action::Download => {
                if let Some(track) = self.selected() {
                    self.download(vec![track]);
                }
            }
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
                self.message = "Воспроизведение остановлено".into();
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
        if let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(result) => {
                    self.searching = false;
                    self.receiver = None;
                    match result {
                        Ok(tracks) => {
                            self.message = format!("Найдено треков: {}", tracks.len());
                            self.results = tracks;
                            self.table.select(Some(0));
                        }
                        Err(error) => {
                            self.last_error = Some(error);
                            self.message =
                                "Ошибка поиска. e - подробности, Esc - закрыть окно".into();
                            self.details = true;
                            self.detail_scroll = 0;
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.searching = false;
                    self.receiver = None;
                    self.message = "Поиск прерван".into();
                }
                Err(mpsc::TryRecvError::Empty) => (),
            }
        }
        if let Some(fetch) = &mut self.fetch
            && let Some(saved) = match fetch.download.poll() {
                Ok(status) => status.map(|status| status.success()),
                Err(_) => Some(false),
            }
        {
            let name = format!("{} - {}", fetch.track.artist, fetch.track.title);
            if saved {
                self.message = format!("В кеше: {name}");
            } else {
                let mut report = format!("{name}\nyt-dlp не смог загрузить трек в кеш.");
                let details = playback::tail(&fetch.log);
                if !details.is_empty() {
                    report = format!("{report}\n\n{}", details.join("\n"));
                }
                self.last_error = Some(report);
                self.message = "Трек не загрузился в кеш. e - подробности".into();
            }
            self.fetch = None;
            self.fetch_next();
        }
        if let Some(player) = &mut self.player {
            let was_loaded = player.loaded;
            let result = player.tick();
            if !was_loaded && player.loaded {
                // After a skip the notice about the failed track stays on screen.
                if self.failures == 0 {
                    self.message = if player.origin == Origin::Cache {
                        "Воспроизведение из кеша. Space - пауза, n - следующий"
                    } else {
                        "Воспроизведение. Space - пауза, n - следующий"
                    }
                    .into();
                }
                self.failures = 0;
            }
            self.volume = player.volume;
            match result {
                Ok(true) => self.next(),
                Err(error) => {
                    let details = player.details();
                    self.failed(error.to_string(), details);
                }
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

pub fn run(
    yt_dlp: String,
    mpv: String,
    proxy: Option<String>,
    direct: bool,
    library: Option<PathBuf>,
    cache: Option<Cache>,
) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("Интерфейсу нужен интерактивный терминал. Для скриптов используйте search или play --first.".into());
    }
    let path = library
        .or_else(|| {
            config::directory("XDG_DATA_HOME", ".local/share")
                .map(|p| p.join("clicloud/library.json"))
        })
        .ok_or("Не удалось определить путь библиотеки; укажите ui --library PATH")?;
    let library = load_library(&path)?;
    let terminate = Arc::new(AtomicBool::new(false));
    for signal in [SIGTERM, SIGHUP, SIGINT] {
        signal_hook::flag::register(signal, terminate.clone())?;
    }
    let mut app = App::new(yt_dlp, mpv, proxy, direct, cache, path, library);
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
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('?') => app.help = true,
                    KeyCode::Char('e') => {
                        app.details = true;
                        app.detail_scroll = 0;
                    }
                    KeyCode::Char('/') => app.action(Action::Search),
                    KeyCode::Char('1') => app.select_view(View::Search),
                    KeyCode::Char('2') => app.select_view(View::Library),
                    KeyCode::Char('3') => app.select_view(View::Queue),
                    KeyCode::Char('4') => app.select_view(View::Recent),
                    KeyCode::Down | KeyCode::Char('j') => {
                        app.focus = None;
                        app.navigate(1);
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        app.focus = None;
                        app.navigate(-1);
                    }
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
                    KeyCode::Char('+') | KeyCode::Char('=') => app.action(Action::Louder),
                    KeyCode::Char('-') => app.action(Action::Quieter),
                    KeyCode::Left => app.control(json!(["seek", -10, "relative"])),
                    KeyCode::Right => app.control(json!(["seek", 10, "relative"])),
                    KeyCode::Delete if app.view == View::Queue => {
                        app.queue.remove(app.table.selected().unwrap_or(0));
                        app.navigate(0);
                    }
                    _ => (),
                }
            }
            Event::Mouse(mouse) if !app.help && !app.details => match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    let pos = Position::new(mouse.column, mouse.row);
                    if let Some((_, action)) = app.hits.iter().find(|(rect, _)| rect.contains(pos))
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
            },
            _ => (),
        }
    }
    Ok(())
}

fn block(title: &str) -> Block<'_> {
    Block::bordered()
        .border_set(BORDER)
        .border_style(MUTED)
        .title(Line::from(vec![
            Span::raw("-"),
            Span::styled(title, ACCENT),
        ]))
}
fn label(frame: &mut Frame, area: Rect, text: impl Into<Text<'static>>, style: Style) {
    frame.render_widget(Paragraph::new(text).style(style), area);
}
fn button(frame: &mut Frame, app: &mut App, area: Rect, text: &str, action: Action, active: bool) {
    let focused = app.focus == Some(app.hits.len());
    app.hits.push((area, action));
    let (style, edges) = if focused {
        (INVERSE.patch(ACCENT), [">", "<"])
    } else if active {
        (INVERSE, ["[", "]"])
    } else {
        (TEXT, ["[", "]"])
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
fn time(value: f64) -> String {
    let seconds = value.max(0.0) as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
fn progress(position: f64, duration: f64, width: u16) -> String {
    let times = format!(" {} / {}", time(position), time(duration));
    let cells = usize::from(width).saturating_sub(times.len() + 2);
    let filled = if duration > 0.0 {
        ((position / duration).clamp(0.0, 1.0) * cells as f64) as usize
    } else {
        0
    };
    let head = if filled > 0 { ">" } else { "" };
    format!(
        "[{}{head}{}]{times}",
        "=".repeat(filled.saturating_sub(1)),
        "-".repeat(cells - filled)
    )
}

fn draw(frame: &mut Frame, app: &mut App) {
    let size = frame.area();
    app.hits.clear();
    app.rows = Rect::default();
    if size.width < 80 || size.height < 24 {
        label(
            frame,
            size,
            "\n  CLICLOUD\n\n  Увеличьте терминал до 80x24.\n  q - выход",
            ACCENT,
        );
        return;
    }
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(3),
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
    draw_header(frame, app, outer[0]);
    draw_search(frame, app, outer[1]);
    draw_navigation(frame, app, body[0]);
    draw_tracks(frame, app, body[1]);
    if let Some(area) = body.get(2) {
        draw_upcoming(frame, app, *area);
    }
    draw_player(frame, app, outer[3]);
    label(
        frame,
        Rect::new(outer[4].x, outer[4].y, outer[4].width, 1),
        clean(&app.message),
        ACCENT,
    );
    label(
        frame,
        Rect::new(outer[4].x, outer[4].y + 1, outer[4].width, 1),
        " / поиск  Enter играть  f избранное  a очередь  d в кеш  ? помощь  q выход",
        MUTED,
    );
    if app.help {
        draw_help(frame, size);
    }
    if app.details {
        draw_details(frame, app, size);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let header = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(20), Constraint::Length(28)])
        .split(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" CLICLOUD ", ACCENT),
            Span::styled("/ your terminal, your music", MUTED),
        ])),
        header[0],
    );
    frame.render_widget(
        Paragraph::new(if app.proxy.is_some() {
            "PROXY ON / SOUNDCLOUD "
        } else {
            "SOUNDCLOUD / STREAMING "
        })
        .alignment(Alignment::Right),
        header[1],
    );
}

fn draw_search(frame: &mut Frame, app: &mut App, area: Rect) {
    let query = if app.query.is_empty() && !app.editing {
        "Найти исполнителя, трек, новый звук...".into()
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
    frame.render_widget(
        Paragraph::new(query)
            .block(
                block(if app.searching {
                    " ПОИСК: загрузка... Esc - отмена "
                } else {
                    " / ПОИСК   Enter - найти "
                })
                .border_style(if app.editing || app.focus == Some(0) {
                    ACCENT
                } else {
                    MUTED
                }),
            )
            .style(if app.editing { TEXT } else { MUTED }),
        search[0],
    );
    button(
        frame,
        app,
        Rect::new(search[1].x, search[1].y + 1, search[1].width, 1),
        "Найти",
        Action::Submit,
        false,
    );
}

fn draw_navigation(frame: &mut Frame, app: &mut App, area: Rect) {
    frame.render_widget(block(" ОБЗОР "), area);
    let nav = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let step = if nav.height >= 7 { 2 } else { 1 };
    for (i, (name, view)) in [
        ("1  Поиск", View::Search),
        ("2  Библиотека", View::Library),
        ("3  Очередь", View::Queue),
        ("4  Недавние", View::Recent),
    ]
    .into_iter()
    .enumerate()
    {
        button(
            frame,
            app,
            Rect::new(nav.x, nav.y + i as u16 * step, nav.width, 1),
            &format!("{name:<width$}", width = usize::from(nav.width) - 2),
            Action::View(view),
            app.view == view,
        );
    }
    if nav.height > 10 {
        let loading = app.pending.len() + usize::from(app.fetch.is_some());
        label(
            frame,
            Rect::new(nav.x, nav.y + 9, nav.width, nav.height - 9),
            format!(
                "{} избранных\n{} в очереди\n{}",
                app.library.favorites.len(),
                app.queue.len(),
                if loading > 0 {
                    format!("{loading} качается\n\nv - в кеше")
                } else if app.cache.is_some() {
                    "\nv - в кеше,\nиграет и\nбез сети".into()
                } else {
                    "\nЛокальная\nбиблиотека".into()
                }
            ),
            MUTED,
        );
    }
}

fn draw_tracks(frame: &mut Frame, app: &mut App, area: Rect) {
    let center = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(2)])
        .split(area);
    let title = match app.view {
        View::Search => " РЕЗУЛЬТАТЫ ",
        View::Library => " МОЯ БИБЛИОТЕКА ",
        View::Queue => " ОЧЕРЕДЬ ВОСПРОИЗВЕДЕНИЯ ",
        View::Recent => " НЕДАВНИЕ ",
    };
    let tracks = app.tracks();
    if tracks.is_empty() {
        let message = match app.view {
            View::Search => {
                "\n\nВаша следующая любимая песня - здесь.\n\nНажмите / и введите поисковый запрос.\nEnter - слушать, f - сохранить"
            }
            View::Library => {
                "\n\nСоберите свою коллекцию.\n\nНажмите f на треке в результатах поиска.\nИзбранное сохранится между запусками."
            }
            View::Queue => {
                "\n\nМузыка без перерывов.\n\nНажмите a, чтобы добавить трек в очередь.\nСледующий трек запустится автоматически."
            }
            View::Recent => {
                "\n\nИстория прослушивания.\n\nЗдесь появятся последние включённые треки.\nEnter - включить снова."
            }
        };
        frame.render_widget(
            Paragraph::new(message)
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: false })
                .style(MUTED)
                .block(block(title)),
            center[0],
        );
    } else {
        let rows: Vec<Row> = tracks
            .iter()
            .enumerate()
            .map(|(i, track)| {
                let favorite = app.library.favorites.iter().any(|t| t.url == track.url);
                let playing = app.current.as_ref().is_some_and(|t| t.url == track.url)
                    && app.player.is_some();
                let stored = if app.cache.as_ref().is_some_and(|c| c.contains(&track.url)) {
                    "v"
                } else if app.fetching(&track.url) {
                    "~"
                } else {
                    "."
                };
                Row::new(vec![
                    if playing {
                        "|>".into()
                    } else {
                        format!("{:02}", i + 1)
                    },
                    clean(&track.title),
                    clean(&track.artist),
                    track.duration.map(time).unwrap_or_else(|| "-".into()),
                    if favorite { "*" } else { "." }.into(),
                    stored.into(),
                ])
                .style(if playing { ACCENT } else { TEXT })
            })
            .collect();
        let table = Table::new(
            rows,
            [
                Constraint::Length(3),
                Constraint::Min(12),
                Constraint::Percentage(27),
                Constraint::Length(6),
                Constraint::Length(1),
                Constraint::Length(1),
            ],
        )
        .header(
            Row::new(["#", "ТРЕК", "ИСПОЛНИТЕЛЬ", "ВРЕМЯ", "*", "v"])
                .style(MUTED)
                .bottom_margin(1),
        )
        .block(block(title))
        .row_highlight_style(INVERSE)
        .highlight_symbol("> ");
        frame.render_stateful_widget(table, center[0], &mut app.table);
        app.rows = Rect::new(
            center[0].x + 1,
            center[0].y + 3,
            center[0].width.saturating_sub(2),
            center[0].height.saturating_sub(4),
        );
    }
    let buttons = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 4); 4])
        .spacing(1)
        .split(Rect::new(
            center[1].x + 1,
            center[1].y,
            center[1].width - 2,
            1,
        ));
    button(frame, app, buttons[0], "|> Играть", Action::Play, false);
    button(
        frame,
        app,
        buttons[1],
        "* Избранное",
        Action::Favorite,
        false,
    );
    button(
        frame,
        app,
        buttons[2],
        "+ В очередь",
        Action::Enqueue,
        false,
    );
    button(frame, app, buttons[3], "v В кеш", Action::Download, false);
}

fn draw_upcoming(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![Line::styled("ДАЛЕЕ", ACCENT), Line::raw("")];
    for track in app.queue.iter().take(4) {
        lines.push(Line::raw(clean(&track.title)));
        lines.push(Line::styled(clean(&track.artist), MUTED));
        lines.push(Line::raw(""));
    }
    if app.queue.is_empty() {
        lines.push(Line::styled("Очередь пока пуста", MUTED));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled("НЕДАВНО", ACCENT));
    for track in app.library.recent.iter().take(3) {
        lines.push(Line::raw(clean(&track.title)));
    }
    frame.render_widget(Paragraph::new(lines).block(block(" НА СЛУХУ ")), area);
}

fn draw_player(frame: &mut Frame, app: &mut App, area: Rect) {
    frame.render_widget(block(" ПЛЕЕР "), area);
    let area = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let (position, duration, state) = if let Some(player) = &app.player {
        let listed = app.current.as_ref().and_then(|t| t.duration).unwrap_or(0.0);
        (
            player.position,
            // While audio arrives, mpv reports the length of what it has got so far.
            if player.duration > 0.0 && !(player.receiving() && listed > 0.0) {
                player.duration
            } else {
                listed
            },
            if !player.loaded {
                "ЗАГРУЗКА"
            } else if player.paused {
                "ПАУЗА"
            } else {
                "ИГРАЕТ"
            },
        )
    } else {
        (0.0, 0.0, "СТОП")
    };
    let song = app
        .current
        .as_ref()
        .map(|t| format!("{} - {}", clean(&t.artist), clean(&t.title)))
        .unwrap_or_else(|| "Выберите трек, чтобы начать".into());
    label(
        frame,
        Rect::new(area.x, area.y, area.width, 1),
        format!("{state}  /  {song}"),
        TEXT,
    );
    label(
        frame,
        Rect::new(area.x, area.y + 2, area.width, 1),
        progress(position, duration, area.width),
        TEXT,
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
            "|| Пауза"
        } else {
            "|> Играть"
        },
        Action::Pause,
        false,
    );
    button(frame, app, controls[2], "n >|", Action::Next, false);
    button(frame, app, controls[3], "s Стоп", Action::Stop, false);
    button(frame, app, controls[5], "-", Action::Quieter, false);
    label(
        frame,
        controls[6],
        format!(" VOL {:3.0}", app.volume),
        MUTED,
    );
    button(frame, app, controls[7], "+", Action::Louder, false);
}

fn draw_help(frame: &mut Frame, size: Rect) {
    let modal = Rect::new(size.width / 2 - 32, size.height / 2 - 9, 64, 17);
    frame.render_widget(Clear, modal);
    let keys = [
        ("/", "Поиск (Enter отправляет, Esc отменяет)"),
        ("1 2 3 4", "Поиск / библиотека / очередь / недавние"),
        ("Up Down, j k", "Выбрать трек     Enter  Проиграть"),
        ("Tab", "Выбрать кнопку   Enter  Нажать"),
        ("f", "Добавить / удалить из избранного"),
        ("a", "Добавить в очередь   Del  Убрать из очереди"),
        ("d / D", "Загрузить в кеш трек / весь список"),
        ("Space", "Пауза / продолжить"),
        ("p / n", "Предыдущий / следующий трек"),
        ("Left / Right", "Перемотка на 10 секунд"),
        ("- / +", "Громкость    s  Стоп    q  Выход"),
        ("e", "Подробности последней ошибки"),
    ];
    let mut lines = vec![Line::raw("")];
    lines.extend(
        keys.iter()
            .map(|(key, action)| Line::raw(format!(" {key:<13}{action}"))),
    );
    lines.push(Line::raw(""));
    lines.push(Line::raw(
        " Мышь работает. Любая клавиша закрывает справку.",
    ));
    frame.render_widget(
        Paragraph::new(lines).block(block(" УПРАВЛЕНИЕ ").border_style(ACCENT)),
        modal,
    );
}

fn draw_details(frame: &mut Frame, app: &App, size: Rect) {
    let modal = Rect::new(
        4,
        4,
        size.width.saturating_sub(8),
        size.height.saturating_sub(8),
    );
    frame.render_widget(Clear, modal);
    let detail = clean_lines(app.last_error.as_deref().unwrap_or(&app.message));
    frame.render_widget(
        Paragraph::new(detail)
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll, 0))
            .block(block(" ПОДРОБНОСТИ | Up/Down прокрутка | Esc закрыть ").border_style(ACCENT)),
        modal,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "yt-dlp".into(),
            "mpv".into(),
            None,
            false,
            None,
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
        let stored = load_library(&path).unwrap();
        assert_eq!(stored.favorites.len(), 1);
        assert_eq!(stored.favorites[0].title, "Ночной эфир");
        app.action(Action::Favorite);
        assert!(load_library(&path).unwrap().favorites.is_empty());
        fs::write(&path, "broken json").unwrap();
        assert!(load_library(&path).is_err());
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
    fn interface_is_monochrome_ascii_apart_from_text() {
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
        for (mut app, width, height) in [
            (busy, 120, 35),
            (empty, 80, 24),
            (help, 80, 24),
            (details, 100, 30),
            (app(PathBuf::new()), 60, 15),
        ] {
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            let buffer = terminal.backend().buffer();
            for y in 0..height {
                for x in 0..width {
                    let cell = &buffer[(x, y)];
                    assert_eq!((cell.fg, cell.bg), (Color::Reset, Color::Reset));
                    assert!(
                        cell.symbol()
                            .chars()
                            .all(|c| c.is_ascii() || c.is_alphabetic()),
                        "{:?} at {x},{y}",
                        cell.symbol()
                    );
                }
            }
        }
    }

    #[test]
    fn progress_bar_fills_its_width() {
        assert_eq!(progress(0.0, 0.0, 24), "[----------] 0:00 / 0:00");
        assert_eq!(progress(30.0, 60.0, 24), "[====>-----] 0:30 / 1:00");
        assert_eq!(progress(90.0, 60.0, 24), "[=========>] 1:30 / 1:00");
        assert_eq!(progress(1.0, 2.0, 3), "[] 0:01 / 0:02");
    }

    fn script(name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("clicloud-{name}-{}", std::process::id()));
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn escape_cancels_a_running_search() {
        let extractor = script("slow-search", "exec sleep 30");
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
        let player = script("idle-player", "exec sleep 30");
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
            assert!(text.contains("CLICLOUD"));
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
