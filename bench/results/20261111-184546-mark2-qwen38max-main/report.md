# mark2 bench — 20261111-184546-mark2-qwen38max-main

- прогон: 2026-11-11T15:45:46.523799Z → 2026-11-11T16:53:16.290592Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 3 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T01 create-file | T02 read-answer | T03 edit-exact | T04 fix-bug-test | T05 multi-file-rename | T06 search-code | T07 bash-pipeline | T08 write-run-script | T09 needle-edit | T10 yaml-surgical | T11 cascade-bugs | T12 decoy-answer | T13 moved-config | T14 csv-report | T15 long-context | T16 permission-deny | T18 memory-recall | T19 memory-store | T20 review-bug | T21 review-clean | T22 ctx-repo-needle | T23 ctx-scatter-synthesis |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| qwen38max | 3/3 · 9.5s | 3/3 · 5.1s | 3/3 · 14.6s | 3/3 · 18.8s | 3/3 · 24.6s | 3/3 · 22.5s | 3/3 · 23.0s | 3/3 · 13.7s | 3/3 · 28.2s | 3/3 · 24.2s | 3/3 · 49.0s | 3/3 · 19.2s | 3/3 · 20.9s | 3/3 · 35.8s | 2/3 · 344.8s | 2/3 · 20.4s | 3/3 · 16.9s | 3/3 · 14.0s | 3/3 · 53.2s | 3/3 · 11.2s | 3/3 · 38.4s | 3/3 · 562.4s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| qwen38max | 64/66 (97%) | 21.1s | 10988 / 524 | 2% (8/364) | 0 | 24 |

Всего: 64/66 (97%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| qwen38max | 3 | 0 | 0 | 11 | 24 | 8 |

## Аномалии

Нет.
