# pacnix — техническое задание / концепция проекта

## 1. Кратко

**pacnix** — пакетный менеджер и единый слой управления программным обеспечением для Arch Linux.

Начальная задача проекта предельно практична:

> объединить удобство `paru`/AUR и Nix так, чтобы пользователь мог устанавливать, искать, обновлять и удалять ПО через один интерфейс, а pacnix помнил, откуда и каким способом оно было установлено.

Проект не должен становиться «Arch для новичков», отдельным дистрибутивом или заменой pacman/Nix. Его задача — закрыть UX-разрывы между уже существующими сильными инструментами, не скрывая нижние слои и не отнимая у пользователя контроль.

Базовый принцип:

> **убирать необходимость помнить лишнее, но не убирать возможность увидеть всё.**

---

## 2. Основные цели

### 2.1. MVP

Первая рабочая версия должна:

1. Работать на Arch Linux.
2. Использовать ALPM/pacman как основной системный backend.
3. Поддерживать AUR.
4. Поддерживать Nix как второй источник/способ установки.
5. Иметь единый resolver пакетов.
6. Помнить выбранный источник и происхождение установленного объекта.
7. Поддерживать pacman-подобный и человекочитаемый CLI.
8. Не связывать core-логику с CLI, чтобы позже без переписывания добавить TUI, GUI и MCP.
9. Хранить собственное дополнительное состояние в SQLite через `rusqlite`.
10. Не дублировать базы pacman/ALPM или Nix.

### 2.2. Долгосрочная цель

**Статус: POST-MVP / архитектурный вариант. Не входит в требования к первой версии.**

Постепенно превратить pacnix в единый management plane для установленного ПО:

- системные пакеты;
- AUR;
- Nix;
- GitHub releases;
- AppImage;
- Flatpak;
- Steam;
- Wine/Proton;
- Python;
- Ollama;
- другие экосистемы через плагины.

При этом **pacman и Nix остаются базой проекта и основными системными backend'ами**.

## 2.3. Статусы требований

Чтобы не смешивать текущую разработку и возможное развитие, в документе используются два статуса:

- **MVP** — то, что требуется для первой самостоятельной полезной версии pacnix.
- **POST-MVP / архитектурный вариант** — сейчас **не реализуется**. Архитектура MVP лишь не должна делать такую возможность заведомо невозможной или требовать полного переписывания core.

Если раздел помечен как `POST-MVP`, это **не требование к первой версии**, не blocker релиза и не основание заранее строить лишнюю инфраструктуру.

Принцип разработки:

> сначала `ALPM/AUR + Nix + rusqlite + unified resolver`; остальное добавляется только по реальной необходимости.

---

## 3. Что pacnix НЕ должен делать

На старте pacnix не должен:

- становиться отдельным дистрибутивом;
- заменять `/usr/bin/pacman`;
- реализовывать собственную dependency model поверх ALPM/Nix;
- пытаться сделать смешанные ALPM/Nix-транзакции «атомарными»;
- считать Nix-пакет допустимым удовлетворением зависимости Arch-пакета;
- скрывать происхождение пакета;
- автоматически доверять AUR;
- писать вручную интеграции со всеми возможными экосистемами;
- включать TUI, GUI, MCP, sandbox-builder, Flatpak, AppImage и plugin marketplace в MVP.

---

# 4. Архитектурные принципы

## 4.1. Core не знает про CLI

Главный инвариант:

```text
CLI → core
TUI → core
GUI → core
MCP → core

core ↛ CLI
```

В core запрещается:

- прямой `println!` для UX;
- чтение stdin;
- интерактивные вопросы пользователю;
- предположение, что frontend — терминал.

Core возвращает структурированные данные, планы и запросы на взаимодействие.

Пример:

```rust
enum Interaction {
    SelectCandidate(Vec<Candidate>),
    Confirm(TransactionPlan),
    RequestPrivilege(PrivilegeRequest),
}
```

CLI может показать список и прочитать номер.
TUI — открыть список.
GUI — показать карточки.
MCP — вернуть структурированный запрос агенту.

---

## 4.2. Backend отвечает только за свою экосистему

Базовая модель:

