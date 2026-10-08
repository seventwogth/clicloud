//! The language of the interface and of the messages.
//!
//! Russian is the language of the sources: a text is written in it where it is used,
//! inside `t!`, and the other languages are looked up by it in the table below. A
//! text without a translation shows in Russian; a test holds the table complete.
use std::cell::Cell;
use std::collections::HashMap;
use std::fmt::{Display, Write};
use std::sync::OnceLock;

/// The text in the current language, with each `{}` replaced by the next argument.
macro_rules! t {
    ($text:literal) => {
        $crate::lang::translate($text)
    };
    ($text:literal, $($argument:expr),+ $(,)?) => {
        $crate::lang::fill($text, &[$(&$argument as &dyn ::std::fmt::Display),+])
    };
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Lang {
    Russian,
    English,
    Japanese,
}

pub const ALL: [Lang; 3] = [Lang::Russian, Lang::English, Lang::Japanese];

impl Lang {
    /// What the settings file calls the language.
    pub fn code(self) -> &'static str {
        match self {
            Self::Russian => "ru",
            Self::English => "en",
            Self::Japanese => "ja",
        }
    }

    /// The language in its own words.
    pub fn name(self) -> &'static str {
        match self {
            Self::Russian => "Русский",
            Self::English => "English",
            Self::Japanese => "日本語",
        }
    }

    /// An unknown code gives Russian, in which everything is written.
    pub fn parse(code: &str) -> Self {
        ALL.into_iter()
            .find(|language| language.code() == code)
            .unwrap_or(Self::Russian)
    }
}

thread_local! {
    // Per thread, so that tests in different languages do not meet. A thread that
    // produces messages of its own is given the language of the one that starts it.
    static CURRENT: Cell<Lang> = const { Cell::new(Lang::Russian) };
}

pub fn set(language: Lang) {
    CURRENT.set(language);
}

pub fn current() -> Lang {
    CURRENT.get()
}

pub fn translate(russian: &'static str) -> &'static str {
    static TABLE: OnceLock<HashMap<&str, (&str, &str)>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        (TEXTS.iter())
            .map(|(russian, english, japanese)| (*russian, (*english, *japanese)))
            .collect()
    });
    match (current(), table.get(russian)) {
        (Lang::English, Some((english, _))) => english,
        (Lang::Japanese, Some((_, japanese))) => japanese,
        _ => russian,
    }
}

pub fn fill(russian: &'static str, arguments: &[&dyn Display]) -> String {
    let mut text = String::new();
    let mut arguments = arguments.iter();
    for (index, part) in translate(russian).split("{}").enumerate() {
        if index > 0
            && let Some(argument) = arguments.next()
        {
            let _ = write!(text, "{argument}");
        }
        text.push_str(part);
    }
    text
}

