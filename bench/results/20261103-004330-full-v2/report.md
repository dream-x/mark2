# mark2 bench — 20261103-004330-full-v2

- прогон: 2026-11-02T21:43:30.274442Z → 2026-11-02T22:10:10.882079Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 3 (из task.toml)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T01 create-file | T02 read-answer | T03 edit-exact | T04 fix-bug-test | T05 multi-file-rename | T06 search-code | T07 bash-pipeline | T08 write-run-script |
|---|---|---|---|---|---|---|---|---|
| qwen9 | 3/3 · 5.6s | 3/3 · 10.9s | 3/3 · 14.6s | 3/3 · 49.6s | 3/3 · 51.3s | 3/3 · 35.6s | 3/3 · 7.4s | 3/3 · 11.4s |
| bolt4 | 3/3 · 2.1s | 3/3 · 3.1s | 3/3 · 12.4s | 3/3 · 69.0s | 2/3 · 32.4s | 3/3 · 19.0s | 3/3 · 4.4s | 3/3 · 4.5s |
| flint40 | 3/3 · 8.3s | 3/3 · 1.6s | 3/3 · 3.0s | 3/3 · 7.0s | 3/3 · 17.6s | 3/3 · 4.8s | 3/3 · 8.6s | 3/3 · 4.1s |
| qwen27 | 3/3 · 7.1s | 3/3 · 5.9s | 3/3 · 7.7s | 3/3 · 8.3s | 3/3 · 10.0s | 3/3 · 9.3s | 3/3 · 7.7s | 3/3 · 7.0s |
| dsflash | 0/3 · 0.5s | 0/3 · 0.5s | 0/3 · 0.3s | 0/3 · 0.2s | 0/3 · 0.5s | 0/3 · 0.3s | 0/3 · 0.2s | 0/3 · 0.2s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты |
|---|---|---|---|---|---|
| qwen9 | 24/24 (100%) | 12.6s | 8110 / 438 | 22% (30/138) | 0 |
| bolt4 | 23/24 (96%) | 8.4s | 4603 / 1127 | 12% (18/146) | 0 |
| flint40 | 24/24 (100%) | 5.1s | 5068 / 333 | 27% (28/103) | 0 |
| qwen27 | 24/24 (100%) | 7.7s | 6310 / 296 | 5% (4/81) | 0 |
| dsflash | 0/24 (0%) | 0.3s | 0 / 0 | — | 0 |

Всего: 95/120 (79%).

## Аномалии

- [dsflash] T01 повтор 1: mark2 вышел с кодом 1
- [dsflash] T01 повтор 1: пустой ответ в stdout
- [dsflash] T01 повтор 2: mark2 вышел с кодом 1
- [dsflash] T01 повтор 2: пустой ответ в stdout
- [dsflash] T01 повтор 3: mark2 вышел с кодом 1
- [dsflash] T01 повтор 3: пустой ответ в stdout
- [dsflash] T02 повтор 1: mark2 вышел с кодом 1
- [dsflash] T02 повтор 1: пустой ответ в stdout
- [dsflash] T02 повтор 2: mark2 вышел с кодом 1
- [dsflash] T02 повтор 2: пустой ответ в stdout
- [dsflash] T02 повтор 3: mark2 вышел с кодом 1
- [dsflash] T02 повтор 3: пустой ответ в stdout
- [dsflash] T03 повтор 1: mark2 вышел с кодом 1
- [dsflash] T03 повтор 1: пустой ответ в stdout
- [dsflash] T03 повтор 2: mark2 вышел с кодом 1
- [dsflash] T03 повтор 2: пустой ответ в stdout
- [dsflash] T03 повтор 3: mark2 вышел с кодом 1
- [dsflash] T03 повтор 3: пустой ответ в stdout
- [dsflash] T04 повтор 1: mark2 вышел с кодом 1
- [dsflash] T04 повтор 1: пустой ответ в stdout
- [dsflash] T04 повтор 2: mark2 вышел с кодом 1
- [dsflash] T04 повтор 2: пустой ответ в stdout
- [dsflash] T04 повтор 3: mark2 вышел с кодом 1
- [dsflash] T04 повтор 3: пустой ответ в stdout
- [dsflash] T05 повтор 1: mark2 вышел с кодом 1
- [dsflash] T05 повтор 1: пустой ответ в stdout
- [dsflash] T05 повтор 2: mark2 вышел с кодом 1
- [dsflash] T05 повтор 2: пустой ответ в stdout
- [dsflash] T05 повтор 3: mark2 вышел с кодом 1
- [dsflash] T05 повтор 3: пустой ответ в stdout
- [dsflash] T06 повтор 1: mark2 вышел с кодом 1
- [dsflash] T06 повтор 1: пустой ответ в stdout
- [dsflash] T06 повтор 2: mark2 вышел с кодом 1
- [dsflash] T06 повтор 2: пустой ответ в stdout
- [dsflash] T06 повтор 3: mark2 вышел с кодом 1
- [dsflash] T06 повтор 3: пустой ответ в stdout
- [dsflash] T07 повтор 1: mark2 вышел с кодом 1
- [dsflash] T07 повтор 1: пустой ответ в stdout
- [dsflash] T07 повтор 2: mark2 вышел с кодом 1
- [dsflash] T07 повтор 2: пустой ответ в stdout
- [dsflash] T07 повтор 3: mark2 вышел с кодом 1
- [dsflash] T07 повтор 3: пустой ответ в stdout
- [dsflash] T08 повтор 1: mark2 вышел с кодом 1
- [dsflash] T08 повтор 1: пустой ответ в stdout
- [dsflash] T08 повтор 2: mark2 вышел с кодом 1
- [dsflash] T08 повтор 2: пустой ответ в stdout
- [dsflash] T08 повтор 3: mark2 вышел с кодом 1
- [dsflash] T08 повтор 3: пустой ответ в stdout