```rust
trait PackageBackend {
    fn search(&self, query: &str) -> Result<Vec<Candidate>>;
    fn installed(&self) -> Result<Vec<InstalledPackage>>;
    fn plan_install(&self, target: &Candidate) -> Result<TransactionPlan>;
    fn plan_remove(&self, target: &InstalledPackage) -> Result<TransactionPlan>;
    fn plan_upgrade(&self, target: &InstalledPackage) -> Result<TransactionPlan>;
}
```

Начальные backend'ы:

```text
ALPM
AUR
Nix
```

Позже:

```text
Flatpak
AppImage
Steam
Python
Wine
Ollama
...
```

---

## 4.3. Resolver и backend — разные сущности

Resolver отвечает на вопрос:

> **что имел в виду пользователь и откуда это можно получить?**

Backend отвечает на вопрос:

> **как этим управлять в конкретной экосистеме?**

Пример:

```text
pacnix install hiddify
        ↓
resolver
        ↓
1. extra/hiddify
2. chaotic-aur/hiddify
3. aur/hiddify-bin
4. nixpkgs#hiddify
        ↓
выбор пользователя
        ↓
backend
```

---

## 4.4. Зависимости остаются backend-local

Нельзя делать:

```text
AUR package requires libfoo
→ satisfy it with nixpkgs#libfoo
```

Если AUR/ALPM-пакет требует Arch dependency, её разрешает Arch/ALPM/AUR-стек.

Если Nix closure требует dependency, её разрешает Nix.

Это принципиально упрощает архитектуру и не ломает модель каждой системы.

---

# 5. CLI

## 5.1. Два синтаксиса одновременно

pacnix должен поддерживать как pacman-style, так и человекочитаемые команды:

```bash
pacnix -S firefox
pacnix install firefox

pacnix -R firefox
pacnix remove firefox

pacnix -Ss hiddify
pacnix search hiddify

pacnix -Qi firefox
pacnix info firefox

pacnix -Q
pacnix list

pacnix -Syu
pacnix upgrade
```

Оба синтаксиса мапятся в один внутренний enum:

```rust
enum Command {
    Install(Vec<TargetSpec>),
    Remove(Vec<TargetSpec>),
    Search(String),
    Info(TargetSpec),
    ListInstalled,
    Upgrade,
}
```

---

## 5.2. Команда не должна ошибочно становиться именем пакета

Если первый positional совпадает с зарезервированным verb:

```text
install
remove
search
info
list
upgrade
...
```

он всегда интерпретируется как команда.

Если реально нужен пакет с именем `install`:

```bash
pacnix -S install
```

или:

```bash
pacnix install -- install
```

Pacman-style синтаксис остаётся полноценным и одновременно служит escape hatch.

---

## 5.3. Upgrade на Arch должен быть безопасным

Человекочитаемый:

```bash
pacnix upgrade
```

должен соответствовать полной безопасной операции обновления, аналогичной:

```bash
pacman -Syu
```

Не следует поощрять модель `update` = только refresh DB, если это создаёт риск partial upgrade.

---

# 6. Источники и приоритеты

## 6.1. Базовый порядок

По умолчанию:

1. официальные Arch sync repositories;
2. подключённые pacman repositories, включая Chaotic-AUR;
3. AUR;
4. Nix.

Это не обязательно должен быть жёсткий глобальный порядок навсегда: resolver может учитывать точность совпадения, пользовательские предпочтения и прошлый выбор.

---

## 6.2. Несколько кандидатов

Если найдено несколько нормальных вариантов:

```text
1) extra/foo
2) chaotic-aur/foo-bin
3) aur/foo-bin
4) aur/foo-git
5) nixpkgs#foo
```

pacnix показывает нумерованный список и позволяет выбрать вариант.

Выбор можно запомнить:

```text
query "foo"
→ provider aur/foo-bin
```

---

# 7. SQLite / rusqlite

## 7.1. Назначение собственной БД

**Статус: MVP.**

SQLite не является второй базой пакетов и не должна копировать содержимое баз ALPM или Nix.

ALPM остаётся источником истины для Arch.
Nix остаётся источником истины для Nix.
Другие backend'ы также сохраняют собственное authoritative state.

