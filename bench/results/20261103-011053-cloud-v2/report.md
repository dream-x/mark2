# mark2 bench — 20261103-011053-cloud-v2

- прогон: 2026-11-02T22:10:53.754142Z → 2026-11-02T22:12:17.994618Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 3 (из task.toml)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T01 create-file | T02 read-answer | T03 edit-exact | T04 fix-bug-test | T05 multi-file-rename | T06 search-code | T07 bash-pipeline | T08 write-run-script |
|---|---|---|---|---|---|---|---|---|
| gemma31c | 3/3 · 1.0s | 3/3 · 1.3s | 3/3 · 2.2s | 3/3 · 4.1s | 3/3 · 7.9s | 3/3 · 5.4s | 3/3 · 3.0s | 3/3 · 1.7s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты |
|---|---|---|---|---|---|
| gemma31c | 24/24 (100%) | 2.8s | 6160 / 163 | 8% (9/112) | 0 |

Всего: 24/24 (100%).

## Аномалии

Нет.
