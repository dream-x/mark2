The file `handlers.py` in the current working directory is a large module of
warehouse event validators. One of them, `validate_shipment()`, has a bug.

The docstring of `validate_shipment()` is the specification and it is correct.
The code of `validate_shipment()` disagrees with it for exactly one boundary.

Fix the code of `validate_shipment()` so that it matches its docstring.

Rules:

- Change only the single wrong token inside `validate_shipment()`. It is a
  one-token edit.
- Every other function in the file is correct and must stay exactly as it is.
- Do not touch any docstring, the helpers, the constants, the blank lines or
  the formatting.
- Do not rewrite or reformat the file, and do not create new files.