pacnix хранит только собственный слой координации:

- происхождение;
- выбранный provider;
- alias/resolution;
- историю выбора;
- дополнительную metadata, которой нет у backend'а;
- cross-backend identity;
- связь между логическим пакетом и конкретными установленными instances.

### Синхронизация с backend'ами

pacnix должен уметь **самостоятельно пересобирать/обновлять своё представление состояния** из ALPM и Nix:

```text
ALPM state ─┐
            ├─→ reconcile → pacnix.db
Nix state ──┘
```

Синхронизация должна быть reconciliation, а не импортом/копированием чужой БД:

1. прочитать текущее authoritative state backend'а;
2. сопоставить его с уже известными pacnix entities;
3. добавить обнаруженные извне установки;
4. обновить изменившиеся instances;
5. отметить исчезнувшие/удалённые instances;
6. сохранить pacnix-specific provenance и aliases, если они всё ещё применимы.

Это требуется потому, что пользователь всегда может выполнить напрямую:

```bash
pacman -S foo
nix profile install nixpkgs#bar
```

pacnix после sync не должен считать такое состояние «чужим» или ломаться из-за него.

Допустимы как автоматическая синхронизация перед операциями/просмотром состояния, так и явная команда ручного reconcile, например:

```bash
pacnix sync
```

Точное имя команды не фиксируется этим ТЗ.

### Несколько версий одной программы

Модель данных **не должна считать имя пакета уникальной установленной сущностью**.

Особенно для Nix допустимы несколько одновременно существующих instances одного логического пакета, например разных версий или разных flake/profile references.

Нужно различать:

```text
logical package
└── installed instances
    ├── backend
    ├── backend-native identity
    ├── version
    ├── profile / scope
    └── store path / иной стабильный backend reference
```

То есть две версии `foo` должны быть двумя instances одного логического software identity, а не перезаписывать друг друга в SQLite.

---

## 7.2. Минимальная схема MVP

**Статус: MVP.**

Конкретная SQL-схема может меняться, но MVP должен различать логическую сущность и установленный instance.

Один из минимальных вариантов:

```sql
CREATE TABLE packages (
    id            INTEGER PRIMARY KEY,
    canonical_name TEXT NOT NULL
);

CREATE TABLE installed_instances (
    id             INTEGER PRIMARY KEY,
    package_id     INTEGER NOT NULL REFERENCES packages(id),
    backend        TEXT NOT NULL,
    backend_ref    TEXT NOT NULL,
    version        TEXT,
    scope          TEXT,
    installed_at   INTEGER,
    last_seen_at   INTEGER NOT NULL,
    UNIQUE (backend, backend_ref)
);

CREATE TABLE aliases (
    query          TEXT PRIMARY KEY,
    backend        TEXT NOT NULL,
    backend_ref    TEXT NOT NULL
);
```

`backend_ref` — не копия backend database, а минимальная ссылка, достаточная для повторного обнаружения конкретного instance.

Примеры:

```text
ALPM: package database identity / repo+name
Nix:  profile element / flake ref / store path, в зависимости от операции
```

Точный формат `backend_ref` является деталью backend implementation.

Позже могут появиться:

- upstream mappings;
- metadata cache;
- transaction history;
- GitHub identity;
- plugin ownership;
- AppImage metadata;
- provenance;
- build reports.

---

## 7.3. Восстановимость

Удаление `pacnix.db` не должно ломать систему.

В идеале pacnix умеет большую часть состояния обнаружить заново через backend'ы.

Потеряться могут:

- история;
- выбранные aliases;
- дополнительные provenance-данные;
- пользовательские решения.

---

# 8. AUR

## 8.1. Не обязательно форкать Paru целиком

Предпочтительный путь:

- использовать отдельные crates/идеи из экосистемы Paru там, где это удобно;
- не наследовать архитектурную привязку Paru к CLI;
- держать AUR orchestration внутри собственного backend/resolver слоя.

Paru рассматривается скорее как:

- reference implementation;
- источник уже решённых edge cases;
- набор переиспользуемых библиотек.

---

## 8.2. AUR metadata

