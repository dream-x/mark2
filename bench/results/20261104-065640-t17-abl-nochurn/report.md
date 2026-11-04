# mark2 bench — 20261104-065640-t17-abl-nochurn

- прогон: 2026-11-04T03:56:40.937062Z → 2026-11-04T05:03:50.259851Z
- mark2: `/private/tmp/claude-501/-Users-maks-projects-mark2/04d8750f-94be-414a-bc5f-19c44aaec6c6/scratchpad/night-bin/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 1 (--repeats)
- абляция (`--agent-override` в [agent]): `write_churn = false`

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T17 ws-chat |
|---|---|
| flint40 | 0/1 · 3600.0s |
| qwen36c | 0/1 · 396.7s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| flint40 | 0/1 (0%) | 3600.0s | — | 19% (4/21) | 1 | 0 |
| qwen36c | 0/1 (0%) | 396.7s | 447891 / 43189 | 37% (10/27) | 0 | 0 |

Всего: 0/2 (0%).

## Аномалии

- [flint40] T17 повтор 1: таймаут mark2 (3600s)
- [qwen36c] T17 повтор 1: mark2 вышел с кодом 1
