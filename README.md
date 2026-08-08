<div align="center">

# pacnix

**Единый package/software-management слой для Arch Linux поверх pacman/ALPM, AUR и Nix.**

Ищи, выбирай, устанавливай, удаляй и обновляй ПО через один интерфейс — при этом pacnix помнит, **откуда оно взялось и кто управляет его lifecycle**.

[Спецификация](docs/spec.md) · [Лицензия](LICENSE) · [Как контрибьютить](CONTRIBUTING.md)

</div>

> [!WARNING]
> **Pre-alpha.** Проект находится в очень ранней разработке: интерфейсы, схема БД и поведение могут меняться без совместимости. ALPM/pacman и Nix уже выполняют реальные операции, но AUR execution pipeline пока не реализован. Не стоит использовать pacnix как единственный пакетный менеджер на рабочей системе.

## Зачем

Arch уже имеет отличный системный пакетный менеджер, AUR закрывает огромный пласт community packaging, а Nix удобен для изолированных пакетов, flakes и software, которое не хочется затаскивать в системный dependency graph.

Проблема в том, что пользователь сам должен помнить:

- где искать конкретную программу;
- чем именно она была установлена;
- какой из нескольких providers он выбрал в прошлый раз;
- кто теперь должен её обновлять и удалять.

pacnix добавляет над этими системами **единый resolver и слой provenance/state**, не пытаясь заменить сами backend'ы.

> **Убирать необходимость помнить лишнее, но не убирать возможность увидеть всё.**

## Что уже работает

| Возможность | Состояние |
| --- | --- |
| Unified resolver: pacman repos → AUR → Nix | ✅ |
| Explainable ranking кандидатов | ✅ |
| Запоминание прошлого выбора provider | ✅ |
| pacman-style и human-readable CLI | ✅ |
| ALPM/pacman search / install / remove / full upgrade | ✅ |
| Nix search / profile install / remove / upgrade | ✅ |
| SQLite provenance/state через `rusqlite` | ✅ |
| Reconcile состояния из pacman и Nix | ✅ |
| Несколько installed instances одного logical package | ✅ |
| Dry-run / confirmation / `--noconfirm` | ✅ |
| Параллельные независимые backend lanes | ✅ |
| Оценка размера install/remove/upgrade для pacman | ✅ |
| AUR RPC search | ✅ |
| AUR build/install pipeline | 🚧 |
| Полноценная privilege abstraction (`sudo` / `sudo-rs`) | 🚧 |

`✅` означает, что код уже существует и выполняет реальные операции, **не** что интерфейс стабилен или production-ready.

## CLI

Одна и та же операция может быть записана в привычном стиле pacman или обычной командой:

```bash
# install
pacnix -S firefox
pacnix install firefox

# remove
pacnix -R firefox
pacnix remove firefox

# search
pacnix -Ss firefox
pacnix search firefox

# info / list
pacnix -Qi firefox
pacnix info firefox
pacnix -Q
pacnix list

# full system/package upgrade
pacnix -Syu
pacnix upgrade

# reconcile pacnix state with authoritative backends
pacnix sync
```

Несколько targets можно планировать одной операцией:

```bash
pacnix install firefox ripgrep htop
```

Перед изменением системы pacnix строит план, показывает выбранные providers и запрашивает подтверждение. Для проверки без выполнения:

```bash
pacnix install firefox --dry-run
```

## Resolver

Каждый backend возвращает структурированные `Candidate`, после чего общий resolver ранжирует их по совпадению имени, backend priority и сохранённым пользовательским предпочтениям.

Упрощённо:

```text
query
  ↓
configured pacman repositories
  ↓
AUR
  ↓
Nix
  ↓
rank + explain
  ↓
Selected / Ambiguous / NotFound
```

Если найдено несколько равнозначных вариантов, frontend предлагает выбор. Успешный выбор может быть запомнен для следующего запроса.

При этом выбор resolver'а не подменяется поведением pacman: если выбран конкретный repo-qualified пакет, в транзакцию передаётся именно он.

## Состояние и provenance

SQLite в pacnix — **не вторая база пакетов**.