// Russian, English, Japanese. The arguments come in the same order in each.
const TEXTS: &[(&str, &str, &str)] = &[
    (
        "Файл библиотеки повреждён; он не был перезаписан.",
        "The library file is damaged; it was not overwritten.",
        "ライブラリのファイルが壊れています。上書きはしていません。",
    ),
    ("Цветовая схема", "Color scheme", "配色"),
    ("Прокси", "Proxy", "プロキシ"),
    ("Адрес прокси", "Proxy address", "プロキシのアドレス"),
    ("Кеш треков", "Track cache", "曲のキャッシュ"),
    ("Размер кеша", "Cache size", "キャッシュの上限"),
    ("Результатов поиска", "Search results", "検索結果の件数"),
    ("Шаг перемотки", "Seek step", "シークの間隔"),
    ("Громкость при запуске", "Start volume", "起動時の音量"),
    (
        "Enter - список схем с предпросмотром, Left/Right - соседняя.\nterminal повторяет цвета терминала, mono обходится без цвета,\nостальные - темы Ghostty.",
        "Enter - list of schemes with a preview, Left/Right - the next one.\nterminal follows the colors of the terminal, mono uses no color,\nthe rest are Ghostty themes.",
        "Enter - プレビュー付きの一覧、Left/Right - 隣の配色。\nterminal は端末の色に合わせ、mono は色を使いません。\nそのほかは Ghostty のテーマです。",
    ),
    (
        "Enter - включить или выключить.\nДействует со следующего поиска и трека.",
        "Enter - switch on or off.\nApplies from the next search and track.",
        "Enter - オンとオフを切り替えます。\n次の検索と曲から有効になります。",
    ),
    (
        "Enter - изменить: http, https, socks5 или socks5h с портом.\nTor: socks5h://127.0.0.1:9050. Пустая строка убирает адрес.",
        "Enter - edit: http, https, socks5 or socks5h with a port.\nTor: socks5h://127.0.0.1:9050. An empty line removes the address.",
        "Enter - 編集: http, https, socks5, socks5h とポート番号。\nTor: socks5h://127.0.0.1:9050  空にするとアドレスを消します。",
    ),
    (
        "Enter - включить или выключить сохранение треков на диск.\nУже сохранённое остаётся на месте.",
        "Enter - switch storing tracks on disk on or off.\nWhat is already stored stays where it is.",
        "Enter - 曲をディスクに保存するかを切り替えます。\n保存済みの曲はそのまま残ります。",
    ),
    (
        "Left/Right - по 256 МБ, ноль снимает ограничение.\nЛишнее удаляется, начиная с давно не игравших треков.",
        "Left/Right - by 256 MB, zero lifts the limit.\nThe excess is removed, starting with tracks not played longest.",
        "Left/Right - 256 MB ずつ、0 で上限なし。\n超えた分は長く再生していない曲から削除します。",
    ),
    (
        "Left/Right - по 5, от 5 до 50 треков на один поиск.",
        "Left/Right - by 5, from 5 to 50 tracks per search.",
        "Left/Right - 5 件ずつ、1 回の検索で 5 から 50 件。",
    ),
    (
        "Left/Right - 5, 10, 15, 30 или 60 секунд на одно нажатие.",
        "Left/Right - 5, 10, 15, 30 or 60 seconds per key press.",
        "Left/Right - 1 回の操作で 5, 10, 15, 30, 60 秒。",
    ),
    (
        "Left/Right - по 5. Применяется при следующем запуске.",
        "Left/Right - by 5. Applies at the next start.",
        "Left/Right - 5 ずつ。次回の起動から有効です。",
    ),
    (
        "Нажмите /, чтобы найти музыку. ? - все клавиши",
        "Press / to find music. ? - all keys",
        "/ で音楽を検索。? - キー一覧",
    ),
    ("Кеш отключён", "The cache is off", "キャッシュはオフです"),
    (
        "Удалено из кеша: {} - {}",
        "Removed from the cache: {} - {}",
        "キャッシュから削除: {} - {}",
    ),
    (
        "Этого трека нет в кеше",
        "This track is not in the cache",
        "この曲はキャッシュにありません",
    ),
    (
        "Не удалось удалить из кеша: {}",
        "Could not remove from the cache: {}",
        "キャッシュから削除できません: {}",
    ),
    (
        "Не удалось сохранить библиотеку: {}",
        "Could not save the library: {}",
        "ライブラリを保存できません: {}",
    ),
    (
        "Файл настроек не определён: изменение действует до выхода",
        "No settings file: the change lasts until exit",
        "設定ファイルがないため、変更は終了まで有効です",
    ),
    (
        "Не удалось сохранить настройки: {}",
        "Could not save the settings: {}",
        "設定を保存できません: {}",
    ),
    ("вкл", "on", "オン"),
    ("выкл", "off", "オフ"),
    (
        "выкл, действует прокси из окружения",
        "off, the proxy from the environment applies",
        "オフ (環境変数のプロキシを使用)",
    ),
    ("не задан", "not set", "未設定"),
    ("без ограничения", "no limit", "上限なし"),
    ("{} МБ", "{} MB", "{} MB"),
    ("{} с", "{} s", "{} 秒"),
    (
        "Сначала задайте адрес прокси строкой ниже",
        "Set the proxy address in the line below first",
        "先に下の行でプロキシのアドレスを設定してください",
    ),
    (
        "Каталог кеша не определён; задайте --cache-dir.",
        "The cache directory is unknown; set --cache-dir.",
        "キャッシュの場所が不明です。--cache-dir を指定してください。",
    ),
    (
        "Поиск уже выполняется...",
        "A search is already running...",
        "検索中です...",
    ),
    (
        "Введите исполнителя или название трека",
        "Enter an artist or a track title",
        "アーティスト名か曲名を入力してください",
    ),
    (
        "Ищем треки... Esc - отмена",
        "Searching... Esc - cancel",
        "検索中... Esc - 中止",
    ),
    ("Поиск отменён", "Search cancelled", "検索を中止しました"),
    ("Трек из кеша", "Track from the cache", "キャッシュの曲"),
    (
        "Загрузка трека; перемотка - в пределах загруженного",
        "Loading the track; seeking works within what has loaded",
        "読み込み中。シークは読み込み済みの範囲で使えます",
    ),
    (
        "Загрузка через прокси, перемотка ограничена буфером",
        "Loading through the proxy, seeking is limited to the buffer",
        "プロキシ経由で読み込み中。シークはバッファの範囲のみ",
    ),
    (
        "Подключение к аудиопотоку...",
        "Connecting to the audio stream...",
        "音声ストリームに接続中...",
    ),
    (
        "Очередь закончилась",
        "The queue has ended",
        "キューが終わりました",
    ),
    (
        "Трек не проигрался, включён следующий. e - подробности",
        "The track failed, the next one is on. e - details",
        "再生できず、次の曲に移りました。e - 詳細",
    ),
    (
        "Очередь остановлена после нескольких ошибок подряд. e - подробности",
        "The queue stopped after several failures in a row. e - details",
        "エラーが続いたためキューを停止しました。e - 詳細",
    ),
    (
        "Кеш отключён: треки не сохраняются",
        "The cache is off: tracks are not stored",
        "キャッシュがオフのため、曲は保存されません",
    ),
    (
        "Уже в кеше или загружается",
        "Already stored or being downloaded",
        "保存済みか、保存中です",
    ),
    (
        "В кеш сохраняются только отдельные треки",
        "Only single tracks are stored",
        "保存できるのは個別の曲だけです",
    ),
    (
        "Загрузка в кеш, осталось треков: {}",
        "Storing, tracks left: {}",
        "保存中、残り {} 曲",
    ),
    (
        "Не удалось начать загрузку: {}",
        "Could not start the download: {}",
        "ダウンロードを開始できません: {}",
    ),
    (
        "Сначала выберите трек и нажмите Enter",
        "Select a track and press Enter first",
        "先に曲を選んで Enter を押してください",
    ),
    (
        "Трек удалён из библиотеки",
        "Removed from the library",
        "ライブラリから削除しました",
    ),
    (
        "Трек добавлен в библиотеку",
        "Added to the library",
        "ライブラリに追加しました",
    ),
    (
        "Добавлено в конец очереди",
        "Added to the end of the queue",
        "キューの最後に追加しました",
    ),
    (
        "Воспроизведение остановлено",
        "Playback stopped",
        "再生を停止しました",
    ),
    (
        "Найдено треков: {}",
        "Tracks found: {}",
        "{} 曲見つかりました",
    ),
    (
        "Ошибка поиска. e - подробности, Esc - закрыть окно",
        "Search failed. e - details, Esc - close the window",
        "検索に失敗しました。e - 詳細、Esc - 閉じる",
    ),
    (
        "Поиск прерван",
        "Search interrupted",
        "検索が中断されました",
    ),
    ("В кеше: {}", "Stored: {}", "保存しました: {}"),
    (
        "{}\nyt-dlp не смог загрузить трек в кеш.",
        "{}\nyt-dlp could not download the track into the cache.",
        "{}\nyt-dlp が曲をキャッシュに保存できませんでした。",
    ),
    (
        "Трек не загрузился в кеш. e - подробности",
        "The track was not stored. e - details",
        "曲を保存できませんでした。e - 詳細",
    ),
    (
        "Воспроизведение из кеша. Space - пауза, n - следующий",
        "Playing from the cache. Space - pause, n - next",
        "キャッシュから再生中。Space - 一時停止、n - 次へ",
    ),
    (
        "Воспроизведение. Space - пауза, n - следующий",
        "Playing. Space - pause, n - next",
        "再生中。Space - 一時停止、n - 次へ",
    ),
    (
        "Интерфейсу нужен интерактивный терминал. Для скриптов используйте search или play --first.",
        "The interface needs an interactive terminal. In scripts use search or play --first.",
        "この画面には対話型の端末が必要です。スクリプトでは search か play --first を使ってください。",
    ),
    (
        "Не удалось определить путь библиотеки; укажите {} --library PATH",
        "The library path is unknown; pass {} --library PATH",
        "ライブラリの場所が不明です。{} --library PATH を指定してください",
    ),
    (
        "\n  CLICLOUD\n\n  Увеличьте терминал до 80x24.\n  q - выход",
        "\n  CLICLOUD\n\n  Enlarge the terminal to 80x24.\n  q - quit",
        "\n  CLICLOUD\n\n  端末を 80x24 以上にしてください。\n  q - 終了",
    ),
    (
        " / поиск  f избранное  a очередь  d в кеш  o настройки  ? помощь  q выход",
        " / search  f favorite  a queue  d store  o settings  ? help  q quit",
        " / 検索  f お気に入り  a キュー  d 保存  o 設定  ? ヘルプ  q 終了",
    ),
    (
        "Найти исполнителя, трек, новый звук...",
        "Find an artist, a track, a new sound...",
        "アーティスト、曲、新しい音を探す...",
    ),
    (
        " ПОИСК {} Esc - отмена ",
        " SEARCH {} Esc - cancel ",
        " 検索 {} Esc - 中止 ",
    ),
    (
        " ПРОГРАММЫ | Esc - продолжить без них ",
        " PROGRAMS | Esc - go on without them ",
        " プログラム | Esc - なしで続行 ",
    ),
    (
        " Нет программ, без которых клиент не работает:",
        " The programs the client cannot work without are missing:",
        " このクライアントに必要なプログラムがありません:",
    ),
    (
        " - поиск и загрузка аудио",
        " - search and audio download",
        " - 検索と音声のダウンロード",
    ),
    (" - воспроизведение", " - playback", " - 再生"),
    (
        "   Сумма SHA-256 из релиза сверяется до запуска файла.",
        "   The SHA-256 of the release is checked before the file is run.",
        "   リリースの SHA-256 を実行前に照合します。",
    ),
    (
        "   Enter - скачать",
        "   Enter - download",
        "   Enter - ダウンロード",
    ),
    (
        "   Системный пакет; поставьте сами:",
        "   A system package; install it yourself:",
        "   システムのパッケージです。ご自身で入れてください:",
    ),
    (
        "Непонятно, куда положить программу",
        "There is nowhere to put the program",
        "プログラムを置く場所が不明です",
    ),
    (
        "Скачиваю и проверяю...",
        "Downloading and checking...",
        "ダウンロードして確認中...",
    ),
    (
        "yt-dlp на месте: {}",
        "yt-dlp is in place: {}",
        "yt-dlp を配置しました: {}",
    ),
    ("Не вышло: {}", "It did not work: {}", "失敗しました: {}"),
    (
        "Загрузка прервалась",
        "The download broke off",
        "ダウンロードが中断しました",
    ),
    (
        "В релизе yt-dlp нет суммы для {}",
        "The yt-dlp release holds no sum for {}",
        "yt-dlp のリリースに {} のサムがありません",
    ),
    (
        "Сумма не совпала: {} вместо {}",
        "The sum does not match: {} instead of {}",
        "サムが一致しません: {} ではなく {}",
    ),
    (
        "Не удалось запустить curl: {}",
        "Could not start curl: {}",
        "curl を起動できませんでした: {}",
    ),
    (
        "curl завершился с {}: {}",
        "curl exited with {}: {}",
        "curl が {} で終了しました: {}",
    ),
    (
        " / ПОИСК   Enter - найти ",
        " / SEARCH   Enter - find ",
        " / 検索   Enter - 実行 ",
    ),
    ("Найти", "Find", "検索"),
    (" ОБЗОР ", " BROWSE ", " 一覧 "),
    ("1  Поиск", "1  Search", "1  検索"),
    ("2  Библиотека", "2  Library", "2  ライブラリ"),
    ("3  Очередь", "3  Queue", "3  キュー"),
    ("4  Недавние", "4  Recent", "4  履歴"),
    ("{} избранных", "favorites: {}", "お気に入り {}"),
    ("{} в очереди", "in queue: {}", "キュー {}"),
    ("{} качается", "storing: {}", "保存中 {}"),
    ("v - в кеше,", "v - stored,", "v - 保存済み"),
    ("играет и", "plays", "オフラインで"),
    ("без сети", "offline", "再生可能"),
    (" РЕЗУЛЬТАТЫ ", " RESULTS ", " 検索結果 "),
    (" МОЯ БИБЛИОТЕКА ", " MY LIBRARY ", " ライブラリ "),
    (
        " МОЯ БИБЛИОТЕКА | избранное: {} | в кеше: {}, {} ",
        " MY LIBRARY | favorites: {} | stored: {}, {} ",
        " ライブラリ | お気に入り: {} | 保存済み: {}, {} ",
    ),
    (" ОЧЕРЕДЬ ВОСПРОИЗВЕДЕНИЯ ", " PLAY QUEUE ", " 再生キュー "),
    (" НЕДАВНИЕ ", " RECENT ", " 再生履歴 "),
    (
        "\n\nВаша следующая любимая песня - здесь.\n\nНажмите / и введите поисковый запрос.\nEnter - слушать, f - сохранить",
        "\n\nYour next favorite song is here.\n\nPress / and type a search query.\nEnter - listen, f - keep",
        "\n\n次のお気に入りの一曲はここに。\n\n/ を押して検索語を入力してください。\nEnter - 再生、f - お気に入り",
    ),
    (
        "\n\nСоберите свою коллекцию.\n\nf - добавить трек в избранное.\nЗдесь же всё, что сохранено в кеш (d)\nи играет без сети.",
        "\n\nBuild your collection.\n\nf - add a track to the favorites.\nEverything stored in the cache (d)\nis here too and plays offline.",
        "\n\nコレクションを作りましょう。\n\nf - 曲をお気に入りに追加。\nキャッシュに保存した曲 (d) もここに並び、\nオフラインで再生できます。",
    ),
    (
        "\n\nМузыка без перерывов.\n\nНажмите a, чтобы добавить трек в очередь.\nСледующий трек запустится автоматически.",
        "\n\nMusic without breaks.\n\nPress a to add a track to the queue.\nThe next track starts by itself.",
        "\n\n途切れない音楽を。\n\na で曲をキューに追加します。\n次の曲は自動で始まります。",
    ),
    (
        "\n\nИстория прослушивания.\n\nЗдесь появятся последние включённые треки.\nEnter - включить снова.",
        "\n\nListening history.\n\nThe last tracks you played appear here.\nEnter - play again.",
        "\n\n再生履歴。\n\n最近再生した曲がここに表示されます。\nEnter - もう一度再生。",
    ),
    ("ТРЕК", "TRACK", "曲名"),
    ("ИСПОЛНИТЕЛЬ", "ARTIST", "アーティスト"),
    ("ВРЕМЯ", "TIME", "時間"),
    ("|> Играть", "|> Play", "|> 再生"),
    ("* Избранное", "* Favorite", "* お気に入り"),
    ("+ В очередь", "+ Queue", "+ キュー"),
    ("x Из кеша", "x Remove", "x 削除"),
    ("v В кеш", "v Store", "v 保存"),
    (
        " НАСТРОЙКИ | Esc - закрыть ",
        " SETTINGS | Esc - close ",
        " 設定 | Esc - 閉じる ",
    ),
    ("Файл: {}", "File: {}", "ファイル: {}"),
    (
        "Файл настроек не определён: изменения действуют до выхода",
        "No settings file: changes last until exit",
        "設定ファイルがないため、変更は終了まで有効です",
    ),
    (
        "Up/Down - выбор   Left/Right - изменить   Enter - переключить",
        "Up/Down - select   Left/Right - change   Enter - switch",
        "Up/Down - 選択   Left/Right - 変更   Enter - 切り替え",
    ),
    (
        " СХЕМА | Enter - выбрать | Esc ",
        " SCHEME | Enter - choose | Esc ",
        " 配色 | Enter - 決定 | Esc ",
    ),
    ("ДАЛЕЕ", "NEXT", "次の曲"),
    ("Очередь пока пуста", "The queue is empty", "キューは空です"),
    ("НЕДАВНО", "RECENT", "最近の曲"),
    (" НА СЛУХУ ", " ON AIR ", " 再生リスト "),
    (" ПЛЕЕР ", " PLAYER ", " プレーヤー "),
    ("ЗАГРУЗКА", "LOADING", "読込中"),
    ("ПАУЗА", "PAUSED", "一時停止"),
    ("ИГРАЕТ", "PLAYING", "再生中"),
    ("СТОП", "STOPPED", "停止"),
    (
        "Выберите трек, чтобы начать",
        "Select a track to start",
        "曲を選んで再生を始めましょう",
    ),
    ("|| Пауза", "|| Pause", "|| 一時停止"),
    ("s Стоп", "s Stop", "s 停止"),
    (
        "Поиск (Enter отправляет, Esc отменяет)",
        "Search (Enter submits, Esc cancels)",
        "検索 (Enter で実行、Esc で中止)",
    ),
    (
        "Поиск, библиотека, очередь, недавние / трек",
        "Search, library, queue, recent / the track",
        "検索、ライブラリ、キュー、履歴 / 曲",
    ),
    (
        "Выбрать трек     Enter  Проиграть",
        "Select a track   Enter  Play",
        "曲を選択         Enter  再生",
    ),
    (
        "Выбрать кнопку   Enter  Нажать",
        "Select a button  Enter  Press",
        "ボタンを選択     Enter  押す",
    ),
    (
        "Добавить / удалить из избранного",
        "Add to / remove from the favorites",
        "お気に入りに追加 / 削除",
    ),
    (
        "Добавить в очередь   Del  Убрать из очереди",
        "Add to the queue     Del  Remove from it",
        "キューに追加         Del  キューから削除",
    ),
    (
        "Загрузить в кеш трек / весь список",
        "Store the track / the whole list",
        "曲 / 一覧すべてをキャッシュに保存",
    ),
    (
        "Удалить трек из кеша",
        "Remove the track from the cache",
        "曲をキャッシュから削除",
    ),
    ("Пауза / продолжить", "Pause / resume", "一時停止 / 再開"),
    (
        "Предыдущий / следующий трек",
        "Previous / next track",
        "前の曲 / 次の曲",
    ),
    (
        "Перемотка        , .  Медленнее / быстрее",
        "Seek             , .  Slower / faster",
        "シーク           , .  遅く / 速く",
    ),
    (
        "Громкость    s  Стоп    q  Выход",
        "Volume       s  Stop    q  Quit",
        "音量         s  停止    q  終了",
    ),
    (
        "Настройки и цветовые схемы",
        "Settings and color schemes",
        "設定と配色",
    ),
    (
        "Подробности последней ошибки",
        "Details of the last error",
        "直前のエラーの詳細",
    ),
    (
        " Мышь работает. Любая клавиша закрывает справку.",
        " The mouse works. Any key closes the help.",
        " マウスも使えます。どのキーでも閉じます。",
    ),
    (" УПРАВЛЕНИЕ ", " KEYS ", " 操作 "),
    (
        " ПОДРОБНОСТИ | Up/Down прокрутка | Esc закрыть ",
        " DETAILS | Up/Down scroll | Esc close ",
        " 詳細 | Up/Down スクロール | Esc 閉じる ",
    ),
    ("Ошибка: {}", "Error: {}", "エラー: {}"),
    ("Кеш: отключён", "Cache: off", "キャッシュ: オフ"),
    ("из {}", "of {}", "/ {}"),
    (
        "Кеш: {}\nТреков в кеше: {}, {} {}",
        "Cache: {}\nTracks in the cache: {}, {} {}",
        "キャッシュ: {}\n保存済みの曲: {}, {} {}",
    ),
    (
        "Для выбора нужен терминал. Используйте --first или ссылку SoundCloud.",
        "Choosing needs a terminal. Use --first or a SoundCloud link.",
        "選択には端末が必要です。--first か SoundCloud のリンクを使ってください。",
    ),
    (
        "Треки не найдены. Попробуйте другой запрос.",
        "No tracks found. Try another query.",
        "曲が見つかりません。別の検索語を試してください。",
    ),
    (
        "Сейчас играет: {} — {}",
        "Now playing: {} — {}",
        "再生中: {} — {}",
    ),
    (
        "Пробел: пауза; ←/→: перемотка; 9/0: громкость; q: выход.",
        "Space: pause; ←/→: seek; 9/0: volume; q: quit.",
        "Space: 一時停止; ←/→: シーク; 9/0: 音量; q: 終了",
    ),
    ("Прокси: {}", "Proxy: {}", "プロキシ: {}"),
    (
        "включён (аудио через yt-dlp)",
        "on (audio through yt-dlp)",
        "オン (音声は yt-dlp 経由)",
    ),
    ("принудительно отключён", "forced off", "強制的にオフ"),
    (
        "не задан в clicloud",
        "not set in clicloud",
        "clicloud では未設定",
    ),
    (
        "Установите отсутствующие программы; инструкции в README.md.",
        "Install the missing programs; see README.md.",
        "足りないプログラムをインストールしてください。手順は README.md にあります。",
    ),
    (
        "Не удалось запустить {}: {}",
        "Could not run {}: {}",
        "{} を実行できません: {}",
    ),
    (
        "{} --version завершился с {}",
        "{} --version ended with {}",
        "{} --version が {} で終了しました",
    ),
    (
        "Ничего не найдено.",
        "Nothing found.",
        "見つかりませんでした。",
    ),
    (
        "Номер трека (1–{}, q — выход): ",
        "Track number (1–{}, q — quit): ",
        "曲の番号 (1–{}, q — 終了): ",
    ),
    (
        "Введите число от 1 до {} или q.",
        "Enter a number from 1 to {} or q.",
        "1 から {} の数字か q を入力してください。",
    ),
    (
        "Не удалось запустить yt-dlp: {}",
        "Could not run yt-dlp: {}",
        "yt-dlp を実行できません: {}",
    ),
    (
        "Воспроизведение прервано сигналом {}.",
        "Playback was interrupted by signal {}.",
        "シグナル {} で再生が中断されました。",
    ),
    (
        "Не удалось запустить mpv: {}",
        "Could not run mpv: {}",
        "mpv を実行できません: {}",
    ),
    (
        "Трек из кеша.",
        "Track from the cache.",
        "キャッシュの曲です。",
    ),
    (
        "mpv завершился с {}.",
        "mpv ended with {}.",
        "mpv が {} で終了しました。",
    ),
    (
        "Трек загружается в кеш; перемотка - в пределах загруженного.",
        "The track is being stored; seeking works within what has loaded.",
        "曲を保存しています。シークは読み込み済みの範囲で使えます。",
    ),
    (
        "mpv завершился с {}. Трек может быть недоступен; подробности выше.",
        "mpv ended with {}. The track may be unavailable; see the details above.",
        "mpv が {} で終了しました。曲が利用できない可能性があります。詳細は上を見てください。",
    ),
    (
        "Прокси: аудио через yt-dlp; перемотка ограничена, воспроизводится один трек.",
        "Proxy: audio through yt-dlp; seeking is limited, a single track is played.",
        "プロキシ: 音声は yt-dlp 経由。シークは制限され、再生は 1 曲だけです。",
    ),
    (
        "yt-dlp завершился с {}; проверьте сеть, прокси и доступность трека.",
        "yt-dlp ended with {}; check the network, the proxy and whether the track is available.",
        "yt-dlp が {} で終了しました。ネットワーク、プロキシ、曲の公開状態を確認してください。",
    ),
    (
        "mpv: {}. Установите mpv и проверьте clicloud doctor.",
        "mpv: {}. Install mpv and check clicloud doctor.",
        "mpv: {}  mpv をインストールし、clicloud doctor で確認してください。",
    ),
    (
        "Плеер ещё подключается",
        "The player is still connecting",
        "プレーヤーに接続中です",
    ),
    (
        "Связь с плеером прервана",
        "The connection to the player is broken",
        "プレーヤーとの接続が切れました",
    ),
    (
        "mpv не открыл IPC-соединение за 5 секунд",
        "mpv did not open its IPC connection within 5 seconds",
        "mpv が 5 秒以内に IPC 接続を開きませんでした",
    ),
    (
        "yt-dlp не смог загрузить аудио. Проверьте сеть, прокси и доступность трека.",
        "yt-dlp could not download the audio. Check the network, the proxy and whether the track is available.",
        "yt-dlp が音声を取得できませんでした。ネットワーク、プロキシ、曲の公開状態を確認してください。",
    ),
    (
        "Ошибка mpv: поток недоступен или отсутствует аудиоустройство.",
        "mpv failed: the stream is unavailable or there is no audio device.",
        "mpv のエラー: ストリームを取得できないか、音声デバイスがありません。",
    ),
    (
        "Плеер завершился до окончания трека.",
        "The player quit before the track ended.",
        "曲の途中でプレーヤーが終了しました。",
    ),
    (
        "Не удалось проиграть трек. Проверьте сеть, прокси или выберите другой.",
        "Could not play the track. Check the network and the proxy, or pick another one.",
        "曲を再生できません。ネットワークとプロキシを確認するか、別の曲を選んでください。",
    ),
    (
        "Поисковый запрос не должен быть пустым.",
        "The search query must not be empty.",
        "検索語を入力してください。",
    ),
    (
        "Поиск в SoundCloud…",
        "Searching SoundCloud…",
        "SoundCloud を検索中…",
    ),
    (
        "Не удалось запустить yt-dlp ({}): {}. Проверьте clicloud doctor.",
        "Could not run yt-dlp ({}): {}. Check clicloud doctor.",
        "yt-dlp ({}) を実行できません: {}  clicloud doctor で確認してください。",
    ),
    (
        "yt-dlp завершился с {}:\n{}",
        "yt-dlp ended with {}:\n{}",
        "yt-dlp が {} で終了しました:\n{}",
    ),
    (
        "Некорректный ответ JSON от yt-dlp: {}",
        "Invalid JSON response from yt-dlp: {}",
        "yt-dlp の JSON 応答が不正です: {}",
    ),
    ("Без названия", "Untitled", "無題"),
    (
        "Неизвестный исполнитель",
        "Unknown artist",
        "不明なアーティスト",
    ),
    (
        "Ожидается HTTP(S)-ссылка SoundCloud.",
        "An HTTP(S) link to SoundCloud is expected.",
        "SoundCloud の HTTP(S) リンクを指定してください。",
    ),
    (
        "Не удалось прочитать настройки {}: {}",
        "Could not read the settings {}: {}",
        "設定 {} を読み込めません: {}",
    ),
    (
        "Некорректные настройки {}: {}",
        "Invalid settings {}: {}",
        "設定 {} が正しくありません: {}",
    ),
    (
        "Некорректный URL прокси.",
        "Invalid proxy URL.",
        "プロキシの URL が正しくありません。",
    ),
    (
        "Прокси: используйте http(s)://host:port или socks5(h)://host:port.",
        "Proxy: use http(s)://host:port or socks5(h)://host:port.",
        "プロキシ: http(s)://host:port か socks5(h)://host:port を使ってください。",
    ),
    (
        "Каталог {} не пуст и не является кешем clicloud. Укажите другой --cache-dir или --no-cache.",
        "The directory {} is not empty and is not a clicloud cache. Pass another --cache-dir or --no-cache.",
        "{} は空ではなく、clicloud のキャッシュでもありません。別の --cache-dir か --no-cache を指定してください。",
    ),
    (
        "Не удалось подготовить кеш {}: {}. Укажите --cache-dir или --no-cache.",
        "Could not prepare the cache {}: {}. Pass --cache-dir or --no-cache.",
        "キャッシュ {} を準備できません: {}  --cache-dir か --no-cache を指定してください。",
    ),
    ("Язык", "Language", "言語"),
    ("Фоновый рисунок", "Backdrop", "背景の絵"),
    ("нет", "none", "なし"),
    ("жнец", "reaper", "死神"),
    ("пентаграмма", "pentagram", "五芒星"),
    (
        "Enter или Left/Right - сменить рисунок за списком треков.\nнет - список без рисунка.",
        "Enter or Left/Right - the drawing behind the list of tracks.\nnone - a list without a drawing.",
        "Enter か Left/Right - 曲の一覧の後ろの絵を切り替えます。\nなし - 絵のない一覧。",
    ),
    (
        "Enter или Left/Right - сменить язык интерфейса и сообщений.\nСправка командной строки (--help) остаётся на русском.",
        "Enter or Left/Right - the language of the interface and messages.\nThe command line help (--help) stays in Russian.",
        "Enter か Left/Right - 画面とメッセージの言語を切り替えます。\nコマンドラインのヘルプ (--help) はロシア語のままです。",
    ),
    (
        "Читаю лайки профиля {}…",
        "Reading the likes of {}…",
        "{} のいいねを読み込み中…",
    ),
    (
        "Ожидается имя профиля SoundCloud или ссылка на него: name или soundcloud.com/name.",
        "A SoundCloud profile name or a link to it is expected: name or soundcloud.com/name.",
        "SoundCloud のプロフィール名かそのリンクを指定してください: name か soundcloud.com/name",
    ),
    (
        "{} - раздел сайта, а не профиль. Имя профиля стоит в адресе его страницы: soundcloud.com/name.",
        "{} is a section of the site, not a profile. The name of a profile is in the address of its page: soundcloud.com/name.",
        "{} はサイトのセクションで、プロフィールではありません。プロフィール名はそのページのアドレスにあります: soundcloud.com/name",
    ),
    ("Получено: {}", "Received: {}", "取得済み: {}"),
    (
        "В списке нет треков.",
        "The list has no tracks.",
        "この一覧に曲はありません。",
    ),
    (
        "Было бы добавлено: {}, уже в избранном: {}, пропущено: {}",
        "Would be added: {}, already among the favorites: {}, skipped: {}",
        "追加予定: {}、お気に入りに登録済み: {}、スキップ: {}",
    ),
    (
        "Добавлено в избранное: {}, уже было: {}, пропущено: {}",
        "Added to the favorites: {}, already there: {}, skipped: {}",
        "お気に入りに追加: {}、登録済み: {}、スキップ: {}",
    ),
    ("Библиотека: {}", "Library: {}", "ライブラリ: {}"),
    (
        "Лайки SoundCloud",
        "SoundCloud likes",
        "SoundCloud のいいね",
    ),
    (
        "Enter - имя профиля или ссылка, ещё раз Enter - добавить его\nлайки в избранное. Лайки должны быть видны в профиле.\nПока список читается, Enter прерывает его.",
        "Enter - a profile name or a link, Enter again - add its likes\nto the favorites. The likes must be visible in the profile.\nWhile the list is being read, Enter stops it.",
        "Enter - プロフィール名かリンク、もう一度 Enter でいいねを\nお気に入りに追加。いいねは公開されている必要があります。\n読み込み中に Enter を押すと中断します。",
    ),
    ("{}: получено {}", "{}: received {}", "{}: {} 件取得"),
    ("{}: прервано", "{}: stopped", "{}: 中断しました"),
    (
        "{}: добавлено {}, уже было {}",
        "{}: added {}, already there {}",
        "{}: 追加 {}、登録済み {}",
    ),
    (
        "{}: оборвалось, добавлено {}",
        "{}: broke off, added {}",
        "{}: 途中で失敗、追加 {}",
    ),
    (
        "Лайки {}: добавлено {}, уже было {}, пропущено {}",
        "Likes of {}: added {}, already there {}, skipped {}",
        "{} のいいね: 追加 {}、登録済み {}、スキップ {}",
    ),
    (
        "Список лайков оборвался. e - подробности, Esc - закрыть окно",
        "The list of likes broke off. e - details, Esc - close the window",
        "いいねの一覧が途中で失敗しました。e - 詳細、Esc - 閉じる",
    ),
    (
        "Чтение лайков прервано",
        "Reading the likes was interrupted",
        "いいねの読み込みが中断されました",
    ),
    ("вперемешку", "shuffled", "シャッフル"),
    ("по порядку", "in order", "順番どおり"),
    ("без повтора", "no repeat", "リピートなし"),
    ("повтор списка", "repeat the list", "リストをリピート"),
    ("повтор трека", "repeat the track", "1曲リピート"),
    ("Порядок: {}", "Order: {}", "再生順: {}"),
    ("Повтор: {}", "Repeat: {}", "リピート: {}"),
    (
        " ФИЛЬТР БИБЛИОТЕКИ   Enter - оставить, Esc - сбросить ",
        " LIBRARY FILTER   Enter - keep, Esc - clear ",
        " ライブラリの絞り込み   Enter - 確定、Esc - 解除 ",
    ),
    (
        "\n\nПо этому фильтру ничего нет.\n\nEsc - сбросить фильтр.",
        "\n\nNothing matches this filter.\n\nEsc - clear the filter.",
        "\n\nこの条件に合う曲はありません。\n\nEsc - 絞り込みを解除",
    ),
    (
        "Листать список   Home End  К краям списка",
        "Page the list    Home End  To its ends",
        "ページ送り       Home End  先頭 / 末尾",
    ),
    (
        "Фильтр библиотеки (Esc сбрасывает)",
        "Filter the library (Esc clears)",
        "ライブラリを絞り込む (Esc で解除)",
    ),
    (
        "Вперемешку / повтор: нет, список, трек",
        "Shuffle / repeat: off, list, track",
        "シャッフル / リピート: なし、リスト、1曲",
    ),
    (
        "Читаю список {}…",
        "Reading the list {}…",
        "一覧 {} を読み込み中…",
    ),
    (
        "Читаю список... Esc - отмена",
        "Reading the list... Esc - cancel",
        "一覧を読み込み中... Esc - 中止",
    ),
    (
        "Раньше ничего не открывалось",
        "Nothing was open before this",
        "これより前の一覧はありません",
    ),
    (
        "У этой ссылки нет страницы автора",
        "This link has no page of an author",
        "このリンクに作者のページはありません",
    ),
    (
        "Станция есть только у отдельного трека",
        "Only a single track has a station",
        "ステーションは曲にしかありません",
    ),
    (
        "В избранное добавлено: {}, уже было: {}",
        "Added to the favorites: {}, already there: {}",
        "お気に入りに追加: {}、登録済み: {}",
    ),
    (
        " АВТОР {} | {} | [ ] - раздел ",
        " AUTHOR {} | {} | [ ] - section ",
        " 作者 {} | {} | [ ] - セクション ",
    ),
    ("треки", "tracks", "曲"),
    ("альбомы", "albums", "アルバム"),
    ("плейлисты", "playlists", "プレイリスト"),
    ("репосты", "reposts", "リポスト"),
    ("лайки", "likes", "いいね"),
    (
        "Похожие треки / автор   [ ]  Его разделы",
        "Similar tracks / author  [ ]  Sections",
        "似た曲 / 作者            [ ]  セクション",
    ),
    (
        "Весь список в избранное / назад к списку",
        "All of the list to favorites / list before",
        "一覧をすべてお気に入りへ / 前の一覧へ",
    ),
    ("Ровная громкость", "Even loudness", "音量をそろえる"),
    ("Аудиоустройство", "Audio device", "オーディオ機器"),
    (
        "Enter - включить или выключить: тихие и громкие треки\nприводятся к одной громкости. Действует со следующего трека.",
        "Enter - switch on or off: quiet and loud tracks are brought\nto one loudness. Applies from the next track.",
        "Enter - オンとオフ: 小さい曲と大きい曲の音量を\nそろえます。次の曲から有効になります。",
    ),
    (
        "Enter или Left/Right - следующее из устройств, что видит mpv.\nавто оставляет выбор за ним. Действует сразу.",
        "Enter or Left/Right - the next of the devices that mpv sees.\nauto leaves the choice to it. Applies at once.",
        "Enter か Left/Right - mpv が見つけた次の機器に切り替えます。\n自動は mpv に任せます。すぐに有効になります。",
    ),
    ("авто", "auto", "自動"),
    (
        "mpv не назвал ни одного устройства",
        "mpv named no device",
        "mpv から機器の一覧を取得できません",
    ),
    ("Скорость: x{}", "Speed: x{}", "速度: x{}"),
    ("Обложка", "Cover", "ジャケット"),
    (
        "Enter или Left/Right - как рисовать обложку на вкладке трека (t):\nблоками или точками Брайля; в её цветах, в цветах или в тонах схемы.",
        "Enter or Left/Right - how the tab of the track (t) draws the cover:\nblocks or Braille dots; its own colors, the scheme's colors, tones.",
        "Enter か Left/Right - 曲のタブ (t) でのジャケットの描き方:\nブロックか点字; 元の色、配色の色、配色の濃淡のいずれか。",
    ),
    ("цветные блоки", "colored blocks", "カラーのブロック"),
    ("цветной брайль", "colored Braille", "カラーの点字"),
    ("блоки в тонах", "blocks in tones", "濃淡のブロック"),
    (
        "блоки в цветах схемы",
        "blocks in scheme colors",
        "配色のブロック",
    ),
    (
        "брайль в цветах схемы",
        "Braille in scheme colors",
        "配色の点字",
    ),
    ("брайль в тонах", "Braille in tones", "濃淡の点字"),
    (" ТРЕК ", " TRACK ", " 曲 "),
    ("Списки", "Lists", "一覧"),
    ("Трек", "Track", "曲"),
    ("Жанр", "Genre", "ジャンル"),
    ("Дата", "Date", "日付"),
    ("Слушали", "Plays", "再生"),
    ("Лайки", "Likes", "いいね"),
    ("Репосты", "Reposts", "リポスト"),
    ("Комментарии", "Comments", "コメント"),
    ("Теги", "Tags", "タグ"),
    (
        "сведений о треке нет",
        "nothing is known of the track",
        "この曲の情報はありません",
    ),
    (
        "ищем сведения о треке...",
        "looking the track up...",
        "曲の情報を調べています...",
    ),
    (
        "\n\nНичего не играет.\n\nEnter на треке в списке включает его.",
        "\n\nNothing is playing.\n\nEnter on a track in a list starts it.",
        "\n\n何も再生していません。\n\n一覧の曲で Enter を押すと再生します。",
    ),
    ("ждём обложку", "cover is coming", "読み込み中..."),
    ("обложки нет", "no cover", "ジャケットなし"),
    (
        "С прошлого раза в очереди треков: {}. n - включить",
        "Tracks in the queue from the last time: {}. n - play",
        "前回のキューに {} 曲あります。n - 再生",
    ),
    (
        "Enter - узнать версию; если yt-dlp скачан клиентом, взять свежий.\nПоставленный иначе обновляется тем же способом, что ставился.",
        "Enter - ask its version; one the client fetched is fetched anew.\nOne installed another way is updated the way it was installed.",
        "Enter - バージョンを確認; クライアントが取得したものは更新します。\nほかの方法で入れたものは、その方法で更新してください。",
    ),
    (
        "{}, поставлен не клиентом",
        "{}, not installed by the client",
        "{}、クライアント以外が導入",
    ),
    ("обновлён: {}", "updated: {}", "更新しました: {}"),
    ("Медиаклавиши", "Media keys", "メディアキー"),
    (
        "Enter - включить или выключить: пауза, стоп и соседние треки с\nклавиатуры и из панели, через плагин mpv-mpris. Со следующего трека.",
        "Enter - switch on or off: pause, stop and the tracks around from\nthe keyboard and the panel, by the plugin mpv-mpris. From the next.",
        "Enter - オンとオフ: キーボードやパネルから一時停止、停止、\n前後の曲。mpv-mpris プラグインを使います。次の曲から有効。",
    ),
    ("вкл: {}", "on: {}", "オン: {}"),
    (
        "вкл, но плагин mpv-mpris не найден",
        "on, but the plugin mpv-mpris is not found",
        "オン (mpv-mpris プラグインが見つかりません)",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    // The texts inside `t!(...)` in a source file.
    fn texts(source: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut rest = source;
        while let Some(at) = rest.find("t!(") {
            let named = rest[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_');
            rest = &rest[at + 3..];
            let Some(literal) = rest.trim_start().strip_prefix('"').filter(|_| !named) else {
                continue;
            };
            let mut text = String::new();
            let mut characters = literal.chars();
            while let Some(character) = characters.next() {
                match character {
                    '"' => break,
                    '\\' => match characters.next() {
                        Some('n') => text.push('\n'),
                        // A line of the source that goes on in the next one.
                        Some('\n') => {
                            let after = characters.as_str().trim_start();
                            characters = after.chars();
                        }
                        Some(other) => text.push(other),
                        None => (),
                    },
                    other => text.push(other),
                }
            }
            found.push(text);
        }
        found
    }

    #[test]
    fn every_text_of_the_sources_is_translated() {
        let sources = [
            include_str!("cache.rs"),
            include_str!("config.rs"),
            include_str!("cover.rs"),
            include_str!("library.rs"),
            include_str!("main.rs"),
            include_str!("playback.rs"),
            include_str!("player.rs"),
            include_str!("setup.rs"),
            include_str!("soundcloud.rs"),
            include_str!("ui.rs"),
        ];
        let used: Vec<String> = sources.iter().flat_map(|source| texts(source)).collect();
        assert!(used.len() > 150, "{}", used.len());
        let missing: Vec<&String> = used
            .iter()
            .filter(|text| !TEXTS.iter().any(|(russian, _, _)| russian == text))
            .collect();
        assert!(missing.is_empty(), "{missing:#?}");
        // Nothing stays in the table after its text has left the sources.
        let unused: Vec<&str> = (TEXTS.iter())
            .map(|(russian, _, _)| *russian)
            .filter(|russian| !used.iter().any(|text| text == russian))
            .collect();
        assert!(unused.is_empty(), "{unused:#?}");
    }

    #[test]
    fn translations_keep_the_shape_of_the_text() {
        for (index, (russian, english, japanese)) in TEXTS.iter().enumerate() {
            assert!(
                !TEXTS[..index].iter().any(|(other, _, _)| other == russian),
                "twice: {russian}"
            );
            for translation in [english, japanese] {
                assert!(!translation.is_empty(), "{russian}");
                // The same arguments, lines and edges: they shape the screen.
                for mark in ["{}", "\n"] {
                    assert_eq!(
                        russian.matches(mark).count(),
                        translation.matches(mark).count(),
                        "{russian}"
                    );
                }
                assert_eq!(
                    russian.starts_with(' '),
                    translation.starts_with(' '),
                    "{russian}"
                );
                assert_eq!(
                    russian.ends_with(' '),
                    translation.ends_with(' '),
                    "{russian}"
                );
            }
        }
    }

    #[test]
    fn language_is_switched_and_arguments_are_filled_in() {
        assert_eq!(current(), Lang::Russian);
        assert_eq!(t!("Найдено треков: {}", 3), "Найдено треков: 3");
        set(Lang::English);
        assert_eq!(t!("Поиск отменён"), "Search cancelled");
        assert_eq!(t!("Найдено треков: {}", 3), "Tracks found: 3");
        assert_eq!(
            t!("Удалено из кеша: {} - {}", "Artist", "Title"),
            "Removed from the cache: Artist - Title"
        );
        set(Lang::Japanese);
        assert_eq!(t!("Найдено треков: {}", 3), "3 曲見つかりました");
        assert_eq!(translate("нет такого текста"), "нет такого текста");
        assert_eq!(Lang::parse("ja"), Lang::Japanese);
        assert_eq!(Lang::parse("xx"), Lang::Russian);
        for language in ALL {
            assert_eq!(Lang::parse(language.code()), language);
        }
    }
}
