---
title: Real-World Scenarios — Bundlebase Examples
description: "Complete end-to-end Bundlebase examples: CRM export pipeline, daily API feed, multi-vendor aggregation, and agent data store."
---

# Real-World Scenarios

## CRM export with PII removal

Monthly export from a CRM, stripped of PII, normalized, and shared with the data science team. Updated monthly without changing the path consumers point at.

=== "Python"

    ```python
    import bundlebase.sync as bb
    from datetime import date

    # First run: set up the bundle
    bundle = (bb.create("s3://team-data/crm-export")
        .set_name("CRM — Enterprise Closed Won")
        .set_description(
            "Monthly CRM export. Enterprise accounts only, closed-won deals. "
            "PII removed. Normalized column names."
        )
        .always_delete("WHERE customer_tier != 'enterprise'")
        .always_delete("WHERE status != 'closed_won'")
        .attach("s3://crm-raw/2026-01.csv")
        .attach("s3://crm-raw/2026-02.csv")
        .attach("s3://crm-raw/2026-03.csv")
        .normalize_column_names()
        .cast_column("amount", "float64")
        .cast_column("close_date", "date32")
        .drop_column("email")
        .drop_column("phone")
        .drop_column("ssn"))

    bundle.commit("Initial Q1 export")
    print(f"Exported {bundle.num_rows:,} rows")

    # Monthly update — rules fire automatically, no need to re-specify filters
    bundle = bb.open("s3://team-data/crm-export").extend()
    bundle.attach(f"s3://crm-raw/{date.today().strftime('%Y-%m')}.csv")
    bundle.commit(f"Monthly refresh — {date.today().strftime('%B %Y')}")
    ```

=== "SQL"

    ```sql
    -- First run
    CREATE 's3://team-data/crm-export';
    SET NAME 'CRM — Enterprise Closed Won';
    SET DESCRIPTION 'Monthly CRM export. Enterprise accounts only, closed-won deals. PII removed. Normalized column names.';
    ALWAYS DELETE WHERE customer_tier != 'enterprise';
    ALWAYS DELETE WHERE status != 'closed_won';
    ATTACH 's3://crm-raw/2026-01.csv';
    ATTACH 's3://crm-raw/2026-02.csv';
    ATTACH 's3://crm-raw/2026-03.csv';
    NORMALIZE COLUMN NAMES;
    CAST COLUMN amount AS FLOAT64;
    CAST COLUMN close_date AS DATE;
    DROP COLUMN email;
    DROP COLUMN phone;
    DROP COLUMN ssn;
    COMMIT 'Initial Q1 export';

    -- Monthly update
    OPEN 's3://team-data/crm-export';
    EXTEND;
    ATTACH 's3://crm-raw/2026-04.csv';
    COMMIT 'Monthly refresh — April 2026';
    ```

