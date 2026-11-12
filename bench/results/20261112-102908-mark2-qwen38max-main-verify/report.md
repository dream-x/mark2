# mark2 bench — 20261112-102908-mark2-qwen38max-main-verify

- прогон: 2026-11-12T07:29:08.028609Z → 2026-11-12T09:13:42.568891Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 3 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T01 create-file | T02 read-answer | T03 edit-exact | T04 fix-bug-test | T05 multi-file-rename | T06 search-code | T07 bash-pipeline | T08 write-run-script | T09 needle-edit | T10 yaml-surgical | T11 cascade-bugs | T12 decoy-answer | T13 moved-config | T14 csv-report | T15 long-context | T16 permission-deny | T18 memory-recall | T19 memory-store | T20 review-bug | T21 review-clean | T22 ctx-repo-needle | T23 ctx-scatter-synthesis |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| qwen38max | 3/3 · 13.6s | 3/3 · 4.7s | 3/3 · 18.4s | 3/3 · 18.9s | 3/3 · 38.0s | 3/3 · 25.0s | 3/3 · 31.7s | 3/3 · 16.1s | 3/3 · 42.3s | 3/3 · 27.5s | 3/3 · 70.3s | 3/3 · 20.1s | 3/3 · 27.2s | 3/3 · 41.9s | 3/3 · 796.7s | 3/3 · 24.6s | 3/3 · 16.1s | 3/3 · 16.3s | 3/3 · 64.9s | 3/3 · 10.6s | 3/3 · 55.5s | 2/3 · 799.5s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| qwen38max | 65/66 (98%) | 25.9s | 13823 / 618 | 3% (15/462) | 1 | 40 |

Всего: 65/66 (98%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| qwen38max | 3 | 0 | 0 | 12 | 40 | 15 |

## Аномалии

- [qwen38max] T23 повтор 1: таймаут mark2 (1200s)
