# maxfetch

Скачивает трек в максимально возможном качестве из нескольких источников в одну
библиотеку. Каждый найденный кандидат проходит ранжирование по качеству, файл
проверяется пробой после загрузки, дубли помечаются по ISRC и длительности, теги
проставляются автоматически. Интерфейсы: подкоманды CLI для скриптов и TUI-шелл
для ручной работы.

## Установка

```sh
cargo install --path .
```

Нужен toolchain с поддержкой Rust edition 2024. Дополнительно:

- `yt-dlp` в `PATH` (или путь в `youtube.yt_dlp_path`) для источников YouTube.
- `ffmpeg` в `PATH` (или путь в `media.ffmpeg_path`) для переноса потока из
  `.webm` в `.opus` без пережатия: используется `-c copy`, транскодинга нет.

## Конфигурация

Все пути уважают XDG-переменные.

- `$XDG_CONFIG_HOME/maxfetch/config.toml` - настройки, не секреты. Ключи:
  `library.path`, `sources.enabled`, `matching.duration_tolerance_sec`,
  `matching.require_isrc`, `download.max_concurrency`, `download.retry_attempts`,
  `download.timeout_sec`, `download.stall_timeout_sec`, `youtube.yt_dlp_path`,
  `media.ffmpeg_path`.
- `$XDG_CONFIG_HOME/maxfetch/credentials.toml` - секреты. Файл создаётся с
  режимом `0600`; если существующий файл доступен более широко, права сужаются
  при старте с предупреждением. Секции: `soulseek`, `bandcamp`, `spotify`,
  `soundcloud`, `qobuz`, `tidal`, `deezer`, `beatport`. YouTube учётной записи
  не требует.
- `$XDG_DATA_HOME/maxfetch/state.db` - база SQLite в режиме WAL: треки, релизы,
  журнал загрузок, пометки дублей.

## Раскладка библиотеки

```
<library>/<Artist>/<Album>/<NN> <Title>.<ext>
```

`<Album>` по умолчанию `Singles`, если релиз не назвал альбом. `NN` - двузначный
номер трека. Файлы никогда не переименовываются и не удаляются неявно: удаление
дублей требует явных флагов и подтверждения.

## Команды CLI

```
maxfetch search <query>
maxfetch fetch [<query>] [--playlist URL] [--spotify-track URL]
               [--spotify-playlist URL] [--page URL] [--soundcloud URL]
               [--input FILE.csv]
maxfetch library scan
maxfetch library dedupe [--apply --delete] [--yes]
maxfetch library list
maxfetch auth login <source>
maxfetch auth status
maxfetch sources check
maxfetch config set <key> <value>
```

Глобальные флаги: `--library`, `--source`, `--dry-run`, `--json`,
`--ndjson-progress`, `--no-tui`.

Текущий этап: ядро готово, реестр источников в этой сборке пуст. Поэтому
`search` и `fetch` возвращают пустой результат, `auth login` - заглушка с
сообщением о будущей авторизации, а `--no-tui` принимается для совместимости со
скриптами и игнорируется. TUI-шелл реализован в библиотеке (`maxfetch::tui`), но
в этой сборке ещё не подключён к точке входа бинаря.

## Клавиши TUI

Раздел описывает реализованный шелл (`maxfetch::tui`).

- `Tab` / `Shift+Tab` - следующий и предыдущий экран.
- `/` - правка запроса, `Enter` - поиск или раскрытие релиза.
- `Space` - отметить релиз, `d` - скачать отмеченное.
- `a` - скачать лучшее, `c` - проверить источники.
- `e` - правка пути библиотеки, `s` - настройки.
- `?` - справка по клавишам, `q` - выход.

## Источники и этапы

| Этап | Источники | Что даёт | Что нужно от пользователя |
|---|---|---|---|
| 1 | Soulseek, YouTube, Spotify (каталог) | Аудио: Soulseek (lossless, если найден), YouTube (lossy opus). Метаданные и ISRC из Spotify-каталога | Учётная запись Soulseek; YouTube и Spotify-каталог без учёток |
| 1б | Bandcamp (каталог и бесплатные промо), SoundCloud (каталог и lossless-оригиналы) | Поиск по артистам и альбомам, бесплатные промо в исходном качестве, ISRC от издателя у SoundCloud | Ничего (SoundCloud без логина только lossy) |
| 2 | Qobuz, Spotify (user-auth), Bandcamp (купленное) | Qobuz FLAC 16/44.1 без подписки, пользовательские плейлисты Spotify, lossless с купленных на Bandcamp альбомов | Аккаунт Qobuz, аккаунт Spotify, cookie `identity` от Bandcamp |
| 3 | Tidal, Deezer, Beatport | Tidal и Deezer без подписки дают lossy и ISRC; Beatport - каталог, ISRC и скачивание купленного | Аккаунты Tidal и Deezer; аккаунт Beatport и владение треками |

## Правовая оговорка

> Скачивание с Qobuz, Tidal и Deezer нарушает их условия использования. Bandcamp и Beatport легальны для уже купленного контента. Soulseek находится в серой зоне. Ответственность за использование несёт пользователь.