**Consumer side:**

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = bb.open("s3://team-data/crm-export")
    df = bundle.to_pandas()
    ```

=== "SQL"

    ```sql
    OPEN 's3://team-data/crm-export';
    SELECT region, SUM(amount) as total FROM bundle GROUP BY region;
    ```

---

## Daily API feed with incremental fetch

An internal API publishes a JSON endpoint. This pipeline fetches new records daily, normalizes them, and keeps a running history.

=== "Python"

    ```python
    import bundlebase.sync as bb
    from datetime import date

    # First run: define the bundle and its source
    bundle = (bb.create("s3://analytics/usage-events")
        .set_name("Customer Usage Events")
        .set_description("Daily pull from /api/v2/events. All accounts, all event types.")
        .always_delete("WHERE event_type = 'internal_test'")
        .always_update("SET event_type = LOWER(event_type)")
        .create_source("http", {
            "url": "https://api.internal/v2/events/export",
            "headers": "Authorization: Bearer TOKEN",
            "format": "json",
            "json_record_path": "events"
        }))

    bundle.fetch("base", "add")
    bundle.commit("Initial load")

    # Daily run — source is already defined, just fetch and commit
    bundle = bb.open("s3://analytics/usage-events").extend()
    bundle.fetch("base", "add")
    bundle.commit(f"Daily refresh — {date.today()}")
    ```

=== "SQL"

    ```sql
    -- First run
    CREATE 's3://analytics/usage-events';
    SET NAME 'Customer Usage Events';
    ALWAYS DELETE WHERE event_type = 'internal_test';
    ALWAYS UPDATE SET event_type = LOWER(event_type);
    CREATE SOURCE FOR base USING http WITH (
        url = 'https://api.internal/v2/events/export',
        headers = 'Authorization: Bearer TOKEN',
        format = 'json',
        json_record_path = 'events'
    );
    FETCH base ADD;
    COMMIT 'Initial load';

    -- Daily run
    OPEN 's3://analytics/usage-events';
    EXTEND;
    FETCH base ADD;
    COMMIT 'Daily refresh';
    ```

---

## Multi-vendor data aggregation

Combine data from multiple vendors (SFTP, HTTP) into a single queryable bundle. Each vendor is a named source with its own fetch cadence.

=== "Python"

    ```python
    import bundlebase.sync as bb
    from datetime import date

    bundle = (bb.create("s3://analytics/vendor-sales")
        .set_name("Vendor Sales — Combined Feed")
        .always_update("SET currency = UPPER(currency)")
        .always_update("SET region = UPPER(region)")
        .create_source("sftp_directory", {
            "url": "sftp://vendor-a.example.com/outbound/",
            "patterns": "sales_*.csv",
            "username": "pipeline-user",
            "key_path": "/etc/keys/vendor_a_rsa"
        })
        .create_source("http", {
            "url": "https://api.vendor-b.com/export/sales",
            "format": "json",
            "json_record_path": "records"
        }, name="vendor_b"))

    bundle.fetch("base", "add")
    bundle.fetch("vendor_b", "add")
    bundle.commit("Initial combined load")

    # Weekly refresh
    bundle = bb.open("s3://analytics/vendor-sales").extend()
    bundle.fetch("base", "add")       # Vendor A: new SFTP drops
    bundle.fetch("vendor_b", "sync")  # Vendor B: full replacement (snapshot API)
    bundle.commit(f"Weekly refresh — {date.today()}")
    ```

=== "SQL"

    ```sql
    -- Setup
    CREATE 's3://analytics/vendor-sales';
    SET NAME 'Vendor Sales — Combined Feed';
    ALWAYS UPDATE SET currency = UPPER(currency);
    ALWAYS UPDATE SET region = UPPER(region);
    CREATE SOURCE FOR base USING sftp_directory WITH (
        url = 'sftp://vendor-a.example.com/outbound/',
        patterns = 'sales_*.csv',
        username = 'pipeline-user',
        key_path = '/etc/keys/vendor_a_rsa'
    );
    CREATE SOURCE FOR vendor_b USING http WITH (
        url = 'https://api.vendor-b.com/export/sales',
        format = 'json',
        json_record_path = 'records'
    );
    FETCH base ADD;
    FETCH vendor_b ADD;
    COMMIT 'Initial combined load';

    -- Weekly refresh
    OPEN 's3://analytics/vendor-sales';
    EXTEND;
    FETCH base ADD;
    FETCH vendor_b SYNC;
    COMMIT 'Weekly refresh';
    ```

---

## Agent data store

A bundle as durable, queryable state for a multi-session agent. The agent can reconstruct its full context from `status`, `schema`, and `history` alone.

=== "Python"

    ```python
    import bundlebase.sync as bb

    # Session 1: initialize
    bundle = (bb.create("s3://agent-workspace/research")
        .set_name("Competitive Research")
        .set_description("Pricing and feature data from competitor APIs, accumulated daily")
        .create_source("http", {
            "url": "https://api.competitor-a.com/pricing",
            "format": "json",
            "json_record_path": "products"
        }))

    bundle.fetch("base", "add")
    bundle.commit("Session 1: initial competitor A fetch")

    # Session 2: the agent opens the bundle and reconstructs context
    bundle = bb.open("s3://agent-workspace/research")
    print(f"{bundle.name}: {bundle.num_rows:,} rows at {bundle.version}")
    for entry in bundle.history():
        print(f"  {entry}")

    # Continue accumulating
    bundle = bundle.extend()
    bundle.fetch("base", "add")
    bundle.commit("Session 2: daily refresh")
    ```

=== "SQL"

    ```sql
    -- Session 1
    CREATE 's3://agent-workspace/research';
    SET NAME 'Competitive Research';
    CREATE SOURCE FOR base USING http WITH (
        url = 'https://api.competitor-a.com/pricing',
        format = 'json',
        json_record_path = 'products'
    );
    FETCH base ADD;
    COMMIT 'Session 1: initial competitor A fetch';

    -- Session 2: reconstruct context
    OPEN 's3://agent-workspace/research';
    SHOW STATUS;
    SHOW HISTORY;

    -- Continue
    EXTEND;
    FETCH base ADD;
    COMMIT 'Session 2: daily refresh';
    ```
