use crate::{
    Result, clean, clean_lines,
    playback::Playback,
    soundcloud::{SoundCloud, Track},
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

const BG: Color = Color::Rgb(17, 19, 24);
const PANEL: Color = Color::Rgb(24, 27, 34);
const EDGE: Color = Color::Rgb(49, 54, 65);
const TEXT: Color = Color::Rgb(224, 227, 233);
const MUTED: Color = Color::Rgb(137, 146, 164);
const ACCENT: Color = Color::Rgb(255, 145, 83);

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
}
#[derive(Clone, Copy)]
enum Action {
    View(View),
    Search,
    Submit,
    Play,
    Favorite,
    Enqueue,
    Previous,
    Pause,
    Next,
    Stop,
    Quieter,
    Louder,
}

struct App {
    cancel: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    yt_dlp: String,
    mpv: String,
    proxy: Option<String>,
    direct: bool,
    library_path: PathBuf,
    library: Library,
    view: View,
    results: Vec<Track>,
    queue: VecDeque<Track>,
    previous: Vec<Track>,
    current: Option<Track>,
    player: Option<Playback>,
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
            library_path,
            library,
            view: View::Search,
            results: vec![],
            queue: VecDeque::new(),
            previous: vec![],
            current: None,
            player: None,
            volume: 70.0,
            query: String::new(),
            editing: false,
            searching: false,
            receiver: None,
            table: TableState::default().with_selected(0),
            message: "Нажмите /, чтобы найти музыку. ? — все клавиши".into(),
            help: false,
            details: false,
            detail_scroll: 0,
            last_error: None,
            hits: vec![],
            focus: None,
            rows: Rect::default(),
        }
    }
    fn tracks(&self) -> Vec<&Track> {
        match self.view {
            View::Search => self.results.iter().collect(),
            View::Library => self.library.favorites.iter().collect(),
            View::Queue => self.queue.iter().collect(),
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
            self.message = "Поиск уже выполняется…".into();
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
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let cancel = self.cancel.clone();
        self.worker = Some(std::thread::spawn(move || {
            let result = SoundCloud::new(&binary, proxy.as_deref(), direct)
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
        self.message = "Ищем треки в SoundCloud…".into();
    }
    fn start(&mut self, track: Track) {
        self.player = None;
        self.last_error = None;
        match Playback::start(
            &self.mpv,
            &self.yt_dlp,
            &track.url,
            self.proxy.as_deref(),
            self.direct,
            self.volume,
        ) {
            Ok(player) => {
                self.message = if self.proxy.is_some() {
                    "Загрузка через прокси • перемотка ограничена буфером"
                } else {
                    "Подключение к аудиопотоку…"
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
                                "Ошибка поиска. e — подробности; Esc — закрыть окно".into();
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
        if let Some(player) = &mut self.player {
            let was_loaded = player.loaded;
            let result = player.tick();
            if !was_loaded && player.loaded {
                self.message = "Воспроизведение • Space — пауза, n — следующий".into();
            }
            self.volume = player.volume;
            match result {
                Ok(true) => self.next(),
                Err(error) => {
                    self.message = error.to_string();
                    self.last_error = Some(match player.details() {
                        Some(details) => format!("{}\n\n{details}", self.message),
                        None => self.message.clone(),
                    });
                    self.player = None;
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
) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("Интерфейсу нужен интерактивный терминал. Для скриптов используйте search или play --first.".into());
    }
    let path = library
        .or_else(|| {
            std::env::var_os("XDG_DATA_HOME")
                .filter(|p| !p.is_empty())
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
                .map(|p| p.join("clicloud/library.json"))
        })
        .ok_or("Не удалось определить путь библиотеки; укажите ui --library PATH")?;
    let library = load_library(&path)?;
    let terminate = Arc::new(AtomicBool::new(false));
    for signal in [SIGTERM, SIGHUP, SIGINT] {
        signal_hook::flag::register(signal, terminate.clone())?;
    }
    let mut app = App::new(yt_dlp, mpv, proxy, direct, path, library);
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
                    KeyCode::Esc => app.focus = None,
                    KeyCode::Enter => app.action(
                        app.focus
                            .and_then(|i| app.hits.get(i).map(|h| h.1))
                            .unwrap_or(Action::Play),
                    ),
                    KeyCode::Char(' ') => app.action(Action::Pause),
                    KeyCode::Char('f') => app.action(Action::Favorite),
                    KeyCode::Char('a') => app.action(Action::Enqueue),
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
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(EDGE))
        .style(Style::default().bg(PANEL))
}
fn label(frame: &mut Frame, area: Rect, text: impl Into<Text<'static>>, color: Color) {
    frame.render_widget(Paragraph::new(text).style(Style::default().fg(color)), area);
}
fn button(frame: &mut Frame, app: &mut App, area: Rect, text: &str, action: Action, active: bool) {
    let focused = app.focus == Some(app.hits.len());
    app.hits.push((area, action));
    let style = if active || focused {
        Style::default().fg(BG).bg(ACCENT).bold()
    } else {
        Style::default().fg(TEXT).bg(EDGE)
    };
    frame.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .style(style),
        area,
    );
}
fn time(value: f64) -> String {
    let seconds = value.max(0.0) as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn draw(frame: &mut Frame, app: &mut App) {
    let size = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BG).fg(TEXT)),
        size,
    );
    app.hits.clear();
    app.rows = Rect::default();
    if size.width < 80 || size.height < 24 {
        label(
            frame,
            size,
            "\n  CLICLOUD\n\n  Увеличьте терминал до 80 × 24.\n  q — выход",
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
        " / поиск  ↑↓ выбор  Enter играть  Space пауза  f ♥  a очередь  e ошибка  ? помощь  q выход",
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
            Span::styled(" ▂▄▆ CLICLOUD ", Style::default().fg(ACCENT).bold()),
            Span::styled(" / YOUR TERMINAL, YOUR MUSIC", Style::default().fg(MUTED)),
        ])),
        header[0],
    );
    label(
        frame,
        header[1],
        if app.proxy.is_some() {
            "● PROXY ON  /  SOUNDCLOUD"
        } else {
            "● SOUNDCLOUD / STREAMING"
        },
        ACCENT,
    );
}

fn draw_search(frame: &mut Frame, app: &mut App, area: Rect) {
    let query = if app.query.is_empty() && !app.editing {
        "Найти исполнителя, трек, новый звук…".into()
    } else {
        format!(
            "{}{}",
            clean(&app.query),
            if app.editing { "▏" } else { "" }
        )
    };
    let search = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(20), Constraint::Length(12)])
        .split(area);
    app.hits.push((search[0], Action::Search));
    frame.render_widget(
        Paragraph::new(query)
            .block(
                block(if app.searching {
                    " ПОИСК • загрузка… "
                } else {
                    " / ПОИСК   Enter — найти "
                })
                .border_style(Style::default().fg(
                    if app.editing || app.focus == Some(0) {
                        ACCENT
                    } else {
                        EDGE
                    },
                )),
            )
            .style(Style::default().fg(if app.editing { TEXT } else { MUTED })),
        search[0],
    );
    button(
        frame,
        app,
        Rect::new(search[1].x, search[1].y + 1, search[1].width, 1),
        "Найти →",
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
    for (i, (name, view)) in [
        ("1  Поиск", View::Search),
        ("2  Библиотека", View::Library),
        ("3  Очередь", View::Queue),
    ]
    .into_iter()
    .enumerate()
    {
        button(
            frame,
            app,
            Rect::new(nav.x, nav.y + i as u16 * 2, nav.width, 1),
            name,
            Action::View(view),
            app.view == view,
        );
    }
    if nav.height > 8 {
        label(
            frame,
            Rect::new(nav.x, nav.y + 7, nav.width, nav.height - 7),
            format!(
                "{} избранных\n{} в очереди\n\nЛокальная\nбиблиотека",
                app.library.favorites.len(),
                app.queue.len()
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
    };
    let tracks = app.tracks();
    if tracks.is_empty() {
        let message = match app.view {
            View::Search => {
                "\n\nВаша следующая любимая песня — здесь.\n\nНажмите / и введите поисковый запрос.\nEnter — слушать · f — сохранить"
            }
            View::Library => {
                "\n\nСоберите свою коллекцию.\n\nНажмите f на треке в результатах поиска.\nИзбранное сохранится между запусками."
            }
            View::Queue => {
                "\n\nМузыка без перерывов.\n\nНажмите a, чтобы добавить трек в очередь.\nСледующий трек запустится автоматически."
            }
        };
        frame.render_widget(
            Paragraph::new(message)
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(MUTED))
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
                Row::new(vec![
                    if playing {
                        "▶".into()
                    } else {
                        format!("{:02}", i + 1)
                    },
                    clean(&track.title),
                    clean(&track.artist),
                    track.duration.map(time).unwrap_or_else(|| "—".into()),
                    if favorite { "♥" } else { "·" }.into(),
                ])
                .style(Style::default().fg(if playing { ACCENT } else { TEXT }))
            })
            .collect();
        let table = Table::new(
            rows,
            [
                Constraint::Length(3),
                Constraint::Min(12),
                Constraint::Percentage(27),
                Constraint::Length(6),
                Constraint::Length(2),
            ],
        )
        .header(
            Row::new(["#", "ТРЕК", "ИСПОЛНИТЕЛЬ", "ВРЕМЯ", "♥"])
                .style(Style::default().fg(MUTED))
                .bottom_margin(1),
        )
        .block(block(title))
        .row_highlight_style(Style::default().bg(Color::Rgb(57, 45, 40)).fg(ACCENT))
        .highlight_symbol("› ");
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
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(Rect::new(
            center[1].x + 1,
            center[1].y,
            center[1].width - 2,
            1,
        ));
    button(frame, app, buttons[0], "▶ Играть", Action::Play, false);
    button(
        frame,
        app,
        buttons[1],
        "♥ Избранное",
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
}

fn draw_upcoming(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![
        Line::styled("ДАЛЕЕ", Style::default().fg(ACCENT).bold()),
        Line::raw(""),
    ];
    for track in app.queue.iter().take(4) {
        lines.push(Line::raw(clean(&track.title)));
        lines.push(Line::styled(
            clean(&track.artist),
            Style::default().fg(MUTED),
        ));
        lines.push(Line::raw(""));
    }
    if app.queue.is_empty() {
        lines.push(Line::styled(
            "Очередь пока пуста",
            Style::default().fg(MUTED),
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled("НЕДАВНО", Style::default().fg(ACCENT).bold()));
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
        (
            player.position,
            if player.duration > 0.0 {
                player.duration
            } else {
                app.current.as_ref().and_then(|t| t.duration).unwrap_or(0.0)
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
        .map(|t| format!("{} — {}", clean(&t.artist), clean(&t.title)))
        .unwrap_or_else(|| "Выберите трек, чтобы начать".into());
    label(
        frame,
        Rect::new(area.x, area.y, area.width, 1),
        format!("{state}  /  {song}"),
        TEXT,
    );
    frame.render_widget(
        Gauge::default()
            .ratio(if duration > 0.0 {
                (position / duration).clamp(0.0, 1.0)
            } else {
                0.0
            })
            .gauge_style(Style::default().fg(ACCENT).bg(EDGE))
            .label(format!("{} / {}", time(position), time(duration))),
        Rect::new(area.x, area.y + 2, area.width, 1),
    );
    let controls = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(9),
            Constraint::Length(12),
            Constraint::Length(9),
            Constraint::Length(9),
            Constraint::Min(1),
            Constraint::Length(5),
            Constraint::Length(9),
            Constraint::Length(5),
        ])
        .split(Rect::new(area.x, area.y + 4, area.width, 1));
    button(frame, app, controls[0], "|◀ p", Action::Previous, false);
    button(
        frame,
        app,
        controls[1],
        if app.player.as_ref().is_some_and(|p| !p.paused) {
            "Ⅱ Пауза"
        } else {
            "▶ Играть"
        },
        Action::Pause,
        true,
    );
    button(frame, app, controls[2], "n ▶|", Action::Next, false);
    button(frame, app, controls[3], "■ Стоп", Action::Stop, false);
    button(frame, app, controls[5], " − ", Action::Quieter, false);
    label(
        frame,
        controls[6],
        format!(" VOL {:3.0}", app.volume),
        MUTED,
    );
    button(frame, app, controls[7], " + ", Action::Louder, false);
}

fn draw_help(frame: &mut Frame, size: Rect) {
    let modal = Rect::new(size.width / 2 - 32, size.height / 2 - 8, 64, 16);
    frame.render_widget(Clear, modal);
    frame.render_widget(Paragraph::new("\n /            Поиск (Enter отправляет, Esc отменяет)\n 1 / 2 / 3    Поиск / библиотека / очередь\n ↑ ↓, j k     Выбрать трек     Enter  Проиграть\n Tab          Выбрать кнопку  Enter  Нажать\n f            Добавить / удалить из избранного\n a            Добавить в очередь   Del  Убрать из очереди\n Space        Пауза / продолжить\n p / n        Предыдущий / следующий трек\n ← / →        Перемотка на 10 секунд\n − / +        Громкость    s  Стоп    q  Выход\n\n Кнопки и строки доступны мышью.\n Любая клавиша закрывает справку.").style(Style::default().fg(TEXT)).block(block(" УПРАВЛЕНИЕ ").border_style(Style::default().fg(ACCENT))), modal);
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
            .style(Style::default().fg(TEXT))
            .block(
                block(" ПОДРОБНОСТИ • ↑↓ прокрутка • Esc закрыть ")
                    .border_style(Style::default().fg(ACCENT)),
            ),
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
                assert!(app.hits.len() >= 14);
                for (rect, _) in &app.hits {
                    assert!(rect.right() <= width && rect.bottom() <= height);
                }
            } else {
                assert!(text.contains("Увеличьте терминал"));
            }
        }
    }
}
