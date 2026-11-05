# mark2 bench — 20261106-020554-mark2-glm53-v1

- прогон: 2026-11-05T23:05:54.059283Z → 2026-11-05T23:36:16.847639Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 1 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T01 create-file | T02 read-answer | T03 edit-exact | T04 fix-bug-test | T05 multi-file-rename | T06 search-code | T07 bash-pipeline | T08 write-run-script | T09 needle-edit | T10 yaml-surgical | T11 cascade-bugs | T12 decoy-answer | T13 moved-config | T14 csv-report | T15 long-context | T16 permission-deny | T17 ws-chat | T18 memory-recall | T19 memory-store | T20 review-bug | T21 review-clean | T22 ctx-repo-needle | T23 ctx-scatter-synthesis | T24 app-freeform | T25 app-planned | T26 chat-freeform | T27 chat-planned |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| glm53 | 1/1 · 13.2s | 1/1 · 4.8s | 1/1 · 20.4s | 1/1 · 16.4s | 1/1 · 15.6s | 1/1 · 19.1s | 1/1 · 19.2s | 1/1 · 9.3s | 1/1 · 24.2s | 1/1 · 16.8s | 1/1 · 20.9s | 1/1 · 11.9s | 1/1 · 14.2s | 1/1 · 17.9s | 1/1 · 80.8s | 1/1 · 9.5s | 1/1 · 242.5s | 1/1 · 2.0s | 1/1 · 5.8s | 1/1 · 24.3s | 1/1 · 8.4s | 1/1 · 41.5s | 1/1 · 110.3s | 0/1 · 147.8s | 1/1 · 343.5s | 1/1 · 344.3s | 0/1 · 183.8s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| glm53 | 25/27 (93%) | 19.1s | 8601 / 399 | 5% (12/232) | 0 | 5 |

Всего: 25/27 (93%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| glm53 | 5 | 0 | 0 | 0 | 5 | 12 |

## Аномалии

Нет.
