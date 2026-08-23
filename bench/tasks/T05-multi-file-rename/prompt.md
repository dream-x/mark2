The current working directory contains two Python files: `lib.py` and `app.py`.

Rename the function `calc_total` to `compute_total`.

Rules:

- Update every occurrence in both files: the definition, every call site, the
  `from lib import ...` line, and the mention inside the docstring of
  `report()` in `app.py`.
- After the rename, the text `calc_total` must not appear anywhere in either
  file.
- Do not rename anything else and do not change any behaviour. Running
  `python3 app.py` must still print exactly the same output as before.
- Do not create backup copies of the files.