**Статус: POST-MVP / архитектурный вариант. Не входит в требования к первой версии.**

Позже pacnix должен показывать больше, чем просто PKGBUILD:

- версия;
- maintainer;
- дата обновления;
- upstream;
- source URLs;
- checksums;
- PGP;
- patches;
- install scripts;
- source type;
- насколько package version отстаёт от upstream.

---

## 8.3. Wrapper purity

**Статус: POST-MVP / архитектурный вариант. Не входит в требования к первой версии.**

В одной из будущих версий pacnix **может** сравнивать `-bin` и wrapper-пакеты с upstream artifact. MVP не выполняет такой анализ.

Пример:

```text
Wrapper purity: CLEAN
Upstream payload: 99.7%
Packaging files:
  .desktop
  icon
  launcher
Code modifications: none
Extra executables: none
Extra network sources: none
```

Или:

```text
Wrapper purity: MODIFIED
+ 2 application-code patches
+ npm install
+ binary helper not present upstream
+ extra source repository
```

Правило:

> wrapper может быть отдельным репозиторием и оборачивать upstream, но не должен молча добавлять функциональный код, не связанный с упаковкой.

---

# 9. Nix

## 9.1. Назначение

Nix используется как:

- альтернативный package backend;
- способ устанавливать ПО, которое неудобно или нежелательно тащить через AUR;
- изолированный способ управления сторонними бинарными артефактами;
- в будущем — временные toolchain environments для агентов/сборок.

---

## 9.2. Реализация MVP

Высокоуровневые операции можно сначала реализовать адаптером вокруг структурированного вывода `nix`:

```text
search
profile install
profile remove
profile upgrade
flake metadata/eval
```

Внутренний интерфейс должен позволять позже заменить реализацию без изменения resolver/core.

---

## 9.3. В будущем

**Статус: POST-MVP / возможная замена реализации backend'а без изменения core API.**

При созревании Rust-реализаций Nix/Tvix можно перейти на нативные Rust API.

То же относится к новому Rust ALPM.

Цель:

```text
pacnix
├── native Rust ALPM
└── native Rust Nix-compatible backend
```

но **проект не должен ждать их готовности для старта**.

---

# 10. GitHub / URL target resolver

**Статус: POST-MVP / архитектурный вариант. Не входит в требования к первой версии.**

## 10.1. URL как TargetSpec

**Статус: POST-MVP / архитектурный вариант. Не входит в требования к первой версии.**

pacnix должен архитектурно позволять:

```bash
pacnix -S https://github.com/foo/bar
```

Также возможные будущие формы:

```bash
pacnix -S github:foo/bar
pacnix -S github:foo/bar@v1.8.2
```

---

## 10.2. Resolver GitHub-проекта

**Статус: POST-MVP / архитектурный вариант. Не входит в требования к первой версии.**

В будущей версии для GitHub repository может использоваться следующий resolver flow:

```text
repo URL
  ↓
identify upstream
  ↓
check Arch repos
  ↓
check configured repos
  ↓
check AUR
  ↓
check Nix
  ↓
inspect GitHub Releases
  ↓
offer candidates
```

Например:

```text
Found project: foo/bar

1) extra/bar
2) chaotic-aur/bar-bin
3) aur/bar-bin
4) nixpkgs#bar
5) GitHub release
6) build from source
```

GitHub URL становится сильным upstream identity и помогает связывать разные пакеты с одним проектом.

---

## 10.3. README/install docs как resolver hints

**Статус: POST-MVP / архитектурный вариант. Не входит в требования к первой версии.**

В будущей версии pacnix может кэшировать README/install docs и извлекать installation hints.

Это **не часть MVP**. Если функция будет реализовываться, возможный минимальный первый вариант:

1. распарсить Markdown;
2. извлечь fenced shell blocks;
3. распознать известные команды.

Например:

```bash
pacman -S foo
paru -S foo-bin
yay -S foo-bin
nix profile install nixpkgs#foo
pip install foo
cargo install foo
flatpak install ...
```

Это не authoritative truth, а дополнительный сигнал.

Если upstream сам пишет:

```bash
yay -S foo-bin
```

это повышает уверенность, что найденный AUR package соответствует данному upstream.

