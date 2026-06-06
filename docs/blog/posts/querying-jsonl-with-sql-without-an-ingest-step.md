---
date: 2026-07-06
categories:
  - Guides
---

# Querying JSONL with SQL without an ingest step

A lot of real developer data shows up as JSONL: logs, traces, agent transcripts, exports from APIs, one-off dumps from somebody else's system. The usual next move is "convert it first."

Sometimes that is the right move. Sometimes it is just up-front overhead.

When I am exploring raw data, I usually do not want to design a pipeline yet. I want to ask a few questions:

- what fields exist?
- which records are weird?
- how many shapes are mixed together?
- can I get to a useful filtered slice quickly?

If the first step is a conversion project, I lose momentum.

## The simpler path

Bundlebase can attach JSONL directly and let me query it with SQL:

```python
import bundlebase

c = await bundlebase.create("./logs")
await c.attach("./events.jsonl")
result = await c.query("""
    SELECT type, COUNT(*) AS n
    FROM bundle
    GROUP BY type
    ORDER BY n DESC
""")
```

That feels a lot closer to how I actually work with raw data.

## Why not just convert to parquet immediately

Parquet is great once you know the data is worth keeping and you understand the shape you want.

But JSONL is often the source format for a reason:

- the producer already emits it
- the shape may be irregular
- you may only need a subset
- you may still be figuring out what the useful columns are

Early conversion can be a fine optimization. It does not have to be the opening ceremony.

## This is especially useful for logs

Developer logs are messy. They have optional fields, nested objects, and a lot of irrelevant noise. A direct query path lets you get to the interesting slice first, then decide whether to clean and commit it.

For example:

```sql
SELECT request_id, status, latency_ms
FROM bundle
WHERE service = 'checkout' AND status >= 500
```

That is a lot nicer than writing custom parsing code before you even know the error distribution.

## Improving the schema as you go

Once you have a few queries working, you start seeing the data more clearly. The field names from the original JSONL might be cryptic or inconsistent. You might notice a nested object that deserves its own column. You might want to coerce a string timestamp into an actual timestamp type.

Bundlebase lets you refine the schema incrementally, directly on the bundle, without touching the source files:

```python
await c.rename_column("ts", "timestamp")
await c.rename_column("req_id", "request_id")
await c.cast_column("timestamp", "timestamp")
```

You can also derive columns from SQL expressions. Once you notice that `end_ts - start_ts` is the number you keep computing in every query, you can promote it to a real column:

```python
await c.add_column("duration_ms", "end_ts - start_ts")
```

Or extract a date from a timestamp to make grouping by day straightforward:

```python
await c.add_column("date", "DATE(timestamp)")
```

These live on the bundle the same way renames do — versioned, queryable, no changes to the original file.

Each of those changes is recorded as a versioned step. Your queries get cleaner as you understand the data better, but the original JSONL is untouched — you have not committed to a particular schema from the start.

The version history also means you can look back at each step along the way. If a rename turns out to be wrong, or you want to see what the raw field names were when you first attached the file, that context is still there.

This is different from transforming data in a separate pipeline. The schema refinement lives with the bundle, alongside the queries that motivated it.

## Performance

Raw JSONL is not a fast format. A full scan over a large file takes real time, and there is no getting around that at the format level.

However, Bundlebase does all it can to improve that out of the box. And when that is still not fast enough, Bundlebase supports indexes on any column:

```python
await c.create_index("service")
```

After that, queries filtering on `service` skip to matching rows directly instead of scanning the file.

## The workflow I like

1. attach JSONL directly
2. explore with SQL
3. clean the slice you care about
4. commit the result
5. GOTO 2

Sometimes the best ingest step is no ingest step yet.

