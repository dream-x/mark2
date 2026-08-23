Count how many lines under the `src/` directory contain the text `TODO`, then
write that count to a file named `report.txt` in the current working directory.

Details:

- Search `src/` recursively, including subdirectories and non-Python files.
- Count lines, matching the plain uppercase text `TODO`. Every occurrence in
  this tree sits on its own line, so the number of matching lines is the number
  you want.
- `report.txt` must contain ONLY the number, for example `42`, followed by a
  newline. No labels, no explanation, no quotes, no other lines.
- Do not modify anything inside `src/`.
