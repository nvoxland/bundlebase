# Versioning

## Manifest System

Bundlebase uses a versioned manifest system for tracking commits:

- Location: `{data_dir}/_bundlebase/` directory
- Format: YAML files with 5-digit version + 12-character hash
- Example: `00001a1b2c3d4e5f6.yaml`
- Each commit can contain multiple "changes" which are the modifications to the bundle
- Until commit() is called, any changes you have made remain in-memory and only used by your bundle. **To share changes, you must commit**

### Example commit file
```yaml
author: nvoxland
message: Indexed
timestamp: 2026-01-08T07:24:18Z
changes:
  - id: 11675b7b-8aca-4187-a38d-bfda4d739e0e
    description: Attach file:///Users/nvoxland/src/nvoxland/bundlebase/test_data/customers-0-100.csv
    operations:
      - type: definePack
        id: '56'
      - type: attachBlock
        source: file:///Users/nvoxland/src/nvoxland/bundlebase/test_data/customers-0-100.csv
        version: 846acd3-64650a58fdd47-4308
        id: '09'
        packId: '56'
        layout: 09-846acd3-64650a58fdd47-4308.rowid.idx
        numRows: 100
        bytes: 17160
        schema:
          fields:
            - name: Customer Id
              data_type: Utf8
              nullable: true
              dict_id: 0
              dict_is_ordered: false
              metadata: {}
            - name: Email
              data_type: Utf8
              nullable: true
              dict_id: 0
              dict_is_ordered: false
              metadata: {}
          metadata: {}
  - id: 678508f2-ba1c-462b-8edc-988c21cd5b42
    description: Index column Email
    operations:
      - type: createIndex
        column: Email
        id: '02'
      - type: indexBlocks
        indexId: '02'
        blocks:
          - 09@846acd3-64650a58fdd47-4308
        path: idx_02_a4d5f7fd-c3f6-46d8-ab16-d62cf744bc4b.idx
        cardinality: 100
```

## Viewing History

The commit tracking gives bundles a built-in versioning system:

- You can view the unique version of the bundle with `.version()`
- You can see the full container history with `.history()`
- You can see uncommitted changes with `.status()`