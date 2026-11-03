Read `sales.csv` in the current working directory. Its header is
`date,region,sku,units,unit_price`.

For each region compute the total revenue: the sum of `units * unit_price` over
all rows of that region, rounded to 2 decimal places ONCE at the end (do not
round per row).

Write `summary.csv` in the current working directory:

- header `region,total`
- one row per region
- rows sorted by total, descending
- totals formatted with exactly 2 decimal places
- the file must end with a single trailing newline

Do not modify `sales.csv`.
