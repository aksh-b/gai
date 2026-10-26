---
name: data-analysis
description: Forecast a metric, find anomalies, explain why a number changed (which segments drove it), and check data quality, on query results saved as CSV. Use for "why did X drop", "what will X be next month", "anything unusual in X", or "can I trust this data".
license: MIT
---
# Data analysis

`analytics.py` (in `.skills/data-analysis/` once this skill is loaded) has
four functions that work on a CSV file, such as the ones `sql_query` saves
under `data/`. Each prints a short Markdown summary, saves a PNG chart in
the working directory, and returns a pandas DataFrame.

Run them with `code_exec` (Python):

```python
import sys
sys.path.insert(0, ".skills/data-analysis")
import analytics

analytics.drivers("data/signups.csv", date="day", dims=["channel", "plan", "country"])
```

## Why did a number change? `drivers`

```python
analytics.drivers(csv, date, dims, metric=None, agg="sum", days=7)
```

Compares the last `days` days with the `days` before them (or pass
`current=("2026-10-08", "2026-10-14")` and `previous=(...)`). `metric` is
the column to add up; leave it out to count rows (one row per signup,
order, ...). For each dimension it lists the values whose change explains
most of the total change, with their share of it. Get one row per event,
or per day and segment, from SQL first:

```sql
SELECT DATE(created_at) AS day, channel, plan, country, COUNT(*) AS signups
FROM signups WHERE created_at >= CURRENT_DATE - INTERVAL '28 days'
GROUP BY 1, 2, 3, 4
```

then `metric="signups"`. Report the overall change, the top two or three
drivers with numbers, and anything that looks like a data problem.

## What next? `forecast`

```python
analytics.forecast(csv, date, value, periods=14, season=None)
```

Holt-Winters (level, trend and a weekly or other season, guessed from the
dates unless `season` is given) with an 80% and 95% range. One row per
day (or week) with a value; aggregate in SQL first.

## Anything unusual? `anomalies`

```python
analytics.anomalies(csv, date, value, window=14, threshold=3.5)
```

Points far from the rolling median (robust z-score over `window` points).
Say which dates, how far off, and whether they are recent.

## Can I trust it? `quality`

```python
analytics.quality(csv, date=None, key=None)
```

Empty and constant columns, missing values, duplicate rows (or keys),
negative numbers where they look odd, and for `date`: missing days, the
last date, and days with unusually few rows. Run it before drawing
conclusions from a surprising number: a drop is often a broken pipeline.
