# maxfetch — design document

Дата: 2026-11-09 (редакция 2 — 2026-11-10)
Статус: на утверждение
Проект: `maxfetch` (одноимённый бинарник), Rust, лицензия MIT или Apache-2.0 (выбрать при инициализации крейта).

---

## 1. Intent и «почему»

Пользователь хочет одну утилиту, которая собирает трек в **максимально доступном качестве** (приоритет — lossless, минимум FLAC) с нескольких площадок, с удобным TUI, и раскладывает результат в аккуратную библиотеку.

Ключевая проблема сегодня: выбор площадки и качества ручной. Один трек может лежать на Soulseek в 24/96 FLAC, на Bandcamp в mp3-320 промо, на YouTube в opus 160k. Пользователь вручную сравнивает, скачивает, переименовывает, ищет дубли. `maxfetch` делает это автоматически: один запрос — и в библиотеку попадает лучшая доступная копия, а дубли отсеиваются по ISRC.

Вторая проблема: у каждой площадки свой API, своя авторизация, свои грабли (ротация ключей, cookie из браузера, WAF-блокировки). `maxfetch` прячет это за единым интерфейсом и общей логикой ранжирования качества.

Третья: качество нельзя узнать до скачивания. Bandcamp и Beatport в API не отдают `bit_depth`/`sample_rate`; у Soulseek у FLAC битрейт = 0. Значит, ранжирование должно опираться на несколько сигналов и уметь уточнять качество после скачивания (по фактическим заголовкам файла).

---

## 2. Цели

1. Единый вход: текстовый запрос, URL плейлиста YouTube, URL плейлиста/трека/альбома Spotify, URL страницы артиста/альбома Bandcamp, CSV-список.
2. Единый выход: файл в библиотеке `Artist/Album/NN Title.ext`, с корректными тегами, в максимально доступном качестве.
3. Единая логика выбора: lossless приоритетнее lossy; внутри группы — сначала битность, затем частота дискретизации; lossy никогда не пережимается.
4. Дедупликация по ISRC: один экземпляр трека в библиотеке, лучший по качеству.
5. TUI для интерактивной работы + headless CLI для скриптов и CI.
6. Секреты в отдельном файле с правами 0600, не в TOML-конфиге и не в коде.
7. Источники не «ломают весь проект», когда площадка лежит или ключи протухли: деградация до статуса «источник недоступен».

## 3. Не-цели

- Транскодирование, ресемплирование, upscaling до 24/192. Файл сохраняется в том виде, в каком его отдала площадка.
- Обход DRM (Widevine, Spotify). Spotify не даёт аудио — это осознанное решение, а не ограничение.
- Автоматическая покупка треков. Beatport без покупки не отдаёт аудио — в maxfetch скачивается только уже купленное.
- Серверный/многопользовательский режим. Приложение однопользовательское, локальное.
- Мобильные/веб-клиенты.
- Индексатор библиотеки в духе ReplayGain-сканера. Сканирование существующей библиотеки — только для дедупа.
- Раздача собственной библиотеки в сети Soulseek. `maxfetch` работает как клиент «только на скачивание».

## 4. Анти-цели (что считается провалом)

- Продвигать в проект lossy-файл, когда lossless-версия существует.
- Скачать дубли и оставить их в библиотеке.
- Требовать от пользователя ручной правки имён файлов или тегов.
- Ломать весь запуск из-за недоступности одного источника.
- Полагаться на нестабильные публичные client_id как на единственный путь к функции, без явной диагностики в UI.
- Удалять файлы библиотеки без явной команды пользователя. Никаких неявных `rm`.

## 5. Ограничения (проверенные факты)

| Ограничение | Источник проверки |
|---|---|
| `api.qobuz.com` не резолвится DNS | curl с машины, 2026-11-09 |
| `api.deezer.com` — DNS есть, TCP не устанавливается | curl, 2026-11-09 |
| `openapi.tidal.com`, `login.tidal.com` — CloudFront 403 «Request blocked» | curl, 2026-11-09 |
| Bandcamp mobile API отдаёт только mp3-128 | curl + разбор ответа `/api/mobile/25/tralbum_details` |
| Beatport v4 API не содержит `bit_depth`/`sample_rate`/`formats` | curl: track, release, release-tracks |
| Beatport `/v4/catalog/tracks/{id}/download/` → 403 с anon-токеном | curl, 2026-11-09 |
| Spotify: публичные плейлисты → 403 даже с client_credentials | curl + OpenAPI-спека Spotify |
| Spotify: web-flow без логина заблокирован (403/451) | curl, 2026-11-09 |
| `yt-dlp` и `ffmpeg` установлены в системе | `which`, 2026-11-09 |
| `lofty` 0.25 не поддерживает Matroska/WebM | docs lofty |
| Bandcamp mobile API не отдаёт ISRC | разбор ответа `/api/mobile/25/tralbum_details` |
| `api-v2.soundcloud.com` отдаёт 401 без `client_id`; `api.soundcloud.com` и `w.soundcloud.com` — 403 | curl, 2026-11-10 |
| SoundCloud: `GET /users/{id}/trackings` и `GET /tracks/{id}/download` — 401 без OAuth | curl, 2026-11-10 |
| SoundCloud без OAuth отдаёт только AAC 96k/160k (HLS) | `yt-dlp -F`, 2026-11-10 |
| Rust 1.99.0 есть; Go и cmake нет | `which`, 2026-11-09 |
| sockseek — C#, AGPL-3.0, HTTP API без аутентификации | исходники `sockseek-master` |

Юридическая сторона (не блокер, но фиксируется в README): скачивание с Qobuz/Tidal/Deezer нарушает их ToS; Bandcamp/Beatport — легально для купленного; Soulseek — серая зона. Ответственность на пользователе.

## 6. Подходы, рассмотренные

### 6.1 Язык и экосистема

| Вариант | Плюсы | Минусы | Вердикт |
|---|---|---|---|
| **Rust** (ratatui + tokio + reqwest + lofty/symphonia) | Готовые крейты для всего нужного; MIT/Apache; async сеть; TUI-экосистема | Нет готового HTTP-клиента в stdlib, но это не проблема | **Выбран** |
| Goose (C++, Apache-2.0) | Нет GC, data stacks, активная разработка | **Сети в stdlib нет вообще** (дословно в docs/stdlib.md: «Subprocesses and networking are not in v1»); HTTP-клиент только во внешнем пакете `goose-http`, blocking-only и не работает на Windows; нет AES/Blowfish (нужны для Deezer/Qobuz); нет терминального TUI (модуль `ui` — это Nuklear-окна, `gfx` — SDL), termios/raw-mode API отсутствует; Goose нет в nixpkgs | Отклонён |

### 6.2 Soulseek: свой клиент или sockseek

| Вариант | Плюсы | Минусы | Вердикт |
|---|---|---|---|
| **Свой клиент на `soulseek-rs-lib` 20.0.0 (MIT)** | Единая логика ранжирования и дедупа; один бинарник без внешних процессов; MIT без оговорок; единственная зависимость — `libc` | Писать протокол самим: connect/login/search/пагинация/download/retry | **Выбран** |
| Интеграция с sockseek-daemon по HTTP | Готовая функциональность (jobs, агрегация альбомов, папки) | C#/.NET runtime; daemon надо поднимать; его политика выбора файлов нам не подчиняется, слой нормализации всё равно нужен; AGPL-3.0 (формально при обращении к отдельному процессу по HTTP заражения нет, но граница тонкая) | Отклонён |
| Оба, переключаемые | Максимум гибкости | Два пути тестирования, две модели результатов | Отклонён |

### 6.3 Структура кода

| Вариант | Вердикт |
|---|---|
| **Монолитный крейт `maxfetch` с модулями `sources/*` и общим trait `Source`** | **Выбран**: для 4 источников церемония workspace не окупается |
| Workspace с крейтом на источник | Отклонён: оверинжиниринг сейчас; при добавлении Qobuz/Tidal/Deezer можно вынести их в отдельные крейты, если появятся тяжёлые крипто-зависимости |
| Динамические плагины (`.so`) | Отклонён: несовместимость на NixOS, ABI-хаос |

### 6.4 Состояние

| Вариант | Вердикт |
|---|---|
| **SQLite (`rusqlite` 0.41, WAL)** | **Выбран**: атомарность, транзакции при дедупе, переживает тысячи треков |
| JSON-файл | Отклонён: неатомарная запись, деградация на объёме |

### 6.5 YouTube

| Вариант | Вердикт |
|---|---|
| **`yt-dlp` как subprocess** | **Выбран**: extractor'ы ломаются постоянно, свой extractor безнадёжен; `yt-dlp` уже установлен |
| Свой extractor на Rust | Отклонён |

### 6.6 Состав источников: MVP = все четыре этапа

По решению владельца проекта **MVP — это полный набор этапов 1, 1б, 2 и 3**. Поставка инкрементами: каждый этап — рабочая, собираемая версия; следующий добавляет источники, не ломая предыдущий и не меняя публичный интерфейс (trait `Source`, формат БД, экраны TUI, команды CLI).

