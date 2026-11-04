# mark2 bench — 20261104-080350-t17-abl-nocoerce

- прогон: 2026-11-04T05:03:50.29017Z → 2026-11-04T06:07:28.306547Z
- mark2: `/private/tmp/claude-501/-Users-maks-projects-mark2/04d8750f-94be-414a-bc5f-19c44aaec6c6/scratchpad/night-bin/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 1 (--repeats)
- абляция (`--agent-override` в [agent]): `arg_coercion = false`

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T17 ws-chat |
|---|---|
| flint40 | 0/1 · 649.5s |
| qwen36c | 0/1 · 3144.8s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| flint40 | 0/1 (0%) | 649.5s | 1521279 / 50235 | 38% (30/80) | 0 | 0 |
| qwen36c | 0/1 (0%) | 3144.8s | 7963 / 260062 | 0% (0/2) | 0 | 0 |

Всего: 0/2 (0%).

## Аномалии

- [flint40] T17 повтор 1: mark2 вышел с кодом 1
