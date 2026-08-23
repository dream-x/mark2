The file `bands.py` in the current working directory has one bug.

The docstring of `classify()` is the specification and it is correct. The code of
`classify()` disagrees with it for exactly one band boundary, so some
temperatures are reported as "hot" when the specification says "mild".

Fix the code so that it matches the docstring.

Rules:

- Change only the single wrong value in `classify()`. It is a one-token edit.
- Do not touch the docstring, the other functions, the constants, the imports,
  the blank lines or the formatting.
- Do not rewrite or reformat the file, and do not create new files.
