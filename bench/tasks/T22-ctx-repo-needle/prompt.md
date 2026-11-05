The current working directory is the backend of an online shop (Python,
`src/ledgerly/`). Support escalated a ticket: customers who ask for a refund
on the 30th day after delivery are turned down with

    E-4127 refund window closed: order is 30 days old (limit 30 days)

The refund policy: a customer may ask for a refund up to and including the
30th day after delivery; from day 31 on the request is rejected.

Fix the code so that refunds follow this policy.

Rules:

- The fix is a one-line change in a single file.
- Everything else in the repository is correct and must stay exactly as it
  is: do not modify, rename or delete any other line or file.
