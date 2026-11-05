`custody/` holds the custody log of a biobank's freezers: one file per duty
day, in chronological order (`day-01.log` ... `day-08.log`). Entries are
sometimes corrected, voided or superseded by later ones.

Question: in which slot is the KESTREL study's reference aliquot at the end of
duty day 08?

Write the answer to `answer.txt`: the slot alone, in the slot notation the
logs use (freezer/rack/box/position, for example `F1/R1/B01/A1`), on a single
line, nothing else.

Rules:

- `custody/` must stay exactly as it is.
- Do not create any file other than `answer.txt`: no scratch notes, no helper
  scripts. If you need to compute something, do it inline, for example with
  `python3 -c "..."`.
