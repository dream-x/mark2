# mark2 bench — 20261110-023350-mark2-qwen27udq8-main

- прогон: 2026-11-09T23:33:50.143072Z → 2026-11-10T03:12:54.779796Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 3 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T01 create-file | T02 read-answer | T03 edit-exact | T04 fix-bug-test | T05 multi-file-rename | T06 search-code | T07 bash-pipeline | T08 write-run-script | T09 needle-edit | T10 yaml-surgical | T11 cascade-bugs | T12 decoy-answer | T13 moved-config | T14 csv-report | T15 long-context | T16 permission-deny | T18 memory-recall | T19 memory-store | T20 review-bug | T21 review-clean | T22 ctx-repo-needle | T23 ctx-scatter-synthesis |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| qwen27-udq8 | 3/3 · 17.5s | 3/3 · 9.7s | 3/3 · 61.5s | 3/3 · 59.8s | 3/3 · 59.3s | 3/3 · 82.4s | 3/3 · 60.4s | 3/3 · 59.6s | 3/3 · 205.7s | 3/3 · 73.2s | 3/3 · 228.6s | 3/3 · 92.8s | 3/3 · 94.4s | 3/3 · 64.4s | 1/3 · 1080.0s | 0/3 · 65.8s | 3/3 · 47.0s | 3/3 · 25.7s | 3/3 · 300.5s | 3/3 · 66.8s | 3/3 · 171.1s | 1/3 · 1440.0s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| qwen27-udq8 | 59/66 (89%) | 67.5s | 10011 / 507 | 3% (11/330) | 5 | 8 |

Всего: 59/66 (89%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| qwen27-udq8 | 3 | 0 | 0 | 0 | 9 | 15 |

## Аномалии

- [qwen27-udq8] T15 повтор 1: таймаут mark2 (1080s)
- [qwen27-udq8] T15 повтор 2: таймаут mark2 (1080s)
- [qwen27-udq8] T23 повтор 1: таймаут mark2 (1440s)
- [qwen27-udq8] T23 повтор 2: таймаут mark2 (1440s)
- [qwen27-udq8] T23 повтор 3: таймаут mark2 (1440s)