---

# 11. Privilege escalation

## 11.1. Нельзя жёстко зависеть от sudo

Core должен оперировать абстракцией:

```rust
trait PrivilegeProvider {
    fn available(&self) -> bool;
    fn elevate(&self, operation: PrivilegedOperation) -> Result<()>;
}
```

Базовые providers:

```text
sudo
sudo-rs
```

Позже:

```text
polkit
```

---

## 11.2. Конфигурация

Пример:

```toml
[privilege]
provider = "auto" # auto | sudo | sudo-rs
```

CLI override:

```bash
pacnix --privilege sudo-rs -S foo
```

Если процесс уже запущен от root — elevation не нужна.

---

## 11.3. GUI/MCP

**Статус: POST-MVP / архитектурный вариант. Не входит в требования к первой версии.**

Пароль никогда не должен передаваться LLM/MCP.

При privileged transaction:

```text
agent / frontend
      ↓
pacnix plan
      ↓
requires privilege
      ↓
local auth dialog / polkit
      ↓
authorized operation
```

---

# 12. MCP / агентный интерфейс — позже

**Статус: POST-MVP / архитектурный вариант. Не входит в требования к первой версии.**

## 12.1. MCP — ещё один frontend

MCP не должен иметь отдельную package-management логику.

```text
CLI ┐
TUI ├──→ pacnix-core
GUI ┤
MCP ┘
```

---

## 12.2. Разделение plan/apply

Агенту лучше дать API уровня:

```text
search
resolve
plan_install
apply
```

а не unrestricted shell.

---

## 12.3. Toolchain requests

В будущем агент должен иметь возможность запросить не конкретный package name, а capability:

```rust
ToolchainRequest {
    executables: ["clang", "cmake"],
    pkg_config: ["openssl"],
    libraries: ["libclang.so"],
}
```

pacnix сам определяет, какими пакетами это удовлетворить.

Главный принцип:

> **агент знает, какая capability нужна; pacnix знает, каким пакетом её обеспечить.**

---

## 12.4. Временные окружения

Для одноразовой сборки pacnix может предпочесть временный Nix environment вместо постоянной системной установки.

Пример:

```text
Need:
  rust
  clang
  cmake
  protobuf

Options:
  system     → ALPM
  temporary  → Nix environment
```

---

# 13. AUR sandbox builder — дальняя перспектива

**Статус: POST-MVP / архитектурный вариант. Не входит в требования к первой версии.**

## 13.1. Модель

AUR PKGBUILD — недоверенный код.

В будущем сборка должна происходить примерно так:

```text
PKGBUILD
  ↓
static inspection
  ↓
ephemeral rootless Podman container
  ↓
install build deps
  ↓
fetch/build/package
  ↓
*.pkg.tar.zst
  ↓
artifact verification
  ↓
container dies
  ↓
ALPM installs artifact
```

Root на хосте нужен только для финальной системной установки.

---

## 13.2. Сеть

Весь egress pod/container должен идти через контролируемый proxy/firewall.

Цели:

- видеть hostname/IP;
- запрещать host/LAN/private ranges;
- логировать undeclared destinations;
- выдавать granular allow/deny;
- ограничивать произвольную сеть во время build.

Не обязательно MITM'ить TLS.

---

## 13.3. Honeypot home

Настоящий `$HOME` пользователя не монтируется вообще.

В контейнере может создаваться синтетический правдоподобный home:

```text
~/.ssh/
~/.config/gh/
~/.gitconfig
~/.cargo/
...
```

Содержимое — полностью искусственное, но структурно правдоподобное.

Цель honeypot — дополнительная поведенческая детекция стилеров/малвари.

Безопасность **не должна зависеть от honeypot**: даже если малварь его распознаёт, настоящих секретов в sandbox нет.

---

## 13.4. Container не считается панацеей

Container escape принципиально возможен, так как kernel общий.

Sandbox — дополнительный защитный слой, а не абсолютная security boundary.

---

# 14. systemd и cgroups — позже

**Статус: POST-MVP / архитектурный вариант. Не входит в требования к первой версии.**

pacnix в будущем может связывать package ownership с runtime-состоянием.

