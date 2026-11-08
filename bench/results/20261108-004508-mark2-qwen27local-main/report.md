# mark2 bench — 20261108-004508-mark2-qwen27local-main

- прогон: 2026-11-07T21:45:08.51614Z → 2026-11-07T22:40:39.67663Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 3 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T01 create-file | T02 read-answer | T03 edit-exact | T04 fix-bug-test | T05 multi-file-rename | T06 search-code | T07 bash-pipeline | T08 write-run-script | T09 needle-edit | T10 yaml-surgical | T11 cascade-bugs | T12 decoy-answer | T13 moved-config | T14 csv-report | T15 long-context | T16 permission-deny | T18 memory-recall | T19 memory-store | T20 review-bug | T21 review-clean | T22 ctx-repo-needle | T23 ctx-scatter-synthesis |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| qwen27-local | 3/3 · 5.8s | 3/3 · 3.2s | 3/3 · 7.6s | 3/3 · 9.4s | 3/3 · 20.3s | 3/3 · 29.9s | 3/3 · 12.5s | 3/3 · 7.9s | 3/3 · 20.6s | 3/3 · 17.8s | 3/3 · 57.6s | 3/3 · 16.5s | 3/3 · 21.8s | 3/3 · 18.6s | 3/3 · 244.6s | 0/3 · 19.0s | 3/3 · 9.5s | 3/3 · 7.9s | 3/3 · 77.8s | 3/3 · 34.4s | 3/3 · 33.0s | 3/3 · 452.7s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| qwen27-local | 63/66 (95%) | 18.9s | 8109 / 411 | 2% (7/293) | 0 | 10 |

Всего: 63/66 (95%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| qwen27-local | 12 | 0 | 0 | 0 | 10 | 7 |

## Аномалии

Нет.
