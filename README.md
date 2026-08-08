# pacnix

Пакетный менеджер и единый слой управления программным обеспечением для Arch Linux:
pacman/ALPM + AUR + Nix в одном интерфейсе, с памятью о происхождении каждого установленного объекта.

> убирать необходимость помнить лишнее, но не убирать возможность увидеть всё.

Спецификация: [docs/spec.md](docs/spec.md).

## Статус

Skeleton / Phase 0. MVP-цель: ALPM/AUR + Nix + rusqlite + unified resolver
(см. разделы Phase 0–4 в спеке).

## Структура монорепозитория

Набор переиспользуемых крейтов: каждый backend — отдельный крейт, зависящий
только от `pacnix-core`; CLI — тонкий фронтенд, собирающий registry backend'ов.

```text
pacnix/
├── crates/
│   ├── pacnix-core/            core-модели, resolver, storage (SQLite), interaction
│   ├── pacnix-backend-alpm/    ALPM/pacman
│   ├── pacnix-backend-aur/     AUR
│   ├── pacnix-backend-nix/     Nix
│   └── pacnix-cli/             CLI-фронтенд (pacman-style + human-readable)
├── docs/
└── Cargo.toml
```

Любой крейт можно использовать как зависимость: `pacnix-backend-*` реализуют
`pacnix_core::PackageBackend`, `pacnix-core::Resolver` собирает их в единый
resolver с приоритетами (extra → AUR → nixpkgs).

## Лицензирование

Двойная лицензия: `MIT OR GPL-3.0-or-later`.

Собственный код — MIT OR GPL-3.0-or-later. Распространяемый бинарный продукт —
под GPL-3.0-or-later из-за GPL-зависимостей. Подробности: [LICENSE](LICENSE),
политика для контрибуторов — [CONTRIBUTING.md](CONTRIBUTING.md).

## Сборка

```bash
cargo build
cargo run -- -S firefox
```