Пример:

```text
foo.service
  active
  runs as: root
  starts: boot
  triggered by: foo.socket

  cgroup:
    memory: 184 MiB
    cpu: 0.7%
    tasks: 12
```

Полезные данные:

- unit files;
- enabled/disabled;
- socket/timer/path activation;
- processes;
- cgroups;
- memory/cpu/tasks;
- journal;
- security/hardening.

Важно показывать реальную активацию, а не только `enabled/disabled`.

---

## 14.1. Least privilege

В перспективе pacnix может предлагать systemd drop-in hardening:

```text
NoNewPrivileges
PrivateTmp
ProtectSystem
ProtectHome
RestrictSUIDSGID
resource limits
```

и cgroup policy:

```text
MemoryHigh
MemoryMax
TasksMax
CPUWeight
IOWeight
```

Но это **не MVP**.

---

# 15. Plugin API — дальняя, но важная точка расширения

**Статус: POST-MVP / архитектурный вариант. Не входит в требования к первой версии.**

## 15.1. Зачем

Нельзя вручную писать core-интеграции для:

- Steam;
- Wine;
- Python;
- Ollama;
- Cargo;
- npm;
- Flatpak;
- десятков других экосистем.

Вместо этого нужен plugin API.

---

## 15.2. Общая модель

Плагин объясняет pacnix:

- как обнаружить managed objects;
- как искать;
- как устанавливать;
- как удалять;
- как обновлять;
- какие metadata доступны.

Пример:

```rust
trait Provider {
    fn detect(&self) -> Vec<Installation>;
    fn search(&self, query: &str) -> Vec<Candidate>;
    fn plan_install(&self, target: TargetSpec) -> Plan;
    fn plan_remove(&self, target: TargetSpec) -> Plan;
    fn plan_update(&self, target: TargetSpec) -> Plan;
}
```

---

## 15.3. Возможные plugin providers

```text
steam
python
wine
ollama
cargo
npm
flatpak
...
```

Пример единого списка:

```text
steam:620         Portal 2
ollama:qwen3      qwen3
python:ruff       ruff
wine:foobar       FooBar
```

---

## 15.4. WASM как возможный plugin runtime

WASM выглядит подходящим вариантом, потому что:

- plugin не получает полный доступ к процессу pacnix;
- можно выдавать capability-based permissions;
- интеграции обычно небольшие;
- plugin можно ограничить host API.

Пример manifest:

```toml
[permissions]
exec = ["ollama"]
network = ["registry.ollama.ai"]
read = ["~/.ollama"]
write = ["~/.ollama"]
```

---

# 16. AppImage и Flatpak

**Статус: POST-MVP / архитектурный вариант. Не входит в требования к первой версии.**

## 16.1. AppImage

AppImage — **не provider**, а managed artifact format.

Примеры:

```bash
pacnix install ./Foo.AppImage
pacnix install https://example.org/Foo.AppImage
```

pacnix может:

- положить artifact в управляемое место;
- извлечь metadata;
- создать desktop integration;
- записать ownership;
- удалить;
- обновить при наличии стабильного update URL.

Не MVP.

---

## 16.2. Flatpak

Flatpak позже может стать отдельным provider/backend.

Не MVP.

---

# 17. TUI и GUI

**Статус: POST-MVP / архитектурный вариант. Не входит в требования к первой версии.**

Архитектура core должна позволять добавить их без переписывания package logic.

Возможный TUI на ratatui:

```text
┌ Packages ────────┐ ┌ Details ───────────────┐
│ firefox          │ │ source: extra           │
│ syncthing        │ │ upstream: ...           │
│ > foo            │ │ services: ...           │
│                  │ │ provenance: ...         │
└──────────────────┘ └─────────────────────────┘
```

GUI в дальнейшем может стать «software center для людей, которые ненавидят software center»:

- несколько providers;
- upstream;
- wrapper diff;
- source URLs;
- version;
- trust/provenance metadata;
- install/remove/update;
- systemd/runtime info.

---

# 18. Лицензирование

## 18.1. Общая идея

Собственный код pacnix желательно лицензировать:

```text
MIT OR GPL-3.0-or-later
```

