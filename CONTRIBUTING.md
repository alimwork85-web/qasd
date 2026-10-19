# Contributing to Qasd

Thank you for helping improve Qasd. Contributions are welcome from any developer: bug reports, fixes, new features, documentation, and translations.

## Before you start

- **Bug reports:** open an [issue](https://github.com/alimwork85-web/qasd/issues) with your Windows version, the Qasd version (`qasd status`), the exact query or steps, and what you expected to happen. Attach the relevant lines from `%LOCALAPPDATA%\Qasd\daemon.log` if the problem involves indexing.
- **Feature ideas:** open an issue first to discuss the idea before writing a large change.
- **Small fixes** (typos, clear bugs with a one-line fix): a pull request is enough.

## Setting up

```powershell
git clone https://github.com/alimwork85-web/qasd
cd qasd
cargo build
cargo test
```

Qasd is written in Rust and targets Windows 10 and 11.

## Making a change

1. Create a branch from `main`, for example `fix/typo-in-query-parser`.
2. Keep the change focused. One pull request should fix one problem or add one feature.
3. Format the code with `cargo fmt`. The project uses `rustfmt.toml` (maximum line width 150).
4. Run `cargo test` and make sure it passes.
5. If the change is visible to users, add an entry to `CHANGELOG.md` in the same format as the existing entries.
6. Open a pull request. Describe what changed, why, and how you tested it.

## Code style

- Follow the patterns already used in the module you are changing.
- Add a test when you fix a bug, so it does not come back.
- Avoid unrelated refactoring in the same pull request.

## Language

Issues and pull requests may be written in Arabic or English. Documentation and user-facing text in the README are bilingual, so if you change one language, please update the other when it is practical.

## Attribution

Qasd is an MIT-licensed project by Ali Alhumedi, built on [fsearch](https://github.com/noahdunnagan/fsearch). By contributing, you agree that your contribution is licensed under the same MIT license.