| Этап | Источники | Что даёт | Что нужно от пользователя |
|---|---|---|---|
| **1** | Soulseek, YouTube, Spotify (каталог) | Аудио: Soulseek (lossless, если найден), YouTube (lossy opus). Метаданные и ISRC из Spotify-каталога | Учётная запись Soulseek. YouTube и Spotify-каталог — без учёток |
| **1б** | Bandcamp (каталог + бесплатные промо), SoundCloud (каталог + lossless-оригиналы) | Поиск по артистам/альбомам, треклист, бесплатные промо в исходном качестве (иногда FLAC/WAV); SoundCloud — ISRC из метаданных издателя и оригинальный файл трека | Ничего (SoundCloud без логина — только lossy) |
| **2** | Qobuz, Spotify (user-auth), Bandcamp (купленное) | Qobuz FLAC 16/44.1 без подписки; пользовательские плейлисты Spotify; lossless с купленных на Bandcamp альбомов | Аккаунт Qobuz, аккаунт Spotify, cookie `identity` от Bandcamp |
| **3** | Tidal, Deezer, Beatport | Tidal и Deezer без подписки дают lossy (AAC / MP3 128) и ISRC; Beatport — каталог, ISRC и скачивание купленного | Аккаунты Tidal и Deezer; аккаунт Beatport + владение треками |

#### Этап 1 — базовый конвейер

**Входит:** ядро (конфиг, SQLite, модель, ранжирование качества, дедуп, теги, библиотека, TUI + CLI), источники Soulseek, YouTube, Spotify-каталог. SoundCloud и Bandcamp реализуются в этапе 1б, но их каркас (`Source` с `Capabilities::metadata_only`) заводится уже в этапе 1, чтобы реестр источников не менялся.

**Ключевые решения этапа:**

- Soulseek — свой клиент на `soulseek-rs-lib` 20.0.0 (MIT): логин, поиск, отбор пиров по слотам и скорости, скачивание с фолбэком на следующего кандидата при зависании.
- YouTube — `yt-dlp` как subprocess + `ffmpeg -c copy` для ремукса `.webm` → `.opus` без пережатия (нужно, чтобы `lofty` смог проставить теги и пометку `lossy`).
- Spotify — только `client_credentials`: поиск и метаданные, ISRC как эталон дедупа и нормализации описаний. Пользовательских плейлистов нет.

**Что считается готовым этапа:** сценарий «трек есть на Soulseek в 24/96 и на YouTube в opus» выбирает Soulseek; «трек есть только на YouTube» даёт opus с тегами и бейджем `LOSSY`; дедуп помечает дубль, но не удаляет файл.

#### Этап 1б — Bandcamp и SoundCloud

**Bandcamp:** поиск (`/api/fuzzysearch/2/app_autocomplete`), метаданные релиза (`/api/mobile/25/tralbum_details`), дискография артиста (`/api/mobile/24/band_details`), разбор URL страницы в `band_id`/`tralbum_id`, скачивание бесплатных промо через цепочку `freeDownloadPage` → `/statdownload/` → `retry_url` → ZIP, распаковка архива по трекам.

**SoundCloud:** каталог и треки через `api-v2.soundcloud.com` с публичным `client_id`, извлечение оригинального файла трека, страницы артистов и плейлистов, ISRC из метаданных издателя.

**Особенности:** Bandcamp не отдаёт ISRC — дедуп по `match_key` и длительности. Промо отдаётся архивом: единица работы остаётся треком (§11), архив — транспорт. Покупки в этом этапе не трогаем, `identity`-cookie не нужен.

#### Этап 2 — lossless без подписки и пользовательские плейлисты

**Входит:**

- **Qobuz** — `qobuz-api` 4.0.0. Авторизация: e-mail + пароль + `app_id` + `app_secret`. Пара берётся из секции `[qobuz]` в `credentials.toml`; если её там нет — скрейпится из `play.qobuz.com/bundle.js` регуляркой `production:{api:{appId:"(\d{9})",appSecret:"(\w{32})"` и сохраняется в конфиг (повторный скрейп — только при ошибке авторизации). Форматы: `format_id` 5 = MP3-320, 6 = FLAC-CD, 7 = FLAC-24≤96, 27 = FLAC-24>96. Потолок качества = минимум из максимума трека и потолка аккаунта: без подписки Studio — FLAC-CD (format_id 6), со Studio — до 27.
- **Spotify user-auth** — OAuth2 Authorization Code + PKCE, `refresh_token` в `credentials.toml` с автообновлением. Открывает пользовательские плейлисты (`GET /v1/playlists/{id}/items`), которые на `client_credentials` дают 403.
- **Bandcamp-lossless** — cookie `identity` из браузера: `fan_id` из `<script data-tralbum-collect-info>` → `POST /api/fancollection/1/search_items` → `redownload_urls` → страница скачивания → `pagedata` → `digital_items[0].downloads.<fmt>.url`.

**Что считается готовым этапа:** `maxfetch fetch --spotify-playlist <id-or-url>` скачивает плейлист; Qobuz отдаёт FLAC 16/44.1 там, где раньше был opus; купленный Bandcamp-альбом приходит в FLAC.

#### Этап 3 — lossy-площадки и Beatport

**Входит:**

