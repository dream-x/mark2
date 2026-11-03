The current working directory contains `README.md` and a small Python library
in `src/`: retry policies for an HTTP client.

The docs in this repo may be stale; trust the code. What is the effective retry
limit that `build_policy("edge")` actually produces?

Rules:

- Print the answer as the final line of your reply, in exactly this form:
  `retry_limit = <value>`
- Use the `retry_limit = ...` form only for that final answer line. Do not
  write it with any other value anywhere in your reply.
- This is a read-only task: do not modify, create or delete any file. That
  includes helper scripts: if you need to execute code, run it inline with
  `python3 -c "..."` instead of writing a file.