Источниками истины остаются:

- pacman/ALPM для Arch packages;
- Nix для Nix profiles/store;
- соответствующий backend для будущих providers.

pacnix хранит только собственный coordination layer: logical package identity, installed instances, aliases, provenance, receipts и дополнительную metadata.

```text
pacman state ─┐
              ├──→ reconcile ───→ pacnix.db
Nix state ────┘
```

Поэтому прямые операции вроде:

```bash
pacman -S foo
nix profile install nixpkgs#bar
```

не должны ломать модель pacnix: `pacnix sync` повторно обнаруживает authoritative state и обновляет своё представление.

Модель не предполагает `package name == installed object`: один logical package может иметь несколько installed instances, что важно в том числе для Nix.

## Архитектура

Core не зависит от терминального UI. CLI — только один frontend.

```text
                    ┌─────────────────┐
                    │   pacnix-cli    │
                    └────────┬────────┘
                             │
                             ▼
                    ┌─────────────────┐
                    │   pacnix-core   │
                    │ resolver/model  │
                    │ executor/state  │
                    └───────┬─────────┘
                            │
          ┌─────────────────┼─────────────────┐
          ▼                 ▼                 ▼
┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐
│ backend-alpm    │ │ backend-aur     │ │ backend-nix     │
│ pacman / ALPM   │ │ AUR RPC/build   │ │ nix profile     │
└─────────────────┘ └─────────────────┘ └─────────────────┘
```

Монорепозиторий:

```text
pacnix/
├── crates/
│   ├── pacnix-core/            models, resolver, executor, SQLite, interaction
│   ├── pacnix-backend-alpm/    pacman / ALPM backend
│   ├── pacnix-backend-aur/     AUR backend
│   ├── pacnix-backend-nix/     Nix backend
│   └── pacnix-cli/             thin CLI frontend
├── docs/
└── Cargo.toml
```

Backend'ы реализуют общий `pacnix_core::PackageBackend`. Зависимости остаются backend-local: pacnix не пытается удовлетворять зависимость Arch-пакета Nix-пакетом и не притворяется, что mixed ALPM/Nix transaction атомарна.

## MVP

Текущая цель первой самостоятельной полезной версии:

1. ALPM/pacman backend;
2. рабочий AUR build/install/update flow;
3. Nix backend;
4. unified resolver;
5. SQLite reconciliation/provenance;
6. нормальный CLI поверх общего core;
7. privilege providers для системных операций.

Всё остальное в [спецификации](docs/spec.md), помеченное как **POST-MVP / архитектурный вариант**, не является blocker'ом первого релиза. GitHub resolver, README install hints, wrapper purity, sandboxed AUR builds, TUI/GUI, MCP, plugins, systemd/cgroups integration и другие идеи закладываются только так, чтобы MVP не закрывал к ним путь.

## Сборка

Нужен актуальный Rust toolchain. Для реальных backend operations также должны быть доступны соответствующие системные инструменты (`pacman`, `nix`; AUR pipeline пока в разработке).

```bash
git clone https://github.com/Mar2ianen/pacnix.git
cd pacnix
cargo build
```

Запуск из workspace:

```bash
cargo run -p pacnix-cli -- search firefox
cargo run -p pacnix-cli -- install firefox --dry-run
```

Или после сборки бинарника:

```bash
./target/debug/pacnix search firefox
```

## Разработка

Сейчас приоритет — не расширять список экосистем, а довести до цельного состояния базовую тройку:

```text
ALPM/pacman + AUR + Nix
```

Полное ТЗ и разделение MVP / POST-MVP находятся в [`docs/spec.md`](docs/spec.md).

## Лицензирование

Проект распространяется на условиях **GPL-3.0-or-later**.

Собственный код pacnix помечается как:

```text
MIT OR GPL-3.0-or-later
```

Это позволяет переиспользовать собственные независимые компоненты permissively, при этом собранный проект может оставаться GPL в соответствии с лицензиями используемого стека. Полные условия см. в [LICENSE](LICENSE), политика contributions — в [CONTRIBUTING.md](CONTRIBUTING.md).
