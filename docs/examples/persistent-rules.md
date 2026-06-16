---
title: Persistent Rules — Bundlebase Examples
description: "Examples for always_delete and always_update — persistent data hygiene rules that fire automatically on every future attach."
---

# Persistent Rules

`always_delete` and `always_update` encode cleanup rules into the bundle itself. Once defined, they fire automatically on every future `attach` or `fetch` — whoever runs the pipeline, whatever tool they use.

## always_delete

Remove rows that should never be in the bundle, regardless of what the source sends:

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = bb.open("s3://analytics/crm-export").extend()

    bundle.always_delete("amount < 0")
    bundle.always_delete("status = 'test'")
    bundle.always_delete("user_id IS NULL")

    bundle.commit("Added persistent cleanup rules")
    ```

=== "SQL"

    ```bash
    bundlebase repl --bundle s3://analytics/crm-export
    ```

    ```sql
    ALWAYS DELETE FROM bundle WHERE amount < 0;
    ALWAYS DELETE FROM bundle WHERE status = 'test';
    ALWAYS DELETE FROM bundle WHERE user_id IS NULL;
    COMMIT 'Added persistent cleanup rules';
    ```

Now every future attach cleans automatically — no need to remember to add the filter:

=== "Python"

    ```python
    bundle = bb.open("s3://analytics/crm-export").extend()

    # These rules fire automatically — dirty rows never make it in
    bundle.attach("jan_raw.csv")
    bundle.commit("January")

    bundle = bb.open("s3://analytics/crm-export").extend()
    bundle.attach("feb_raw.csv")
    bundle.commit("February")
    ```

=== "SQL"

    ```sql
    ATTACH 'jan_raw.csv';   -- rules fire automatically
    COMMIT 'January';

    ATTACH 'feb_raw.csv';   -- rules fire automatically
    COMMIT 'February';
    ```

---

## always_update

Normalize or transform column values on every incoming record:

=== "Python"

    ```python
    bundle.always_update("SET currency = UPPER(currency) WHERE currency IS NOT NULL")
    bundle.always_update("SET region = 'EMEA' WHERE region IN ('Europe', 'EU', 'europe')")
    bundle.always_update("SET amount = ROUND(amount, 2) WHERE amount IS NOT NULL")
    bundle.always_update("SET event_type = LOWER(event_type) WHERE event_type IS NOT NULL")
    bundle.commit("Added normalization rules")
    ```

=== "SQL"

    ```sql
    ALWAYS UPDATE bundle SET currency = UPPER(currency) WHERE currency IS NOT NULL;
    ALWAYS UPDATE bundle SET region = 'EMEA' WHERE region IN ('Europe', 'EU', 'europe');
    ALWAYS UPDATE bundle SET amount = ROUND(amount, 2) WHERE amount IS NOT NULL;
    ALWAYS UPDATE bundle SET event_type = LOWER(event_type) WHERE event_type IS NOT NULL;
    COMMIT 'Added normalization rules';
    ```

---

## Combining delete and update rules

Rules are applied in definition order — deletes first, then updates:

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = bb.create("s3://analytics/events")

    # Remove records that should never be ingested
    bundle.always_delete("event_type = 'internal_test'")
    bundle.always_delete("user_id IS NULL")

    # Normalize what remains
    bundle.always_update("SET event_type = LOWER(event_type) WHERE event_type IS NOT NULL")
    bundle.always_update("SET region = UPPER(region) WHERE region IS NOT NULL")
    bundle.always_update("SET currency = UPPER(currency) WHERE currency IS NOT NULL")

    bundle.commit("Initial setup with cleanup rules")
    ```

=== "SQL"

    ```bash
    bundlebase repl --bundle s3://analytics/events --create
    ```

    ```sql
    ALWAYS DELETE FROM bundle WHERE event_type = 'internal_test';
    ALWAYS DELETE FROM bundle WHERE user_id IS NULL;
    ALWAYS UPDATE bundle SET event_type = LOWER(event_type) WHERE event_type IS NOT NULL;
    ALWAYS UPDATE bundle SET region = UPPER(region) WHERE region IS NOT NULL;
    ALWAYS UPDATE bundle SET currency = UPPER(currency) WHERE currency IS NOT NULL;
    COMMIT 'Initial setup with cleanup rules';
    ```

---

## Viewing defined rules

```bash
bundlebase repl --bundle s3://analytics/events
```

```sql
SHOW ALWAYS DELETES;
SHOW ALWAYS UPDATES;
```

---

## Why this matters

Without persistent rules, every engineer who runs the pipeline has to remember to apply the same filters. When someone forgets — or writes a new pipeline from scratch — dirty data gets in. Persistent rules make the cleanup part of the bundle definition, not the pipeline script.