- **Tidal** — `tidalrs` 0.6.2. OAuth2 с `client_id`/`client_secret` **из секции `[tidal]` в `credentials.toml`; встроенной запасной пары нет** — без ключей источник отключается со статусом «нет учётных данных Tidal». Качество: 0/1 = AAC, 2 = LOSSLESS FLAC, 3 = HI_RES. Без подписки HiFi доступен только AAC, поэтому этот источник даёт lossy и ISRC. Известная проблема: ключи периодически ломаются (issue yaronzz#1213, «all API keys broken as of 2026-03-21») — нужна диагностика «ключ протух» и возможность задать свой.
- **Deezer** — `deezer-client` 0.1.4. Авторизация — cookie `arl` из браузера, дешифровка трека Blowfish-CBC (ключ — первые 16 hex-символов MD5 от `"<id>" + secret`). Без подписки HiFi — MP3 128. ISRC присутствует.
- **Beatport** — каталог через OAuth2 `client_credentials` (`client_id`/`client_secret` **только из секции `[beatport]` в `credentials.toml`; опубликованной пары по умолчанию нет** — без ключей источник отключается со статусом «нет учётных данных Beatport»), плюс user-auth (`POST /v4/auth/login/` → `/v4/auth/o/authorize/` → code → `/v4/auth/o/token/`) для скачивания купленного: `GET /v4/catalog/tracks/{id}/download/` отдаёт 403 без владения треком. Каталог отдаёт ISRC, но **не отдаёт** `bit_depth`/`sample_rate`/`formats` для анонимного токена.

**Что считается готовым этапа:** lossy-кандидаты с этих площадок участвуют в общем ранжировании и уступают любому lossless; при отсутствии lossless в библиотеку попадает лучший lossy с честной пометкой.

**Известные блокеры этапов 2 и 3** (сетевые, проверяются на каждом этапе; при блокировке источник просто отключается, остальные работают):

- `api.qobuz.com` — DNS не резолвится с этой машины.
- `openapi.tidal.com`, `login.tidal.com` — CloudFront 403 «Request blocked».
- `api.deezer.com` — DNS есть, TCP не устанавливается.
- `api.beatport.com` отвечает, но скачивание требует владения треком.

**Границы MVP по учётным записям.** Сейчас у пользователя нет Spotify-аккаунта и нет покупок на Bandcamp, поэтому этапы 1 и 1б полностью работоспособны сразу, а этапы 2 и 3 требуют завести учётные записи и, для Bandcamp, иметь покупки. Это подготовительные действия пользователя, а не ограничения проекта: код всех этапов пишется и проверяется на фикстурах, а на живой сети — при наличии учёток.

---

## 7. Архитектура

### 7.1 Модули

```
src/
  main.rs              точка входа, режимы TUI/CLI
  cli.rs               clap-derive: подкоманды и флаги
  config.rs            config.toml + credentials.toml (0600) + XDG-пути
  db.rs                rusqlite, миграции, репозитории
  model.rs             Track, Release, Candidate, Quality, SourceId
  quality.rs           ранжирование и сравнение качества
  match.rs             нормализация и матчинг кандидатов между источниками
  dedup.rs             дедуп по ISRC: пометка дублей, явное удаление по команде
  library.rs           шаблоны путей, санитизация имён, индексация (scan)
  tags.rs              lofty: запись тегов
  probe.rs             symphonia/flacenc: реальное качество скачанного файла
  fetch.rs             загрузчик: HTTP и файловые, .part → rename
  youtube.rs           обёртка над yt-dlp subprocess
  media.rs             ffmpeg: ремукс без пережатия (webm → opus), sidecar-метаданные
  sources/
    mod.rs             trait Source, реестр источников
    soulseek.rs
    bandcamp.rs
    spotify.rs
    soundcloud.rs
    qobuz.rs
    tidal.rs
    deezer.rs
    beatport.rs
  tui/
    mod.rs             App, состояние, event loop
    views/             search, results, candidates, downloads, sources, settings, log
```

### 7.2 Модель данных

```rust
enum SourceId { Soulseek, Bandcamp, Youtube, Spotify, SoundCloud, Qobuz, Tidal, Deezer, Beatport }

struct Quality {
    lossless: bool,
    container: Container,      // Flac, Wav, Aiff, Alac, M4aAac, Mp3, Opus, Webm
    bit_depth: Option<u8>,     // 16 / 24 / 32
    sample_rate: Option<u32>,  // 44100 / 48000 / 96000 / 192000
    bitrate_kbps: Option<u32>, // для lossy
}

struct Release {              // логический трек/альбом
    title: String,
    artist: String,
    album: Option<String>,
    duration: Option<Duration>,
    isrc: Option<String>,
    year: Option<i32>,
    sources: Vec<SourceId>,
}

struct Candidate {            // конкретный вариант файла на конкретном источнике
    source: SourceId,
    release: Release,
    quality: Quality,
    size_bytes: Option<u64>,
    locator: Locator,         // source-specific handle на скачивание
}
```

`Locator` — непрозрачный для ядра handle: он несёт всё, что нужно источнику для скачивания, включая идентификатор предмета и выбранный формат. У SoundCloud локатор содержит `track_id` **и** формат (`aac_160k`/`aac_96k` либо lossless-оригинал), поэтому AAC-кандидат и lossless-оригинал — разные кандидаты с разными локаторами. У Bandcamp локатор несёт URL страницы скачивания и кодировку (`enc`), а идентификатор релиза (`tralbum_id`) извлекается из URL-фрагмента.

### 7.3 Trait источника

```rust
#[async_trait]
pub trait Source: Send + Sync {
    fn id(&self) -> SourceId;
    fn capabilities(&self) -> Capabilities;  // audio / metadata_only / lossless / needs_auth
    fn auth_state(&self) -> AuthState;       // NotNeeded, Missing, Authenticated, Expired
    async fn search(&self, q: &Query) -> Result<Vec<Release>>;
    async fn resolve(&self, item: &ItemRef) -> Result<Release>;
    async fn candidates(&self, rel: &Release) -> Result<Vec<Candidate>>;
    async fn download(&self, cand: &Candidate, dest: &Path, sink: &ProgressSink) -> Result<()>;
}
```

Реестр источников — статический список. Добавление источника = один файл + строка в реестре + запись в `capabilities`.

`registry()` возвращает **все** зарегистрированные в сборке источники без фильтрации по конфигу: реестр отвечает только на вопрос «что существует». Отбор включённых — отдельная функция `enabled_in(ids: &[SourceId], config: &Config) -> Vec<SourceId>`; её вызывают конвейер (перед `search`/`candidates`) и TUI (экран Sources). Ни конвейер, ни TUI не фильтруют список источников самостоятельно, чтобы состав и порядок совпадали.

### 7.4 Ранжирование качества

Ключ сравнения (больше — лучше):

```
lossless: true > false
lossless-группа:  bit_depth desc, затем sample_rate desc
lossy-группа:     bitrate_kbps desc, затем sample_rate desc
```

Это ровно то правило, которое выбрано пользователем: сначала битность, потом частота; поэтому 24/48 FLAC побеждает 16/96 FLAC.

Правила применения:

- Lossy **никогда** не вытесняет lossless. Если для релиза есть хотя бы один lossless-кандидат, lossy-кандидаты в выдачу не попадают.
- Lossy-кандидат допускается только если lossless не найден ни на одном источнике (включая ситуацию «источник недоступен» — тогда результат помечается как «лучшее из найденного»).
- Отсутствующие характеристики не выдумываются: `bit_depth = None` не считается за 24. Неизвестное качество сортируется ниже известного того же класса.
- У Soulseek у FLAC атрибут битрейта = 0/отсутствует, поэтому упор на `sample_rate` + `bit_depth` из атрибутов, а битрейт используется только в lossy-группе.

### 7.5 Уточнение качества после скачивания

После скачивания файл «прощупывается» (`probe.rs`): `symphonia` читает формат/частоту/битность из контейнера, для FLAC — заголовок STREAMINFO. Это истина в последней инстанции:

1. Кандидат ранжируется по данным источника.
2. Файл скачивается.
3. Реальное качество читается из файла и **пересчитывает** ранг.
4. **Правило отбраковки:** если измеренное качество **хуже заявленного** источником (например, источник обещал 24/96 FLAC, а в файле 16/44.1), кандидат отклоняется, и конвейер пробует **следующего кандидата по убыванию качества**.
5. Если кандидаты кончились, а последний принятый файл — лучшее из найденного, он всё равно принимается с пометкой «фактическое качество ниже заявленного источником» в логе и в UI.

**Цикл повторов живёт внутри конвейера загрузки, а не в вызывающем коде.** `fetch` (ядро) сам проходит по упорядоченному списку кандидатов: скачать → probe → принять или отклонить по правилу пункта 4 → перейти к следующему. TUI и headless CLI только запрашивают «достать лучший вариант для релиза» и получают итоговый результат; перебор кандидатов, повторные попытки и решение «когда остановиться» не дублируются ни в TUI, ни в CLI.

Это не «качать top-N пачкой»: кандидаты перебираются последовательно, один за другим, и обычно первый же совпадает. Так мы ловим ложные заявления, не раздувая трафик и не меняя модель очереди.

Качество, заявленное источником, но не подтверждённое файлом, нигде не сохраняется как истина: в БД пишется только фактическое.

Для lossless-оригиналов (SoundCloud с OAuth, Bandcamp с `identity`-cookie) probe после скачивания обязателен: именно он отличает настоящий WAV/FLAC от переименованного AAC.

### 7.6 Дедупликация по ISRC

Три уровня идентификации:

1. **ISRC** — точное совпадение. Основной уровень.
2. Если ISRC нет (YouTube, часть Soulseek) — **фолбэк по нормализованному ключу**: `norm(artist) + norm(title) + norm(album)`, где `norm` убирает пунктуацию, приводит к lowercase/unicode NFC, выкидывает скобочные ремарки (`(Remastered)`, `[Live]`, `- 2011 Remaster`), схлопывает пробелы. Для треков без альбома ключ = `norm(artist) + norm(title)`.
3. Дополнительно — **сопоставление по длительности**: если ISRC нет у обоих, но длительности совпадают в пределах допуска, а нормализованные имена похожи, считаем дублем с пометкой «matched by duration». Допуск настраивается: `MatchingConfig::duration_tolerance_sec` (по умолчанию 2, единица — секунды) в конфиге, а не константой в коде.

**Регистрация скачанного трека в индексе дедупа выполняется в конвейере** сразу после переноса файла в библиотеку и **до** записи в БД: конвейер вычисляет ISRC/match_key по фактическим тегам, прогоняет сравнение с существующими записями и только затем фиксирует транзакцию. Вызывающий код (TUI, CLI) регистрацию не выполняет — иначе дубль можно было бы пропустить, забыв повторить шаги в одном из входов.

Правило выживания: при коллизии остаётся экземпляр с лучшим `Quality`; при равенстве — более новый.

**Что происходит с проигравшим файлом (решение владельца проекта).** `maxfetch` **не удаляет файлы сам и никогда не удаляет молча**. Работает так:

1. При скачивании новой версии трека проигравший экземпляр **помечается в БД** как `duplicate_of = <id победителя>`, его запись не удаляется, файл остаётся на диске.
2. Никаких карантинных папок: файл либо удаляется пользователем через отдельную команду, либо остаётся.
3. Удаление — **только явной командой** `maxfetch library dedupe --apply --delete`, которая спрашивает подтверждение (в TUI) или требует `--yes` (в CLI) и показывает точный список файлов перед удалением.
4. Без `--apply` команда работает в режиме dry-run: только показывает, что будет удалено.
5. Коллизии, где качество неразличимо (оба lossy, одинаковый битрейт), помечаются в UI флагом `conflict` для ручного разбора; дефолт — «оставить более новый», но флаг `conflict` блокирует автоудаление даже при `--apply --delete` (такие файлы удаляются только после ручного подтверждения по списку).

**Дедуп глобальный, а не в пределах релиза.** Один и тот же ISRC может прийти из разных альбомов (сборник, ремикс, другой релиз). Совпадение находится по ISRC в пределах всей библиотеки; если альбом при этом разный, файл-победитель остаётся в своём альбоме, а второй помечается дублем. Так в библиотеке не появляется «трек с дыркой в треклисте».

**`library scan` только индексирует.** Команда обходит дерево библиотеки, читает теги (lofty) и заголовки, сопоставляет с БД, заводит недостающие записи. Она **не переименовывает, не переносит и не удаляет** файлы. Переименование и раскладку по `Artist/Album/NN Title.ext` делает только `fetch` — то есть файлы, которые вытащил сам `maxfetch`.

### 7.7 Структура библиотеки

```
<library>/<Artist>/<Album>/<NN> <Title>.<ext>
```

- `NN` — номер трека (двузначный, с ведущим нулём), из метаданных источника; при отсутствии — порядок появления в релизе.
- `<Album>` при отсутствии альбома: `Singles`.
- Расширение — по фактическому контейнеру скачанного файла.
- Замена символов в именах: `/ \ : * ? " < > |` → `_`; ведущие/хвостовые точки и пробелы обрезаются; имена Windows-Reserved (`CON`, `PRN`, `AUX`, `NUL`, `COM1..9`, `LPT1..9`) получают суффикс `_`; пробелы схлопываются; длина пути ограничивается 240 байт на сегмент с обрезкой по границе слова.

Дефолтная папка: `~/Music/maxfetch` (XDG `xdg-user-dir MUSIC`). Патч переопределяется в TUI (экран настроек) и через `--library`; изменение сохраняется в `config.toml`.

### 7.8 Пометка lossy

Lossy-файл сохраняется **как есть**, без пережатия. Пометка несётся в трёх местах:

- **Тег `COMMENT`**: `maxfetch: source=lossy; container=Opus; source_id=YouTube; acquired=<ts>`
- **БД**: колонка `lossless INTEGER NOT NULL` + `container`.
- **TUI**: бейдж `LOSSY` в списке, в колонке «качество».

Имя файла остаётся чистым — пометка в имени ломала бы сортировку и переносимость библиотеки.

Единственное исключение — YouTube, где контейнер несовместим с тегами (см. §8.3): там файл приводится к `.opus` ремуксом без пережатия, и после этого `COMMENT` пишется штатно.

### 7.9 Состояние (SQLite)

```sql
PRAGMA journal_mode=WAL;

CREATE TABLE schema_version (version INTEGER NOT NULL);

CREATE TABLE tracks (
  id            INTEGER PRIMARY KEY,
  isrc          TEXT,
  match_key     TEXT NOT NULL,      -- нормализованный ключ для фолбэк-дедупа
  title         TEXT NOT NULL,
  artist        TEXT NOT NULL,
  album         TEXT,
  track_no      INTEGER,
  duration_sec  REAL,
  lossless      INTEGER NOT NULL,
  container     TEXT NOT NULL,
  bit_depth     INTEGER,
  sample_rate   INTEGER,
  bitrate_kbps  INTEGER,
  size_bytes    INTEGER,
  path          TEXT NOT NULL UNIQUE,
  source_id     TEXT NOT NULL,
  acquired_at   TEXT NOT NULL,
  duplicate_of  INTEGER REFERENCES tracks(id)   -- NULL = канонический экземпляр
);
CREATE INDEX idx_tracks_isrc     ON tracks(isrc);
CREATE INDEX idx_tracks_matchkey ON tracks(match_key);
CREATE INDEX idx_tracks_dup      ON tracks(duplicate_of);

CREATE TABLE releases (            -- кэш метаданных релизов
  match_key  TEXT PRIMARY KEY,     -- тот же нормализованный ключ, что и в tracks.match_key
  source_id  TEXT NOT NULL,
  payload    TEXT NOT NULL,        -- JSON
  fetched_at TEXT NOT NULL,
  expires_at TEXT NOT NULL         -- fetched_at + 7 дней
);

CREATE TABLE downloads (           -- журнал
  id         INTEGER PRIMARY KEY,
  track_id   INTEGER REFERENCES tracks(id),
  source_id  TEXT NOT NULL,
  state      TEXT NOT NULL,       -- queued|running|done|failed|skipped
  reason     TEXT,
  started_at TEXT,
  ended_at   TEXT
);

CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
```

Таблица `releases` служит **кэшем метаданных**. Ключ — `match_key`, TTL — 7 дней (`expires_at`). При попадании в кэш (запись есть и `expires_at` ещё не наступил) источники повторно **не опрашиваются**: `search`/`resolve`/`candidates` отдают данные из кэша. Результаты поиска пишутся в кэш сразу после ответа источника, поэтому повторный запрос того же релиза в пределах TTL идёт без сети.

Дедуп выполняется в одной транзакции: `BEGIN IMMEDIATE` → найти по ISRC, затем по match_key → сравнить качество → проигравшему проставить `duplicate_of`, победителя оставить каноническим. Файлы при этом **не трогаются**; удаление — отдельная явная команда (§7.6).

### 7.10 Конфигурация и секреты

XDG-пути:
- `$XDG_CONFIG_HOME/maxfetch/config.toml` — несекретные настройки.
- `$XDG_CONFIG_HOME/maxfetch/credentials.toml` — **секреты, права 0600**, создаётся с 0600, при старте проверяем права и предупреждаем, если шире 0600.
- `$XDG_DATA_HOME/maxfetch/state.db` — SQLite.

**Проверка прав `credentials.toml`.** При каждом старте maxfetch проверяет режим доступа к файлу `credentials.toml`. Если права шире 0600 (то есть секреты читает кто-то ещё — группа или все), пользователю выводится предупреждение в лог и в TUI: «Внимание: права файла credentials.toml шире 0600, секреты доступны другим пользователям — исправьте через `chmod 600 <путь>`». Это предупреждение, а не ошибка: запуск продолжается, источники работают.

```toml
# config.toml
[library]
path = "~/Music/maxfetch"

[sources]
enabled = ["soulseek", "bandcamp", "youtube", "spotify"]

[matching]
duration_tolerance_sec = 2.0     # MatchingConfig::duration_tolerance_sec, допуск по длительности при дедупе
require_isrc = false

[download]
max_concurrency = 4
retry_attempts = 3
timeout_sec = 60

[youtube]
yt_dlp_path = "yt-dlp"          # из PATH

[media]
ffmpeg_path = "ffmpeg"          # из PATH, нужен для ремукса YouTube
```

```toml
# credentials.toml (0600)
[soulseek]
username = "..."
password = "..."

[bandcamp]
identity_cookie = "..."         # cookie identity из браузера

[spotify]
client_id = "..."
client_secret = "..."
refresh_token = "..."            # user-auth, OAuth2 Authorization Code + PKCE

[soundcloud]
client_id = "..."               # скрейпится из JS-бандла soundcloud.com
oauth_token = "..."             # опционально, даёт оригинальный lossless-файл

[qobuz]
email = "..."
password = "..."
app_id = "..."                   # скрейпится из play.qobuz.com/bundle.js
app_secret = "..."

[tidal]
client_id = "..."                # только отсюда; встроенной запасной пары нет
client_secret = "..."

[deezer]
arl = "..."                      # cookie arl из браузера
secret = "..."                   # для деривации ключа Blowfish (MD5 от "<id>" + secret)

[beatport]
client_id = "..."                # только отсюда; опубликованной пары по умолчанию нет
client_secret = "..."
username = "..."                 # user-auth для скачивания купленного
password = "..."
```

Все секции читаются опционально: отсутствующая секция означает «источник не авторизован», источник помечается в TUI accordingly и выключается, остальные работают. Для Tidal и Beatport это означает конкретные статусы: «нет учётных данных Tidal» и «нет учётных данных Beatport» — встроенных запасных пар нет ни у одного из них.

Исключение — `[soundcloud] client_id`: он не секрет, а публичный идентификатор приложения. Если его нет в конфиге, maxfetch сам скрейпит его с `soundcloud.com` и сохраняет; вручную задавать не нужно. `oauth_token` опционален: без него SoundCloud отдаёт только lossy (`aac_160k`/`aac_96k`), с ним — оригинальный lossless-файл.

Авторизация Spotify — OAuth2 Authorization Code + PKCE, `refresh_token` сохраняется и обновляется автоматически. Bandcamp-lossless — cookie `identity`, копируется пользователем из браузера (в TUI есть подсказка, где именно). Qobuz — e-mail + пароль плюс `app_id`/`app_secret`: берутся из секции `[qobuz]`, а если их там нет — скрейпятся регуляркой из `bundle.js` и сохраняются в конфиг (повторный скрейп — только при ошибке авторизации). Deezer — cookie `arl` плюс `secret` для деривации ключа расшифровки. Beatport — пара client_id/secret (только из секции `[beatport]`) плюс user-логин для скачивания купленного.

### 7.11 TUI

`ratatui` 0.30 + `crossterm` 0.29. Экраны:

| Экран | Содержимое |
|---|---|
| **Search** | поле запроса; выбор типа входа (запрос / YouTube-плейлист / Spotify-плейлист / Spotify-трек-или-альбом / SoundCloud-ссылка / Bandcamp-страница / CSV). Пункты, требующие авторизации, помечены и дают понятное объяснение, если учётки нет |
| **Results** | список релизов: артист — трек, альбом, год, источники-бейджи, ISRC |
| **Candidates** | варианты файла по источникам, отсортированные по качеству: бейдж качества, битность/частота/битрейт, размер, источник; флаги `lossy`, `conflict` |
| **Downloads** | очередь: прогресс-бар, скорость, ETA, состояние, кнопка «отмена». Для Soulseek дополнительно: имя пира, его скорость и число свободных слотов; пир с `slots == 0` помечается недоступным |
| **Sources** | статус каждого источника (Soulseek, YouTube, Spotify, SoundCloud, Bandcamp, Qobuz, Tidal, Deezer, Beatport): доступен / нет авторизации / ключ протух / недоступен по сети + кнопка «проверить» |
| **Settings** | путь библиотеки (правится в рантайме и сохраняется), тумблеры источников, лимиты, путь к `yt-dlp`, путь к `ffmpeg` |
| **Log** | последние события с уровнями |

Клавиши: `Tab`/`Shift+Tab` — смена экрана, `/` — поиск, `Enter` — раскрыть кандидатов, `Space` — отметить, `d` — скачать отмеченное, `a` — скачать лучшее по всем отмеченным релизам, `s` — настройки, `?` — помощь, `q` — выход.

### 7.12 Headless CLI

```
maxfetch search "artist - title" [--source soulseek,bandcamp] [--json]
maxfetch fetch <query>...            # текстовый запрос
maxfetch fetch --playlist <yt-url>
maxfetch fetch --spotify-track <id-or-url>     # метаданные Spotify как описание трека
maxfetch fetch --spotify-playlist <id-or-url>  # пользовательский плейлист (нужен user-auth)
maxfetch fetch --page <url>                      # страница: маршрутизация по домену URL
maxfetch fetch --soundcloud <track-or-artist-url> # явно источник SoundCloud
maxfetch fetch --input <csv>                    # CSV-вход: artist,title,album,isrc
maxfetch fetch --input -             # stdin
maxfetch library scan                # проиндексировать существующее (только чтение)
maxfetch library dedupe              # dry-run: показать кандидатов на удаление
maxfetch library dedupe --apply --delete --yes # явное удаление помеченных дублей
maxfetch library list [--json]
maxfetch auth login spotify
maxfetch auth login soundcloud
maxfetch auth login qobuz
maxfetch auth login tidal
maxfetch auth login deezer
maxfetch auth login beatport
maxfetch auth status
maxfetch sources check               # проверить доступность и авторизацию
maxfetch config set library /path
```

CSV-формат (заголовок опционален):
```
artist,title,album,isrc
```

`maxfetch fetch --input <csv>` читает CSV-вход с колонками `artist,title,album,isrc` (разделитель — запятая, первая строка может быть заголовком; колонки выбираются по имени, порядок не важен, недостающие необязательны). `--input -` читает тот же формат из stdin.

Общие флаги: `--library PATH`, `--source a,b`, `--dry-run`, `--json` (итог JSON), `--ndjson-progress` (NDJSON-поток прогресса в stderr, по одному JSON-объекту на событие), `--no-tui`.

**Маршрутизация URL.** `--page <url>` определяет источник **по домену URL**: `bandcamp.com` → Bandcamp, `soundcloud.com` → SoundCloud, `open.spotify.com` → Spotify, `youtube.com`/`youtu.be` → YouTube, `beatport.com` → Beatport; для незнакомого домена — понятная ошибка со списком поддерживаемых. `--soundcloud <url>` — отдельная опция, которая **явно** выбирает источник SoundCloud независимо от домена (используется, когда URL отдан в сокращённом виде или требуется принудительный выбор источника); домен в URL при этом всё равно должен быть SoundCloud.

`--dry-run` показывает, что было бы скачано (источник, формат, качество, путь), ничего не качая.

`maxfetch library dedupe` **по умолчанию ничего не удаляет** — печатает список. Удаление требует `--apply --delete`, а в CLI ещё и `--yes` (иначе интерактивное подтверждение). Файлы с пометкой `conflict` исключаются из списка автоматически.

---

## 8. Источники: детали реализации

### 8.1 Soulseek (свой клиент, крейт `soulseek-rs-lib` 20.0.0 = `soulseek_rs`, MIT)

Блокирующий API → выполняется в `tokio::task::spawn_blocking`.

- Логин: `Client::new(username, password)` → `connect()` → `login() -> Result<bool>`. Учётка в `credentials.toml`.
- Поиск: `search(&self, query, timeout) -> Result<Vec<SearchResult>>` → у каждого результата `username`, `slots`, `speed` и список файлов; по файлу доступны `filename`, `size`, `bitrate`, `length`, `attributes` (атрибуты приходят картой по числовым кодам протокола).
- Атрибуты файла: `{0: bitrate, 1: length, 4: sample rate Hz, 5: bit depth}`. Код `2` (VBR) **не используется** — по нему ничего не определяем.
- `SearchResult` содержит `slots` (число свободных слотов; `0` — пир недоступен) и `speed` (условная скорость 1-4).
- **Классификация lossless по расширению**: `flac`, `wav`, `alac`, `aiff`, `ape`, `wv` → lossless; `mp3`, `m4a/aac`, `opus`, `ogg/vorbis` → lossy.
- У FLAC битрейт = 0 → для lossless ранжирование по `sample_rate` + `bit_depth`; битрейт используется только в lossy-группе.
- Скачивание: `download(...) -> (Download, Receiver<DownloadStatus>)`, где `Receiver` — из `std::sync::mpsc`; пишем в `<name>.part`, после завершения — probe → rename в конечный путь.
- Ограничение нагрузки: пауза между запросами, чтобы не быть отключённым за флуд.

**Докачи нет.** Протокол Soulseek не поддерживает возобновление скачивания с оффсета: у него нет операции «продолжить с позиции N». Поэтому незавершённый файл `.part` **удаляется**, а скачивание следующего кандидата начинается заново с нуля. Никакого «сначала»-режима с серверной поддержкой докачи в API нет — это ограничение протокола, а не режим конфигурации.

**Отображение пира в UI.** Для активной передачи показываются: имя пира (username), число свободных слотов и скорость. Пир с `slots == 0` помечается как **недоступный** и в списке кандидатов, и в экране Downloads (значок/подпись «пир недоступен»), чтобы пользователь видел причину отбраковки, а не просто пустую строку.

**Шаринг и входящий порт (решение владельца проекта).** `maxfetch` — клиент **только на скачивание**. Не поднимает входящий порт, не шарит загруженное, не использует `shared_directories`, не трогает UPnP/NAT-PMP. В настройках Soulseek-профиля нет раздела «что раздавать».

**Выбор пира.** У `SearchResult` есть `slots` (свободные слоты для загрузок) и `speed` (условная скорость 1-4). Правила:

- Кандидаты без свободных слотов (`slots == 0`) отбрасываются на этапе отбора: пир физически не отдаст файл.
- Кандидаты ранжируются по (качество файла → наличие слотов → скорость → размер файла).
- Топ-кандидат пробуется первым. Если передача **зависла** (нет байтов дольше `stall_timeout_sec`, дефолт 30 с) или пир отвалился — конвейер переключается на следующего кандидата и скачивает его **с нуля** (протокол не поддерживает возобновление с оффсета, незавершённый `.part` удаляется).
- В UI видно, какой пир выбран и почему (например, «пир A: нет слотов»).

**Оценка надёжности источника.** Для Soulseek считаем `availability` за последние N попыток; при устойчиво низком — источник помечается в UI как ненадёжный, но не отключается.

### 8.2 Bandcamp (этап 1б: каталог и промо; этап 2: купленное)

Этап 1б — каталог и бесплатные промо, без учётной записи. Этап 2 — купленное lossless по cookie `identity`.

- Поиск: `GET /api/fuzzysearch/2/app_autocomplete?q=<urlencoded>&param_with_locations=true` → `results[]` с `type` (`b` артист / `a` альбом / `t` трек) и `id`. Ответ содержит `location` (массив объектов с типом и id): **обязательно сохраняем `band_id` из `location` альбома** — без него нельзя вызвать `tralbum_details` (у альбома в `location` лежит `band_id`, а не только `id` альбома).
- Метаданные релиза: `GET /api/mobile/25/tralbum_details?band_id=&tralbum_id=&tralbum_type=a|t` → `title`, `tralbum_artist`, `release_date`, `free_download`, `is_purchasable`, `tracks[]` (`track_id`, `title`, `track_num`, `duration`, `band_name`, `album_title`).
- Дискография артиста: `GET /api/mobile/24/band_details?band_id={id}` → `discography[]` с `item_id`, `item_type`, `title`, `release_date` — по этим полям строится список релизов артиста.
- URL → id: парсим HTML страницы, регексы `&amp;band_id=([0-9]+)` и `"name":"item_id","value":([0-9]+)`. Идентификатор релиза (`tralbum_id`) извлекается из URL-фрагмента; локатор кандидата хранит URL страницы скачивания и кодировку.
- **Бесплатное промо (lossless бывает)**: цепочка строго в таком порядке:
  1. Из `<script data-tralbum="...">` достаём `freeDownloadPage` → GET этой страницы.
  2. Страница отдаёт 403 «Download Expired» — это ожидаемое поведение, не ошибка.
  3. В URL подменяем `/download/` → `/statdownload/`.
  4. К URL добавляем параметр `.rand=<случайное int>`; разделитель выбирается по наличию query-строки: `?` если её нет, `&` если она есть.
  5. В теле ответа ищем `var _statDL_result` — JSON с полем `retry_url`.
  6. Повторный GET по `retry_url` → ZIP-архив.
- **Обработка сбоев цепочки:**
  - Если в `var _statDL_result` нет `retry_url` — источник сообщает, что бесплатного промо у релиза нет (в лог/UI: «промо недоступно»), и **не падает**: запрос считается неудачным, а не ошибкой программы.
  - Если ответ на `retry_url` не ZIP (нет сигнатуры `PK\x03\x04` в начале файла) — файл отбрасывается с предупреждением в лог и в UI.
  - Если у члена архива неизвестный `enc` (формат) — он логируется с указанием формата и названия трека, и **сам член исключается** из разбора (трек не создаётся), остальные члены обрабатываются.
- **Купленное (этап 2)**: cookie `identity` из браузера; `fan_id` из `<script data-tralbum-collect-info>` → `POST /api/fancollection/1/search_items` `{"fan_id":N,"search_key":"<album title>","search_type":"collection"}` → `redownload_urls` → та же страница скачивания → `pagedata` → `digital_items[0].downloads.<fmt>.url`.
- Форматы в `downloads.<fmt>`: `flac`, `mp3-v0`, `mp3-320`, `mp3-128`, `aac-hi`, `wav`, `alac`, `vorbis`. Есть `size_mb` и `encoding_name`.

**Формат → контейнер → lossless? → битрейт:**

| Формат | Контейнер | Lossless? | Известный битрейт |
|---|---|---|---|
| `flac` | FLAC | да | неизвестен |
| `wav` | WAV | да | неизвестен |
| `alac` | ALAC (в контейнере m4a) | да | неизвестен |
| `vorbis` | Opus-контейнер | нет | неизвестен |
| `mp3-320` | MP3 | нет | 320 кбит/с |
| `mp3-128` | MP3 | нет | 128 кбит/с |
| `mp3-v0` | MP3 | нет | неизвестен |
| `aac-hi` | M4A | нет | неизвестен |

- Альбом качается ZIP → распаковываем во временную папку → раскладываем по трекам.
- Bandcamp не отдаёт ISRC → дедуп по match_key + длительности.

### 8.3 YouTube (yt-dlp subprocess)

- Плейлист: `yt-dlp -J --flat-playlist <url>` → список видео с id/title/длительность.
- Одиночное видео: `yt-dlp -J <url>` → форматы, `abr`, `asr`, `acodec`.
- Аудио: `yt-dlp -f bestaudio -o <tmp>/<id>.%(ext)s <url>` → обычно WebM/Opus 160k (lossy).
- **Ремукс без пережатия (решение владельца проекта).** `lofty` не умеет тегамировать Matroska, а YouTube отдаёт `.webm`. Поэтому аудио переносится в контейнер, который lofty понимает, **без перекодирования**: `ffmpeg -i in.webm -c copy out.opus`. Это копирование потока, качество не меняется, видеоряд отбрасывается. Дальше файл тегируется штатно и получает пометку `lossy` в `COMMENT`.
  - Если `ffmpeg` недоступен или ремукс не удался — файл сохраняется как `.webm` без тегов, а метаданные пишутся в соседний `<name>.json` (sidecar). Библиотека в этом случае содержит файлы без тегов — помечаем в UI бейджем `UNTAGGED`.
  - `ffmpeg` вызывается с `-c copy` и без видео; пережатия в проекте нет вообще.
- Метаданные для тегов: `artist`, `title`, `album` из поля `track`/`album` в `-J`-выводе, иначе из `uploader`/`channel` и заголовка.
- ISRC нет → дедуп по match_key + длительность.
- Путь к `yt-dlp` настраивается; при отсутствии — источник помечается недоступным с понятной диагностикой. То же для `ffmpeg` (только когда реально нужен ремукс).

### 8.4 Spotify (только метаданные, `rspotify` 0.16.1)

**Этап 1 — только `client_credentials`**: поиск и метаданные, без пользовательской авторизации. **Этап 2 — user-auth** (Authorization Code + PKCE), открывает пользовательские плейлисты.

- `client_credentials`: `GET /v1/search?q=&type=track,album,artist`, `GET /v1/tracks/{id}`, `GET /v1/albums/{id}` — работают.
- **Используемые поля ответа трека** (`GET /v1/tracks/{id}`): `name` (название трека), `artists[].name` (исполнители), `album.name` (альбом), `album.release_date` (дата выхода альбома), `track_number` (номер трека), `duration_ms` (длительность), `external_ids.isrc` (ISRC). Этих полей достаточно, чтобы собрать `Release`/`Candidate` и эталон дедупа; остальные поля ответа не используются.
- Плейлисты: **только с user-auth** (Authorization Code + PKCE), client_credentials даёт 403. Открывается в этапе 2; до этого пункт в UI помечен как требующий авторизации.
- Роль в проекте: каталог (что вообще существует), ISRC (эталон для дедупа), обложки, дата выпуска, длительность. Аудио не скачивается (DRM).
- Spotify-трек используется как «эталон описания»: найденный по ISRC файл с других площадок получает от Spotify нормализованные `title/artist/album/isrc`, даже если сам источник ISRC не отдал.

**Семантика состояния авторизации (`AuthState`).** Значения: `NotNeeded`, `Missing`, `Authenticated`, `Expired`.

- **Режим client_credentials**: если нет `client_id` или `client_secret` в секции `[spotify]` файла `credentials.toml` → `Missing`; иначе → `Authenticated`.
- **Режим user-auth**: если нет `refresh_token` → `Missing`; если токен отозван или истёк → `Expired`; иначе → `Authenticated`.

**Особый случай: HTTP 200 с телом `{"error":"invalid_client"}`.** Spotify отвечает HTTP 200, но тело говорит об отказе ключей. Это **не успех**: состояние переводится в `Expired`, в UI и в лог выводится «ключи Spotify отклонены», и источник считается неавторизованным (запросы не повторяются в цикле — только один повтор после обновления токена).

### 8.5 Qobuz (этап 2, `qobuz-api` 4.0.0)

- Авторизация: e-mail + пароль + `app_id` + `app_secret`.
- **Ключи `app_id`/`app_secret` берутся из секции `[qobuz]` в `credentials.toml`.** Если пары там нет — она **скрейпится** из `play.qobuz.com/bundle.js` регуляркой `production:{api:{appId:"(\\d{9})",appSecret:"(\\w{32})"` и **сохраняется в `credentials.toml`** для последующих запусков. Повторный скрейп делается **только при ошибке авторизации** (ключи протухли), а не при каждом старте: пока в конфиге есть рабочая пара, источник её не трогает.
- Форматы: `format_id` 5 = MP3-320, 6 = FLAC-CD, 7 = FLAC-24≤96, 27 = FLAC-24>96.
- **Потолок качества = минимум из двух ограничений**: максимального формата конкретного трека и потолка аккаунта. Без подписки Studio потолок аккаунта — **FLAC-CD, `format_id` 6**; со Studio — до `format_id` 27. То есть для трека, у которого максимум `format_id` 7, аккаунт без Studio всё равно получит 6. В UI показывается честно, какой именно потолок сработал.
- API-ответ содержит метаданные трека (исполнитель, название, альбом, ISRC, длительность) и ссылку на файл с подписью.
- Формат подписи и шифрование трека — внутри крейта `qobuz-api`; если крейт не справится, реализация переносится в `sources/qobuz.rs` с собственным кодеком.
- Блокер на этой машине: `api.qobuz.com` не резолвится DNS. Источник отключается с понятным статусом, остальные работают.

### 8.6 Tidal (этап 3, `tidalrs` 0.6.2)

- Авторизация: OAuth2. **`client_id`/`client_secret` берутся ТОЛЬКО из секции `[tidal]` в `credentials.toml` — встроенной запасной пары нет.** Если секции или пары нет — источник отключён со статусом «нет учётных данных Tidal»; остальные источники работают как обычно.
- Ключи периодически протухают (issue yaronzz#1213, «all API keys broken as of 2026-03-21»), поэтому в Sources — статус «ключ протух» с подсказкой, что делать, и в лог — понятная диагностика.
- Качество: `quality` 0/1 = AAC, 2 = LOSSLESS FLAC, 3 = HI_RES. **Без подписки HiFi доступен только AAC** — источник даёт lossy и ISRC.
- Метаданные: исполнитель, название, альбом, ISRC, длительность, обложка.
- Блокер: `openapi.tidal.com` и `login.tidal.com` отдают CloudFront 403 «Request blocked».

### 8.7 Deezer (этап 3, `deezer-client` 0.1.4)

- **Крейты:** `deezer-client` 0.1.4, `blowfish` 0.10, `aes` 0.9, а также `md-5` 0.10 — последний нужен именно для деривации ключа.
- Авторизация: cookie `arl` из браузера.
- Дальше — поиск по каталогу (артист/альбом/трек), метаданные с ISRC и длительностью.
- Скачивание: трек зашифрован Blowfish-CBC, ключ выводится из идентификатора трека и `md5`-суммы. Дешифровка — либо в крейте, либо своя реализация в `sources/deezer.rs`.
- **Деривация ключа:** берутся первые 16 hex-символов MD5 от строки `"<id>" + secret`, где `id` — идентификатор трека, а `secret` — значение из секции `[deezer]` в `credentials.toml`. Полученные 16 hex-символов разбираются в 8 байт и используются как ключ Blowfish-CBC с 16-байтным IV.
- **Без подписки HiFi отдаётся MP3 128** — lossy, в ранжировании уступает любому lossless.
- Блокер: `api.deezer.com` — DNS резолвится, но TCP-соединение не устанавливается.

### 8.8 Beatport (этап 3, каталог + купленное)

- Каталог: OAuth2 `client_credentials`. **`client_id`/`client_secret` берутся ТОЛЬКО из секции `[beatport]` в `credentials.toml` — опубликованной пары по умолчанию нет.** Если секции или пары нет — источник отключён со статусом «нет учётных данных Beatport»; остальные источники работают.
- Запросы каталога: `/v4/catalog/search/?q=&type=tracks|releases`, `/v4/catalog/releases/{id}/`, `/v4/catalog/releases/{id}/tracks/`, `/v4/catalog/tracks/{id}/`.
- **Релизы:** `GET /v4/catalog/releases/{id}/` — метаданные релиза (название, лейбл, дата выхода), `GET /v4/catalog/releases/{id}/tracks/` — его треклист. Эти два эндпоинта нужны, чтобы по результату поиска разрешить релиз и получить список его треков (трек-релизация → треки релиза), а не полагаться только на поиск по трекам.
- Поля трека: `name`, `mix_name`, `artists[].name`, `length_ms`, `bpm`, `key`, `isrc`, `price`, `release.name`, `release.label.name`, `publish_date`, `catalog_number`, `sample_url` (превью mp3).
- **Каталог не отдаёт `bit_depth`/`sample_rate`/`formats`** для анонимного токена — качество известно только после скачивания (probe), поэтому такие кандидаты ранжируются ниже известных.
- Скачивание купленного: user-auth — `POST /v4/auth/login/` → `/v4/auth/o/authorize/?response_type=code&client_id=&redirect_uri=https://api.beatport.com/v4/auth/o/post-message/` → код из `Location` → `POST /v4/auth/o/token/` c `grant_type=authorization_code` → `GET /v4/catalog/tracks/{id}/download/`. **Без владения треком — 403**, поэтому скачивается только уже купленное.

### 8.9 SoundCloud (этап 1б, `api-v2.soundcloud.com`)

Проверено на машине 2026-11-10 (`curl` + `yt-dlp`).

**Доступность:** `soundcloud.com` — 200; `api-v2.soundcloud.com` — 401 без `client_id`; `api.soundcloud.com` и `w.soundcloud.com` — 403. Значит, без `client_id` нельзя ничего.

**`client_id`.** Публичный 32-символьный `client_id` добывается из JS-бандла SoundCloud. `yt-dlp` делает это регуляркой `client_id\s*:\s*"([0-9a-zA-Z]{32})"` по скриптам со страницы `https://soundcloud.com/` (в порядке, обратном порядку объявления) и кэширует результат. На проверенной машине yt-dlp 2026.10.29 получил `iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz` и положил его в `~/.cache/yt-dlp/soundcloud/client_id.json`.

**maxfetch делает то же самое сам**, без зависимости от кэша yt-dlp:

1. GET `https://soundcloud.com/` → собрать `src` всех `<script>`.
2. Обойти в обратном порядке, загрузить каждый JS, применить регекс `client_id\s*:\s*"([0-9a-zA-Z]{32})"`.
3. Найденный `client_id` сохранить в `credentials.toml` (секция `[soundcloud]`), при отсутствии — перескрейпить.

`client_id` публичный и ротируется; при ответе 401/403 источник перескрейпит его сам и повторит запрос. Это тот же паттерн, что у Qobuz (`app_id`/`app_secret`), только проще.

**Эндпоинты (проверены, все с `?client_id=`):**

| Запрос | Ответ | Использование |
|---|---|---|
| `GET /tracks/{id}` | 200 | метаданные трека |
| `GET /users/{id}` | 200 | профиль артиста |
| `GET /users/{id}/trackings?limit=…` | 401 | треки артиста — **нужен OAuth** |
| `GET /tracks/{id}/download` | 401 | оригинал — **нужен OAuth** |
| `GET /playlists/{id}/sets` | 404 | плейлисты по старой схеме — не работает |

**Профиль артиста** берётся из `GET /users/{id}` через `api-v2` (с `?client_id=`), а не только через `yt-dlp`. Используемые поля: `username`, `permalink`, `followers_count`, `city`, `country` — из них собирается `Release` артиста (имя, ссылка, для отображения — статистика и география). `yt-dlp` остаётся запасным путём для страниц артистов и плейлистов, где `api-v2` требует OAuth.

**Метаданные трека** (`GET /tracks/{id}`): `id`, `title`, `duration` (мс), `created_at`, `last_modified`, `genre`, `license`, `permalink_url`, `artwork_url`, `publisher_metadata` (**`isrc`, `artist`, `publisher`, `writer_composer`**), `label_name`, `user{ id, username, permalink }`, `media.transcodings[]`.

**ISRC есть** — в `publisher_metadata.isrc` (проверено: `DEP960300042`). Это заметный выигрыш: SoundCloud даёт третий источник ISRC наряду с Spotify и Beatport, и работает без учётной записи.

**Форматы аудио.** `media.transcodings[]` для трека выше:

| preset | protocol | mime | качество |
|---|---|---|---|
| `aac_160k` | hls | `audio/mp4; codecs="mp4a.40.2"` | sq |
| `aac_96k` | hls | `audio/mp4; codecs="mp4a.40.2"` | lq |

`yt-dlp -F` на этой же дорожке показал ровно два формата: `hls_aac_96k` и `hls_aac_160k` (M3U8, 160 kbps AAC). То есть **без авторизации SoundCloud даёт только `aac_160k` и `aac_96k`** — оба lossy.

**Оригинальный файл (lossless) требует авторизации.** В треке есть `downloadable: true` и `has_downloads_left: true`, но `GET /tracks/{id}/download` без OAuth даёт 401. `yt-dlp` это подтверждает: «Original download format is only available for registered users». Оригинал у загрузчика — это прогрессивный поток (`http_mp3`/`http_opus`/`http_aac`/`progressive`), то есть в исходном битрейте автора, который может быть FLAC/WAV у lossless-релиза. **Доступ к lossless-оригиналу (`http_mp3`/`http_opus`/`http_aac`/`progressive`) возможен только при наличии OAuth-токена**; без него доступны исключительно lossy-пресеты `aac_160k` и `aac_96k`.

**Lossless-оригинал и AAC-кандидат — разные кандидаты.** Локатор SoundCloud несёт **идентификатор трека и формат** (`track_id` + preset/протокол), поэтому lossless-оригинал и AAC-кандидат не сливаются в один кандидат и не конкурируют за один и тот же файл. При автовыборе **lossless-оригинал приоритетнее** (§7.4), а lossy-кандидат **остаётся видимым в списке** — как fallback, если оригинал недоступен или отбракован по фактическому качеству (probe).

**Вывод по качеству:** SoundCloud в MVP без авторизации — это lossy-кандидат (`aac_160k`/`aac_96k`) с ценным ISRC. Если пользователь авторизуется (этап 2), тот же трек может дать оригинал.

**Выбор реализации — гибрид.** `yt-dlp` уже умеет SoundCloud (extractor `soundcloud.py`): логин по OAuth-токену (`--username oauth --password <oauth_token>`), оригинальный файл для зарегистрированных, страницы плейлистов и артистов. Но его JSON-вывод — чужой формат, а нам нужен единый `Release` для всех площадок и надёжный ISRC.

Поэтому:

- **Основной путь — собственный клиент `api-v2`** (`sources/soundcloud.rs`): даёт `publisher_metadata.isrc`, точный `duration`, `user`, `media.transcodings[]` в нашем формате `Release`. Работает без учётной записи.
- **Запасной путь — `yt-dlp`**: оригинальный lossless-файл при наличии OAuth, плейлисты (`yt-dlp -J --flat-playlist` работает: проверено, плейлист «Soulhack» отдал 11 записей) и страницы артистов, где `api-v2` требует OAuth.

Оба пути дают один и тот же `Release` — расхождение в структуре не доходит до ядра.

**Вход для SoundCloud:** URL трека, URL артиста, URL плейлиста, текстовый запрос. Треки артиста без OAuth недоступны через `api-v2` (`/users/{id}/trackings` → 401) — берём их через `yt-dlp`.

### 8.10 Матчинг кандидатов между источниками

Релиз из разных источников сопоставляется по: точному ISRC → match_key → (match_key-подобные имена + длительность в пределах допуска `MatchingConfig::duration_tolerance_sec`, по умолчанию ±2 с). При объединении релизы берут лучшие метаданные из Spotify (если доступен), иначе из источника с самыми полными полями.

---

## 9. Поток выполнения

1. Разобрать вход → `Query`.
2. Для каждого включённого источника параллельно: `search`/`resolve` → `Release`. Список включённых берётся из `registry()` и отбирается функцией `enabled_in(ids, config)` (§7.3) — реестр сам по себе не фильтрует. Источник, который не авторизован или недоступен по сети, пропускается с записью в журнал — он не ломает запуск.
3. Сопоставить релизы между источниками → группы.
4. Для каждой группы: собрать `Candidate` из всех источников, отранжировать по правилу §7.4, отфильтровать lossy при наличии lossless.
5. Показать в TUI / выдать в JSON (headless).
6. По команде: скачать выбранные кандидаты.
7. Скачать → `.part` → probe → (для YouTube: ремукс в `.opus` без пережатия) → теги (lofty) → дедуп в БД (транзакция, без удаления файлов) → rename в конечный путь.
8. Если проигравший экземпляр помечен как дубль — он остаётся на диске и ждёт явной команды `library dedupe --apply --delete`.

---

## 10. Тестирование

- **Юнит**: ранжирование качества (все комбинации), нормализация имён, санитизация путей, шаблоны, match_key, порог длительности.
- **Фикстуры HTTP**: записи ответов Bandcamp mobile API, `statdownload`-цепочки, `fancollection`, Spotify API, SoundCloud `api-v2` (трек, артист, `media.transcodings`), Qobuz API, Tidal API, Deezer API, Beatport v4 — реплей через локальный mock-сервер, без сети.
- **Кодеки**: дешифровка Deezer (Blowfish-CBC) и подпись Qobuz — юнит-тесты на сохранённых бинарных фикстурах с ожидаемым выходом.
- **Интеграция**: сценарий «трек есть на Soulseek (24/96) и YouTube (opus)» → выбирается Soulseek, lossy отбрасывается; сценарий «трек только на YouTube» → opus принимается с пометкой lossy (после ремукса в `.opus`, теги на месте); сценарий «тот же ISRC с двух источников, второй хуже» → остаётся лучший, проигравший помечен `duplicate_of`, **файл не удалён**.
- **Дедуп**: `library dedupe` без флагов ничего не удаляет; с `--apply --delete --yes` удаляет ровно помеченные; файл с `conflict` в список не попадает.
- **Ремукс**: `ffmpeg -c copy` на фикстуре webm → opus с тем же битрейтом и без потерь (сравнение длительности и размера потока).
- **Soulseek**: кандидат без свободных слотов отбрасывается; зависшая передача переключается на следующего кандидата.
- **SoundCloud**: скрейп `client_id` с фикстуры HTML+JS; при 401 источник перескрейпит `client_id` и повторит запрос; ISRC извлекается из `publisher_metadata`.
- **E2E-набор** — за cargo-фичей `e2e`, не в CI по умолчанию. Набор разбит по источникам на отдельные файлы: `tests/e2e.rs` (этап 1: Soulseek, YouTube, Spotify-каталог), `tests/e2e_stage1b.rs` (Bandcamp, SoundCloud), `tests/e2e_stage2.rs` (Qobuz, Spotify user-auth, Bandcamp купленное). Каждый набор проверяет сценарий **«поиск → fetch → library scan → library dedupe --dry-run»** на локальном mock-сервере (без сети). Источники этапа 3 (Tidal, Deezer, Beatport) получают такой же отдельный набор вместе со своим этапом. Для каждого источника — отдельный тест с `--source <id>`; источник, недоступный по сети, помечается `skipped`, а не `failed`.

---

## 11. Что остаётся за строителем

Решения, которые я принимаю сам и фиксирую здесь:

- Лицензия: MIT (короче, совпадает с духом «своё, можно форкнуть»).
- Имя крейта: `maxfetch`; бинарник: `maxfetch`.
- Позиция артефактов: бинарник — в `target/release/`, распространяется отдельно, без установщика на первом этапе.
- Формат логов: `tracing` + `tracing-subscriber`, в TUI — свой буфер, в CLI — stderr.
- HTTP-клиент: `reqwest` с `rustls-tls` (не OpenSSL), чтобы не тянуть системные библиотеки.
- Кэш метаданных: **таблица `releases` в БД** (`state.db`), ключ — `match_key`, TTL 7 дней (`expires_at`); кнопка «обновить» в TUI.
- **Единица работы — трек.** Альбом — группировка и треклист, но вся оркестрация (поиск, ранжирование, дедуп, отчёт) ведётся по трекам. Bandcamp отдаёт альбом ZIP'ом — это транспорт: архив распаковывается, и каждый трек из него становится отдельным кандидатом. Частичный успех (3 трека из 12 скачались) — норма, не ошибка: результат фиксируется по трекам, а не по альбому.
- **Удаление дублей — только явной командой** (§7.6), без карантина, `scan` только индексирует.
- **Soulseek — клиент только на скачивание**, без шаринга и входящего порта (§8.1).
- **YouTube приводится к `.opus` ремуксом без пережатия** (§8.3); при недоступном `ffmpeg` — `.webm` + sidecar `.json`.
- **SoundCloud — гибрид**: свой клиент `api-v2` как основной путь, `yt-dlp` как запасной для оригинального lossless-файла и плейлистов (§8.9).
- **Состав MVP = все этапы (1, 1б, 2, 3)** (§6.6). Поставка инкрементами: каждый этап — рабочая собираемая версия, публичный интерфейс (trait `Source`, схема БД, экраны TUI, команды CLI) не меняется между этапами.
- Таймаут зависшей передачи: `stall_timeout_sec` = 30 с, порог настраивается в конфиге.
- Настройки без второго tier-а: полосы прокрутки в результатах и кандидатах, подтверждение удаления в TUI, показ фактического качества в колонке «качество».

---

## 12. Риски

| Риск | Что делаем |
|---|---|
| Ротация client_id (Beatport, Tidal, Spotify) | Ключи только в `credentials.toml`, не зашиты; в UI видно «ключ протух»; диагностика вместо паники |
| WAF/гео-блокировки площадок | Источник изолирован: его недоступность не ломает остальные; статус виден в TUI |
| Bandcamp меняет структуру `data-tralbum` | Парсинг вынесен в один модуль, версия API (24/25) централизована |
| `yt-dlp` отстаёт от YouTube | Версию `yt-dlp` проверяем при старте и предупреждаем |
| Большая библиотека | SQLite WAL + индексы; операции дедупа в одной транзакции |
| Рост числа источников | Трейт + реестр; если понадобится, выносим в отдельные крейты без ломки API |
| Юридические риски площадок | Документируем в README, ответственность на пользователе; Qobuz/Tidal/Deezer — в MVP, но их включение и выключение — на пользователе |
| Ключи Spotify/Tidal/Beatport протухают или отклоняются | Ключи живут только в `credentials.toml` и не зашиты; в Sources видно «ключ протух»; можно задать свои; источник отключается без паники |
| `qobuz-api` не покрывает нужный формат/подпись | Реализация переносится в `sources/qobuz.rs` со своим кодеком; интерфейс `Source` не меняется |
| Deezer-дешифровка Blowfish не совпадает с текущим форматом | Свой кодек в `sources/deezer.rs` + тест-фикстура на реальном треке |
| Beatport не отдаёт битность/частоту | Такие кандидаты ранжируются ниже известных; качество уточняется probe'ом после скачивания |
| `ffmpeg` может отсутствовать в системе | YouTube-файлы останутся `.webm` без тегов + sidecar `.json`; в Sources видно «нет ffmpeg» |
| `soulseek-rs-lib` не умеет докачку с середины (протокол её не поддерживает) | При зависании незавершённый файл удаляется, загрузка начинается заново; лимит попыток ограничен, чтобы не крутить цикл |
| Пользователь случайно удалит нужный файл через `dedupe --delete` | Перед удалением показываем точный список; `conflict`-файлы исключены; требуется `--yes` |
| Плагин YouTube отдаёт не Opus, а другой кодек | `ffmpeg -c copy` переносит поток как есть; имя расширения определяется по фактическому кодеку, а не по ожиданию |
| `client_id` SoundCloud протухает | Перескрейп с `soundcloud.com` при 401/403, повтор запроса; значение кэшируется в конфиге |
| Меняется структура `media.transcodings` | Доступ к аудио идёт по полям `preset`/`format.protocol`/`url`, а не по индексам массива |