При этом проект/собранный продукт на раннем этапе может распространяться по GPL из-за GPL-зависимостей или перенесённого GPL-кода.

Схема:

```text
own pacnix code      MIT OR GPL-3.0-or-later
GPL dependencies     GPL
copied GPL code      GPL
combined binary      distributed under GPL terms
```

Если в будущем GPL-зависимости исчезнут и соответствующий код будет заменён, собственные dual-licensed части уже смогут использоваться permissively.

---

## 18.2. Contributions

В `CONTRIBUTING.md` желательно сразу зафиксировать, что contributions принимаются под той же лицензией:

```text
MIT OR GPL-3.0-or-later
```

Иначе будущая смена dependency stack может упереться в невозможность нормально переиспользовать код permissively.

---

# 19. Предлагаемая структура workspace

```text
pacnix/
├── crates/
│   ├── pacnix-core/
│   │   ├── resolver/
│   │   ├── model/
│   │   ├── transactions/
│   │   ├── storage/
│   │   └── interaction/
│   │
│   ├── pacnix-backend-alpm/
│   ├── pacnix-backend-aur/
│   ├── pacnix-backend-nix/
│   └── pacnix-cli/
│
├── docs/
└── Cargo.toml
```

POST-MVP, только при необходимости:

```text
pacnix-tui/
pacnix-gui/
pacnix-mcp/
pacnix-plugin-sdk/
pacnix-build-sandbox/
```

---

# 20. Реальный порядок разработки

**Разделы Phase 0–4 ниже определяют текущий MVP. Всё, что в документе помечено `POST-MVP`, не должно блокировать его выпуск.**

## Phase 0 — skeleton

- workspace;
- core models;
- command enum;
- backend trait;
- SQLite storage;
- CLI parser;
- privilege abstraction.

## Phase 1 — ALPM

- search;
- installed packages;
- install/remove;
- info;
- upgrade;
- pacman-style compatibility.

## Phase 2 — AUR

- search;
- candidate resolution;
- build/install flow;
- AUR aliases;
- основные edge cases.

## Phase 3 — Nix

- search;
- install/remove;
- installed;
- upgrade;
- pacnix-managed Nix profile;
- запись provenance в SQLite.

## Phase 4 — unified resolver

- объединённый search;
- ranking;
- multiple candidate selection;
- remembered provider;
- human-readable CLI.

После этого pacnix уже должен быть полезным самостоятельным продуктом.

---

# 21. Возможные следующие этапы после MVP

**Статус: POST-MVP / архитектурный вариант. Не входит в требования к первой версии.**

Этот список **не является текущим backlog MVP**. К нему следует возвращаться только после рабочей версии Phase 0–4; порядок определяется реальной потребностью пользователей.

1. GitHub URL resolver.
2. README/install-hint parser.
3. richer package metadata.
4. TUI.
5. provenance/upstream mapping.
6. AppImage lifecycle.
7. Flatpak backend.
8. MCP.
9. plugin API.
10. systemd/cgroups integration.
11. sandboxed AUR builder.
12. GUI.
13. server-side metadata/cache/build infrastructure — только если появится достаточно пользователей.

---

# 22. Главный UX-инвариант

Пользователь должен иметь возможность написать:

```bash
pacnix install foo
```

или:

```bash
pacnix -S foo
```

и получить:

1. что найдено;
2. откуда это можно установить;
3. какой вариант выбран;
4. что изменится в системе;
5. какое повышение привилегий требуется;
6. кто после установки будет владельцем lifecycle;
7. возможность позже понять, откуда эта штука вообще взялась.

При этом опытный пользователь всегда может спуститься ниже:

```text
pacman
PKGBUILD
Nix
systemd
plugin backend
raw metadata
```

pacnix не должен скрывать систему. Он должен **помнить и связывать то, что сейчас обязан помнить человек**.

---

# 23. Одной фразой

> **pacnix — единый пакетный и software-management слой для Arch, построенный вокруг pacman/ALPM и Nix, который помнит происхождение ПО, объединяет resolver и lifecycle разных источников и постепенно закрывает UX-дыры Linux package management без замены нижележащих систем.**
